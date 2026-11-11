#!/usr/bin/env node
// Queue a PR into the Mergify merge queue, idempotently (AGENTS.md "Review and
// merge cadence", worker-contract.json "merge"). Safe to run any number of
// times: it reads Mergify's own status payload (the "-*- Mergify Payload -*-"
// version-1 JSON in its PR comment) and acts only on what is missing.
//
//   node scripts/dev/queue-pr.mjs <pr> [--head <sha>] [--ready] [--dry-run]
//                                [--wait-seconds 60] [--repo owner/name]
//
// What it does, in order:
//   - a draft is reported, and marked ready only with --ready;
//   - with --head, waits (up to --wait-seconds) until GitHub shows that head:
//     a queue comment Mergify reads before a push lands is cancelled by it;
//   - adds the `queue` label when it is missing;
//   - posts `@Mergifyio queue` when Mergify holds no live request for the
//     current head: none yet, a push cancelled it, or it was dequeued on an
//     older head. Never twice for one head, never while queued;
//   - posts `@Mergifyio refresh` once when Mergify waits on checks GitHub
//     reports passed (Mergify missed the event, #339);
//   - reports Mergify's dequeue reason when the PR left the queue on this head.
// It never merges, never pushes, and never runs `gh pr merge`.
//
// Exit codes: 0 queued, or queued already; 1 error; 2 usage; 3 GitHub did not
// show --head in time (nothing posted); 10 a person must act (draft, an
// exclusion label, a conflict, or dequeued on this head; the reason is
// printed); 13 merged or closed.
//
// This module is also the shared Mergify client: pr-watch, pr-doctor and the
// PR keeper read PR and Mergify state through these functions.

import { spawnSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { parseArgs as parseCli } from "node:util";
import { pathToFileURL } from "node:url";

import {
  DOCTOR_SKIP_LABEL, NEEDS_MERGE_LABEL, QUEUE_COMMENT, QUEUE_EXCLUSION_LABELS, QUEUE_LABEL, REFRESH_COMMENT,
} from "../pr-constants.mjs";

export { QUEUE_COMMENT, QUEUE_LABEL, REFRESH_COMMENT, QUEUE_EXCLUSION_LABELS };
export const EXIT = Object.freeze({ OK: 0, ERROR: 1, USAGE: 2, HEAD_TIMEOUT: 3, PERSON: 10, CLOSED: 13 });
/** Mergify normally lags GitHub by up to a minute; act on a stale view only after this. */
export const ADMIT_GRACE_MINUTES = 5;

export const FAILED = new Set(["FAILURE", "TIMED_OUT", "STARTUP_FAILURE", "ACTION_REQUIRED", "ERROR"]);
export const PASSED = new Set(["SUCCESS", "NEUTRAL", "SKIPPED"]);
export const GATE = "Build and check";
const GATE_INPUT = /^(Classify changes|Docs and queue checks|Accessibility|macOS|Windows)\b/; // feed the gate (ci.yml)

// --- Reading PR and Mergify state (pure) ----------------------------------------

/** The newest result per check name on the head commit (re-runs leave older entries). */
export function latestChecks(contexts) {
  const byName = new Map();
  for (const node of contexts ?? []) {
    const status = node.__typename === "StatusContext";
    const runId = status ? null : node.checkSuite?.workflowRun?.databaseId ?? null;
    const check = { name: status ? node.context : node.name, required: !!node.isRequired, runId,
      state: status ? node.state : node.status === "COMPLETED" ? node.conclusion ?? "NEUTRAL" : node.status ?? "PENDING",
      completedAt: status ? node.createdAt ?? null : node.completedAt ?? null,
      order: `${String(runId ?? 0).padStart(15, "0")}|${node.startedAt ?? ""}` };
    const seen = byName.get(check.name);
    if (!seen || check.order >= seen.order) byName.set(check.name, { ...check, required: check.required || !!seen?.required });
  }
  return [...byName.values()];
}

const PAYLOAD = /-\*- Mergify Payload -\*-\s*(\{[\s\S]*?\})\s*-\*- Mergify Payload End -\*-/;
const leaves = (nodes) => (nodes ?? []).flatMap((n) => n.match ? [] : n.subconditions?.length ? leaves(n.subconditions) : [n.description]);
const isMergify = (author) => /^mergify(\[bot\])?$/.test(author ?? "");
const at = (c) => c.createdAt ?? "";
const byCreated = (comments) => [...(comments ?? [])].sort((a, b) => at(a).localeCompare(at(b)));
const payloadOf = (body) => { try { return JSON.parse(PAYLOAD.exec(body ?? "")?.[1] ?? "null"); } catch { return null; } };

/** Mergify's view of the PR, from its comments in creation order: each queue request gets a new
 * status comment, and Mergify rewrites the old one as finished after it (#346), so the
 * newest-created payload is current, unless a later comment says a push cancelled it. A queue
 * command newer than every status comment is `requested` (Mergify answers in seconds), unless
 * Mergify replies that one is already running (#339). `unmet` lists what a waiting request lacks
 * (the payload's tree, else the checklist above <details>: a waiting payload's tree is empty). */
export function mergifyStatus(comments) {
  let status = { state: "none", unmet: [] }, live = status;
  for (const { author, body = "" } of byCreated(comments)) {
    if (!isMergify(author)) {
      if (/^\s*@mergifyio (queue|requeue)\b/i.test(body)) { live = status; status = { state: "requested", unmet: [] }; }
      continue;
    }
    const payload = payloadOf(body);
    if (payload?.version === 1) {
      const fromList = [...body.split("<details>")[0].matchAll(/^\s*- \[ \] `([^`]+)`/gm)].map((m) => m[1]);
      // A dequeue quotes the conditions that failed under **Reason** (#349): "> - [ ] `-label = needs-merge`".
      const quoted = [...body.matchAll(/^>[\s>]*- \[ \] `([^`]+)`/gm)].map((m) => m[1]);
      const unmet = payload.state === "waiting" ? [...new Set(leaves(payload.required_conditions).concat(fromList))]
        : payload.state === "dequeued" ? [...new Set(quoted)] : [];
      status = { state: payload.state, step: payload.step ?? null, unmet, reason: payload.dequeue_reason?.message ?? null, code: payload.dequeue_reason?.code ?? null };
    } else if (/request has been cancelled|Queue command cancelled/.test(body)) status = { state: "cancelled", unmet: [] };
    else if (/already running/.test(body) && status.state === "requested") status = live;
  }
  return status;
}

/** The current (newest-created) Mergify status comment: when it last changed, and for a
 * dequeue the short head SHA it names under "Left the queue" (the head it left on). */
export function statusComment(comments) {
  const current = byCreated(comments).filter((c) => isMergify(c.author) && payloadOf(c.body)?.version === 1).at(-1);
  if (!current) return null;
  const leftHead = /\*\*Left the queue\*\*[^\n`]*`([0-9a-f]{7,40})`/.exec(current.body)?.[1] ?? null;
  return { createdAt: current.createdAt ?? null, updatedAt: current.updatedAt ?? current.createdAt ?? null, leftHead };
}

/** A command comment (`@Mergifyio refresh`, say) from anyone but Mergify, created after `since`. */
export function commandSince(comments, command, since) {
  const re = new RegExp(`^\\s*${command.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\b`, "i");
  return (comments ?? []).some((c) => !isMergify(c.author) && re.test(c.body ?? "") && (!since || at(c) > since));
}

export function snapshotFromGraphql(data) {
  const pr = data.repository.pullRequest;
  const labels = (pr.labels?.nodes ?? []).map((label) => label.name);
  const comments = (pr.comments?.nodes ?? []).map((c) => ({ author: c.author?.login ?? "", body: c.body ?? "", createdAt: c.createdAt, updatedAt: c.updatedAt }));
  return {
    number: pr.number, state: pr.state, isDraft: !!pr.isDraft, mergeable: pr.mergeable, mergeStateStatus: pr.mergeStateStatus,
    baseRefName: pr.baseRefName ?? "main", headRefName: pr.headRefName ?? "", isCrossRepository: !!pr.isCrossRepository,
    autoMerge: pr.autoMergeRequest != null, labels, labeled: labels.includes(QUEUE_LABEL),
    blockers: labels.filter((name) => QUEUE_EXCLUSION_LABELS.includes(name)),
    headOid: pr.headRefOid, comments,
    checks: latestChecks(pr.commits?.nodes?.[0]?.commit?.statusCheckRollup?.contexts?.nodes), mergify: mergifyStatus(comments),
  };
}

const byGateLast = (a, b) => (a.name === GATE) - (b.name === GATE); // name the job that broke, not the gate

/** Failures, cancellations and waits among the checks that block a merge. Green means
 * the gate itself passed: before it reports, every other check can be green while Windows failed.
 * `greenSince` is when the last blocking check finished, once green (null if unknown). */
export function analyzeChecks(checks) {
  const blocking = checks.filter((c) => c.required || GATE_INPUT.test(c.name));
  const gate = checks.find((c) => c.name === GATE);
  const gatePassed = !!gate && PASSED.has(gate.state);
  const failed = blocking.filter((c) => FAILED.has(c.state)).sort(byGateLast);
  // Once the gate passes, a cancelled input (a starved macos-15 the fallback replaced) does not matter.
  const cancelled = gatePassed ? [] : blocking.filter((c) => c.state === "CANCELLED").sort(byGateLast);
  const pending = blocking.filter((c) => !FAILED.has(c.state) && !PASSED.has(c.state) && c.state !== "CANCELLED");
  const green = gatePassed && checks.filter((c) => c.required).every((c) => PASSED.has(c.state)) && !failed.length && !pending.length;
  const finished = blocking.filter((c) => PASSED.has(c.state)).map((c) => c.completedAt);
  const greenSince = green && finished.length && finished.every(Boolean) ? finished.sort().at(-1) : null;
  return { failed, cancelled, pending, green, greenSince };
}

/** The unmet conditions of a waiting request when every one is a check GitHub already
 * reports passed: Mergify missed the check event (#339) and needs a refresh. [] otherwise. */
export function staleConditions(snap) {
  const { state, unmet } = snap.mergify;
  if (state !== "waiting" || !unmet.length) return [];
  const names = unmet.map((c) => /^check-(?:success|neutral|skipped) = (.+)$/.exec(c)?.[1]);
  if (!names.every((name) => name && PASSED.has(snap.checks.find((check) => check.name === name)?.state))) return [];
  return [...new Set(names)];
}

const minutes = (from, to) => (Date.parse(to) - Date.parse(from)) / 60000;
const QUEUED = new Set(["requested", "waiting", "checking", "merged"]);

/** What queue-pr would do for one snapshot: { outcome, exitCode, actions, message }.
 * `actions` are in order, each { kind: "ready" | "label" | "queue" | "refresh" }. Pure:
 * running it again on the snapshot after the actions land yields no actions. */
export function planQueue(snap, { now = new Date().toISOString(), ready = false, graceMinutes = ADMIT_GRACE_MINUTES } = {}) {
  const n = snap.number;
  const actions = [];
  let blocked = "";
  const result = (outcome, exitCode, message) => ({ outcome: blocked && exitCode === EXIT.OK ? `${outcome}+blocked` : outcome,
    exitCode: blocked && exitCode === EXIT.OK ? EXIT.PERSON : exitCode, actions, message: `#${n} ${message}${blocked}` });
  if (snap.state === "MERGED") return result("merged", EXIT.CLOSED, "is merged");
  if (snap.state === "CLOSED") return result("closed", EXIT.CLOSED, "is closed without merging");
  if (snap.isDraft) {
    if (!ready) return result("draft", EXIT.PERSON, `is a draft, which Mergify never takes: mark it ready (gh pr ready ${n}, or rerun with --ready)`);
    actions.push({ kind: "ready" });
  }
  if (!snap.labeled) actions.push({ kind: "label" });
  if (snap.blockers.includes(DOCTOR_SKIP_LABEL)) {
    return result(`blocked:${DOCTOR_SKIP_LABEL}`, EXIT.PERSON, `carries ${DOCTOR_SKIP_LABEL}, an opt-out from the queue: remove it to let it queue, then rerun`);
  }
  // needs-merge: Mergify holds a request until the label goes, so queue now and say what blocks it.
  blocked = snap.blockers.includes(NEEDS_MERGE_LABEL)
    ? `; it carries ${NEEDS_MERGE_LABEL}, which Mergify waits on (.mergify.yml queue_conditions): resolve the conflict and push (pr-doctor or the keeper clears the label), or remove the label if it is stale` : "";
  if (snap.mergeStateStatus === "DIRTY" || snap.mergeable === "CONFLICTING") {
    return result("conflict", EXIT.PERSON, "conflicts with main, so Mergify cannot take it: merge main in (or wait for pr-doctor), push, then rerun");
  }
  const { state, step, reason, unmet } = snap.mergify;
  if (state === "dequeued") {
    const left = statusComment(snap.comments)?.leftHead;
    const sameHead = !left || String(snap.headOid ?? "").startsWith(left);
    if (sameHead) {
      const which = unmet.length ? ` (failed: ${unmet.map((c) => `\`${c}\``).join(", ")})` : "";
      return result("dequeued", EXIT.PERSON, `left the Mergify queue on this head: ${reason ?? "see Mergify's comment"}${which}. Push a fix and rerun, or for a flake comment "@mergifyio requeue"`);
    }
    actions.push({ kind: "queue" });
    return result("requeued", EXIT.OK, `left the queue on ${left} (${reason ?? "no reason given"}); the head moved, so queue it again`);
  }
  if (state === "waiting") {
    const stale = staleConditions(snap);
    const checks = analyzeChecks(snap.checks);
    const since = statusComment(snap.comments)?.updatedAt ?? null;
    const lagging = (stale.length || (!unmet.length && checks.green))
      && checks.greenSince && minutes(checks.greenSince, now) >= graceMinutes;
    if (lagging && !commandSince(snap.comments, REFRESH_COMMENT, since)) {
      actions.push({ kind: "refresh" });
      return result("refresh", EXIT.OK, `is queued and Mergify waits on ${stale.length ? stale.join(", ") : "nothing it names"} though GitHub reports ${stale.length ? "them" : "every check"} passed; refresh Mergify`);
    }
    if (lagging) return result("refresh-ignored", EXIT.PERSON, `is queued and Mergify still waits on checks GitHub reports passed after a refresh; check https://dashboard.mergify.com`);
    return result("waiting", EXIT.OK, `is queued; Mergify is waiting for ${unmet.length ? unmet.map((c) => `\`${c}\``).join(", ") : "its conditions"}`);
  }
  if (QUEUED.has(state)) return result(state, EXIT.OK, state === "checking" ? `is in the Mergify queue (${step ?? "checking"})` : state === "requested" ? "has a queue request Mergify has not answered yet" : "is merged by Mergify; GitHub will show it shortly");
  actions.push({ kind: "queue" });
  return result("queue", EXIT.OK, state === "cancelled" ? "lost its queue request to a push; queue it again" : "has no queue request; queue it");
}

/** Comment `text` once GitHub reports `sha` as the PR's head, polling every `stepMs` for at
 * most `limitMs`: a comment Mergify reads before the push lands is cancelled by it. Past the
 * limit it comments anyway when `force` (pr-doctor: a late request beats none), otherwise not.
 * A failed read counts as not yet. */
export function queueAfterPush({ sha, headOf, comment, wait, text = QUEUE_COMMENT, limitMs = 60000, stepMs = 5000, force = true }) {
  let seen = null;
  for (let waited = 0; ; waited += stepMs) {
    try { seen = headOf(); } catch { seen = null; }
    if (seen === sha || waited >= limitMs) break;
    wait(stepMs);
  }
  const settled = seen === sha;
  if (settled || force) comment(text);
  return { settled, seen, commented: settled || force };
}

// --- I/O ------------------------------------------------------------------------

export const QUERY = `query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){
 pullRequest(number:$number){ number state isDraft mergeable mergeStateStatus headRefOid headRefName baseRefName isCrossRepository autoMergeRequest{enabledAt}
  labels(first:30){nodes{name}} comments(last:50){nodes{author{login} body createdAt updatedAt}}
  commits(last:1){nodes{commit{statusCheckRollup{contexts(first:100){nodes{ __typename
   ... on CheckRun{name status conclusion startedAt completedAt isRequired(pullRequestNumber:$number) checkSuite{workflowRun{databaseId}}}
   ... on StatusContext{context state createdAt isRequired(pullRequestNumber:$number)}}}}}}}}}}`;

export function gh(args) {
  const result = spawnSync("gh", args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`gh ${args.slice(0, 2).join(" ")} exited ${result.status}: ${(result.stderr || result.stdout).trim().split("\n")[0]}`);
  return result.stdout;
}

/** One read-only GraphQL call: the PR's snapshot. */
export function fetchSnapshot(repo, number) {
  const [owner, name] = repo.split("/");
  const data = JSON.parse(gh(["api", "graphql", "-F", `owner=${owner}`, "-F", `name=${name}`, "-F", `number=${number}`, "-f", `query=${QUERY}`]));
  if (data.errors?.length) throw new Error(data.errors[0].message);
  return snapshotFromGraphql(data.data);
}

/** Apply planQueue's actions with gh. `log` gets one line per action. */
export function applyActions(repo, n, actions, { log = console.log, run = gh } = {}) {
  for (const { kind } of actions) {
    if (kind === "ready") run(["pr", "ready", String(n), "--repo", repo]);
    else if (kind === "label") run(["pr", "edit", String(n), "--repo", repo, "--add-label", QUEUE_LABEL]);
    else if (kind === "queue") run(["pr", "comment", String(n), "--repo", repo, "--body", QUEUE_COMMENT]);
    else if (kind === "refresh") run(["pr", "comment", String(n), "--repo", repo, "--body", REFRESH_COMMENT]);
    else throw new Error(`unknown action ${kind}`);
    log(`#${n}: ${describe(kind)}`);
  }
}

export const describe = (kind) => ({ ready: "marked ready for review", label: `added the ${QUEUE_LABEL} label`, queue: `commented "${QUEUE_COMMENT}"`, refresh: `commented "${REFRESH_COMMENT}"` })[kind] ?? kind;

export function parseArgs(argv) {
  const { values, positionals } = parseCli({ args: argv, allowPositionals: true, options: {
    head: { type: "string" }, ready: { type: "boolean", default: false }, "dry-run": { type: "boolean", default: false },
    "wait-seconds": { type: "string", default: "60" }, repo: { type: "string" }, help: { type: "boolean", short: "h", default: false } } });
  if (positionals.length > 1) throw new Error(`unknown argument ${positionals[1]}`);
  const pr = positionals.length === 1 && /^#?\d+$/.test(positionals[0]) ? Number(positionals[0].replace("#", "")) : null;
  const opts = { pr, head: values.head ?? null, ready: values.ready, dryRun: values["dry-run"], waitSeconds: Number(values["wait-seconds"]), repo: values.repo ?? null, help: values.help };
  if (!opts.help && pr === null) throw new Error(positionals.length ? `unknown argument ${positionals[0]}` : "missing PR number");
  if (opts.head !== null && !/^[0-9a-f]{7,40}$/.test(opts.head)) throw new Error("--head is a commit SHA");
  if (!(opts.waitSeconds >= 0)) throw new Error("--wait-seconds is a number of seconds");
  return opts;
}

const USAGE = "usage: node scripts/dev/queue-pr.mjs <pr> [--head <sha>] [--ready] [--dry-run] [--wait-seconds 60] [--repo owner/name]";
const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

function main() {
  let opts;
  try { opts = parseArgs(process.argv.slice(2)); } catch (error) { console.error(`${error.message}\n${USAGE}`); return EXIT.USAGE; }
  if (opts.help) { console.log(USAGE); return EXIT.OK; }
  const repo = opts.repo ?? gh(["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"]).trim();
  let snap = fetchSnapshot(repo, opts.pr);
  if (opts.head) {
    const want = opts.head;
    const { settled } = queueAfterPush({ sha: want, force: false, comment: () => {}, wait: sleep, limitMs: opts.waitSeconds * 1000,
      headOf: () => { snap = fetchSnapshot(repo, opts.pr); return snap.headOid.startsWith(want) ? want : snap.headOid; } });
    if (!settled) { console.log(`#${opts.pr} GitHub still shows head ${String(snap.headOid).slice(0, 12)}, not ${want}, after ${opts.waitSeconds}s; nothing posted. Rerun once it does.`); return EXIT.HEAD_TIMEOUT; }
  }
  const plan = planQueue(snap, { ready: opts.ready });
  console.log(`${plan.message}${opts.dryRun && plan.actions.length ? ` (dry run; would: ${plan.actions.map((a) => describe(a.kind)).join(", ")})` : ""}`);
  if (!opts.dryRun) applyActions(repo, opts.pr, plan.actions);
  return plan.exitCode;
}

// Compare real paths: argv[1] may go through a symlink (macOS /var -> /private/var).
function isEntryPoint() {
  try { return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href; } catch { return false; }
}
if (isEntryPoint()) {
  try { process.exit(main()); } catch (error) { console.error(`queue-pr: ${error.message}`); process.exit(EXIT.ERROR); }
}
