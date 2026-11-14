// Tests for scripts/dev/pr-fixer-report.mjs: receipt parsing (v1 and v2), the
// summary and its success measure, run pruning on a temporary work root, and
// the CLI's JSON output. Nothing here reads the real fixer state.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { fateOfPush, parseReceipts, parseSince, percentile, pruneRuns, summarizeReceipts } from "../dev/pr-fixer-report.mjs";

const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), "..", "dev", "pr-fixer-report.mjs");
const DAY = 86_400_000;
const NOW = Date.parse("2026-11-14T12:00:00Z");
const at = (hoursAgo) => new Date(NOW - hoursAgo * 3_600_000).toISOString();
const repair = (over = {}) => ({
  v: 2, kind: "repair", at: at(10), bead: "brwr-a", pr: 7, attempt: 1, maxAttempts: 2, outcome: "pushed", code: "ok", reason: "",
  files: [], changedFiles: 1, agent: "codex", agentVersion: "codex 1.0", tokens: null,
  durationsMs: { agent: null, review: null, total: null }, review: null, sha: "s1", exhausted: false, ...over,
});
const outcome = (over = {}) => ({ v: 2, kind: "outcome", at: at(5), bead: "brwr-a", pr: 7, result: "merged", pushedSha: "s1", attemptsUsed: 1, hoursSincePush: 5, ...over });
const pass = (over = {}) => ({ v: 2, kind: "pass", at: at(10), outcome: "idle", closed: 0, queued: 0, durationMs: 1000, ...over });
const tmp = () => mkdtempSync(join(tmpdir(), "pr-fixer-report-"));

test("parseReceipts keeps v1 and v2 lines and counts the bad ones", () => {
  const text = [
    JSON.stringify({ at: at(30), outcome: "pushed", bead: "brwr-old", pr: 3, sha: "abc" }),
    JSON.stringify(pass()),
    "",
    "not json",
    JSON.stringify([1, 2]),
    JSON.stringify({ outcome: "idle" }),
    JSON.stringify({ at: "yesterday-ish", outcome: "idle" }),
    JSON.stringify({ v: 3, kind: "pass", at: at(1) }),
    JSON.stringify({ v: 2, kind: "mystery", at: at(1) }),
    JSON.stringify({ at: at(1) }),
    JSON.stringify(repair()),
    JSON.stringify(outcome()),
  ].join("\n");
  const { entries, skipped } = parseReceipts(text);
  assert.equal(skipped, 7);
  assert.deepEqual(entries.map((e) => [e.v, e.kind]), [[1, "pass"], [2, "pass"], [2, "repair"], [2, "outcome"]]);
  assert.equal(entries[0].bead, "brwr-old");
  assert.deepEqual(parseReceipts(""), { entries: [], skipped: 0 });
});

test("success counts a merge only when no re-conflict came between push and merge, and reports pending", () => {
  const entries = [
    repair({ bead: "b-ok", sha: "s-ok", at: at(20) }),
    outcome({ bead: "b-ok", pushedSha: "s-ok", at: at(10) }),
    repair({ bead: "b-rc", sha: "s-rc", at: at(20) }),
    outcome({ bead: "b-rc", pushedSha: "s-rc", result: "reconflicted", at: at(15) }),
    outcome({ bead: "b-rc", pushedSha: "s-rc", result: "merged", at: at(8) }),
    repair({ bead: "b-closed", sha: "s-c", at: at(20) }),
    outcome({ bead: "b-closed", pushedSha: "s-c", result: "closed-unmerged", at: at(12) }),
    repair({ bead: "b-wait", sha: "s-w", at: at(2) }),
    outcome({ bead: "b-wait", pushedSha: "s-w", result: "requeue-failed", at: at(1) }),
    // Re-conflicted, repaired again with a new push, then merged: the second push succeeds.
    repair({ bead: "b-two", sha: "t1", at: at(30) }),
    outcome({ bead: "b-two", pushedSha: "t1", result: "reconflicted", at: at(25) }),
    repair({ bead: "b-two", sha: "t2", attempt: 2, at: at(24) }),
    outcome({ bead: "b-two", pushedSha: "t2", result: "merged", at: at(20) }),
    // An outcome from before the push does not count for it.
    repair({ bead: "b-early", sha: "e1", at: at(5) }),
    outcome({ bead: "b-early", pushedSha: "e1", at: at(6) }),
  ];
  const s = summarizeReceipts(entries, { since: "all", now: NOW }).success;
  assert.deepEqual(s, { pushed: 7, succeeded: 2, reconflicted: 2, closedUnmerged: 1, pending: 2, decided: 5, rate: 2 / 5 });
  assert.equal(fateOfPush(repair({ bead: "x", sha: "q" }), []), "pending");
  assert.equal(summarizeReceipts([], {}).success.rate, null);
});

test("summarizeReceipts counts passes, repairs, failure codes, tokens, files and outcomes", () => {
  const entries = [
    { v: 1, kind: "pass", at: at(3), outcome: "pushed" },
    pass({ outcome: "idle" }),
    pass({ outcome: "refused", code: "sandbox-refused" }),
    pass({ outcome: "claim-refused", code: "claim-refused" }),
    pass({ outcome: "failed" }),
    pass({ outcome: "failed" }),
    pass({ outcome: "pushed" }),
    repair({ outcome: "failed", code: "checks-failed", sha: undefined, files: ["a.js", "b.js"], tokens: 1000 }),
    repair({ outcome: "failed", code: "checks-failed", sha: undefined, files: ["a.js"], tokens: 500, exhausted: true }),
    repair({ outcome: "failed", code: "agent-timeout", sha: undefined, files: ["c.js"], tokens: null }),
    repair({ files: ["a.js", "c.js"], tokens: 250 }),
    outcome({ result: "reconflicted", at: at(9) }),
    outcome({ result: "requeue-failed", bead: "brwr-z", at: at(9) }),
    outcome({ result: "requeue-failed", bead: "brwr-y", at: at(9) }),
  ];
  const r = summarizeReceipts(entries, { since: "all", now: NOW });
  assert.deepEqual(r.passes, { total: 7, v1: 1, byOutcome: { pushed: 2, idle: 1, refused: 1, "claim-refused": 1, failed: 2 } });
  assert.deepEqual(r.repairs, { attempted: 4, pushed: 1, failed: 3, exhausted: 1 });
  assert.deepEqual(r.failures, [
    { code: "checks-failed", count: 2 }, { code: "agent-timeout", count: 1 }, { code: "claim-refused", count: 1 }, { code: "sandbox-refused", count: 1 },
  ]);
  assert.deepEqual(r.tokens, { total: 1750, repairsWithTokens: 3 });
  assert.deepEqual(r.topFiles, [{ file: "a.js", count: 3 }, { file: "c.js", count: 2 }, { file: "b.js", count: 1 }]);
  assert.equal(r.outcomes.reconflicted, 1);
  assert.equal(r.outcomes.requeueFailed, 2);
  assert.equal(r.success.reconflicted, 1);
});

test("durations use nearest-rank median and p90, ignoring steps that did not run", () => {
  assert.equal(percentile([], 0.5), null);
  assert.equal(percentile([5], 0.9), 5);
  assert.equal(percentile([1, 2, 3, 4], 0.5), 2);
  assert.equal(percentile([10, 1, 9, 2, 8, 3, 7, 4, 6, 5], 0.9), 9);
  assert.equal(percentile([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11], 0.9), 10);
  const entries = [10, 20, 30, 40, 50].map((s, i) => repair({ durationsMs: { agent: s * 1000, review: i < 2 ? 1000 * (i + 1) : null, total: s * 2000 } }));
  const d = summarizeReceipts(entries, { since: "all" }).durations;
  assert.deepEqual(d.agent, { n: 5, medianMs: 30_000, p90Ms: 50_000 });
  assert.deepEqual(d.review, { n: 2, medianMs: 1000, p90Ms: 2000 });
  assert.deepEqual(d.total, { n: 5, medianMs: 60_000, p90Ms: 100_000 });
});

test("since limits passes and repairs but reads a push's fate from every outcome", () => {
  const entries = [
    pass({ at: at(24 * 40), outcome: "failed" }),
    repair({ at: at(24 * 40), outcome: "failed", code: "push-failed", sha: undefined }),
    pass({ at: at(24 * 2), outcome: "pushed" }),
    repair({ at: at(24 * 2), sha: "n1" }),
    outcome({ at: at(1), pushedSha: "n1" }),
  ];
  const r = summarizeReceipts(entries, { since: "30d", now: NOW });
  assert.equal(r.since, new Date(NOW - 30 * DAY).toISOString());
  assert.deepEqual(r.passes.byOutcome, { pushed: 1 });
  assert.deepEqual(r.repairs, { attempted: 1, pushed: 1, failed: 0, exhausted: 0 });
  assert.deepEqual(r.failures, []);
  assert.equal(r.success.rate, 1);
  assert.equal(summarizeReceipts(entries, { since: "all" }).repairs.attempted, 2);
  assert.equal(summarizeReceipts(entries, { since: "2026-11-01T00:00:00Z" }).repairs.attempted, 1);
  assert.equal(parseSince("12h", NOW), NOW - DAY / 2);
  assert.throws(() => parseSince("soon"), /bad --since/);
});

test("pruneRuns removes old run folders only, keeps receipts and refuses symlinks", () => {
  const root = tmp();
  try {
    const runs = join(root, "runs");
    mkdirSync(runs, { recursive: true });
    writeFileSync(join(root, "receipts.jsonl"), "{}\n");
    const old = new Date(NOW - 40 * DAY);
    for (const name of ["old-1", "old-2", "new-1"]) {
      mkdirSync(join(runs, name));
      writeFileSync(join(runs, name, "run.log"), "x");
    }
    utimesSync(join(runs, "old-1"), old, old);
    utimesSync(join(runs, "old-2"), old, old);
    writeFileSync(join(runs, "stray.txt"), "keep");
    utimesSync(join(runs, "stray.txt"), old, old);
    const outside = join(root, "outside");
    mkdirSync(outside);
    writeFileSync(join(outside, "precious"), "keep");
    utimesSync(outside, old, old);
    symlinkSync(outside, join(runs, "link-out"));

    const result = pruneRuns(root, { keepDays: 30, now: NOW });
    assert.deepEqual(result.removed.sort(), ["old-1", "old-2"]);
    assert.deepEqual(result.refused, ["link-out"]);
    assert.ok(existsSync(join(runs, "new-1", "run.log")));
    assert.ok(existsSync(join(runs, "stray.txt")));
    assert.ok(existsSync(join(outside, "precious")));
    assert.ok(existsSync(join(root, "receipts.jsonl")));
    assert.deepEqual(pruneRuns(root, { keepDays: 30, now: NOW }).removed, []);
    // Under a day could remove a pass that is still running (pruning precedes the lock).
    for (const keepDays of [-1, 0, 0.5, Number.NaN]) assert.throws(() => pruneRuns(root, { keepDays }), /bad keepDays/);
    assert.deepEqual(pruneRuns(join(root, "missing")), { removed: [], refused: [] });
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("pruneRuns refuses a symlinked runs folder", () => {
  const root = tmp();
  try {
    const elsewhere = join(root, "elsewhere");
    mkdirSync(join(elsewhere, "old"), { recursive: true });
    const old = new Date(NOW - 40 * DAY);
    utimesSync(join(elsewhere, "old"), old, old);
    const work = join(root, "work");
    mkdirSync(work);
    symlinkSync(elsewhere, join(work, "runs"));
    assert.deepEqual(pruneRuns(work, { keepDays: 30, now: NOW }), { removed: [], refused: ["runs"] });
    assert.ok(existsSync(join(elsewhere, "old")));
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("the CLI prints JSON for a work root, or a human summary", () => {
  const root = tmp();
  try {
    const now = Date.now();
    const iso = (h) => new Date(now - h * 3_600_000).toISOString();
    const lines = [
      { at: iso(5), outcome: "idle" },
      { v: 2, kind: "pass", at: iso(4), outcome: "pushed", closed: 0, queued: 0, durationMs: 9000 },
      repair({ at: iso(4), sha: "c1", files: ["x.md"], tokens: 42, durationsMs: { agent: 60_000, review: 30_000, total: 100_000 } }),
      outcome({ at: iso(1), pushedSha: "c1" }),
    ];
    writeFileSync(join(root, "receipts.jsonl"), `${lines.map((l) => JSON.stringify(l)).join("\n")}\nbroken\n`);
    const json = spawnSync(process.execPath, [SCRIPT, "--work-root", root, "--since", "30d", "--json"], { encoding: "utf8" });
    assert.equal(json.status, 0, json.stderr);
    const report = JSON.parse(json.stdout);
    assert.equal(report.skipped, 1);
    assert.equal(report.passes.total, 2);
    assert.equal(report.success.rate, 1);
    assert.equal(report.tokens.total, 42);
    assert.deepEqual(report.topFiles, [{ file: "x.md", count: 1 }]);
    const human = spawnSync(process.execPath, [SCRIPT, "--work-root", root], { encoding: "utf8" });
    assert.equal(human.status, 0, human.stderr);
    assert.match(human.stdout, /Success \(merged with no re-conflict\): 100%/);
    const empty = spawnSync(process.execPath, [SCRIPT, "--work-root", join(root, "none"), "--json"], { encoding: "utf8" });
    assert.equal(JSON.parse(empty.stdout).passes.total, 0);
    const bad = spawnSync(process.execPath, [SCRIPT, "--work-root", root, "--since", "whenever"], { encoding: "utf8" });
    assert.equal(bad.status, 2);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
