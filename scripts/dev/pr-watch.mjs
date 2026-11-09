#!/usr/bin/env node
// PR watch (AGENTS.md "Review and merge cadence"): wait on a PR until you must
// act, then exit with a code and one next action. A thin client: queue state is
// Mergify's own status payload (the "-*- Mergify Payload -*-" JSON in its PR
// comment), conflict state is pr-doctor's own label and comment. One GraphQL
// call per poll, read-only.
//
//   node scripts/dev/pr-watch.mjs <pr> [--until merged|green] [--interval 120]
//                                [--max-minutes N] [--repo owner/name] [--once]
//
// Exit codes: 0 merged (or green with --until green); 1 error; 2 usage; 3 still
// waiting (--once, --max-minutes); 10 a required check failed or the PR left the
// queue; 11 conflicts need you; 12 not queued, or GitHub auto-merge armed; 13
// closed; 14 draft; 15 a check was cancelled and nothing replaced it; 16 queued
// but Mergify will not take it (exclusion label, stale condition, or stuck).

import { spawnSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { parseArgs as parseCli } from "node:util";
import { pathToFileURL } from "node:url";

import { LABEL as NEEDS_MERGE_LABEL, MARKER as DOCTOR_MARKER, SKIP_LABEL as DOCTOR_SKIP_LABEL } from "../pr-doctor.mjs";

export const EXIT = Object.freeze({ DONE: 0, ERROR: 1, USAGE: 2, WAITING: 3, CHECK_FAILED: 10, CONFLICT: 11, ENQUEUE: 12, CLOSED: 13, DRAFT: 14, CANCELLED: 15, QUEUE_BLOCKED: 16 });
export const QUEUE_LABEL = "queue";
export const QUEUE_COMMENT = "@Mergifyio queue"; // worker-contract.json merge.comment
export const REFRESH_COMMENT = "@Mergifyio refresh";
export const QUEUE_EXCLUSION_LABELS = Object.freeze([NEEDS_MERGE_LABEL, DOCTOR_SKIP_LABEL]);
export const ADMIT_GRACE_MINUTES = 5;
// pr-doctor runs every 20 minutes and on each push to main; allow a cycle plus the run and cron lateness.
export const DOCTOR_GRACE_MINUTES = 30;

const FAILED = new Set(["FAILURE", "TIMED_OUT", "STARTUP_FAILURE", "ACTION_REQUIRED", "ERROR"]);
const PASSED = new Set(["SUCCESS", "NEUTRAL", "SKIPPED"]);
const GATE = "Build and check";
const GATE_INPUT = /^(Classify changes|Docs and queue checks|Accessibility|macOS|Windows)\b/; // feed the gate (ci.yml)

export function parseArgs(argv) {
  const { values, positionals } = parseCli({ args: argv, allowPositionals: true, options: {
    until: { type: "string", default: "merged" }, interval: { type: "string", default: "120" }, "max-minutes": { type: "string" },
    repo: { type: "string" }, once: { type: "boolean", default: false }, help: { type: "boolean", short: "h", default: false } } });
  const pr = positionals.length === 1 && /^#?\d+$/.test(positionals[0]) ? Number(positionals[0].replace("#", "")) : null;
  if (positionals.length > 1) throw new Error(`unknown argument ${positionals[1]}`);
  const opts = { until: values.until, interval: Number(values.interval), once: values.once, maxMinutes: values["max-minutes"] == null ? null : Number(values["max-minutes"]), repo: values.repo ?? null, pr, help: values.help };
  if (!opts.help && pr === null) throw new Error(positionals.length ? `unknown argument ${positionals[0]}` : "missing PR number");
  if (!["merged", "green"].includes(opts.until)) throw new Error("--until is merged or green");
  if (!(opts.interval >= 30)) throw new Error("--interval is at least 30 seconds");
  if (opts.maxMinutes !== null && !(opts.maxMinutes > 0)) throw new Error("--max-minutes must be positive");
  return opts;
}

/** The newest result per check name on the head commit (re-runs leave older entries). */
export function latestChecks(contexts) {
  const byName = new Map();
  for (const node of contexts ?? []) {
    const status = node.__typename === "StatusContext";
    const runId = status ? null : node.checkSuite?.workflowRun?.databaseId ?? null;
    const check = { name: status ? node.context : node.name, required: !!node.isRequired, runId,
      state: status ? node.state : node.status === "COMPLETED" ? node.conclusion ?? "NEUTRAL" : node.status ?? "PENDING",
      order: `${String(runId ?? 0).padStart(15, "0")}|${node.startedAt ?? ""}` };
    const seen = byName.get(check.name);
    if (!seen || check.order >= seen.order) byName.set(check.name, { ...check, required: check.required || !!seen?.required });
  }
  return [...byName.values()];
}

const PAYLOAD = /-\*- Mergify Payload -\*-\s*(\{[\s\S]*?\})\s*-\*- Mergify Payload End -\*-/;
const leaves = (nodes) => (nodes ?? []).flatMap((n) => n.match ? [] : n.subconditions?.length ? leaves(n.subconditions) : [n.description]);

/** Mergify's view of the PR, from its comments in update order (it edits its status comment
 * in place): the latest status payload,
 * unless a later comment says a push cancelled the request. `unmet` lists the
 * conditions a waiting request still lacks (the payload's tree, else the checklist
 * above <details>, since a waiting payload carries an empty tree). */
export function mergifyStatus(comments) {
  let status = { state: "none", unmet: [] };
  const at = (c) => c.updatedAt ?? c.createdAt ?? "";
  for (const { author, body = "" } of [...(comments ?? [])].sort((a, b) => at(a).localeCompare(at(b)))) {
    if (!/^mergify(\[bot\])?$/.test(author ?? "")) continue;
    let payload = null;
    try { payload = JSON.parse(PAYLOAD.exec(body)?.[1] ?? "null"); } catch { /* not a status comment */ }
    if (payload?.version === 1) {
      const fromList = [...body.split("<details>")[0].matchAll(/^\s*- \[ \] `([^`]+)`/gm)].map((m) => m[1]);
      const unmet = payload.state === "waiting" ? [...new Set(leaves(payload.required_conditions).concat(fromList))] : [];
      status = { state: payload.state, step: payload.step ?? null, unmet, reason: payload.dequeue_reason?.message ?? null };
    } else if (/request has been cancelled|Queue command cancelled/.test(body)) status = { state: "cancelled", unmet: [] };
  }
  return status;
}

export function snapshotFromGraphql(data) {
  const pr = data.repository.pullRequest;
  const labels = (pr.labels?.nodes ?? []).map((label) => label.name);
  const comments = (pr.comments?.nodes ?? []).map((c) => ({ author: c.author?.login ?? "", body: c.body ?? "", createdAt: c.createdAt, updatedAt: c.updatedAt }));
  return {
    number: pr.number, state: pr.state, isDraft: !!pr.isDraft, mergeable: pr.mergeable, mergeStateStatus: pr.mergeStateStatus,
    autoMerge: pr.autoMergeRequest != null, labels, labeled: labels.includes(QUEUE_LABEL),
    blockers: labels.filter((name) => QUEUE_EXCLUSION_LABELS.includes(name)),
    headOid: pr.headRefOid, comments,
    checks: latestChecks(pr.commits?.nodes?.[0]?.commit?.statusCheckRollup?.contexts?.nodes), mergify: mergifyStatus(comments),
  };
}

const short = (names, max = 3) => names.length <= max ? names.join(", ") : `${names.slice(0, max).join(", ")} +${names.length - max}`;
const minutes = (from, to) => (Date.parse(to) - Date.parse(from)) / 60000;
const act = (exitCode, action, message) => ({ exitCode, action, message, done: true });
const wait = (action, message) => ({ exitCode: EXIT.WAITING, action, message, done: false });
const byGateLast = (a, b) => (a.name === GATE) - (b.name === GATE); // name the job that broke, not the gate

/** Failures, cancellations and waits among the checks that block a merge. Green means
 * the gate itself passed: before it reports, every other check can be green while Windows failed. */
export function analyzeChecks(checks) {
  const blocking = checks.filter((c) => c.required || GATE_INPUT.test(c.name));
  const gate = checks.find((c) => c.name === GATE);
  const gatePassed = !!gate && PASSED.has(gate.state);
  const failed = blocking.filter((c) => FAILED.has(c.state)).sort(byGateLast);
  // Once the gate passes, a cancelled input (a starved macos-15 the fallback replaced) does not matter.
  const cancelled = gatePassed ? [] : blocking.filter((c) => c.state === "CANCELLED").sort(byGateLast);
  const pending = blocking.filter((c) => !FAILED.has(c.state) && !PASSED.has(c.state) && c.state !== "CANCELLED");
  const green = gatePassed && checks.filter((c) => c.required).every((c) => PASSED.has(c.state)) && !failed.length && !pending.length;
  return { failed, cancelled, pending, green };
}

/** The pr-doctor sticky comment's outcome and listed files, or null. */
export function doctorComment(comments) {
  const comment = [...(comments ?? [])].reverse().find((c) => (c.body ?? "").startsWith(DOCTOR_MARKER));
  if (!comment) return null;
  return { outcome: /pr-doctor outcome=([\w-]+)/.exec(comment.body)?.[1] ?? null, files: [...comment.body.matchAll(/^- `([^`]+)`$/gm)].map((m) => m[1]) };
}

/** DIRTY: pr-doctor repairs queue-file and append-only Markdown conflicts by itself; hand you
 * the merge only once it says it cannot (the needs-merge label, or no push token) or a cycle passed. */
export function decideConflict(snap, { now, dirtySince = null } = {}) {
  const n = snap.number;
  const doctor = doctorComment(snap.comments);
  const merge = `merge main yourself: git fetch origin && git merge origin/main, resolve ${doctor?.files.length ? short(doctor.files, 6) : "the conflicts"}, run the focused checks, push, then comment "${QUEUE_COMMENT}"`;
  if (snap.labels.includes(NEEDS_MERGE_LABEL)) return act(EXIT.CONFLICT, "conflict-labeled", `#${n} conflicts with main and pr-doctor labeled it ${NEEDS_MERGE_LABEL}: ${merge}`);
  if (doctor?.outcome === "report-only") return act(EXIT.CONFLICT, "conflict-report-only", `#${n} conflicts with main and pr-doctor reported without repairing (no push token): ${merge}`);
  if (minutes(dirtySince ?? now, now) >= DOCTOR_GRACE_MINUTES) return act(EXIT.CONFLICT, "conflict-doctor-timeout", `#${n} has conflicted with main for ${DOCTOR_GRACE_MINUTES}+ minutes with no pr-doctor repair: ${merge}`);
  return wait("conflict-wait-doctor", `#${n} conflicts with main; waiting up to ${DOCTOR_GRACE_MINUTES} minutes for pr-doctor (it repairs queue-file and append-only Markdown conflicts and requeues a labeled PR); do not merge main yet`);
}

export const enqueueCommand = (snap) => `${snap.labeled ? "" : `gh pr edit ${snap.number} --add-label ${QUEUE_LABEL} && `}gh pr comment ${snap.number} --body "${QUEUE_COMMENT}"`;
const disarmCommand = (snap) => `gh pr merge --disable-auto ${snap.number}`;

/** One snapshot (+ what the shell gathered) -> { action, message, exitCode, done }. `idleSince`:
 * when the PR was first seen green with no queue run (it times the 12 and 16 exits). */
export function decide(snap, { until = "merged", now = new Date().toISOString(), dirtySince = null, idleSince = null } = {}) {
  const n = snap.number;
  const { state, step, unmet, reason } = snap.mergify;
  if (snap.state === "MERGED") return act(EXIT.DONE, "merged", `#${n} merged`);
  if (snap.state === "CLOSED") return act(EXIT.CLOSED, "closed", `#${n} was closed without merging; reopen it or ask the coordinator`);
  const checks = analyzeChecks(snap.checks);
  const dirty = snap.mergeStateStatus === "DIRTY" || snap.mergeable === "CONFLICTING";
  const queued = ["waiting", "checking"].includes(state);
  if (until === "green" && checks.green && !dirty) return act(EXIT.DONE, "green", `#${n} required checks are green; Mergify: ${state}${queued ? "" : ` (not queued: ${enqueueCommand(snap)})`}`);
  if (snap.isDraft) return act(EXIT.DRAFT, "draft", `#${n} is a draft, which never merges: gh pr ready ${n} when it is done`);
  // The gate failing only because an input was cancelled is a cancellation.
  if (checks.failed.length && !(checks.failed.every((c) => c.name === GATE) && checks.cancelled.length)) {
    const [first, ...rest] = checks.failed;
    const also = dirty ? " It also conflicts with main; merge main in with the fix." : "";
    if (first.name === "PR owner") return act(EXIT.CHECK_FAILED, "failed-pr-owner", `#${n} failed PR owner: the body needs a filled-in Owner: line from .github/pull_request_template.md; gh pr edit ${n} --body-file <file> (the edit reruns the check).${also}`);
    return act(EXIT.CHECK_FAILED, `failed:${first.name}`, `#${n} required check failed: ${first.name}${rest.length ? ` (also ${short(rest.map((c) => c.name))})` : ""}. Read it with: ${first.runId ? `gh run view ${first.runId} --log-failed` : `gh pr checks ${n}`}.${also}`);
  }
  if (checks.cancelled.length && !checks.pending.length) {
    const first = checks.cancelled[0];
    return act(EXIT.CANCELLED, `cancelled:${first.name}`, `#${n} ${first.name} was cancelled on the current head and no newer run replaced it: ${first.runId ? `gh run rerun ${first.runId} --failed` : `gh pr checks ${n}`}`);
  }
  if (dirty) return decideConflict(snap, { now, dirtySince });
  if (snap.autoMerge) return act(EXIT.ENQUEUE, "auto-merge-armed", `#${n} has GitHub auto-merge armed, which merges around the Mergify queue: ${disarmCommand(snap)}`);
  if (snap.blockers.length && (snap.labeled || queued)) {
    return act(EXIT.QUEUE_BLOCKED, `queue-blocked:${snap.blockers[0]}`, `#${n} is queued but Mergify will not take it while it carries ${snap.blockers.join(", ")} (.mergify.yml queue_conditions; pr-doctor clears ${NEEDS_MERGE_LABEL} once the PR merges cleanly): ${snap.blockers.map((l) => `gh pr edit ${n} --remove-label ${l}`).join(" && ")}`);
  }
  if (state === "dequeued") return act(EXIT.CHECK_FAILED, "dequeued", `#${n} left the Mergify queue: ${reason ?? "see Mergify's comment"}. Fix and push, then comment "${QUEUE_COMMENT}" (a new head does not requeue it); for a flake comment "@mergifyio requeue".`);
  if (state === "merged") return wait("mergify-merged", `#${n}: Mergify reports it merged; waiting for GitHub to show it.`);
  if (state === "checking") return wait(`checking:${step}`, `#${n} is in the Mergify queue (${step ?? "checking"}); Mergify tests it on top of main and merges it.`);
  const pending = checks.pending.length ? `waiting on ${short(checks.pending.map((c) => c.name))}` : !checks.green ? "waiting on checks to start" : null;
  const stuck = idleSince && minutes(idleSince, now) >= ADMIT_GRACE_MINUTES;
  if (state === "waiting") {
    const unmetText = unmet.length ? `unmet: ${short(unmet.map((c) => `\`${c}\``), 6)}` : "no unmet condition reported";
    // A check condition Mergify still lacks though GitHub reports it passed: Mergify missed the event (#339).
    const stale = unmet.length && unmet.every((c) => {
      const name = /^check-(?:success|neutral|skipped) = (.+)$/.exec(c)?.[1];
      return name && PASSED.has(snap.checks.find((check) => check.name === name)?.state);
    });
    if (stale || (stuck && !unmet.length)) return act(EXIT.QUEUE_BLOCKED, "queue-refresh", `#${n} is queued and Mergify is waiting (${unmetText}) though GitHub reports those checks passed; Mergify missed the update (a repeat queue comment is ignored while one runs): gh pr comment ${n} --body "${REFRESH_COMMENT}"`);
    const fix = snap.labeled ? `fix those, or comment "${REFRESH_COMMENT}" if they already hold` : `gh pr edit ${n} --add-label ${QUEUE_LABEL}`;
    if (stuck) return act(EXIT.QUEUE_BLOCKED, "queue-conditions-unmet", `#${n} is green and queued but Mergify has waited ${ADMIT_GRACE_MINUTES}+ minutes (${unmetText}): ${fix}`);
    return wait(`waiting:${unmet.join(",")}`, `#${n} is queued; Mergify is waiting for its conditions (${pending ?? unmetText}).`);
  }
  const why = state === "cancelled" ? "a push cancelled its queue request" : "Mergify has no queue request for it";
  if (pending) return wait(`unqueued:${pending}`, `#${n} ${pending}; ${why} yet, queue it now: ${enqueueCommand(snap)}`);
  if (stuck) return act(EXIT.ENQUEUE, "enqueue", `#${n} required checks are green but ${why}: ${enqueueCommand(snap)}`);
  return wait("unqueued-green", `#${n} required checks are green but ${why}; waiting ${ADMIT_GRACE_MINUTES} minutes for it to be queued (${enqueueCommand(snap)})`);
}

export function statusLine(snap, decision) {
  const counts = { ok: 0, pending: 0, failed: 0 };
  for (const c of snap.checks) if (c.state !== "CANCELLED") counts[FAILED.has(c.state) ? "failed" : PASSED.has(c.state) ? "ok" : "pending"]++;
  const mergify = snap.mergify.step ? `${snap.mergify.state}/${snap.mergify.step}` : snap.mergify.state;
  return `${snap.state}/${snap.mergeStateStatus} | head ${String(snap.headOid ?? "").slice(0, 8)} | checks ${counts.ok} ok, ${counts.pending} pending, ${counts.failed} failed | mergify ${mergify}${snap.labeled ? "" : ", unlabeled"}\n  -> ${decision.message}`;
}

// --- I/O ----------------------------------------------------------------------

const QUERY = `query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){
 pullRequest(number:$number){ number state isDraft mergeable mergeStateStatus headRefOid autoMergeRequest{enabledAt}
  labels(first:30){nodes{name}} comments(last:50){nodes{author{login} body createdAt updatedAt}}
  commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100){nodes{ __typename
   ... on CheckRun{name status conclusion startedAt isRequired(pullRequestNumber:$number) checkSuite{workflowRun{databaseId}}}
   ... on StatusContext{context state isRequired(pullRequestNumber:$number)}}}}}}}}}}`;

function gh(args) {
  const result = spawnSync("gh", args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`gh ${args.slice(0, 2).join(" ")} exited ${result.status}: ${(result.stderr || result.stdout).trim().split("\n")[0]}`);
  return result.stdout;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const stamp = () => new Date().toISOString().slice(11, 19) + "Z";
const USAGE = "usage: node scripts/dev/pr-watch.mjs <pr> [--until merged|green] [--interval 120] [--max-minutes N] [--repo owner/name] [--once]";

async function main() {
  let opts;
  try { opts = parseArgs(process.argv.slice(2)); } catch (error) { console.error(`${error.message}\n${USAGE}`); return EXIT.USAGE; }
  if (opts.help) { console.log(USAGE); return EXIT.DONE; }
  const repo = opts.repo ?? gh(["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"]).trim();
  const [owner, name] = repo.split("/");
  const started = Date.now();
  const state = { dirtySince: null, idleSince: null };
  let lastLine = null, lastHead = null, lastAction = null;
  for (;;) {
    let snap, decision;
    try {
      const data = JSON.parse(gh(["api", "graphql", "-F", `owner=${owner}`, "-F", `name=${name}`, "-F", `number=${opts.pr}`, "-f", `query=${QUERY}`]));
      if (data.errors?.length) throw new Error(data.errors[0].message);
      snap = snapshotFromGraphql(data.data);
      const now = new Date().toISOString();
      const dirty = snap.mergeStateStatus === "DIRTY" || snap.mergeable === "CONFLICTING";
      state.dirtySince = dirty ? state.dirtySince ?? now : null;
      const idle = analyzeChecks(snap.checks).green && !["checking", "merged"].includes(snap.mergify.state);
      state.idleSince = idle ? state.idleSince ?? now : null;
      decision = decide(snap, { until: opts.until, now, dirtySince: state.dirtySince, idleSince: state.idleSince });
    } catch (error) {
      if (opts.once) { console.error(`pr-watch: ${error.message}`); return EXIT.ERROR; }
      console.error(`${stamp()} gh failed (${error.message}); retrying`);
      if (opts.maxMinutes && Date.now() - started >= opts.maxMinutes * 60000) return EXIT.ERROR;
      await sleep(opts.interval * 1000);
      continue;
    }
    if (lastHead && snap.headOid !== lastHead) console.log(`${stamp()} head moved ${lastHead.slice(0, 8)} -> ${snap.headOid.slice(0, 8)}; a push cancels a pending queue request.`);
    lastHead = snap.headOid;
    // A cancellation may precede its replacement run, the enqueue workflow disarms auto-merge
    // within a minute, and Mergify may lag GitHub by a poll: act on these only when two polls agree.
    const transient = /^(cancelled:|auto-merge-armed$|queue-refresh$)/.test(decision.action);
    const confirmed = !transient || opts.once || lastAction === decision.action;
    lastAction = decision.action;
    const line = statusLine(snap, decision);
    if (line !== lastLine) { console.log(`${stamp()} #${snap.number} ${line}`); lastLine = line; }
    if (opts.once || (decision.done && confirmed)) return decision.exitCode;
    if (opts.maxMinutes && Date.now() - started >= opts.maxMinutes * 60000) {
      console.log(`${stamp()} stopped after ${opts.maxMinutes} minutes, still waiting: ${decision.message}`);
      return EXIT.WAITING;
    }
    await sleep(opts.interval * 1000);
  }
}

// Compare real paths: argv[1] may go through a symlink (macOS /var -> /private/var).
function isEntryPoint() {
  try { return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href; } catch { return false; }
}
if (isEntryPoint()) main().then((code) => process.exit(code), (error) => { console.error(`pr-watch: ${error.message}`); process.exit(EXIT.ERROR); });
