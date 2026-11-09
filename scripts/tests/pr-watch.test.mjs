import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  ADMIT_GRACE_MINUTES, DOCTOR_GRACE_MINUTES, EXIT, QUEUE_COMMENT, REFRESH_COMMENT,
  analyzeChecks, decide, decideConflict, doctorComment, enqueueCommand, latestChecks, mergifyStatus, parseArgs, snapshotFromGraphql, statusLine,
} from "../dev/pr-watch.mjs";

// Mergify's own comments, recorded from this repository (fixtures/mergify-comments.json `_source`).
const FX = JSON.parse(readFileSync(new URL("./fixtures/mergify-comments.json", import.meta.url), "utf8"));

// --- Check fixtures in the recorded shape of the GraphQL query (PR #243, 2026-11-02) --

let runSeq = 41500842838;
const run = (name, status, conclusion, { required = false, runId = runSeq, startedAt = "2026-11-02T18:33:19Z" } = {}) =>
  ({ __typename: "CheckRun", name, status, conclusion, isRequired: required, startedAt, checkSuite: { workflowRun: { databaseId: runId } } });
const done = (name, conclusion = "SUCCESS", opts) => run(name, "COMPLETED", conclusion, opts);
const status = (context, state, required = true) => ({ __typename: "StatusContext", context, state, isRequired: required });

const GREEN = [
  done("Classify changes"),
  done("PR owner", "SUCCESS", { required: true, runId: 41500842845 }),
  done("Rust dependency policy", "SUCCESS", { required: true }),
  done("Dev tools", "SUCCESS", { runId: 41500842845 }),
  done("Docs and queue checks", "SKIPPED"),
  done("macOS / Build and check (macos-15)", "SKIPPED"),
  done("Windows / Build and check (windows-latest)"),
  done("Build and check", "SUCCESS", { required: true }),
  status("merge-hold", "SUCCESS"),
];
function rollup(overrides = [], base = GREEN) {
  const names = new Set(overrides.map((node) => node.name ?? node.context));
  return [...base.filter((node) => !names.has(node.name ?? node.context)), ...overrides];
}
const gql = (c) => ({ author: { login: c.author }, body: c.body, createdAt: c.createdAt, updatedAt: c.updatedAt });
function graphql({ pr = {}, contexts = GREEN, comments = [FX.checking339] } = {}) {
  return { repository: { pullRequest: {
    number: 243, state: "OPEN", isDraft: false, mergeable: "MERGEABLE", mergeStateStatus: "CLEAN",
    headRefOid: "6629a6b3a95ba330a0a564eb5f22a0e9fe7a843a", autoMergeRequest: null,
    labels: { nodes: [{ name: "queue" }] }, comments: { nodes: comments.map(gql) },
    commits: { nodes: [{ commit: { statusCheckRollup: { contexts: { nodes: contexts } } } }] },
    ...pr,
  } } };
}
const snap = (opts) => snapshotFromGraphql(graphql(opts));
const NOW = "2026-11-02T20:00:00Z";
const ago = (m) => new Date(Date.parse(NOW) - m * 60000).toISOString();
const UNLABELED = { labels: { nodes: [] } };

// --- parseArgs ------------------------------------------------------------------

test("parseArgs takes a PR number and defaults to waiting for the merge", () => {
  assert.deepEqual(parseArgs(["243"]), { until: "merged", interval: 120, once: false, maxMinutes: null, repo: null, pr: 243, help: false });
  const opts = parseArgs(["#243", "--until", "green", "--interval", "60", "--once", "--repo", "o/r", "--max-minutes", "9"]);
  assert.deepEqual([opts.pr, opts.until, opts.interval, opts.once, opts.repo, opts.maxMinutes], [243, "green", 60, true, "o/r", 9]);
});

test("parseArgs refuses a missing PR, a bad --until, a hammering interval and unknown input", () => {
  assert.throws(() => parseArgs([]), /missing PR/);
  assert.throws(() => parseArgs(["1", "--until", "queued"]), /--until/);
  assert.throws(() => parseArgs(["1", "--interval", "5"]), /at least 30/);
  assert.throws(() => parseArgs(["1", "--bogus"]), /unknown option/i);
  assert.throws(() => parseArgs(["1", "2"]), /unknown argument 2/);
});

// --- latestChecks -----------------------------------------------------------------

test("latestChecks keeps the newest run of a repeated check name", () => {
  const checks = latestChecks([
    done("Merge hold", "FAILURE", { runId: 41500842462 }), done("Merge hold", "SUCCESS", { runId: 41503934800 }), done("Merge hold", "SUCCESS", { runId: 41503871045 }),
  ]);
  assert.deepEqual(checks.map((c) => [c.runId, c.state]), [[41503934800, "SUCCESS"]]);
});

test("latestChecks: a re-run in progress replaces the failed attempt", () => {
  const checks = latestChecks([done("Build and check", "FAILURE", { required: true, runId: 100 }), run("Build and check", "IN_PROGRESS", null, { required: true, runId: 200 })]);
  assert.deepEqual(checks.map((c) => [c.state, c.runId]), [["IN_PROGRESS", 200]]);
});

// --- mergifyStatus on recorded comments ---------------------------------------------

test("mergifyStatus: #339 waiting reads the unmet checklist, since the waiting payload's tree is empty", () => {
  assert.match(FX.waiting339.body, /"required_conditions": \[\]/);
  const s = mergifyStatus([FX.waiting339]);
  assert.equal(s.state, "waiting");
  assert.deepEqual(s.unmet, ["check-success = Build and check", "check-neutral = Build and check", "check-skipped = Build and check"]);
});

test("mergifyStatus: #339 in the queue is checking with its step and no unmet list", () => {
  assert.deepEqual(mergifyStatus([FX.checking339, FX.ignored339, FX.refreshed339]), { state: "checking", step: "checks_running", unmet: [], reason: null, code: null });
});

test("mergifyStatus: #299 dequeued carries Mergify's reason", () => {
  const s = mergifyStatus([FX.dequeued299]);
  assert.equal(s.state, "dequeued");
  assert.equal(s.reason, "The pull request conflicts with main");
  assert.equal(s.code, "CONFLICT_WITH_BASE_BRANCH");
});

test("mergifyStatus: a push cancels the request until a later status comment (#329)", () => {
  assert.equal(mergifyStatus(FX.pr329.slice(0, 2)).state, "cancelled");
  assert.equal(mergifyStatus(FX.pr329).state, "merged");
});

test("mergifyStatus: the newest-created status comment is current, though Mergify rewrites the old one after it (#346)", () => {
  assert.equal(mergifyStatus(FX.pr346).state, "waiting");
  assert.equal(mergifyStatus([...FX.pr346].reverse()).state, "waiting"); // GraphQL order does not matter
  assert.equal(mergifyStatus([FX.pr329[2], FX.pr329[0], FX.pr329[1], FX.pr329[3]]).state, "merged");
});

const queueCmd = (createdAt) => ({ author: "jrausercisco", createdAt, body: "@Mergifyio queue" });

test("mergifyStatus: a queue comment newer than every status comment is requested, not the old dequeue (#346)", () => {
  const old = { ...FX.dequeued299, createdAt: "2026-11-09T11:44:20Z" };
  const fresh = { ...FX.waiting339, createdAt: "2026-11-09T12:18:45Z" };
  assert.equal(mergifyStatus([old]).state, "dequeued");
  assert.equal(mergifyStatus([old, queueCmd("2026-11-09T12:18:36Z")]).state, "requested");
  assert.equal(mergifyStatus([old, queueCmd("2026-11-09T12:18:36Z"), fresh]).state, "waiting");
});

test("mergifyStatus: a repeat queue comment Mergify ignores as already running keeps the live status (#339)", () => {
  const s = mergifyStatus([FX.waiting339, queueCmd("2026-11-09T10:04:10Z"), FX.ignored339]);
  assert.equal(s.state, "waiting");
  assert.ok(s.unmet.includes("check-success = Build and check"));
});

test("requested: wait for Mergify's answer, then exit 16 after the grace", () => {
  const s = snap({ comments: [FX.dequeued299, queueCmd("2026-11-09T12:00:00Z")] });
  assert.equal(decide(s, { now: NOW, idleSince: ago(1) }).action, "requested");
  assert.equal(decide(s, { now: NOW, idleSince: ago(ADMIT_GRACE_MINUTES) }).action, "queue-unanswered");
});

test("mergifyStatus: command replies alone, or a payload from anyone but Mergify, are no request", () => {
  assert.equal(mergifyStatus([FX.ignored339, FX.refreshed339]).state, "none");
  assert.equal(mergifyStatus([{ ...FX.checking339, author: "rausified" }]).state, "none");
  assert.equal(mergifyStatus([]).state, "none");
  assert.equal(mergifyStatus([{ ...FX.checking339, author: "mergify[bot]" }]).state, "checking"); // REST login
});

// --- decide: terminal and check states ------------------------------------------------

test("merged exits 0; closed exits CLOSED; draft says how to mark it ready", () => {
  assert.equal(decide(snap({ pr: { state: "MERGED" } })).exitCode, EXIT.DONE);
  assert.equal(decide(snap({ pr: { state: "CLOSED" } })).exitCode, EXIT.CLOSED);
  const d = decide(snap({ pr: { isDraft: true } }));
  assert.equal(d.exitCode, EXIT.DRAFT);
  assert.match(d.message, /gh pr ready 243/);
});

test("--until green finishes on green and says whether Mergify has it", () => {
  assert.match(decide(snap(), { until: "green" }).message, /green; Mergify: checking$/);
  const d = decide(snap({ pr: UNLABELED, comments: [] }), { until: "green" });
  assert.equal(d.exitCode, EXIT.DONE);
  assert.match(d.message, /not queued: gh pr edit 243 --add-label queue && gh pr comment 243 --body "@Mergifyio queue"/);
});

test("green needs the gate itself: before it reports, a failed input is a failure (PR #264)", () => {
  const contexts = rollup([done("Windows / Build and check (windows-latest)", "FAILURE"), run("macOS / Build and check (macos-15)", "IN_PROGRESS", null)])
    .filter((c) => c.name !== "Build and check");
  assert.equal(decide(snap({ contexts }), { until: "green" }).exitCode, EXIT.CHECK_FAILED);
  const pendingOnly = rollup([run("macOS / Build and check (macos-15)", "IN_PROGRESS", null)]).filter((c) => c.name !== "Build and check");
  assert.equal(decide(snap({ contexts: pendingOnly }), { until: "green" }).done, false);
});

test("a failed gate input names the job and its log command, before the gate finishes", () => {
  const contexts = rollup([done("Windows / Build and check (windows-latest)", "FAILURE", { runId: 41571195112 }), run("Build and check", "QUEUED", null, { required: true })]);
  const d = decide(snap({ contexts }));
  assert.equal(d.exitCode, EXIT.CHECK_FAILED);
  assert.match(d.message, /Windows .*gh run view 41571195112 --log-failed/);
});

test("a failed Accessibility job blocks like the platform builds (PR #275)", () => {
  const d = decide(snap({ contexts: rollup([done("Accessibility", "FAILURE")]) }));
  assert.equal(d.exitCode, EXIT.CHECK_FAILED);
  assert.match(d.message, /Accessibility/);
});

test("PR owner failure points at the PR template, not a log", () => {
  const d = decide(snap({ contexts: rollup([done("PR owner", "FAILURE", { required: true })]) }));
  assert.equal(d.action, "failed-pr-owner");
  assert.match(d.message, /Owner: line/);
});

test("a failing check outside the gate does not stop the watch", () => {
  assert.equal(decide(snap({ contexts: rollup([done("Dev tools", "FAILURE")]) })).done, false);
});

test("a cancelled input with nothing replacing it says rerun; the gate failing only from it is a cancellation", () => {
  const contexts = rollup([done("macOS / Build and check (macos-15)", "CANCELLED", { runId: 41553653914 }), done("Build and check", "FAILURE", { required: true })]);
  const d = decide(snap({ contexts }));
  assert.equal(d.exitCode, EXIT.CANCELLED);
  assert.match(d.message, /gh run rerun 41553653914 --failed/);
});

test("a cancelled input while its replacement runs is a wait; once the gate passes it no longer matters", () => {
  const starved = [done("macOS / Build and check (macos-15)", "CANCELLED"), run("macOS fallback / Build and check (macos-15-intel)", "IN_PROGRESS", null)];
  assert.equal(decide(snap({ contexts: rollup([...starved, run("Build and check", "QUEUED", null, { required: true })]) })).done, false);
  const passed = rollup([done("macOS / Build and check (macos-15)", "CANCELLED"), done("macOS fallback / Build and check (macos-15-intel)")]);
  assert.equal(analyzeChecks(latestChecks(passed)).green, true);
  assert.equal(decide(snap({ contexts: passed })).action, "checking:checks_running");
});

// --- decide: Mergify ------------------------------------------------------------------

test("in the queue: wait, naming Mergify's step; BEHIND changes nothing", () => {
  const d = decide(snap({ pr: { mergeStateStatus: "BEHIND" } }));
  assert.equal(d.exitCode, EXIT.WAITING);
  assert.match(d.message, /in the Mergify queue \(checks_running\)/);
  assert.doesNotMatch(d.message, /merge main/);
});

test("#339: waiting on Build and check that GitHub reports passed advises refresh, not a repeat queue", () => {
  const s = snap({ comments: [FX.waiting339] });
  assert.equal(decide(s, { now: NOW, idleSince: ago(1) }).done, false); // ordinary Mergify lag (#346)
  const d = decide(s, { now: NOW, idleSince: ago(ADMIT_GRACE_MINUTES) });
  assert.equal(d.exitCode, EXIT.QUEUE_BLOCKED);
  assert.equal(d.action, "queue-refresh");
  assert.match(d.message, new RegExp(`gh pr comment 243 --body "${REFRESH_COMMENT}"`));
  assert.doesNotMatch(d.message, /@Mergifyio queue"/);
});

test("waiting on Build and check while it still runs is a plain wait", () => {
  const contexts = rollup([run("Build and check", "IN_PROGRESS", null, { required: true })]);
  const d = decide(snap({ comments: [FX.waiting339], contexts }), { now: NOW });
  assert.equal(d.done, false);
  assert.match(d.message, /queued; Mergify is waiting .*waiting on Build and check/);
});

test("waiting on a non-check condition past the grace names it; unlabeled says add the label", () => {
  const body = FX.waiting339.body.replace("- [ ] `check-success = Build and check`", "- [ ] `label = queue`");
  const s = snap({ pr: UNLABELED, comments: [{ ...FX.waiting339, body }] });
  assert.equal(decide(s, { now: NOW, idleSince: ago(1) }).done, false);
  const d = decide(s, { now: NOW, idleSince: ago(ADMIT_GRACE_MINUTES) });
  assert.equal(d.action, "queue-conditions-unmet");
  assert.match(d.message, /`label = queue`.*gh pr edit 243 --add-label queue/);
});

test("dequeued for a conflict (#299): merge main in or wait for pr-doctor, push, then requeue on the new head", () => {
  // GitHub has not recomputed mergeability yet, so the DIRTY branch does not run.
  const d = decide(snap({ pr: { mergeable: "UNKNOWN", mergeStateStatus: "UNKNOWN" }, comments: [FX.dequeued299] }));
  assert.equal(d.exitCode, EXIT.CHECK_FAILED);
  assert.equal(d.action, "dequeued:conflict");
  assert.match(d.message, /conflicts with main/);
  assert.match(d.message, /merge main in/i);
  assert.match(d.message, /pr-doctor/);
  assert.match(d.message, /new head.*gh pr comment 243 --body "@Mergifyio queue"/);
});

test("dequeued for queue conditions (#349): name the condition that failed, then requeue on the new head", () => {
  assert.deepEqual(mergifyStatus([FX.dequeued349]).unmet, ["-label = needs-merge"]);
  const d = decide(snap({ comments: [FX.dequeued349] }));
  assert.equal(d.exitCode, EXIT.CHECK_FAILED);
  assert.equal(d.action, "dequeued:conditions");
  assert.match(d.message, /Queue conditions are not satisfied/);
  assert.match(d.message, /`-label = needs-merge`/);
  assert.match(d.message, /new head.*gh pr comment 243 --body "@Mergifyio queue"/);
});

test("dequeued for another reason keeps the generic advice, including requeue for a flake", () => {
  const body = FX.dequeued299.body.replace("CONFLICT_WITH_BASE_BRANCH", "CHECKS_FAILED").replace("conflict with base branch", "checks failed")
    .replace("The pull request conflicts with main", "Checks failed in the queue");
  const d = decide(snap({ comments: [{ ...FX.dequeued299, body }] }));
  assert.equal(d.action, "dequeued");
  assert.match(d.message, /Checks failed in the queue.*"@Mergifyio queue".*@mergifyio requeue/);
});

test("not queued while checks run: keep waiting, and print the queue commands now", () => {
  const contexts = rollup([run("Build and check", "IN_PROGRESS", null, { required: true })]);
  const d = decide(snap({ pr: UNLABELED, comments: [], contexts }));
  assert.equal(d.done, false);
  assert.match(d.message, /queue it now: gh pr edit 243 --add-label queue && gh pr comment 243/);
});

test("the #326 race: green and not yet queued waits out the grace before exit 12", () => {
  const s = snap({ pr: UNLABELED, comments: [] });
  assert.equal(decide(s, { now: NOW, idleSince: NOW }).done, false);
  assert.equal(decide(s, { now: NOW, idleSince: ago(ADMIT_GRACE_MINUTES - 1) }).done, false);
  const d = decide(s, { now: NOW, idleSince: ago(ADMIT_GRACE_MINUTES) });
  assert.equal(d.exitCode, EXIT.ENQUEUE);
  assert.match(d.message, /no queue request/);
});

test("labeled but the push cancelled the request: exit 12 asks only for the comment", () => {
  const d = decide(snap({ comments: FX.pr329.slice(0, 2) }), { now: NOW, idleSince: ago(ADMIT_GRACE_MINUTES) });
  assert.equal(d.exitCode, EXIT.ENQUEUE);
  assert.match(d.message, /a push cancelled its queue request: gh pr comment 243 --body "@Mergifyio queue"$/);
  assert.equal(enqueueCommand(snapshotFromGraphql(graphql())), `gh pr comment 243 --body "${QUEUE_COMMENT}"`);
});

test("GitHub auto-merge armed is exit 12: it would merge around the queue", () => {
  const d = decide(snap({ pr: { autoMergeRequest: { enabledAt: NOW } } }));
  assert.equal(d.exitCode, EXIT.ENQUEUE);
  assert.match(d.message, /gh pr merge --disable-auto 243/);
});

for (const label of ["pr-doctor:skip", "needs-merge"]) {
  test(`labeled queue plus ${label}: exit 16 naming the label to remove`, () => {
    const d = decide(snap({ pr: { labels: { nodes: [{ name: "queue" }, { name: label }] } } }));
    assert.equal(d.exitCode, EXIT.QUEUE_BLOCKED);
    assert.match(d.message, new RegExp(`gh pr edit 243 --remove-label ${label}`));
  });
}

test("the queue comment pr-watch advises is the worker contract's merge.comment", () => {
  const contract = JSON.parse(readFileSync(new URL("../../worker-contract.json", import.meta.url), "utf8"));
  assert.equal(QUEUE_COMMENT, contract.merge.comment);
});

// --- decide: conflicts --------------------------------------------------------------

const dirty = (pr = {}, comments = [FX.checking339]) => snap({ pr: { mergeStateStatus: "DIRTY", mergeable: "CONFLICTING", ...pr }, comments });
const DOCTOR = { author: "rausified", createdAt: NOW, body: "<!-- pr-doctor -->\n<!-- pr-doctor outcome=needs-person -->\n**PR doctor**\n\nThe files:\n\n- `host/src/lib.rs`\n\nMerge" };

test("DIRTY: wait for pr-doctor, never merge main yet", () => {
  const d = decide(dirty(), { now: NOW, dirtySince: ago(10) });
  assert.equal(d.done, false);
  assert.match(d.message, /waiting up to 30 minutes for pr-doctor.*do not merge main yet/);
});

test("DIRTY and labeled needs-merge: merge main yourself, listing the doctor's files", () => {
  assert.deepEqual(doctorComment([DOCTOR]).files, ["host/src/lib.rs"]);
  const d = decide(dirty({ labels: { nodes: [{ name: "queue" }, { name: "needs-merge" }] } }, [DOCTOR]), { now: NOW });
  assert.equal(d.exitCode, EXIT.CONFLICT);
  assert.match(d.message, /labeled it needs-merge.*git merge origin\/main, resolve host\/src\/lib\.rs.*"@Mergifyio queue"/);
});

test("DIRTY and pr-doctor could only report: act at once", () => {
  const report = { ...DOCTOR, body: DOCTOR.body.replace("needs-person", "report-only") };
  assert.equal(decideConflict(dirty({}, [report]), { now: NOW, dirtySince: NOW }).action, "conflict-report-only");
});

test("DIRTY a whole doctor grace period with no repair: act", () => {
  assert.equal(decideConflict(dirty(), { now: NOW, dirtySince: ago(DOCTOR_GRACE_MINUTES - 1) }).done, false);
  assert.equal(decideConflict(dirty(), { now: NOW, dirtySince: ago(DOCTOR_GRACE_MINUTES) }).action, "conflict-doctor-timeout");
});

test("a real check failure outranks DIRTY and mentions the conflict", () => {
  const d = decide(snap({ pr: { mergeStateStatus: "DIRTY", mergeable: "CONFLICTING" }, contexts: rollup([done("Rust dependency policy", "FAILURE", { required: true, runId: 5 })]) }), { now: NOW });
  assert.equal(d.exitCode, EXIT.CHECK_FAILED);
  assert.match(d.message, /also conflicts with main/);
});

// --- statusLine ------------------------------------------------------------------

test("statusLine summarizes state, checks and Mergify on one line", () => {
  const s = snap({ pr: UNLABELED });
  assert.equal(statusLine(s, decide(s)).split("\n")[0], "OPEN/CLEAN | head 6629a6b3 | checks 9 ok, 0 pending, 0 failed | mergify checking/checks_running, unlabeled");
});
