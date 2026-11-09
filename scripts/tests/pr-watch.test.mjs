import assert from "node:assert/strict";
import test from "node:test";
import { Buffer } from "node:buffer";

import {
  ADMIT_GRACE_MINUTES, CONDITIONS_GRACE_MINUTES, QUEUE_COMMENT, DOCTOR_GRACE_MINUTES, EXIT, analyzeChecks, classifyConflicts, decide, decideConflict,
  doctorComment, enqueueCommand, mergifyMeta, mergifyQueueStatus, staleQueueConditions, unmetConditions, latestChecks, parseArgs, snapshotFromGraphql, statusLine,
} from "../dev/pr-watch.mjs";

// --- Fixtures in the recorded shape of the GraphQL query (PR #243, 2026-11-02) --

let runSeq = 41500842838;
const run = (name, status, conclusion, { required = false, runId = runSeq, workflow = "CI", startedAt = "2026-11-02T18:33:19Z", title = null, summary = null } = {}) => ({
  __typename: "CheckRun", name, status, conclusion, isRequired: required, startedAt, title, summary,
  checkSuite: { workflowRun: { databaseId: runId, workflow: { name: workflow } } },
});
const done = (name, conclusion = "SUCCESS", opts) => run(name, "COMPLETED", conclusion, opts);
const status = (context, state, description, required = true) =>
  ({ __typename: "StatusContext", context, state, description, isRequired: required });

// The "Mergify Merge Queue" check run, with the titles seen live (2026-11-03)
// and, optionally, the state Mergify embeds in its summary.
function mq(title, { status = "COMPLETED", conclusion = status === "COMPLETED" ? "NEUTRAL" : null, meta = null, markdown = "" } = {}) {
  const span = meta ? `\n<span itemprop="mergify.v1.${Buffer.from(JSON.stringify(meta)).toString("base64url")}"></span>` : "";
  return run("Mergify Merge Queue", status, conclusion, { workflow: "", title, summary: `${markdown}${span}` });
}
// PR #268's condition tree (labeled needs-merge, checks not run), as Mergify embedded it.
const TREE_268 = [{"description": "-conflict", "match": false, "subconditions": []}, {"description": "any of", "match": false, "subconditions": [{"description": "all of", "match": false, "subconditions": [{"description": "-label = needs-merge", "match": false, "subconditions": []}, {"description": "check-success = Build and check", "match": false, "subconditions": []}, {"description": "check-success = PR owner", "match": false, "subconditions": []}, {"description": "check-success = Rust dependency policy", "match": false, "subconditions": []}, {"description": "label = queue", "match": false, "subconditions": []}, {"description": "any of", "match": false, "subconditions": [{"description": "check-neutral = Build and check", "match": false, "subconditions": []}, {"description": "check-skipped = Build and check", "match": false, "subconditions": []}, {"description": "check-success = Build and check", "match": false, "subconditions": []}]}, {"description": "any of", "match": false, "subconditions": [{"description": "check-neutral = Rust dependency policy", "match": false, "subconditions": []}, {"description": "check-skipped = Rust dependency policy", "match": false, "subconditions": []}, {"description": "check-success = Rust dependency policy", "match": false, "subconditions": []}]}, {"description": "any of", "match": false, "subconditions": [{"description": "check-neutral = PR owner", "match": false, "subconditions": []}, {"description": "check-skipped = PR owner", "match": false, "subconditions": []}, {"description": "check-success = PR owner", "match": false, "subconditions": []}]}, {"description": "-draft", "match": true, "subconditions": []}, {"description": "-label = pr-doctor:skip", "match": true, "subconditions": []}, {"description": "base = main", "match": true, "subconditions": []}, {"description": "any of", "match": true, "subconditions": [{"description": "check-success = merge-hold", "match": true, "subconditions": []}, {"description": "check-neutral = merge-hold", "match": false, "subconditions": []}, {"description": "check-skipped = merge-hold", "match": false, "subconditions": []}]}]}]}, {"description": "-closed", "match": true, "subconditions": []}, {"description": "-draft", "match": true, "subconditions": []}, {"description": "any of", "match": true, "subconditions": [{"description": "-mergify-configuration-changed", "match": true, "subconditions": []}, {"description": "check-success = @mergify/Configuration changed", "match": false, "subconditions": []}, {"description": "check-success = @mergify/Configuration has been deleted", "match": false, "subconditions": []}]}];
const WAITING_META = (required_conditions) => ({ version: 1, state: "waiting", queue_rule_name: null, queued_at: null, required_conditions });
// A labeled, green PR whose .mergify.yml change still lacks Mergify's "Configuration changed" check.
const CONFIG_TREE = [
  { description: "-closed", match: true, subconditions: [] },
  { description: "any of", match: false, subconditions: [
    { description: "-mergify-configuration-changed", match: false, subconditions: [] },
    { description: "check-success = @mergify/Configuration changed", match: false, subconditions: [] },
    { description: "check-success = @mergify/Configuration has been deleted", match: false, subconditions: [] },
  ] },
];

const NO_HOLD = status("merge-hold", "SUCCESS", "Retired: the Mergify queue replaced the final-stretch hold");
const GREEN = [
  done("Classify changes"),
  done("PR owner", "SUCCESS", { required: true, runId: 41500842845, workflow: "PR hygiene" }),
  done("Rust dependency policy", "SUCCESS", { required: true }),
  done("Dev tools", "SUCCESS", { runId: 41500842845, workflow: "PR hygiene" }),
  done("Docs and queue checks", "SKIPPED"),
  done("macOS / Build and check (macos-15)"),
  done("Windows / Build and check (windows-latest)"),
  done("Detect macOS runner starvation", "SKIPPED"),
  done("macOS fallback", "SKIPPED"),
  done("Build and check", "SUCCESS", { required: true }),
  NO_HOLD,
  // Mergify admitted the PR (it is labeled queue in the default fixture).
  mq("Queued · estimated merge in 25 minutes", { status: "IN_PROGRESS" }),
];
const NOT_ADMITTED = GREEN.filter((node) => node.name !== "Mergify Merge Queue");

/** Replaces checks by name in GREEN (or appends). */
function rollup(overrides = [], base = GREEN) {
  const names = new Set(overrides.map((node) => node.name ?? node.context));
  return [...base.filter((node) => !names.has(node.name ?? node.context)), ...overrides];
}

function graphql({ pr = {}, contexts = GREEN } = {}) {
  return {
    repository: {
      main: { target: { oid: "m".repeat(40) } },
      pullRequest: {
        number: 243, state: "OPEN", isDraft: false, mergeable: "MERGEABLE", mergeStateStatus: "CLEAN",
        headRefName: "feat/confluence-starter", headRefOid: "6629a6b3a95ba330a0a564eb5f22a0e9fe7a843a",
        autoMergeRequest: null,
        labels: { nodes: [{ name: "queue" }] },
        commits: { nodes: [{ commit: { statusCheckRollup: { contexts: { nodes: contexts } } } }] },
        ...pr,
      },
    },
  };
}
const snap = (opts) => snapshotFromGraphql(graphql(opts));
const NOW = "2026-11-02T20:00:00Z";

// --- parseArgs ------------------------------------------------------------------

test("parseArgs takes a PR number and defaults to waiting for the merge", () => {
  assert.deepEqual(parseArgs(["243"]), { until: "merged", interval: 120, once: false, maxMinutes: null, repo: null, pr: 243 });
  const opts = parseArgs(["#243", "--until", "green", "--interval", "60", "--once", "--repo", "o/r"]);
  assert.equal(opts.pr, 243);
  assert.equal(opts.until, "green");
  assert.equal(opts.interval, 60);
  assert.equal(opts.once, true);
  assert.equal(opts.repo, "o/r");
});

test("parseArgs refuses a missing PR, a bad --until and a hammering interval", () => {
  assert.throws(() => parseArgs([]), /missing PR/);
  assert.throws(() => parseArgs(["1", "--until", "queued"]), /--until/);
  assert.throws(() => parseArgs(["1", "--interval", "5"]), /at least 30/);
  assert.throws(() => parseArgs(["1", "--bogus"]), /unknown argument/);
});

// --- snapshot + latestChecks ---------------------------------------------------

test("snapshotFromGraphql reads the recorded field shapes", () => {
  const s = snap({ pr: { labels: { nodes: [{ name: "queue" }, { name: "queued" }] }, autoMergeRequest: { enabledAt: "2026-11-02T16:14:08Z" } } });
  assert.equal(s.number, 243);
  assert.equal(s.autoMerge, true);
  assert.equal(s.queueRequested, true);
  assert.deepEqual(s.queueBlockers, []);
  assert.deepEqual(s.labels, ["queue", "queued"]);
  assert.equal(snap({ pr: { labels: { nodes: [{ name: "queued" }] } } }).queueRequested, false);
  assert.deepEqual(snap({ pr: { labels: { nodes: [{ name: "queue" }, { name: "pr-doctor:skip" }] } } }).queueBlockers, ["pr-doctor:skip"]);
  assert.equal(analyzeChecks(s.checks).inQueue, true);
  assert.equal(analyzeChecks(snap({ contexts: NOT_ADMITTED }).checks).inQueue, false);
  assert.equal(s.mainOid, "m".repeat(40));
  const gate = s.checks.find((check) => check.name === "Build and check");
  assert.deepEqual([gate.state, gate.required, gate.runId], ["SUCCESS", true, 41500842838]);
});

test("latestChecks keeps the newest run of a repeated check name", () => {
  const checks = latestChecks([
    done("Merge hold", "FAILURE", { runId: 41500842462, workflow: "Merge hold" }),
    done("Merge hold", "SUCCESS", { runId: 41503934800, workflow: "Merge hold" }),
    done("Merge hold", "SUCCESS", { runId: 41503871045, workflow: "Merge hold" }),
  ]);
  assert.equal(checks.length, 1);
  assert.equal(checks[0].runId, 41503934800);
  assert.equal(checks[0].state, "SUCCESS");
});

test("latestChecks: a re-run in progress replaces the failed attempt", () => {
  const checks = latestChecks([
    done("Build and check", "FAILURE", { required: true, runId: 100 }),
    run("Build and check", "IN_PROGRESS", null, { required: true, runId: 200 }),
  ]);
  assert.deepEqual(checks.map((c) => [c.state, c.runId]), [["IN_PROGRESS", 200]]);
});

// --- decide: terminal states ------------------------------------------------------

test("merged exits 0; closed exits CLOSED", () => {
  assert.equal(decide(snap({ pr: { state: "MERGED" } })).exitCode, EXIT.DONE);
  assert.equal(decide(snap({ pr: { state: "CLOSED" } })).exitCode, EXIT.CLOSED);
});

test("--until green finishes as soon as required checks pass", () => {
  const d = decide(snap({ pr: { labels: { nodes: [] } } }), { until: "green" });
  assert.equal(d.exitCode, EXIT.DONE);
  assert.match(d.message, /not queued \(gh pr edit 243 --add-label queue\)/);
  assert.match(decide(snap(), { until: "green" }).message, /green and it is in the Mergify queue/);
  assert.match(decide(snap({ pr: { labels: { nodes: [{ name: "queue" }, { name: "needs-merge" }] } } }), { until: "green" }).message,
    /labeled queue but needs-merge keeps it out of the queue/);
});

test("--until green waits while the aggregate gate has not reported (PR #264, Codex P2)", () => {
  const contexts = rollup([
    done("PR owner", "SUCCESS", { required: true }),
    done("Classify changes", "SUCCESS"),
    done("Windows / Build and check (windows-latest)", "FAILURE"),
    run("macOS / Build and check (macos-15)", "IN_PROGRESS", null),
  ]);
  const d = decide(snap({ contexts }), { until: "green" });
  assert.notEqual(d.action, "green");
  assert.equal(d.exitCode, EXIT.CHECK_FAILED);
  const pendingOnly = rollup([done("PR owner", "SUCCESS", { required: true }), run("macOS / Build and check (macos-15)", "IN_PROGRESS", null)]);
  assert.equal(decide(snap({ contexts: pendingOnly }), { until: "green" }).done, false);
});

test("a draft says how to mark it ready", () => {
  const d = decide(snap({ pr: { isDraft: true, mergeStateStatus: "DRAFT" } }));
  assert.equal(d.exitCode, EXIT.DRAFT);
  assert.match(d.message, /gh pr ready 243/);
});

// --- decide: waiting states never advise merging main ------------------------------

for (const [name, opts, expect] of [
  ["BEHIND with checks pending", { pr: { mergeStateStatus: "BEHIND" }, contexts: rollup([run("macOS / Build and check (macos-15)", "IN_PROGRESS", null), run("Build and check", "QUEUED", null, { required: true })]) }, /waiting on macOS .*BEHIND main: that is fine, do not merge main in/],
  ["BEHIND, green, queued", { pr: { mergeStateStatus: "BEHIND" } }, /in the Mergify queue \(BEHIND\).*do not merge main in/],
  ["queued while Mergify tests its draft", {}, /in the Mergify queue \(CLEAN\)/],
  ["green and labeled, Mergify not yet admitted", { contexts: NOT_ADMITTED }, /labeled queue; waiting for Mergify to admit it/],
  ["BLOCKED only by pending checks", { pr: { mergeStateStatus: "BLOCKED" }, contexts: rollup([run("Build and check", "IN_PROGRESS", null, { required: true })]) }, /waiting on Build and check/],
  ["no checks reported yet", { pr: { mergeStateStatus: "BLOCKED" }, contexts: [] }, /waiting on checks to start/],
]) {
  test(`waits: ${name}`, () => {
    const d = decide(snap(opts));
    assert.equal(d.done, false);
    assert.equal(d.exitCode, EXIT.WAITING);
    assert.match(d.message, expect);
    assert.doesNotMatch(d.message, /git merge origin\/main/);
  });
}

test("a pending merge-hold (retired, always green once posted) is an ordinary pending check", () => {
  const contexts = rollup([status("merge-hold", "PENDING", "")]);
  const d = decide(snap({ pr: { mergeStateStatus: "BLOCKED" }, contexts }));
  assert.equal(d.done, false);
  assert.match(d.message, /waiting on merge-hold/);
});

test("a failed Mergify queue run on a queued PR says it was dequeued", () => {
  const contexts = rollup([mq("The pull request has been removed from the queue", { conclusion: "FAILURE" })]);
  const d = decide(snap({ contexts }));
  assert.equal(d.exitCode, EXIT.CHECK_FAILED);
  assert.equal(d.action, "dequeued");
  assert.match(d.message, /@mergifyio requeue/);
  assert.doesNotMatch(statusLine(snap({ contexts }), d), /non-blocking failures/);
});

// --- decide: macOS starvation and cancellations -------------------------------------

test("macos-15 cancelled while the starvation check runs: keep waiting, fallback expected", () => {
  const contexts = rollup([
    done("macOS / Build and check (macos-15)", "CANCELLED"),
    run("Detect macOS runner starvation", "IN_PROGRESS", null),
    run("Build and check", "QUEUED", null, { required: true }),
  ]);
  const d = decide(snap({ pr: { mergeStateStatus: "BLOCKED" }, contexts }));
  assert.equal(d.done, false);
  assert.match(d.message, /runner starvation: the Intel fallback is expected/);
});

test("macos-15 starved and the Intel fallback running: keep waiting", () => {
  const contexts = rollup([
    done("macOS / Build and check (macos-15)", "CANCELLED"),
    done("Detect macOS runner starvation"),
    done("macOS fallback", "SKIPPED"),
    run("macOS fallback / Build and check (macos-15-intel)", "IN_PROGRESS", null),
    run("Build and check", "QUEUED", null, { required: true }),
  ]);
  const checks = latestChecks(contexts);
  assert.equal(analyzeChecks(checks).starved, true);
  const d = decide(snap({ pr: { mergeStateStatus: "BLOCKED" }, contexts }));
  assert.equal(d.done, false);
  assert.match(d.message, /Intel fallback is expected/);
});

test("starved macOS with a passing fallback and a green gate is green", () => {
  const contexts = rollup([
    done("macOS / Build and check (macos-15)", "CANCELLED"),
    done("Detect macOS runner starvation"),
    done("macOS fallback / Build and check (macos-15-intel)"),
  ]);
  assert.equal(decide(snap({ contexts })).action, "queued:CLEAN");
});

test("macOS cancelled without starvation (recorded run 41553653914): rerun the cancelled job", () => {
  const contexts = rollup([
    done("macOS / Build and check (macos-15)", "CANCELLED", { runId: 41553653914 }),
    done("Detect macOS runner starvation", "SUCCESS", { runId: 41553653914 }),
    done("macOS fallback", "SKIPPED", { runId: 41553653914 }),
    done("Build and check", "FAILURE", { required: true, runId: 41553653914 }),
  ]);
  const d = decide(snap({ pr: { mergeStateStatus: "BLOCKED" }, contexts }));
  assert.equal(d.exitCode, EXIT.CANCELLED);
  assert.match(d.message, /macOS \/ Build and check \(macos-15\) was cancelled/);
  assert.match(d.message, /gh run rerun 41553653914 --failed/);
});

// --- decide: failures ------------------------------------------------------------------

test("a real macOS failure (recorded run 41571195112) names the job and the log command", () => {
  const contexts = rollup([
    done("macOS / Build and check (macos-15)", "FAILURE", { runId: 41571195112 }),
    done("Build and check", "FAILURE", { required: true, runId: 41571195112 }),
  ]);
  const d = decide(snap({ pr: { mergeStateStatus: "BLOCKED" }, contexts }));
  assert.equal(d.exitCode, EXIT.CHECK_FAILED);
  assert.match(d.message, /failed: macOS \/ Build and check \(macos-15\) \(also Build and check\)/);
  assert.match(d.message, /gh run view 41571195112 --log-failed/);
});

test("a gate input failing is reported before the gate itself finishes", () => {
  const contexts = rollup([
    done("Windows / Build and check (windows-latest)", "FAILURE", { runId: 7 }),
    run("Build and check", "QUEUED", null, { required: true }),
  ]);
  const d = decide(snap({ pr: { mergeStateStatus: "BLOCKED" }, contexts }));
  assert.equal(d.exitCode, EXIT.CHECK_FAILED);
  assert.match(d.message, /gh run view 7 --log-failed/);
});

test("PR owner failure points at the PR template, not a log", () => {
  const contexts = rollup([done("PR owner", "FAILURE", { required: true, runId: 9, workflow: "PR hygiene" })]);
  const d = decide(snap({ pr: { mergeStateStatus: "BLOCKED" }, contexts }));
  assert.equal(d.exitCode, EXIT.CHECK_FAILED);
  assert.match(d.message, /Owner: line from \.github\/pull_request_template\.md/);
});

test("a failing non-required check outside the gate does not stop the watch", () => {
  const contexts = rollup([done("Review new dependencies", "FAILURE")]);
  const s = snap({ contexts });
  assert.equal(decide(s).action, "queued:CLEAN");
  assert.match(statusLine(s, decide(s)), /non-blocking failures: Review new dependencies/);
});

// --- Mergify queue membership: the check's title, never its status ---------------------

test("unmetConditions reads PR #268's real condition tree, without branch-protection repeats", () => {
  assert.deepEqual(unmetConditions(TREE_268), [
    "-conflict", "-label = needs-merge", "check-success = Build and check", "check-success = PR owner",
    "check-success = Rust dependency policy", "label = queue",
  ]);
  assert.deepEqual(unmetConditions(CONFIG_TREE),
    ["-mergify-configuration-changed or check-success = @mergify/Configuration changed or check-success = @mergify/Configuration has been deleted"]);
});

test("mergifyMeta decodes the embedded state and tolerates its absence", () => {
  const check = latestChecks([mq("Waiting for queue conditions to match", { meta: WAITING_META(TREE_268) })])[0];
  assert.equal(mergifyMeta(check.summary).state, "waiting");
  assert.equal(mergifyMeta("no span here"), null);
  assert.equal(mergifyMeta('<span itemprop="mergify.v1.!!!"></span>'), null);
});

for (const [title, opts, phase] of [
  ["Merge queue is ready", {}, "ready"],
  ["Waiting for queue conditions to match", {}, "waiting"],
  ["Waiting for queue conditions", { status: "IN_PROGRESS" }, "waiting"],
  ["Queued · estimated merge in 25 minutes", { status: "IN_PROGRESS" }, "queued"],
  ["Running merge queue checks", { status: "IN_PROGRESS" }, "queued"],
  ["Merged via merge queue", { conclusion: "SUCCESS" }, "merged"],
  ["The pull request has been removed from the queue", { conclusion: "FAILURE" }, "dequeued"],
  ["Something new", { status: "IN_PROGRESS" }, "unknown"],
]) {
  test(`Mergify title "${title}" is phase ${phase}`, () => {
    const status = mergifyQueueStatus(latestChecks([mq(title, opts)])[0]);
    assert.equal(status.phase, phase);
    assert.equal(analyzeChecks(latestChecks([mq(title, opts)])).inQueue, phase === "queued");
  });
}

test("the embedded state outranks an unfamiliar title", () => {
  const queued = mq("Some future wording", { status: "IN_PROGRESS", meta: { version: 1, state: "queued", queued_at: "2026-11-03T16:00:00Z" } });
  assert.equal(mergifyQueueStatus(latestChecks([queued])[0]).phase, "queued");
  const waiting = mq("Some future wording", { meta: WAITING_META(CONFIG_TREE) });
  assert.equal(mergifyQueueStatus(latestChecks([waiting])[0]).phase, "waiting");
});

test("a pending Mergify check that is only evaluating conditions is not queue membership", () => {
  const contexts = rollup([mq("Waiting for queue conditions", { status: "IN_PROGRESS" })]);
  const d = decide(snap({ contexts }));
  assert.equal(d.action, "awaiting-admission");
  assert.doesNotMatch(statusLine(snap({ contexts }), d), /in queue/);
});

test('"Merge queue is ready" on an unlabeled green PR: not queued, advise the label', () => {
  const d = decide(snap({ pr: { labels: { nodes: [] } }, contexts: rollup([mq("Merge queue is ready")]) }));
  assert.equal(d.exitCode, EXIT.ENQUEUE);
});

test('labeled and green but stuck at "Waiting for queue conditions to match": exit 16 naming the unmet condition', () => {
  const contexts = rollup([mq("Waiting for queue conditions to match", { meta: WAITING_META(CONFIG_TREE) })]);
  const early = decide(snap({ contexts }), { now: NOW, admitWaitSince: NOW });
  assert.equal(early.action, "awaiting-admission");
  assert.match(early.message, /Unmet: `-mergify-configuration-changed or check-success = @mergify\/Configuration changed/);
  const since = new Date(Date.parse(NOW) - (CONDITIONS_GRACE_MINUTES + 1) * 60000).toISOString();
  const d = decide(snap({ contexts }), { now: NOW, admitWaitSince: since });
  assert.equal(d.exitCode, EXIT.QUEUE_BLOCKED);
  assert.equal(d.action, "queue-conditions-unmet");
  assert.match(d.message, /still says "Waiting for queue conditions to match"/);
  assert.match(d.message, /@mergify\/Configuration changed/);
  assert.match(d.message, /@Mergifyio queue/);
});

test("stuck waiting with no embedded state falls back to the summary's unchecked lines", () => {
  // An unmet check the rollup does not have (a passed one is stale Mergify state: refresh).
  const markdown = "### Queue entry conditions\n\n- [X] `base = main`\n- [ ] `check-success = @mergify/Configuration changed`\n";
  const contexts = rollup([mq("Waiting for queue conditions to match", { markdown })]);
  const since = new Date(Date.parse(NOW) - (CONDITIONS_GRACE_MINUTES + 1) * 60000).toISOString();
  const d = decide(snap({ contexts }), { now: NOW, admitWaitSince: since });
  assert.equal(d.action, "queue-conditions-unmet");
  assert.match(d.message, /Unmet: `check-success = @mergify\/Configuration changed`\./);
});

test("queued and running titles keep the watcher waiting in the queue", () => {
  for (const title of ["Queued · estimated merge in 25 minutes", "Running merge queue checks"]) {
    const d = decide(snap({ contexts: rollup([mq(title, { status: "IN_PROGRESS" })]) }));
    assert.equal(d.action, "queued:CLEAN");
  }
});

// --- decide: enqueueing ------------------------------------------------------------------

test("GitHub auto-merge armed is an action: it would merge around the queue", () => {
  const armed = { enabledAt: "2026-11-02T16:14:08Z" };
  const unlabeled = decide(snap({ pr: { labels: { nodes: [] }, autoMergeRequest: armed } }));
  assert.equal(unlabeled.exitCode, EXIT.ENQUEUE);
  assert.equal(unlabeled.action, "auto-merge-armed");
  assert.match(unlabeled.message, /gh pr merge --disable-auto 243 && gh pr edit 243 --add-label queue/);
  // Pending checks are no excuse: the merge happens the moment they pass.
  const pending = rollup([run("Build and check", "IN_PROGRESS", null, { required: true })]);
  assert.equal(decide(snap({ pr: { autoMergeRequest: armed }, contexts: pending })).action, "auto-merge-armed");
  assert.match(decide(snap({ pr: { autoMergeRequest: armed } })).message, /--disable-auto 243$/);
});

for (const label of ["pr-doctor:skip", "needs-merge"]) {
  test(`labeled queue plus ${label}: Mergify never admits it, so act naming the label`, () => {
    const s = snap({ pr: { labels: { nodes: [{ name: "queue" }, { name: label }] } }, contexts: NOT_ADMITTED });
    const d = decide(s);
    assert.equal(d.exitCode, EXIT.QUEUE_BLOCKED);
    assert.equal(d.action, `queue-blocked:${label}`);
    assert.match(d.message, new RegExp(`carries ${label}`));
    assert.match(d.message, new RegExp(`gh pr edit 243 --remove-label ${label}`));
    assert.notEqual(d.action, "awaiting-admission");
    assert.match(statusLine(s, d), new RegExp(`blocked by ${label}`));
  });
}

test('labeled, green and stuck at "Merge queue is ready": advise the contract\'s queue comment', () => {
  // Label-only auto-queue needs Mergify's Merge Protections activated in the
  // dashboard; without them the PR sits at "ready" until someone comments.
  const contexts = rollup([mq("Merge queue is ready")]);
  assert.equal(decide(snap({ contexts }), { now: NOW, admitWaitSince: NOW }).action, "awaiting-admission");
  const since = new Date(Date.parse(NOW) - (ADMIT_GRACE_MINUTES + 1) * 60000).toISOString();
  const d = decide(snap({ contexts }), { now: NOW, admitWaitSince: since });
  assert.equal(d.exitCode, EXIT.QUEUE_BLOCKED);
  assert.equal(d.action, "queue-ready-not-admitted");
  assert.match(d.message, /gh pr comment 243 --body "@Mergifyio queue"/);
  assert.match(d.message, /Merge Protections/);
});

// What Mergify really reports for a labeled, green PR it has not queued
// (#265, 2026-11-04): the "ready" title, but an embedded state of "waiting"
// with nothing unmet. The title must win, or the comment advice never shows.
const READY_REAL = () => mq("Merge queue is ready", {
  meta: WAITING_META([]),
  markdown: "This pull request can be added to the merge queue.\n\nUse `@Mergifyio queue` or the [Mergify dashboard](https://dashboard.mergify.com) to queue this pull request for merging.\n",
});

// The "Waiting for queue conditions" check Mergify left on #329 and #343
// (2026-11-09): every required check had passed, but Mergify missed those
// check-run completions and still listed them unchecked until a refresh.
const STALE_WAITING_MARKDOWN = [
  "### ⏳ Waiting for queue conditions",
  "",
  "This pull request is waiting for the following conditions to be met before it can enter the queue:",
  "",
  "- [ ] `check-success = Build and check`",
  "- [ ] `check-success = PR owner`",
  "- [ ] `check-success = Rust dependency policy`",
  "- [ ] any of · required by GitHub branch protection:",
  "  - [ ] `check-neutral = Build and check`",
  "  - [ ] `check-skipped = Build and check`",
  "  - [ ] `check-success = Build and check`",
  "- [x] `-draft`",
  "- [x] `base = main`",
  "- [x] `label = queue`",
  "- [x] `-label = needs-merge`",
  "- [x] `-label = pr-doctor:skip`",
  "",
].join("\n");
const STALE_WAITING = () => mq("Waiting for queue conditions", { markdown: STALE_WAITING_MARKDOWN });
const AFTER_CONDITIONS_GRACE = new Date(Date.parse(NOW) - (CONDITIONS_GRACE_MINUTES + 1) * 60000).toISOString();

test("Mergify still waiting on checks that already passed: advise @Mergifyio refresh (#329, #343)", () => {
  const d = decide(snap({ contexts: rollup([STALE_WAITING()]) }), { now: NOW, admitWaitSince: AFTER_CONDITIONS_GRACE });
  assert.equal(d.exitCode, EXIT.QUEUE_BLOCKED);
  assert.equal(d.action, "queue-stale-refresh");
  assert.match(d.message, /gh pr comment 243 --body "@Mergifyio refresh"/);
  assert.match(d.message, /missed/);
  assert.match(d.message, /Build and check/);
});

test("staleQueueConditions: unmet check conditions Mergify lists although the rollup has them passed", () => {
  const status = mergifyQueueStatus(latestChecks([STALE_WAITING()])[0]);
  assert.deepEqual(status.unmet, ["check-success = Build and check", "check-success = PR owner", "check-success = Rust dependency policy",
    "check-neutral = Build and check", "check-skipped = Build and check"]);
  assert.deepEqual(staleQueueConditions(status.unmet, latestChecks(GREEN)), status.unmet);
  const running = latestChecks(rollup([run("Rust dependency policy", "IN_PROGRESS", null, { required: true })]));
  assert.deepEqual(staleQueueConditions(status.unmet, running), []);
  // Not only checks, or a check the rollup lacks: Mergify is right to wait.
  assert.deepEqual(staleQueueConditions([...status.unmet, "label = queue"], latestChecks(GREEN)), []);
  assert.deepEqual(staleQueueConditions(["check-success = @mergify/Configuration changed"], latestChecks(GREEN)), []);
  // A skipped check does not satisfy check-success.
  const skipped = latestChecks(rollup([done("PR owner", "SKIPPED", { required: true })]));
  assert.deepEqual(staleQueueConditions(["check-success = PR owner"], skipped), []);
});

test("Mergify waiting on a check that is still running: no refresh advice", () => {
  const contexts = rollup([STALE_WAITING(), run("Rust dependency policy", "IN_PROGRESS", null, { required: true })]);
  const d = decide(snap({ contexts }), { now: NOW, admitWaitSince: AFTER_CONDITIONS_GRACE });
  assert.equal(d.exitCode, EXIT.WAITING);
  assert.match(d.action, /^pending:/);
  assert.doesNotMatch(d.message, /refresh/i);
});

test('a real-shaped "Merge queue is ready" check with a "waiting" payload is phase ready', () => {
  const check = latestChecks([READY_REAL()])[0];
  assert.equal(mergifyQueueStatus(check).phase, "ready");
});

test('labeled, green, stuck at a real-shaped "ready" check: advise the queue comment, not the conditions', () => {
  const contexts = rollup([READY_REAL()]);
  const since = new Date(Date.parse(NOW) - (ADMIT_GRACE_MINUTES + 1) * 60000).toISOString();
  const d = decide(snap({ contexts }), { now: NOW, admitWaitSince: since });
  assert.equal(d.action, "queue-ready-not-admitted");
  assert.match(d.message, /gh pr comment 243 --body "@Mergifyio queue"/);
});

test("the queue comment pr-watch advises is the worker contract's merge.comment", async () => {
  const { readFileSync } = await import("node:fs");
  const contract = JSON.parse(readFileSync(new URL("../../worker-contract.json", import.meta.url), "utf8"));
  assert.equal(QUEUE_COMMENT, contract.merge.comment);
});

test("green and labeled but never admitted: act after the admission grace period", () => {
  const s = snap({ contexts: NOT_ADMITTED });
  assert.equal(decide(s, { now: NOW, admitWaitSince: NOW }).done, false);
  const since = new Date(Date.parse(NOW) - (ADMIT_GRACE_MINUTES + 1) * 60000).toISOString();
  const d = decide(s, { now: NOW, admitWaitSince: since });
  assert.equal(d.exitCode, EXIT.QUEUE_BLOCKED);
  assert.equal(d.action, "queue-not-admitted");
  assert.match(d.message, /Mergify app is installed/);
});

test("green, unlabeled and without auto-merge advises the queue label", () => {
  const d = decide(snap({ pr: { labels: { nodes: [] } } }));
  assert.equal(d.exitCode, EXIT.ENQUEUE);
  assert.match(d.message, /gh pr edit 243 --add-label queue/);
  assert.equal(enqueueCommand(snap()), "gh pr edit 243 --add-label queue");
});

// --- DIRTY: pr-doctor first --------------------------------------------------------------

const dirty = (pr = {}) => snap({ pr: { mergeStateStatus: "DIRTY", mergeable: "CONFLICTING", ...pr } });
const doctorRun = (createdAt, status = "completed") => ({ databaseId: 41575562712, status, conclusion: "success", createdAt, updatedAt: createdAt });

test("classifyConflicts uses pr-doctor's own scope", () => {
  const appended = "# Feed\n<<<<<<< HEAD\n## A\n||||||| 1234\n=======\n## B\n>>>>>>> x\n";
  const edited = "<<<<<<< HEAD\nA\n||||||| 1234\nbase\n=======\nB\n>>>>>>> x\n";
  assert.deepEqual(classifyConflicts([
    { path: ".beads/issues.jsonl" }, { path: "BOSUN_FEATURE_BACKLOG.md" },
    { path: "BRAUSER_IMPROVEMENT_FEED.md", text: appended }, { path: "README.md", text: edited },
    { path: "host/src/config.rs" }, { path: "TESTING.md" },
  ]), {
    doctor: [".beads/issues.jsonl", "BOSUN_FEATURE_BACKLOG.md", "BRAUSER_IMPROVEMENT_FEED.md"],
    person: ["README.md", "host/src/config.rs"],
    unknown: ["TESTING.md"],
  });
});

test("DIRTY in bookkeeping only: wait for pr-doctor, never merge main yet", () => {
  const conflicts = { doctor: [".beads/issues.jsonl"], person: [], unknown: [] };
  const d = decide(dirty(), { now: NOW, conflict: { dirtySince: NOW, doctorRuns: [doctorRun("2026-11-02T19:40:51Z")], conflicts } });
  assert.equal(d.done, false);
  assert.match(d.message, /only in files pr-doctor resolves \(\.beads\/issues\.jsonl\)/);
  assert.match(d.message, /do not merge main yet/);
});

test("DIRTY with an active doctor run says so", () => {
  const d = decideConflict(dirty(), { now: NOW, dirtySince: NOW, doctorRuns: [doctorRun(NOW, "in_progress")] });
  assert.match(d.message, /pr-doctor run 41575562712 is in_progress/);
});

test("DIRTY outside the doctor's scope: merge main yourself at once", () => {
  const conflicts = { doctor: [".beads/issues.jsonl"], person: ["host/src/config.rs"], unknown: [] };
  const d = decide(dirty(), { now: NOW, conflict: { dirtySince: NOW, doctorRuns: [], conflicts } });
  assert.equal(d.exitCode, EXIT.CONFLICT);
  assert.match(d.message, /does not resolve \(host\/src\/config\.rs\)/);
  assert.match(d.message, /git merge origin\/main/);
});

test("DIRTY and labeled needs-merge: merge main yourself, listing the doctor's files", () => {
  const comment = doctorComment([{ body: "<!-- pr-doctor -->\n<!-- pr-doctor outcome=needs-person -->\n**PR doctor**\n\nThe files:\n\n- `host/src/lib.rs`\n\nMerge" }]);
  assert.deepEqual(comment.files, ["host/src/lib.rs"]);
  const d = decide(dirty({ labels: { nodes: [{ name: "needs-merge" }] } }), { now: NOW, conflict: { comment } });
  assert.equal(d.exitCode, EXIT.CONFLICT);
  assert.match(d.message, /labeled it needs-merge.*host\/src\/lib\.rs/);
});

test("DIRTY after a doctor run that started after the conflict appeared: act", () => {
  const d = decideConflict(dirty(), {
    now: NOW, dirtySince: "2026-11-02T19:30:00Z", doctorRuns: [doctorRun("2026-11-02T19:40:00Z")],
    conflicts: { doctor: ["BOSUN_FEATURE_BACKLOG.md"], person: [], unknown: [] },
  });
  assert.equal(d.exitCode, EXIT.CONFLICT);
  assert.match(d.action, /doctor-passed/);
});

test("DIRTY a whole doctor grace period with no repair: act", () => {
  const since = new Date(Date.parse(NOW) - (DOCTOR_GRACE_MINUTES + 1) * 60000).toISOString();
  const d = decideConflict(dirty(), { now: NOW, dirtySince: since, doctorRuns: [] });
  assert.equal(d.exitCode, EXIT.CONFLICT);
  assert.match(d.action, /doctor-timeout/);
});

test("a real check failure outranks DIRTY and mentions the conflict", () => {
  const contexts = rollup([done("Rust dependency policy", "FAILURE", { required: true, runId: 5 })]);
  const d = decide(snap({ pr: { mergeStateStatus: "DIRTY", mergeable: "CONFLICTING" }, contexts }), { now: NOW });
  assert.equal(d.exitCode, EXIT.CHECK_FAILED);
  assert.match(d.message, /also conflicts with main/);
});

test("a failed Accessibility job blocks like the platform builds (PR #275)", () => {
  const contexts = rollup([
    done("PR owner", "SUCCESS", { required: true }),
    done("Accessibility", "FAILURE"),
    run("Windows / Build and check (windows-latest)", "IN_PROGRESS", null),
  ]);
  const d = decide(snap({ contexts }));
  assert.equal(d.exitCode, EXIT.CHECK_FAILED);
  assert.match(d.message, /Accessibility/);
});
