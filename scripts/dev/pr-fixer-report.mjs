#!/usr/bin/env node
// PR fixer report (docs/agents/pr-fixer.md "Reviewing its activity"): reads the
// fixer's receipts.jsonl (format v2, with v1 pass lines still accepted) and
// summarizes passes, repairs, failure codes, durations, tokens, conflicted files
// and the success measure: a pushed repair succeeds when its bead later gets a
// `merged` outcome with no `reconflicted` outcome between the push and the merge.
// It also owns pruneRuns, which pr-fixer.mjs calls at the start of each pass.
//
//   node scripts/dev/pr-fixer-report.mjs [--work-root DIR] [--since 30d] [--json]

import { existsSync, lstatSync, readdirSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { join, sep } from "node:path";
import { pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

const DAY_MS = 86_400_000;
const KINDS = new Set(["pass", "repair", "outcome"]);
const TOP_FILES = 10;

/** The fixer's default work root, as in pr-fixer.mjs runPass. */
export const defaultWorkRoot = (env = process.env) => join(env.XDG_STATE_HOME || join(homedir(), ".local", "state"), "brauser-pr-fixer");

const timeOf = (at) => (typeof at === "string" ? Date.parse(at) : NaN);
const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);

/** receipts.jsonl text -> { entries, skipped }. A line without `v` is a v1 pass line and
 * becomes { v: 1, kind: "pass", ... }. A line that is not a JSON object, has no valid `at`,
 * or has an unknown `v` or `kind` is skipped and counted. Blank lines are ignored. */
export function parseReceipts(text) {
  const entries = [];
  let skipped = 0;
  for (const line of String(text ?? "").split("\n")) {
    if (!line.trim()) continue;
    let value;
    try { value = JSON.parse(line); } catch { skipped += 1; continue; }
    if (!isObject(value) || Number.isNaN(timeOf(value.at))) { skipped += 1; continue; }
    if (value.v === undefined) {
      if (typeof value.outcome !== "string") { skipped += 1; continue; }
      entries.push({ ...value, v: 1, kind: "pass" });
    } else if (value.v === 2 && KINDS.has(value.kind)) {
      entries.push(value);
    } else {
      skipped += 1;
    }
  }
  return { entries, skipped };
}

/** "30d", "12h", an ISO date, a Date or epoch ms -> epoch ms; "all", null or undefined -> null. */
export function parseSince(since, now = Date.now()) {
  if (since === undefined || since === null || since === "all") return null;
  if (since instanceof Date) return since.getTime();
  if (typeof since === "number" && Number.isFinite(since)) return since;
  const m = /^(\d+(?:\.\d+)?)([dh])$/.exec(String(since).trim());
  if (m) return now - Number(m[1]) * (m[2] === "d" ? DAY_MS : DAY_MS / 24);
  const t = Date.parse(String(since));
  if (Number.isNaN(t)) throw new Error(`bad --since value: ${since} (use 30d, 12h, all or an ISO date)`);
  return t;
}

/** Nearest-rank percentile of a list of numbers (p in 0..1); null for an empty list. */
export function percentile(values, p) {
  const sorted = values.filter((v) => typeof v === "number" && Number.isFinite(v)).sort((a, b) => a - b);
  if (!sorted.length) return null;
  return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(p * sorted.length) - 1))];
}

const countBy = (items, keyOf) => {
  const counts = {};
  for (const item of items) { const key = keyOf(item); counts[key] = (counts[key] ?? 0) + 1; }
  return counts;
};
/** { key: count } -> [{ key, count }] by count descending, then key. */
const ranked = (counts, name) => Object.entries(counts)
  .map(([key, count]) => ({ [name]: key, count }))
  .sort((a, b) => b.count - a.count || (a[name] < b[name] ? -1 : a[name] > b[name] ? 1 : 0));

/** What became of one pushed repair: succeeded, reconflicted, closed-unmerged, or pending.
 * Outcomes are matched on the bead; `merged` and `closed-unmerged` also on the pushed sha when
 * both sides carry one. Any `reconflicted` outcome after the push and before the merge fails it. */
export function fateOfPush(push, outcomes) {
  const pushedAt = timeOf(push.at);
  const later = outcomes
    .filter((o) => o.bead === push.bead && timeOf(o.at) >= pushedAt)
    .sort((a, b) => timeOf(a.at) - timeOf(b.at));
  const sameSha = (o) => !push.sha || !o.pushedSha || o.pushedSha === push.sha;
  for (const o of later) {
    if (o.result === "reconflicted" && sameSha(o)) return "reconflicted";
    if (o.result === "merged" && sameSha(o)) return "succeeded";
    if (o.result === "closed-unmerged" && sameSha(o)) return "closed-unmerged";
  }
  return "pending";
}

/** Parsed entries -> the report. `since` (see parseSince) limits passes, repairs and outcome
 * counts to lines at or after it; a pushed repair's fate is read from every outcome given. */
export function summarizeReceipts(entries, { since, now = Date.now() } = {}) {
  const from = parseSince(since, now);
  const inWindow = (e) => from === null || timeOf(e.at) >= from;
  const all = entries.filter((e) => isObject(e) && !Number.isNaN(timeOf(e.at)));
  const outcomesAll = all.filter((e) => e.kind === "outcome");
  const windowed = all.filter(inWindow);
  const passes = windowed.filter((e) => e.kind === "pass");
  const repairs = windowed.filter((e) => e.kind === "repair");
  const outcomes = windowed.filter((e) => e.kind === "outcome");
  const pushed = repairs.filter((r) => r.outcome === "pushed");
  const failed = repairs.filter((r) => r.outcome === "failed");

  const failureCodes = [
    ...failed.map((r) => r.code || "unknown"),
    ...passes.filter((p) => (p.outcome === "refused" || p.outcome === "claim-refused") && p.code).map((p) => p.code),
  ];
  const fates = countBy(pushed, (p) => fateOfPush(p, outcomesAll));
  const succeeded = fates.succeeded ?? 0;
  const pending = fates.pending ?? 0;
  const decided = pushed.length - pending;
  const durations = (step) => {
    const values = repairs.map((r) => r.durationsMs?.[step]).filter((v) => typeof v === "number" && Number.isFinite(v));
    return { n: values.length, medianMs: percentile(values, 0.5), p90Ms: percentile(values, 0.9) };
  };
  const tokenValues = repairs.map((r) => r.tokens).filter((v) => typeof v === "number" && Number.isFinite(v));
  const results = countBy(outcomes, (o) => o.result);

  return {
    since: from === null ? null : new Date(from).toISOString(),
    passes: { total: passes.length, v1: passes.filter((p) => p.v === 1).length, byOutcome: countBy(passes, (p) => p.outcome) },
    repairs: { attempted: repairs.length, pushed: pushed.length, failed: failed.length, exhausted: repairs.filter((r) => r.exhausted === true).length },
    failures: ranked(countBy(failureCodes, (c) => c), "code"),
    success: {
      pushed: pushed.length, succeeded, reconflicted: fates.reconflicted ?? 0, closedUnmerged: fates["closed-unmerged"] ?? 0,
      pending, decided, rate: decided ? succeeded / decided : null,
    },
    durations: { agent: durations("agent"), review: durations("review"), total: durations("total") },
    tokens: { total: tokenValues.reduce((sum, v) => sum + v, 0), repairsWithTokens: tokenValues.length },
    topFiles: ranked(countBy(repairs.flatMap((r) => (Array.isArray(r.files) ? r.files : [])), (f) => f), "file").slice(0, TOP_FILES),
    outcomes: { byResult: results, reconflicted: results.reconflicted ?? 0, requeueFailed: results["requeue-failed"] ?? 0 },
  };
}

/** Delete `<workRoot>/runs/*` folders whose mtime is more than keepDays old. Never touches
 * anything outside runs/ (receipts.jsonl lives beside it), refuses symlinks (a symlinked runs/
 * refuses the whole prune) and entries that resolve outside runs/. Returns { removed, refused }.
 * keepDays must be at least 1. runPass prunes only while holding its lock, so no pass is running
 * then; the floor still guards a manual run (a pass lasts about 65 minutes at most). */
export function pruneRuns(workRoot, { keepDays = 30, now = Date.now() } = {}) {
  const days = Number(keepDays);
  if (!Number.isFinite(days) || days < 1) throw new Error(`bad keepDays: ${keepDays} (at least 1)`);
  const runsDir = join(workRoot, "runs");
  const removed = [];
  const refused = [];
  if (!existsSync(runsDir)) return { removed, refused };
  if (!lstatSync(runsDir).isDirectory()) return { removed, refused: ["runs"] };
  const root = realpathSync(runsDir);
  const cutoff = now - days * DAY_MS;
  for (const name of readdirSync(runsDir)) {
    if (name === "receipts.jsonl") continue;
    const path = join(runsDir, name);
    try {
      const st = lstatSync(path);
      if (st.isSymbolicLink()) { refused.push(name); continue; }
      if (!st.isDirectory() || st.mtimeMs >= cutoff) continue;
      if (!realpathSync(path).startsWith(root + sep)) { refused.push(name); continue; }
      rmSync(path, { recursive: true, force: true });
      removed.push(name);
    } catch {
      refused.push(name);
    }
  }
  return { removed, refused };
}

const fmtMs = (ms) => (ms === null ? "-" : ms >= 60_000 ? `${(ms / 60_000).toFixed(1)}m` : `${(ms / 1000).toFixed(1)}s`);
const fmtCounts = (counts) => Object.entries(counts).sort(([a], [b]) => (a < b ? -1 : 1)).map(([k, v]) => `${k} ${v}`).join(", ") || "none";

/** The report -> a short human summary. */
export function formatSummary(report, { skipped = 0, file } = {}) {
  const s = report.success;
  const lines = [
    `PR fixer receipts${file ? ` (${file})` : ""}, since ${report.since ?? "the start"}${skipped ? `; ${skipped} unreadable line(s) skipped` : ""}`,
    `Passes: ${report.passes.total}${report.passes.v1 ? ` (${report.passes.v1} v1)` : ""}: ${fmtCounts(report.passes.byOutcome)}`,
    `Repairs: ${report.repairs.attempted} attempted, ${report.repairs.pushed} pushed, ${report.repairs.failed} failed, ${report.repairs.exhausted} exhausted`,
    `Success (merged with no re-conflict): ${s.rate === null ? "-" : `${Math.round(s.rate * 100)}%`} (${s.succeeded}/${s.decided} decided; ${s.reconflicted} re-conflicted, ${s.closedUnmerged} closed unmerged, ${s.pending} pending)`,
    `Failures: ${report.failures.map((f) => `${f.code} ${f.count}`).join(", ") || "none"}`,
    `Durations (median / p90): ${["agent", "review", "total"].map((k) => `${k} ${fmtMs(report.durations[k].medianMs)} / ${fmtMs(report.durations[k].p90Ms)}`).join("; ")}`,
    `Tokens: ${report.tokens.total} over ${report.tokens.repairsWithTokens} repair(s)`,
    `Top conflicted files: ${report.topFiles.map((f) => `${f.file} (${f.count})`).join(", ") || "none"}`,
    `Outcomes: ${report.outcomes.reconflicted} re-conflicted, ${report.outcomes.requeueFailed} requeue failed (${fmtCounts(report.outcomes.byResult)})`,
  ];
  return lines.join("\n");
}

function main(argv) {
  const { values } = parseArgs({
    args: argv,
    options: { "work-root": { type: "string" }, since: { type: "string", default: "30d" }, json: { type: "boolean" }, help: { type: "boolean" } },
  });
  if (values.help) {
    console.log("usage: node scripts/dev/pr-fixer-report.mjs [--work-root DIR] [--since 30d|12h|all|ISO date] [--json]");
    return 0;
  }
  const workRoot = values["work-root"] ?? defaultWorkRoot();
  const file = join(workRoot, "receipts.jsonl");
  const text = existsSync(file) ? readFileSync(file, "utf8") : "";
  const { entries, skipped } = parseReceipts(text);
  const report = summarizeReceipts(entries, { since: values.since });
  if (values.json) console.log(JSON.stringify({ file, skipped, ...report }, null, 2));
  else console.log(formatSummary(report, { skipped, file }));
  return 0;
}

function isEntryPoint() {
  try { return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href; } catch { return false; }
}
if (isEntryPoint()) {
  try { process.exit(main(process.argv.slice(2))); } catch (error) { console.error(`pr-fixer-report: ${error.message}`); process.exit(2); }
}
