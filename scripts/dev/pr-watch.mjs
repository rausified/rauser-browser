#!/usr/bin/env node
// PR watch (AGENTS.md "Review and merge cadence"): the one supported way to
// wait on a PR after opening it, instead of hand-rolled polling loops. It
// polls GitHub with one GraphQL call (plus a few more only while the PR
// conflicts with main), prints a line only when something changes, and exits
// with a distinct code and one rule-based next action whenever you must act.
//
//   node scripts/dev/pr-watch.mjs <pr> [--until merged|green] [--interval 120]
//                                [--max-minutes N] [--repo owner/name] [--once]
//
// Rules it encodes (each one cost a PR hours before it existed):
//   - BEHIND, pending checks, a merge-hold or a merge-queue entry: keep
//     waiting. Never merge main into a PR just to be current; every push
//     cancels the in-flight ~40-minute macOS run (ci.yml cancel-in-progress).
//   - DIRTY: pr-doctor (every 20 minutes and on every push to main) repairs
//     bookkeeping and both-sides-appended Markdown conflicts. The watcher
//     predicts whether the doctor can, from the same pure functions, and waits
//     up to one doctor cycle; it says "merge main yourself" only when the
//     conflict is outside the doctor's scope, the doctor labeled the PR
//     `needs-merge`, or a doctor cycle passed without a repair.
//   - A cancelled macos-15 job with the starvation check or Intel fallback
//     still running is GitHub runner starvation: keep waiting.
//
// Exit codes (also in EXIT below): 0 done (merged, or green with
// --until green); 1 error; 2 usage; 3 still waiting (--once, or
// --max-minutes reached); 10 a required check failed; 11 conflicts need you;
// 12 checks green but auto-merge is off; 13 closed without merging; 14 draft;
// 15 a required check was cancelled and nothing replaced it.
//
// Read-only on GitHub. Locally it runs only `git merge-tree` and `git cat-file`
// (GIT_OPTIONAL_LOCKS=0; merge-tree may add objects, never touches the index,
// refs or working tree), and only when the local clone already has both commits.

import { spawnSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";

import { LABEL as NEEDS_MERGE_LABEL, MARKER as DOCTOR_MARKER, QUEUE_FILES, resolveAppendOnly } from "../pr-doctor.mjs";
import { HOLD_LABEL, STATUS_CONTEXT as HOLD_CONTEXT } from "../merge-hold.mjs";

export const EXIT = Object.freeze({
  DONE: 0, ERROR: 1, USAGE: 2, WAITING: 3,
  CHECK_FAILED: 10, CONFLICT: 11, ARM_AUTO_MERGE: 12, CLOSED: 13, DRAFT: 14, CANCELLED: 15,
});

// The work queue left main for the beads branch; a PR that still edits it
// conflicts there, and pr-doctor carries the edits over (scripts/pr-doctor.mjs).
export const DOCTOR_BOOKKEEPING = QUEUE_FILES;
export const DOCTOR_CYCLE_MINUTES = 20;
// One doctor cycle plus the run's own few minutes and cron lateness.
export const DOCTOR_GRACE_MINUTES = 30;

const FAILED = new Set(["FAILURE", "TIMED_OUT", "STARTUP_FAILURE", "ACTION_REQUIRED", "ERROR"]);
const PASSED = new Set(["SUCCESS", "NEUTRAL", "SKIPPED"]);

// CI jobs that feed the required "Build and check" gate (ci.yml build-gate):
// a failure here fails the gate, so it is reported before the gate finishes.
const GATE_INPUT = /^(Classify changes|Docs and queue checks|macOS|Windows|Detect macOS runner starvation)\b/;
const MACOS_PRIMARY = /^macOS \/ .*\(macos-15\)$/;
const STARVATION_CHECK = "Detect macOS runner starvation";
const MACOS_FALLBACK = /^macOS fallback\b/;

// --- Pure parts (scripts/tests/pr-watch.test.mjs) -----------------------------

export function parseArgs(argv) {
  const opts = { until: "merged", interval: 120, once: false, maxMinutes: null, repo: null, pr: null };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const value = () => {
      if (i + 1 >= argv.length) throw new Error(`${arg} needs a value`);
      return argv[++i];
    };
    if (arg === "--once") opts.once = true;
    else if (arg === "--until") opts.until = value();
    else if (arg === "--interval") opts.interval = Number(value());
    else if (arg === "--max-minutes") opts.maxMinutes = Number(value());
    else if (arg === "--repo") opts.repo = value();
    else if (arg === "-h" || arg === "--help") opts.help = true;
    else if (/^#?\d+$/.test(arg) && opts.pr === null) opts.pr = Number(arg.replace("#", ""));
    else throw new Error(`unknown argument ${arg}`);
  }
  if (!opts.help && opts.pr === null) throw new Error("missing PR number");
  if (!["merged", "green"].includes(opts.until)) throw new Error("--until is merged or green");
  if (!(opts.interval >= 30)) throw new Error("--interval is at least 30 seconds");
  if (opts.maxMinutes !== null && !(opts.maxMinutes > 0)) throw new Error("--max-minutes must be positive");
  return opts;
}

/** The latest result of each check name on the head commit. A re-run or a
 * second trigger of the same workflow leaves older entries in the rollup. */
export function latestChecks(contexts) {
  const byName = new Map();
  for (const node of contexts ?? []) {
    const check = node.__typename === "StatusContext"
      ? { kind: "status", name: node.context, state: node.state, required: !!node.isRequired,
        description: node.description ?? "", runId: null, order: "" }
      : { kind: "check", name: node.name, required: !!node.isRequired,
        state: node.status === "COMPLETED" ? node.conclusion ?? "NEUTRAL" : node.status ?? "PENDING",
        runId: node.checkSuite?.workflowRun?.databaseId ?? null,
        workflow: node.checkSuite?.workflowRun?.workflow?.name ?? "",
        order: `${String(node.checkSuite?.workflowRun?.databaseId ?? 0).padStart(15, "0")}|${node.startedAt ?? ""}` };
    const seen = byName.get(check.name);
    if (!seen || check.order >= seen.order) byName.set(check.name, { ...check, required: check.required || !!seen?.required });
  }
  return [...byName.values()];
}

/** Normalizes the GraphQL response (fetchSnapshot's query) into the decision input. */
export function snapshotFromGraphql(data) {
  const repo = data.repository;
  const pr = repo.pullRequest;
  const commit = pr.commits?.nodes?.[0]?.commit;
  return {
    number: pr.number,
    state: pr.state,
    isDraft: !!pr.isDraft,
    mergeable: pr.mergeable,
    mergeStateStatus: pr.mergeStateStatus,
    isInMergeQueue: !!pr.isInMergeQueue,
    queuePosition: pr.mergeQueueEntry?.position ?? null,
    mergeQueueEnabled: !!repo.mergeQueue,
    autoMerge: pr.autoMergeRequest != null,
    labels: (pr.labels?.nodes ?? []).map((label) => label.name),
    headRefName: pr.headRefName,
    headOid: pr.headRefOid,
    mainOid: repo.main?.target?.oid ?? null,
    checks: latestChecks(commit?.statusCheckRollup?.contexts?.nodes),
  };
}

const isPending = (check) => !FAILED.has(check.state) && !PASSED.has(check.state) && check.state !== "CANCELLED";
const short = (names, max = 3) => names.length <= max ? names.join(", ") : `${names.slice(0, max).join(", ")} +${names.length - max}`;

/** What the head commit's checks say: failures, cancellations, waits, the hold. */
export function analyzeChecks(checks) {
  const blocking = checks.filter((check) => check.required || (check.kind === "check" && GATE_INPUT.test(check.name)));
  const hold = checks.find((check) => check.kind === "status" && check.name === HOLD_CONTEXT);
  const holders = hold && hold.state === "PENDING" ? [...hold.description.matchAll(/#(\d+)/g)].map((m) => Number(m[1])) : [];

  const macos = checks.find((check) => MACOS_PRIMARY.test(check.name));
  const starvation = checks.find((check) => check.name === STARVATION_CHECK);
  // A skipped fallback reports under the caller's name, a running one under
  // "macOS fallback / Build and check (macos-15-intel)": any live entry counts.
  const fallbackLive = checks.some((check) => MACOS_FALLBACK.test(check.name) && check.state !== "SKIPPED" && check.state !== "CANCELLED");
  // Starved: macos-15 cancelled and the starvation check is still deciding,
  // or it decided yes and the Intel fallback is queued, running or passed.
  const starved = macos?.state === "CANCELLED" && (
    !starvation || isPending(starvation)
    || (starvation.state === "SUCCESS" && fallbackLive));

  const failed = blocking.filter((check) => FAILED.has(check.state) && check.name !== HOLD_CONTEXT);
  // Prefer the job that broke over the gate that reports it.
  failed.sort((a, b) => (a.name === "Build and check") - (b.name === "Build and check"));
  const cancelled = blocking.filter((check) => check.state === "CANCELLED" && !(starved && check === macos));
  cancelled.sort((a, b) => (a.name === "Build and check") - (b.name === "Build and check"));
  const pending = blocking.filter((check) => isPending(check) && check.name !== HOLD_CONTEXT);
  // Green means the aggregate gate itself reported success: before the
  // platform builds report, "Build and check" is absent from the rollup and
  // every check that has reported can be green while Windows already failed.
  const gate = checks.find((check) => check.name === "Build and check");
  const requiredGreen = checks.some((check) => check.required)
    && checks.filter((check) => check.required).every((check) => PASSED.has(check.state))
    && Boolean(gate) && PASSED.has(gate.state)
    && failed.length === 0 && pending.length === 0;
  const advisoryFailed = checks.filter((check) => !blocking.includes(check) && FAILED.has(check.state)).map((check) => check.name);
  return { failed, cancelled, pending, holders, held: holders.length > 0 || hold?.state === "PENDING", starved, requiredGreen, advisoryFailed };
}

/** Sorts conflicted files by whether pr-doctor resolves them.
 * `files` is [{ path, text? }], text being the merged file with diff3 markers
 * (git merge-tree output); without text a Markdown file is "unknown". */
export function classifyConflicts(files) {
  const doctor = [], person = [], unknown = [];
  for (const { path, text } of files) {
    if (DOCTOR_BOOKKEEPING.includes(path)) doctor.push(path);
    else if (!path.endsWith(".md")) person.push(path);
    else if (typeof text !== "string") unknown.push(path);
    else (resolveAppendOnly(text).resolved ? doctor : person).push(path);
  }
  return { doctor, person, unknown };
}

/** The pr-doctor sticky comment's outcome and listed files, or null. */
export function doctorComment(comments) {
  const comment = [...(comments ?? [])].reverse().find((item) => (item.body ?? "").startsWith(DOCTOR_MARKER));
  if (!comment) return null;
  const outcome = /pr-doctor outcome=([\w-]+)/.exec(comment.body)?.[1] ?? null;
  const files = [...comment.body.matchAll(/^- `([^`]+)`$/gm)].map((m) => m[1]);
  return { outcome, files, updatedAt: comment.updatedAt ?? comment.updated_at ?? null };
}

const minutesBetween = (from, to) => (Date.parse(to) - Date.parse(from)) / 60000;
const mergeMainAdvice = (files) =>
  `merge main yourself: git fetch origin && git merge origin/main, resolve ${files.length ? short(files, 6) : "the conflicts"} (queue files: scripts/dev/queue adopt --from HEAD, then git rm them), run the focused checks, push`;

/** The DIRTY branch of decide(): wait for pr-doctor, or hand the merge to you. */
export function decideConflict(snap, { now, dirtySince = null, doctorRuns = [], comment = null, conflicts = null } = {}) {
  const n = snap.number;
  const commentFiles = comment?.files ?? [];
  if (snap.labels.includes(NEEDS_MERGE_LABEL)) {
    return act(EXIT.CONFLICT, "conflict-labeled",
      `#${n} conflicts with main and pr-doctor labeled it ${NEEDS_MERGE_LABEL}: ${mergeMainAdvice(commentFiles)}`);
  }
  if (comment?.outcome === "report-only") {
    return act(EXIT.CONFLICT, "conflict-report-only",
      `#${n} conflicts with main and pr-doctor reported without repairing (no push token): ${mergeMainAdvice(commentFiles)}`);
  }
  if (conflicts && conflicts.person.length > 0) {
    return act(EXIT.CONFLICT, "conflict-outside-doctor",
      `#${n} conflicts with main in files pr-doctor does not resolve (${short(conflicts.person, 6)}): ${mergeMainAdvice([...conflicts.person, ...conflicts.doctor])}`);
  }
  const scope = conflicts
    ? (conflicts.unknown.length ? `conflicts in ${short([...conflicts.doctor, ...conflicts.unknown], 6)} (Markdown not checked locally)` : `conflicts only in files pr-doctor resolves (${short(conflicts.doctor, 6)})`)
    : "conflict files unknown (fetch origin to let the watcher check them)";
  const since = dirtySince ?? now;
  const done = doctorRuns.find((run) => run.status === "completed" && Date.parse(run.createdAt) > Date.parse(since));
  const active = doctorRuns.find((run) => run.status !== "completed");
  if (done && minutesBetween(done.updatedAt ?? done.createdAt, now) >= 2) {
    return act(EXIT.CONFLICT, "conflict-doctor-passed",
      `#${n} still conflicts after a pr-doctor run (${done.databaseId}) that started after the conflict appeared: ${mergeMainAdvice(conflicts ? [...conflicts.doctor, ...conflicts.unknown] : commentFiles)}`);
  }
  if (minutesBetween(since, now) >= DOCTOR_GRACE_MINUTES) {
    return act(EXIT.CONFLICT, "conflict-doctor-timeout",
      `#${n} has conflicted with main for ${DOCTOR_GRACE_MINUTES}+ minutes with no pr-doctor repair: ${mergeMainAdvice(conflicts ? [...conflicts.doctor, ...conflicts.unknown] : commentFiles)}`);
  }
  const last = doctorRuns[0];
  const next = active ? `pr-doctor run ${active.databaseId} is ${active.status}`
    : last ? `last pr-doctor run started ${last.createdAt}; next within ${DOCTOR_CYCLE_MINUTES} minutes`
      : `pr-doctor runs every ${DOCTOR_CYCLE_MINUTES} minutes`;
  return wait("conflict-wait-doctor",
    `#${n} conflicts with main; ${scope}. Waiting for pr-doctor (${next}); do not merge main yet`);
}

function act(exitCode, action, message) { return { exitCode, action, message, done: true }; }
function wait(action, message) { return { exitCode: EXIT.WAITING, action, message, done: false }; }

/** One snapshot (+ context the shell gathered) → { action, message, exitCode, done }. */
export function decide(snap, { until = "merged", now = new Date().toISOString(), conflict = {} } = {}) {
  const n = snap.number;
  if (snap.state === "MERGED") return act(EXIT.DONE, "merged", `#${n} merged`);
  if (snap.state === "CLOSED") return act(EXIT.CLOSED, "closed", `#${n} was closed without merging; reopen it or ask the coordinator`);

  const checks = analyzeChecks(snap.checks);
  const dirty = snap.mergeStateStatus === "DIRTY" || snap.mergeable === "CONFLICTING";

  if (until === "green" && checks.requiredGreen && !dirty && !checks.held) {
    return act(EXIT.DONE, "green", `#${n} required checks are green${snap.autoMerge ? " and auto-merge is armed" : `; auto-merge is off (${armCommand(snap)})`}`);
  }
  if (snap.isDraft) return act(EXIT.DRAFT, "draft", `#${n} is a draft, which never merges: gh pr ready ${n} when it is done`);

  // The gate failing only because an input was cancelled is a cancellation.
  const gateOnly = checks.failed.length > 0 && checks.failed.every((c) => c.name === "Build and check") && checks.cancelled.length > 0;
  if (checks.failed.length > 0 && !gateOnly) {
    const first = checks.failed[0];
    const also = dirty ? " It also conflicts with main; merge main in with the fix." : "";
    if (first.name === "PR owner") {
      return act(EXIT.CHECK_FAILED, "failed-pr-owner",
        `#${n} failed PR owner: the body needs a filled-in Owner: line from .github/pull_request_template.md; gh pr edit ${n} --body-file <file> (the edit reruns the check).${also}`);
    }
    const others = checks.failed.length > 1 ? ` (also ${short(checks.failed.slice(1).map((c) => c.name))})` : "";
    const log = first.runId ? `gh run view ${first.runId} --log-failed` : `gh pr checks ${n}`;
    return act(EXIT.CHECK_FAILED, `failed:${first.name}`, `#${n} required check failed: ${first.name}${others}. Read it with: ${log}.${also}`);
  }
  if (checks.cancelled.length > 0 && checks.pending.length === 0 && !checks.starved) {
    const first = checks.cancelled[0];
    const rerun = first.runId ? `gh run rerun ${first.runId} --failed` : `gh pr checks ${n}`;
    return act(EXIT.CANCELLED, `cancelled:${first.name}`,
      `#${n} ${first.name} was cancelled on the current head and no newer run replaced it (not runner starvation): ${rerun}`);
  }

  if (dirty) return decideConflict(snap, { now, ...conflict });

  if (checks.held) {
    const by = checks.holders.length ? checks.holders.map((h) => `#${h}`).join(", ") : "a PR";
    return wait("held", `#${n} merge-hold: ${by} carries ${HOLD_LABEL}; waiting for it to land or drop the label`);
  }
  if (snap.isInMergeQueue) {
    return wait("queued", `#${n} is in the merge queue${snap.queuePosition ? ` at position ${snap.queuePosition}` : ""}; the queue tests it on top of main`);
  }
  const behind = snap.mergeStateStatus === "BEHIND" ? " PR is BEHIND main: that is fine, do not merge main in (a push restarts the macOS run)." : "";
  if (checks.pending.length > 0 || !checks.requiredGreen) {
    const starved = checks.starved ? " macos-15 runner starvation: the Intel fallback is expected, keep waiting." : "";
    const names = checks.pending.map((c) => c.name);
    return wait(`pending:${names.join(",")}:${checks.starved}`,
      `#${n} waiting on ${names.length ? short(names) : "checks to start"}.${starved}${behind}`);
  }
  if (!snap.autoMerge) {
    return act(EXIT.ARM_AUTO_MERGE, "arm-auto-merge", `#${n} required checks are green but auto-merge is off: ${armCommand(snap)}`);
  }
  if (snap.mergeStateStatus === "UNKNOWN") return wait("unknown", `#${n} green, auto-merge armed; GitHub is computing mergeability`);
  return wait(`armed:${snap.mergeStateStatus}`,
    `#${n} green, auto-merge armed (${snap.mergeStateStatus}); GitHub ${snap.mergeQueueEnabled ? "enqueues" : "merges"} it.${behind}`);
}

export function armCommand(snap) {
  return snap.mergeQueueEnabled ? `gh pr merge --auto ${snap.number}` : `gh pr merge --auto --merge ${snap.number}`;
}

/** The one-line state summary; the shell prints it when it changes. */
export function statusLine(snap, decision) {
  const checks = analyzeChecks(snap.checks);
  const counts = { passed: 0, pending: 0, failed: 0 };
  for (const check of snap.checks) {
    if (FAILED.has(check.state)) counts.failed++;
    else if (PASSED.has(check.state)) counts.passed++;
    else if (check.state !== "CANCELLED") counts.pending++;
  }
  const bits = [
    `${snap.state}/${snap.mergeStateStatus}`,
    `head ${String(snap.headOid ?? "").slice(0, 8)}`,
    `checks ${counts.passed} ok, ${counts.pending} pending, ${counts.failed} failed`,
    `auto-merge ${snap.autoMerge ? "on" : "off"}`,
  ];
  if (checks.advisoryFailed.length) bits.push(`non-blocking failures: ${short(checks.advisoryFailed)}`);
  return `${bits.join(" | ")}\n  -> ${decision.message}`;
}

// --- I/O ----------------------------------------------------------------------

const QUERY = `query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){
 mergeQueue(branch:"main"){id}
 main: ref(qualifiedName:"refs/heads/main"){target{oid}}
 pullRequest(number:$number){
  number state isDraft mergeable mergeStateStatus isInMergeQueue headRefName headRefOid
  autoMergeRequest{enabledAt}
  mergeQueueEntry{position}
  labels(first:30){nodes{name}}
  commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100){nodes{
   __typename
   ... on CheckRun{name status conclusion startedAt isRequired(pullRequestNumber:$number) checkSuite{workflowRun{databaseId workflow{name}}}}
   ... on StatusContext{context state description isRequired(pullRequestNumber:$number)}
  }}}}}}
 }}}`;

function run(cmd, args, { check = true, cwd } = {}) {
  const result = spawnSync(cmd, args, { cwd, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, env: { ...process.env, GIT_OPTIONAL_LOCKS: "0" } });
  if (result.error) throw result.error;
  if (check && result.status !== 0) throw new Error(`${cmd} ${args.slice(0, 3).join(" ")} exited ${result.status}: ${(result.stderr || result.stdout).trim().split("\n")[0]}`);
  return result;
}
const ghJson = (args) => JSON.parse(run("gh", args).stdout);

function resolveRepo(repo) {
  return repo ?? run("gh", ["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"]).stdout.trim();
}

function fetchSnapshot(repo, number) {
  const [owner, name] = repo.split("/");
  const data = ghJson(["api", "graphql", "-F", `owner=${owner}`, "-F", `name=${name}`, "-F", `number=${number}`, "-f", `query=${QUERY}`]);
  if (data.errors?.length) throw new Error(data.errors[0].message);
  return snapshotFromGraphql(data.data);
}

/** Conflicted files from a local `git merge-tree`, or null when the clone lacks either commit. */
export function localConflicts(snap) {
  if (!snap.mainOid || !snap.headOid) return null;
  for (const oid of [snap.mainOid, snap.headOid]) {
    if (run("git", ["cat-file", "-e", `${oid}^{commit}`], { check: false }).status !== 0) return null;
  }
  const merged = run("git", ["-c", "merge.conflictStyle=diff3", "merge-tree", "--write-tree", "--name-only", "--no-messages", snap.mainOid, snap.headOid], { check: false });
  if (merged.status !== 1) return merged.status === 0 ? { doctor: [], person: [], unknown: [] } : null;
  const [tree, ...paths] = merged.stdout.split("\n").filter(Boolean);
  return classifyConflicts([...new Set(paths)].map((path) => {
    const blob = path.endsWith(".md") ? run("git", ["cat-file", "-p", `${tree}:${path}`], { check: false }) : null;
    return { path, text: blob?.status === 0 ? blob.stdout : undefined };
  }));
}

function conflictContext(repo, snap, state) {
  const doctorRuns = ghJson(["api", `repos/${repo}/actions/workflows/pr-doctor.yml/runs?per_page=3`, "--jq", "[.workflow_runs[] | {databaseId: .id, status, conclusion, createdAt: .created_at, updatedAt: .updated_at}]"]);
  const comments = ghJson(["api", `repos/${repo}/issues/${snap.number}/comments?per_page=100`, "--jq", "[.[] | {body, updatedAt: .updated_at}]"]);
  const key = `${snap.mainOid}..${snap.headOid}`;
  if (state.conflictKey !== key) {
    state.conflictKey = key;
    state.conflicts = localConflicts(snap);
  }
  return { dirtySince: state.dirtySince, doctorRuns, comment: doctorComment(comments), conflicts: state.conflicts };
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const stamp = () => new Date().toISOString().slice(11, 19) + "Z";

const USAGE = `usage: node scripts/dev/pr-watch.mjs <pr> [--until merged|green] [--interval 120] [--max-minutes N] [--repo owner/name] [--once]`;

async function main() {
  let opts;
  try {
    opts = parseArgs(process.argv.slice(2));
  } catch (error) {
    console.error(`${error.message}\n${USAGE}`);
    return EXIT.USAGE;
  }
  if (opts.help) { console.log(USAGE); return EXIT.DONE; }
  const repo = resolveRepo(opts.repo);
  const started = Date.now();
  const state = { dirtySince: null, conflictKey: null, conflicts: null };
  let lastLine = null, lastHead = null, lastMergeState = null, lastAction = null;
  for (;;) {
    let snap, decision;
    try {
      snap = fetchSnapshot(repo, opts.pr);
      const dirty = snap.mergeStateStatus === "DIRTY" || snap.mergeable === "CONFLICTING";
      if (!dirty) state.dirtySince = null;
      else state.dirtySince ??= new Date().toISOString();
      const conflict = dirty && snap.state === "OPEN" ? conflictContext(repo, snap, state) : {};
      decision = decide(snap, { until: opts.until, conflict });
    } catch (error) {
      if (opts.once) { console.error(`pr-watch: ${error.message}`); return EXIT.ERROR; }
      console.error(`${stamp()} gh failed (${error.message}); retrying`);
      if (opts.maxMinutes && Date.now() - started >= opts.maxMinutes * 60000) {
        console.log(`${stamp()} stopped after ${opts.maxMinutes} minutes; the last poll failed: ${error.message}`);
        return EXIT.ERROR;
      }
      await sleep(opts.interval * 1000);
      continue;
    }
    if (lastHead && snap.headOid !== lastHead) {
      console.log(`${stamp()} head moved ${lastHead.slice(0, 8)} -> ${snap.headOid.slice(0, 8)}: the push cancelled the in-flight CI run; waiting on the new one.${lastMergeState === "BEHIND" ? " The PR was only BEHIND: merging main was not needed (AGENTS.md)." : ""}`);
    }
    lastHead = snap.headOid;
    lastMergeState = snap.mergeStateStatus;
    // A cancellation can be a moment before a replacement run shows up: act
    // on it only when two polls in a row agree.
    const confirmed = !decision.action.startsWith("cancelled:") || opts.once || lastAction === decision.action;
    lastAction = decision.action;
    const line = statusLine(snap, decision);
    if (line !== lastLine) {
      console.log(`${stamp()} #${snap.number} ${line}`);
      lastLine = line;
    }
    if (opts.once) return decision.exitCode;
    if (decision.done && confirmed) return decision.exitCode;
    if (opts.maxMinutes && Date.now() - started >= opts.maxMinutes * 60000) {
      console.log(`${stamp()} stopped after ${opts.maxMinutes} minutes, still waiting: ${decision.message}`);
      return EXIT.WAITING;
    }
    await sleep(opts.interval * 1000);
  }
}

// Compare real paths: argv[1] may go through a symlink (macOS /var -> /private/var)
// or contain characters a file URL escapes (spaces, Windows drive letters).
function isEntryPoint() {
  try { return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href; } catch { return false; }
}

if (isEntryPoint()) {
  main().then((code) => process.exit(code), (error) => { console.error(`pr-watch: ${error.message}`); process.exit(EXIT.ERROR); });
}
