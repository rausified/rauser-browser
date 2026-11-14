import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { KEEPER_MARKER } from "../pr-constants.mjs";
import {
  KEEPER_GREEN_MINUTES, TRUSTED_AUTHORS, alertBody, alertTable, trustedComment, decideKeeper, issueBody, repairBeadArgs, repairRef, repairsToFile, settleSeconds, skipReason,
} from "../pr-keeper.mjs";
import { QUEUE_COMMENT, REFRESH_COMMENT } from "../dev/queue-pr.mjs";
import { FX, NOW, ago, command, done, greenChecks, rollup, run, snap } from "./pr-fixtures.mjs";

const kinds = (d) => d.actions.map((a) => a.kind);
const alertKinds = (d) => d.alerts.map((a) => a.kind);
const DOCTOR = { author: "doctor-bot", createdAt: ago(30), body: "<!-- pr-doctor -->\n<!-- pr-doctor outcome=needs-person -->\n**PR doctor**\n\nThe files:\n\n- `host/src/lib.rs`\n\nMerge" };

test("skipReason: forks, other bases, Mergify's batch drafts, pr-doctor:skip and closed PRs", () => {
  assert.equal(skipReason(snap()), null);
  assert.equal(skipReason(snap({ pr: { isCrossRepository: true } })), "fork");
  assert.equal(skipReason(snap({ pr: { baseRefName: "release" } })), "base release");
  assert.equal(skipReason(snap({ pr: { headRefName: "mergify/merge-queue/0a1b" } })), "Mergify batch draft");
  assert.equal(skipReason(snap({ labels: ["queue", "pr-doctor:skip"] })), "pr-doctor:skip");
  assert.equal(skipReason(snap({ pr: { state: "MERGED" } })), "merged");
  const d = decideKeeper(snap({ pr: { isCrossRepository: true }, labels: ["queue", "needs-merge"] }), { now: NOW, mergeTreeClean: true });
  assert.deepEqual([d.actions, d.alerts], [[], []]);
});

// (a) queue a labelled PR green for KEEPER_GREEN_MINUTES that Mergify has not admitted.

test("(a) labelled, green 15+ minutes, no request: queue it", () => {
  const d = decideKeeper(snap({ contexts: greenChecks(ago(KEEPER_GREEN_MINUTES)) }), { now: NOW });
  assert.deepEqual(kinds(d), ["queue"]);
  assert.deepEqual(d.alerts, []);
});

test("(a) green for less than the window: wait, and say when to look again", () => {
  const d = decideKeeper(snap({ contexts: greenChecks(ago(5)) }), { now: NOW });
  assert.deepEqual(kinds(d), []);
  assert.equal(d.retryInMinutes, KEEPER_GREEN_MINUTES - 5);
  assert.match(d.notes.join(" "), /green for under 15 minutes/);
});

test("(a) never labels: an unlabelled PR is its owner's to queue", () => {
  const d = decideKeeper(snap({ labels: [] }), { now: NOW });
  assert.deepEqual([kinds(d), d.alerts], [[], []]);
  assert.match(d.notes.join(" "), /not labelled queue/);
});

test("(a) already requested, waiting or checking: nothing; idempotent after its own comment", () => {
  for (const comments of [[command(QUEUE_COMMENT, ago(1))], [FX.checking339], FX.pr329]) {
    assert.deepEqual(kinds(decideKeeper(snap({ comments }), { now: NOW })), []);
  }
});

test("(a) a push cancelled the request (#329): requeue the green head", () => {
  assert.deepEqual(kinds(decideKeeper(snap({ comments: FX.pr329.slice(0, 2) }), { now: NOW })), ["queue"]);
});

// (b) refresh a stale Mergify.

test("(b) #339: Mergify waits on a check GitHub reports passed: refresh once, then alert", () => {
  const d = decideKeeper(snap({ comments: [FX.waiting339] }), { now: NOW });
  assert.deepEqual(kinds(d), ["refresh"]);
  const after = decideKeeper(snap({ comments: [FX.waiting339, command(REFRESH_COMMENT, "2026-11-09T10:10:40Z"), FX.refreshed339] }), { now: NOW });
  assert.deepEqual(kinds(after), []);
  assert.deepEqual(alertKinds(after), ["refresh-ignored"]);
});

// (c) clear a stale needs-merge.

test("(c) needs-merge with a clean merge-tree: remove the label, then queue in the same run", () => {
  const d = decideKeeper(snap({ labels: ["queue", "needs-merge"], comments: [DOCTOR] }), { now: NOW, mergeTreeClean: true });
  assert.deepEqual(kinds(d), ["unlabel", "queue"]);
  assert.equal(d.actions[0].label, "needs-merge");
  assert.deepEqual(d.alerts, []);
});

test("(c) needs-merge with an unknown merge-tree result: leave the label", () => {
  const d = decideKeeper(snap({ labels: ["queue", "needs-merge"] }), { now: NOW, mergeTreeClean: null });
  assert.ok(!kinds(d).includes("unlabel"));
});

// (d) alerts only a person can clear.

test("(d) a draft labelled queue alerts and is never queued", () => {
  const d = decideKeeper(snap({ pr: { isDraft: true } }), { now: NOW });
  assert.deepEqual([kinds(d), alertKinds(d)], [[], ["draft-queued"]]);
});

test("(d) red but labelled or queued alerts with the failed job's log command", () => {
  const contexts = rollup([done("Windows / Build and check (windows-latest)", "FAILURE", { runId: 777 }), done("Build and check", "FAILURE", { required: true })]);
  const d = decideKeeper(snap({ contexts }), { now: NOW });
  assert.deepEqual([kinds(d), alertKinds(d)], [[], ["red-queued"]]);
  assert.match(d.alerts[0].text, /Windows .*failed .*gh run view 777 --log-failed/);
  // Red and unlabelled is the owner's business, not an alert.
  assert.deepEqual(decideKeeper(snap({ contexts, labels: [] }), { now: NOW }).alerts, []);
});

test("(d) dequeued on the current head alerts with Mergify's reason; a new head is requeued instead", () => {
  const same = decideKeeper(snap({ pr: { headRefOid: "5652acd0000000000000000000000000000000000" }, comments: [FX.dequeued349] }), { now: NOW });
  assert.deepEqual([kinds(same), alertKinds(same)], [[], ["dequeued"]]);
  assert.match(same.alerts[0].text, /Queue conditions are not satisfied/);
  const moved = decideKeeper(snap({ comments: [FX.dequeued349] }), { now: NOW });
  assert.deepEqual([kinds(moved), alertKinds(moved)], [["queue"], []]);
});

// (e) the unattended repair path.

test("(e) pr-doctor's needs-person verdict with a real conflict: request a repair bead and alert", () => {
  const s = snap({ labels: ["queue", "needs-merge"], comments: [DOCTOR], pr: { mergeable: "CONFLICTING", mergeStateStatus: "DIRTY" } });
  const d = decideKeeper(s, { now: NOW, mergeTreeClean: false, trusted: ["doctor-bot"] });
  assert.deepEqual(kinds(d), ["repair"]);
  assert.deepEqual(alertKinds(d), ["needs-person"]);
  assert.equal(d.actions[0].ref, repairRef(243, s.headOid));
  assert.deepEqual(d.actions[0].files, ["`host/src/lib.rs`"]);
  assert.match(d.alerts[0].text, /host\/src\/lib\.rs/);
});

test("repairBeadArgs: a P1 bug with the per-head external ref, and no mention of anyone", () => {
  const args = repairBeadArgs("o/r", { number: 243, head: "6629a6b3a95ba330", files: ["a.rs"], ref: "gh-pr-243-6629a6b3a95b" });
  assert.equal(args[0], "Merge main into PR #243 by hand: pr-doctor could not resolve the conflict");
  for (const [flag, value] of [["-p", "1"], ["-t", "bug"], ["--external-ref", "gh-pr-243-6629a6b3a95b"], ["-l", "pr-keeper"]]) assert.equal(args[args.indexOf(flag) + 1], value);
  assert.doesNotMatch(args.join(" "), /@\w/);
});

test("repairsToFile: one open repair bead per PR; never twice for one head", () => {
  const req = (number, head) => ({ number, head, ref: repairRef(number, head) });
  assert.deepEqual(repairsToFile([req(1, "aaaa"), req(2, "bbbb")], []).map((r) => r.ref), ["gh-pr-1-aaaa", "gh-pr-2-bbbb"]);
  assert.deepEqual(repairsToFile([req(1, "cccc")], [{ external_ref: "gh-pr-1-aaaa", status: "in_progress" }]), []);
  assert.deepEqual(repairsToFile([req(1, "cccc")], [{ external_ref: "gh-pr-1-aaaa", status: "closed" }]).map((r) => r.ref), ["gh-pr-1-cccc"]);
  assert.deepEqual(repairsToFile([req(1, "aaaa")], [{ external_ref: "gh-pr-1-aaaa", status: "closed" }]), []);
  assert.deepEqual(repairsToFile([req(1, "aaaa"), req(1, "cccc")], []).map((r) => r.ref), ["gh-pr-1-aaaa"]);
  // gh-pr-1- must not match PR 12's beads.
  assert.equal(repairsToFile([req(1, "aaaa")], [{ external_ref: "gh-pr-12-aaaa", status: "open" }]).length, 1);
});

// Review of #427: spoofed markers, Cookie's held label, fighting pr-doctor.

test("I1: a needs-person comment from an untrusted author is ignored: no bead, no alert", () => {
  const s = snap({ labels: ["queue", "needs-merge"], comments: [DOCTOR], pr: { mergeable: "CONFLICTING", mergeStateStatus: "DIRTY" } });
  const d = decideKeeper(s, { now: NOW, mergeTreeClean: false });
  assert.deepEqual([kinds(d), alertKinds(d)], [[], []]);
  assert.ok(TRUSTED_AUTHORS.includes("github-actions[bot]"));
  // Only the trusted author's marker comment counts, even when a newer spoof follows it.
  const spoof = { author: "outsider", createdAt: ago(1), body: "<!-- pr-keeper -->\nfake" };
  const mine = { author: "doctor-bot", createdAt: ago(5), body: "<!-- pr-keeper -->\nreal" };
  assert.equal(trustedComment([spoof, mine], "<!-- pr-keeper -->", ["doctor-bot"]).body, "<!-- pr-keeper -->\nreal");
  assert.equal(trustedComment([spoof], "<!-- pr-keeper -->", ["doctor-bot"]), null);
});

test("I1: file names from the doctor comment cannot mention anyone in alerts or beads", () => {
  const body = DOCTOR.body.replace("- `host/src/lib.rs`", "- `@someone please run this`");
  const s = snap({ labels: ["queue", "needs-merge"], comments: [{ ...DOCTOR, body }], pr: { mergeable: "CONFLICTING", mergeStateStatus: "DIRTY" } });
  const d = decideKeeper(s, { now: NOW, mergeTreeClean: false, trusted: ["doctor-bot"] });
  const outside = (text) => text.replace(/`[^`]*`/g, "");
  assert.doesNotMatch(d.alerts[0].text, /@someone/);
  assert.doesNotMatch(outside(alertBody(d.alerts)), /@/);
  assert.doesNotMatch(repairBeadArgs("o/r", { number: 243, head: "abc", files: d.actions[0].files, ref: "gh-pr-243-abc" }).join(" "), /@someone/);
});

test("I2: Cookie's held label: the keeper never queues or refreshes it", () => {
  assert.equal(skipReason(snap({ labels: ["queue", "held"] })), "held");
  const d = decideKeeper(snap({ labels: ["queue", "held"], comments: FX.pr329.slice(0, 2) }), { now: NOW });
  assert.deepEqual(kinds(d), []);
  assert.deepEqual(kinds(decideKeeper(snap({ labels: ["queue", "held"], comments: [FX.waiting339] }), { now: NOW })), []);
});

test("I3: needs-merge stays while GitHub still reports a conflict, though merge-tree is clean", () => {
  const s = snap({ labels: ["queue", "needs-merge"], pr: { mergeable: "CONFLICTING", mergeStateStatus: "DIRTY" } });
  assert.ok(!kinds(decideKeeper(s, { now: NOW, mergeTreeClean: true })).includes("unlabel"));
  const unknown = snap({ labels: ["queue", "needs-merge"], pr: { mergeable: "UNKNOWN", mergeStateStatus: "UNKNOWN" } });
  assert.ok(kinds(decideKeeper(unknown, { now: NOW, mergeTreeClean: true })).includes("unlabel"));
});

test("Minor 3: the tracking issue's table ignores the timestamp, with or without alerts", () => {
  assert.equal(alertTable(issueBody([], "2026-11-12T01:00:00Z")), alertTable(issueBody([], "2026-11-12T02:00:00Z")));
  const rows = [{ number: 1, alerts: [{ kind: "draft-queued", text: "x" }] }];
  assert.equal(alertTable(issueBody(rows, "2026-11-12T01:00:00Z")), alertTable(issueBody(rows, "2026-11-12T02:00:00Z")));
  assert.notEqual(alertTable(issueBody(rows, "2026-11-12T01:00:00Z")), alertTable(issueBody([], "2026-11-12T01:00:00Z")));
});

test("Minor 5: the keeper checkout keeps no credential; only the bead step gets one", () => {
  const yml = readFileSync(new URL("../../.github/workflows/pr-doctor.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const keeper = yml.split("\n  keeper:\n")[1];
  assert.match(keeper, /persist-credentials: false/);
  assert.match(keeper.split("- name: File repair beads")[1], /gh auth setup-git/);
});

// Output.

test("alertBody is sticky and never mentions anyone; it clears to 'No open alerts' only once it existed", () => {
  const d = decideKeeper(snap({ pr: { isDraft: true } }), { now: NOW });
  const body = alertBody(d.alerts);
  assert.ok(body.startsWith(KEEPER_MARKER));
  assert.doesNotMatch(body.replace(/`[^`]*`/g, ""), /@/);
  assert.equal(alertBody([]), null);
  assert.match(alertBody([], { hadComment: true }), /No open alerts/);
});

test("issueBody lists every open alert in one table", () => {
  const body = issueBody([{ number: 1, alerts: [{ kind: "draft-queued", text: "is a | draft" }] }, { number: 2, alerts: [] }], NOW);
  assert.match(body, /\| #1 \| draft-queued \| is a \\\| draft \|/);
  assert.doesNotMatch(body, /#2/);
  assert.match(issueBody([], NOW), /No open PR alerts/);
});

test("settleSeconds waits for the latest PR due, within the window", () => {
  assert.equal(settleSeconds([null, null]), null);
  assert.equal(settleSeconds([2, 10.5, null]), 10.5 * 60 + 60);
  assert.equal(settleSeconds([99]), KEEPER_GREEN_MINUTES * 60 + 60);
});

test("mergeTreeClean uses only merge-tree flags git 2.38+ has (no --quiet)", () => {
  const source = readFileSync(new URL("../pr-keeper.mjs", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const call = /git\(\["merge-tree"[^\]]*\]/.exec(source)?.[0] ?? "";
  assert.match(call, /"--write-tree"/);
  assert.doesNotMatch(call, /--quiet/);
});

test("the keeper never merges, pushes or force-pushes (hard denies)", () => {
  const source = readFileSync(new URL("../pr-keeper.mjs", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const code = source.split("\n").filter((line) => !/^\s*(\/\/|\*|\/\*\*)/.test(line)).join("\n");
  assert.doesNotMatch(code, /"merge"(?!-tree)|"push"|--force|--admin|"--auto"/);
});

test("the workflow runs the keeper after the doctor, on a schedule and when CI completes", () => {
  const yml = readFileSync(new URL("../../.github/workflows/pr-doctor.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  assert.match(yml, /workflow_run:\n\s+workflows: \[CI\]\n\s+types: \[completed\]/);
  assert.match(yml, /cron: "\*\/15 \* \* \* \*"/);
  assert.match(yml, /keeper:\n\s+name: PR keeper\n\s+needs: doctor/);
  assert.match(yml, /node scripts\/pr-keeper\.mjs/);
  // merge-tree needs the merge base: the keeper's checkout is a full clone.
  assert.match(yml.split("keeper:\n")[1], /fetch-depth: 0/);
  // The doctor never runs on workflow_run: only the keeper does.
  assert.match(yml, /github\.event_name != 'workflow_run' &&/);
});

test("a pending check is not a red alert and does not queue before green", () => {
  const contexts = rollup([run("Build and check", "IN_PROGRESS", null, { required: true })]);
  const d = decideKeeper(snap({ contexts }), { now: NOW });
  assert.deepEqual([kinds(d), d.alerts], [[], []]);
});

test("the doctor's copied tools include every module pr-doctor.mjs imports, transitively", () => {
  const yml = readFileSync(new URL("../../.github/workflows/pr-doctor.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const copied = new Set();
  for (const m of yml.matchAll(/cp ((?:[^\n]|\\\n)+?) "\$RUNNER_TEMP\/pr-doctor\/(dev\/)?"/g)) {
    for (const file of m[1].replace(/\\\n/g, " ").split(/\s+/).filter(Boolean)) copied.add(`${m[2] ?? ""}${file.split("/").pop()}`);
  }
  const seen = new Set();
  const visit = (rel) => {
    if (seen.has(rel)) return;
    seen.add(rel);
    assert.ok(copied.has(rel), `the workflow does not copy ${rel}`);
    const source = readFileSync(new URL(`../${rel}`, import.meta.url), "utf8").replace(/\r\n/g, "\n");
    for (const [, spec] of source.matchAll(/^import [^;]*? from "(\.{1,2}\/[^"]+)";/gms)) {
      const next = new URL(spec, new URL(`../${rel}`, import.meta.url)).pathname.split("/scripts/").pop();
      visit(next);
    }
  };
  visit("pr-doctor.mjs");
  assert.ok(seen.has("dev/queue-pr.mjs") && seen.has("pr-constants.mjs"));
});

// brwr-mva0 follow-ups from the #427 review.

// timing-ok: fixture times, passed to decideKeeper as `now`; never compared with the clock
const AT = (hhmmss) => `2026-11-09T${hhmmss}Z`;
const pushed = greenChecks(AT("10:31:00")).map((c) => ({ ...c, startedAt: AT("10:30:25") }));

test("Minor 2: a queue request Mergify never answers is an alert after the grace, and never a second comment", () => {
  const s = snap({ comments: [command(QUEUE_COMMENT, AT("10:40:00"), "github-actions[bot]")], contexts: pushed, committedDate: AT("10:30:00") });
  const soon = decideKeeper(s, { now: AT("10:42:00") });
  assert.deepEqual([kinds(soon), alertKinds(soon)], [[], []]);
  const late = decideKeeper(s, { now: AT("11:30:00") });
  assert.deepEqual([kinds(late), alertKinds(late)], [[], ["queue-unanswered"]]);
  assert.match(late.alerts[0].text, /github-actions\[bot\]/);
  assert.doesNotMatch(alertBody(late.alerts).replace(/`[^`]*`/g, ""), /@/);
});

test("Minor 1: 'already running' with no live request: the keeper does not comment again on this head", () => {
  const ignored = { ...FX.ignored339, createdAt: AT("10:40:10"), updatedAt: AT("10:40:10") };
  const s = snap({ comments: [command(QUEUE_COMMENT, AT("10:40:00"), "keeper-bot"), ignored], contexts: pushed, committedDate: AT("10:30:00") });
  assert.deepEqual(kinds(decideKeeper(s, { now: AT("11:30:00") })), []);
});

test("Minor 6 (Codex P2): the keeper's own alert naming the bead does not stop the request; the queue's beads decide", () => {
  const conflicting = { labels: ["queue", "needs-merge"], pr: { mergeable: "CONFLICTING", mergeStateStatus: "DIRTY" } };
  const first = decideKeeper(snap({ ...conflicting, comments: [DOCTOR] }), { now: NOW, mergeTreeClean: false, trusted: ["doctor-bot"] });
  assert.match(first.alerts[0].text, /is requested/);
  const sticky = { author: "doctor-bot", createdAt: ago(20), body: alertBody(first.alerts) };
  const again = decideKeeper(snap({ ...conflicting, comments: [DOCTOR, sticky] }), { now: NOW, mergeTreeClean: false, trusted: ["doctor-bot"] });
  assert.deepEqual(kinds(again), ["repair"]);
  // A filing that never happened (no token, a transient failure) is retried: repairsToFile finds no bead.
  assert.equal(repairsToFile([{ number: 243, ...again.actions[0] }], []).length, 1);
});

test("Minor 6: the bead step needs the push token, and says so instead of failing red without it", () => {
  const yml = readFileSync(new URL("../../.github/workflows/pr-doctor.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const step = yml.split("- name: File repair beads")[1];
  assert.match(step, /HAS_PUSH_TOKEN: \$\{\{ secrets\.PR_DOCTOR_TOKEN != '' \}\}/);
  assert.match(step, /::warning title=PR keeper::/);
});

test("Minor 9: CI completions run the keeper only for pull_request runs on PR branches, not Mergify drafts or main", () => {
  const yml = readFileSync(new URL("../../.github/workflows/pr-doctor.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const cond = yml.split("\n  keeper:\n")[1].split("concurrency:")[0];
  assert.match(cond, /github\.event\.workflow_run\.event == 'pull_request'/);
  assert.match(cond, /!startsWith\(github\.event\.workflow_run\.head_branch, 'mergify\/'\)/);
});

test("brwr-33uk: one trusted-author list, shared by the keeper and pr-watch", async () => {
  const constants = await import("../pr-constants.mjs");
  assert.equal(TRUSTED_AUTHORS, constants.TRUSTED_AUTHORS);
});
