import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  EXIT, QUEUE_COMMENT, REFRESH_COMMENT, analyzeChecks, applyActions, commandSince, parseArgs, planQueue, queueAfterPush, staleConditions, statusComment,
} from "../dev/queue-pr.mjs";
import { FX, NOW, ago, command, done, graphql, greenChecks, rollup, run, snap } from "./pr-fixtures.mjs";
import { snapshotFromGraphql } from "../dev/queue-pr.mjs";

const kinds = (plan) => plan.actions.map((a) => a.kind);
const HEAD = "6629a6b3a95ba330a0a564eb5f22a0e9fe7a843a";

// --- parseArgs ------------------------------------------------------------------

test("parseArgs takes a PR, an optional head and the flags", () => {
  assert.deepEqual(parseArgs(["243"]), { pr: 243, head: null, ready: false, dryRun: false, waitSeconds: 60, repo: null, help: false });
  const opts = parseArgs(["#243", "--head", "abc1234", "--ready", "--dry-run", "--wait-seconds", "5", "--repo", "o/r"]);
  assert.deepEqual([opts.pr, opts.head, opts.ready, opts.dryRun, opts.waitSeconds, opts.repo], [243, "abc1234", true, true, 5, "o/r"]);
  assert.throws(() => parseArgs([]), /missing PR/);
  assert.throws(() => parseArgs(["1", "--head", "<new head>"]), /commit SHA/);
  assert.throws(() => parseArgs(["1", "2"]), /unknown argument 2/);
});

// --- planQueue: the idempotent queue -----------------------------------------------

test("no request: label and queue it; the plan on the result asks for nothing more", () => {
  const first = planQueue(snap({ labels: [] }), { now: NOW });
  assert.deepEqual(kinds(first), ["label", "queue"]);
  assert.equal(first.exitCode, EXIT.OK);
  // After the actions land: labelled, with the queue comment newer than any status comment.
  const after = planQueue(snap({ comments: [command(QUEUE_COMMENT, ago(0))] }), { now: NOW });
  assert.deepEqual(kinds(after), []);
  assert.equal(after.outcome, "requested");
});

test("queue at open: pending checks still queue it (Mergify holds the request until they pass)", () => {
  const contexts = rollup([run("Build and check", "IN_PROGRESS", null, { required: true })]);
  assert.deepEqual(kinds(planQueue(snap({ contexts }), { now: NOW })), ["queue"]);
});

test("already queued, waiting, checking or merged by Mergify: nothing to do", () => {
  for (const comments of [[FX.checking339], [FX.waiting339], FX.pr329]) {
    const contexts = rollup([run("Build and check", "IN_PROGRESS", null, { required: true })]);
    const plan = planQueue(snap({ comments, contexts }), { now: NOW });
    assert.deepEqual(kinds(plan), [], plan.message);
    assert.equal(plan.exitCode, EXIT.OK);
  }
});

test("a push cancelled the request (#329): queue again, once", () => {
  const plan = planQueue(snap({ comments: FX.pr329.slice(0, 2) }), { now: NOW });
  assert.deepEqual(kinds(plan), ["queue"]);
  assert.match(plan.message, /lost its queue request to a push/);
  const again = planQueue(snap({ comments: [...FX.pr329.slice(0, 2), command(QUEUE_COMMENT, "2026-11-09T08:20:00Z")] }), { now: NOW });
  assert.deepEqual(kinds(again), []);
});

test("dequeued on this head: report Mergify's reason and post nothing", () => {
  const plan = planQueue(snap({ pr: { headRefOid: "b67ded8000000000000000000000000000000000" }, comments: [FX.dequeued299] }), { now: NOW });
  assert.deepEqual(kinds(plan), []);
  assert.equal(plan.exitCode, EXIT.PERSON);
  assert.match(plan.message, /on this head: The pull request conflicts with main/);
});

test("dequeued on an older head: the head moved, so queue it again", () => {
  assert.equal(statusComment([FX.dequeued299]).leftHead, "b67ded8");
  const plan = planQueue(snap({ comments: [FX.dequeued299] }), { now: NOW });
  assert.deepEqual(kinds(plan), ["queue"]);
  assert.equal(plan.outcome, "requeued");
});

test("dequeued for queue conditions (#349) names the failed condition", () => {
  const plan = planQueue(snap({ pr: { headRefOid: "5652acd0000000000000000000000000000000000" }, comments: [FX.dequeued349] }), { now: NOW });
  assert.equal(plan.exitCode, EXIT.PERSON);
  assert.match(plan.message, /`-label = needs-merge`/);
});

test("a draft is reported, and marked ready only with --ready", () => {
  const draft = snap({ pr: { isDraft: true }, labels: [] });
  const plan = planQueue(draft, { now: NOW });
  assert.deepEqual(kinds(plan), []);
  assert.equal(plan.exitCode, EXIT.PERSON);
  assert.match(plan.message, /gh pr ready 243/);
  assert.deepEqual(kinds(planQueue(draft, { now: NOW, ready: true })), ["ready", "label", "queue"]);
});

test("needs-merge: queue anyway (Mergify waits on the label) and report it; pr-doctor:skip is an opt-out", () => {
  const blocked = planQueue(snap({ labels: ["queue", "needs-merge"] }), { now: NOW });
  assert.deepEqual(kinds(blocked), ["queue"]);
  assert.equal(blocked.exitCode, EXIT.PERSON);
  assert.match(blocked.message, /carries needs-merge/);
  const skip = planQueue(snap({ labels: ["queue", "pr-doctor:skip"] }), { now: NOW });
  assert.deepEqual(kinds(skip), []);
  assert.match(skip.message, /opt-out/);
});

test("a conflict with main posts nothing: Mergify cannot take it until main is merged in", () => {
  const plan = planQueue(snap({ pr: { mergeable: "CONFLICTING", mergeStateStatus: "DIRTY" } }), { now: NOW });
  assert.deepEqual(kinds(plan), []);
  assert.equal(plan.outcome, "conflict");
});

test("merged and closed PRs exit 13 with nothing to do", () => {
  assert.equal(planQueue(snap({ pr: { state: "MERGED" } })).exitCode, EXIT.CLOSED);
  assert.equal(planQueue(snap({ pr: { state: "CLOSED" } })).exitCode, EXIT.CLOSED);
});

// --- planQueue: stale Mergify (#339) ----------------------------------------------------

test("staleConditions: every unmet condition is a check GitHub reports passed", () => {
  assert.deepEqual(staleConditions(snap({ comments: [FX.waiting339] })), ["Build and check"]);
  const contexts = rollup([run("Build and check", "IN_PROGRESS", null, { required: true })]);
  assert.deepEqual(staleConditions(snap({ comments: [FX.waiting339], contexts })), []);
  assert.deepEqual(staleConditions(snap({ comments: [FX.checking339] })), []);
});

test("#339: refresh once after the grace, then report that the refresh did not help", () => {
  const justGreen = snap({ comments: [FX.waiting339], contexts: greenChecks(ago(1)) });
  assert.deepEqual(kinds(planQueue(justGreen, { now: NOW })), [], "Mergify lags GitHub by up to a minute");
  const stale = snap({ comments: [FX.waiting339], contexts: greenChecks(ago(10)) });
  const plan = planQueue(stale, { now: NOW });
  assert.deepEqual(kinds(plan), ["refresh"]);
  assert.match(plan.message, /Build and check/);
  // The refresh comment is newer than the status comment, which Mergify did not change.
  const after = snap({ comments: [FX.waiting339, command(REFRESH_COMMENT, "2026-11-09T10:10:40Z"), FX.refreshed339], contexts: greenChecks(ago(10)) });
  const again = planQueue(after, { now: NOW });
  assert.deepEqual(kinds(again), []);
  assert.equal(again.outcome, "refresh-ignored");
  assert.equal(again.exitCode, EXIT.PERSON);
});

test("a waiting request with nothing left unmet and green checks is stale too (#422 at 17:11:02Z)", () => {
  const body = FX.waiting339.body.replace(/\n- \[ \] `check-success = Build and check`[\s\S]*?(?=\n\n<details>)/, "");
  const plan = planQueue(snap({ comments: [{ ...FX.waiting339, body }], contexts: greenChecks(ago(10)) }), { now: NOW });
  assert.deepEqual(kinds(plan), ["refresh"]);
});

test("commandSince ignores Mergify's own replies and older commands", () => {
  const since = "2026-11-09T10:00:00Z";
  assert.equal(commandSince([FX.refreshed339], REFRESH_COMMENT, since), false);
  assert.equal(commandSince([command(REFRESH_COMMENT, "2026-11-09T09:00:00Z")], REFRESH_COMMENT, since), false);
  assert.equal(commandSince([command("@mergifyio REFRESH", "2026-11-09T10:01:00Z")], REFRESH_COMMENT, since), true);
});

test("analyzeChecks.greenSince is when the last blocking check finished, only once green", () => {
  assert.equal(analyzeChecks(snap({ contexts: greenChecks(ago(7)) }).checks).greenSince, ago(7));
  const contexts = rollup([done("Build and check", "SUCCESS", { required: true, completedAt: ago(2) })], greenChecks(ago(7)));
  assert.equal(analyzeChecks(snap({ contexts }).checks).greenSince, ago(2));
  const pending = rollup([run("Build and check", "IN_PROGRESS", null, { required: true })]);
  assert.equal(analyzeChecks(snap({ contexts: pending }).checks).greenSince, null);
});

// --- queueAfterPush and applyActions -----------------------------------------------------

test("queueAfterPush waits for the head; without force it posts nothing when the head never shows", () => {
  const posted = [];
  const heads = ["old", "new"];
  assert.deepEqual(queueAfterPush({ sha: "new", headOf: () => heads.shift(), comment: (t) => posted.push(t), wait: () => {} }), { settled: true, seen: "new", commented: true });
  assert.deepEqual(posted, [QUEUE_COMMENT]);
  const none = [];
  const result = queueAfterPush({ sha: "new", headOf: () => "old", comment: (t) => none.push(t), wait: () => {}, limitMs: 10000, force: false });
  assert.deepEqual([result.settled, result.commented, none.length], [false, false, 0]);
});

test("applyActions runs gh for each action and nothing that merges", () => {
  const calls = [];
  applyActions("o/r", 243, [{ kind: "ready" }, { kind: "label" }, { kind: "queue" }, { kind: "refresh" }], { run: (args) => calls.push(args.join(" ")), log: () => {} });
  assert.deepEqual(calls, [
    "pr ready 243 --repo o/r", "pr edit 243 --repo o/r --add-label queue",
    `pr comment 243 --repo o/r --body ${QUEUE_COMMENT}`, `pr comment 243 --repo o/r --body ${REFRESH_COMMENT}`,
  ]);
});

test("queue-pr never merges, pushes or force-pushes", () => {
  const source = readFileSync(new URL("../dev/queue-pr.mjs", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const code = source.split("\n").filter((line) => !/^\s*(\/\/|\*|\/\*\*)/.test(line)).join("\n");
  assert.doesNotMatch(code, /"merge"|"push"|--force|--admin/);
});

test("the snapshot carries the PR's branch fields for the keeper", () => {
  const s = snapshotFromGraphql(graphql({ pr: { headRefName: "mergify/merge-queue/abc", isCrossRepository: true } }));
  assert.deepEqual([s.headRefName, s.baseRefName, s.isCrossRepository, s.headOid], ["mergify/merge-queue/abc", "main", true, HEAD]);
});
