// PR keeper (AGENTS.md "Review and merge cadence"): moves stalled PRs forward
// with no agent session watching. Runs on a schedule and on CI completion
// (.github/workflows/pr-doctor.yml, job `keeper`), after pr-doctor.
//
// For each open same-repo PR into main it:
//   (a) queues a `queue`-labelled PR whose checks have been green for
//       KEEPER_GREEN_MINUTES and that Mergify holds no request for
//       (scripts/dev/queue-pr.mjs planQueue: once per head);
//   (b) comments `@Mergifyio refresh` once when Mergify waits on checks GitHub
//       reports passed;
//   (c) removes `needs-merge` when `git merge-tree` finds no conflict with main
//       (pr-doctor clears it only once GitHub recomputes mergeability);
//   (d) raises alerts for a draft labelled `queue`, a labelled or queued PR with a
//       failed required check, a PR dequeued on its current head, and a refresh
//       Mergify ignored: one sticky comment per PR that mentions nobody, one
//       tracking issue (label `pr-keeper`) listing every open alert, and the job
//       summary;
//   (e) for pr-doctor's needs-person verdict, files a bead for the repair path:
//       a P1 bead (external ref gh-pr-<n>-<head>) filed through the queue
//       tooling, once per head, plus an alert.
// Every action is idempotent and logged. It never merges, never pushes, never
// runs `gh pr merge`, and never force-pushes.
//
// Usage: node scripts/pr-keeper.mjs [--dry-run] [--repo owner/name]
//        node scripts/pr-keeper.mjs --file-repairs <repairs.json>
//   --dry-run reads only: it prints what it would do and changes nothing.
//   The first form writes the repairs it wants filed to $PR_KEEPER_REPAIRS (a
//   file path) when set; the second files them as beads (run where bd and a
//   bootstrapped queue workspace are available).

import { spawnSync } from "node:child_process";
import { appendFileSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

import { DOCTOR_SKIP_LABEL, HELD_LABEL, KEEPER_MARKER, NEEDS_MERGE_LABEL, TRUSTED_AUTHORS } from "./pr-constants.mjs";
import { analyzeChecks, describe, fetchSnapshot, planQueue, QUEUE_COMMENT, REFRESH_COMMENT, trustedAuthors, trustedComment } from "./dev/queue-pr.mjs";
import { doctorComment } from "./dev/pr-watch.mjs";

export { TRUSTED_AUTHORS, trustedComment };

export const KEEPER_GREEN_MINUTES = 15;
export const ISSUE_TITLE = "PR keeper alerts";
export const ISSUE_LABEL = "pr-keeper";
export const repairRef = (n, head) => `gh-pr-${n}-${String(head ?? "").slice(0, 12)}`;

const minutes = (from, to) => (Date.parse(to) - Date.parse(from)) / 60000;

/** A file name from a comment, safe to quote: in a code span, with no backtick or "@". */
const safeName = (file) => `\`${String(file).replace(/[`@]/g, "_")}\``;

// --- Decisions (pure; scripts/tests/pr-keeper.test.mjs) -------------------------

/** Why the keeper leaves a PR alone, or null. */
export function skipReason(snap) {
  if (snap.state !== "OPEN") return snap.state.toLowerCase();
  if (snap.baseRefName !== "main") return `base ${snap.baseRefName}`;
  if (snap.isCrossRepository) return "fork";
  if (/^mergify\//.test(snap.headRefName ?? "")) return "Mergify batch draft";
  if (snap.labels.includes(DOCTOR_SKIP_LABEL)) return DOCTOR_SKIP_LABEL;
  // Cookie's driver holds it (a follow-on of a dequeued PR, or red past its repair budget).
  if (snap.labels.includes(HELD_LABEL)) return HELD_LABEL;
  return null;
}

/** One PR -> { skip, actions, alerts, notes, retryInMinutes }. `mergeTreeClean`: true when `git merge-tree`
 * against main is clean, false when it conflicts, null when not checked. Actions are in the
 * order to run them: { kind: "unlabel" | "queue" | "refresh" | "repair" }. */
export function decideKeeper(snap, { now = new Date().toISOString(), mergeTreeClean = null, greenMinutes = KEEPER_GREEN_MINUTES, trusted = TRUSTED_AUTHORS } = {}) {
  const skip = skipReason(snap);
  const actions = [], alerts = [], notes = [];
  let retryInMinutes = null;
  if (skip) return { skip, actions, alerts, notes, retryInMinutes };
  const n = snap.number;
  const checks = analyzeChecks(snap.checks);
  const { state, reason } = snap.mergify;
  const queued = ["requested", "waiting", "checking"].includes(state);
  const needsMerge = snap.labels.includes(NEEDS_MERGE_LABEL);
  let unlabeled = false;

  // (c) needs-merge with no conflict left: Mergify waits on the label alone (#384 waited an hour).
  // Not while GitHub still reports a conflict: pr-doctor would label it again every run.
  if (needsMerge && mergeTreeClean === true && snap.mergeable !== "CONFLICTING") {
    actions.push({ kind: "unlabel", label: NEEDS_MERGE_LABEL });
    unlabeled = true;
  }
  // (e) the doctor gave up: file a repair bead (once per PR; see repairsToFile, which reads the
  // queue's own beads: an alert naming the ref is no proof one was filed).
  const doctor = doctorComment(snap.comments, { trusted });
  if (needsMerge && mergeTreeClean !== true && doctor?.outcome === "needs-person") {
    const files = doctor.files.map(safeName);
    actions.push({ kind: "repair", head: snap.headOid, files, ref: repairRef(n, snap.headOid) });
    alerts.push({ kind: "needs-person", text: `conflicts with main in files pr-doctor does not resolve${files.length ? ` (${files.join(", ")})` : ""}; a repair bead ${repairRef(n, snap.headOid)} is requested. Merge main into the branch by hand` });
  }
  // (d) states only a person can clear.
  if (snap.isDraft && snap.labeled) alerts.push({ kind: "draft-queued", text: `is labelled \`queue\` but is a draft, which Mergify never takes: \`gh pr ready ${n}\` when it is done` });
  if (checks.failed.length && (snap.labeled || queued)) {
    const first = checks.failed[0];
    alerts.push({ kind: "red-queued", text: `is ${queued ? "queued" : "labelled `queue`"} but ${first.name} failed${first.runId ? ` (\`gh run view ${first.runId} --log-failed\`)` : ""}: push a fix, then \`node scripts/dev/queue-pr.mjs ${n}\`` });
  }

  // (a) and (b) through queue-pr's plan, only for labelled, ready, unfailed PRs.
  if (snap.labeled && !snap.isDraft && !checks.failed.length) {
    const view = unlabeled ? { ...snap, labels: snap.labels.filter((l) => l !== NEEDS_MERGE_LABEL), blockers: snap.blockers.filter((l) => l !== NEEDS_MERGE_LABEL) } : snap;
    const plan = planQueue(view, { now, trusted });
    const greenLong = checks.green && checks.greenSince && minutes(checks.greenSince, now) >= greenMinutes;
    for (const action of plan.actions) {
      if (action.kind === "refresh") actions.push({ kind: "refresh" });
      else if (action.kind === "queue" && greenLong) actions.push({ kind: "queue" });
      else if (action.kind === "queue") {
        notes.push(`not queued yet: ${checks.green ? `green for under ${greenMinutes} minutes` : "checks not green"}`);
        if (checks.green && checks.greenSince) retryInMinutes = Math.max(0, greenMinutes - minutes(checks.greenSince, now));
      }
    }
    if (plan.outcome === "refresh-ignored") alerts.push({ kind: "refresh-ignored", text: `is queued but Mergify still waits on checks GitHub reports passed, after a refresh: check the Mergify dashboard` });
    if (plan.outcome === "queue-unanswered") alerts.push({ kind: "queue-unanswered", text: plan.message.replace(/^#\d+ /, "") });
    if (plan.outcome === "dequeued") alerts.push({ kind: "dequeued", text: `left the Mergify queue on its current head: ${reason ?? "see Mergify's comment"}. Push a fix (the keeper requeues a new head once it is green), or comment \`@mergifyio requeue\` for a flake` });
    notes.push(plan.message.replace(/^#\d+ /, ""));
  } else if (!snap.labeled) {
    notes.push(state === "dequeued" ? `dequeued (${reason ?? "no reason"}) and not labelled queue: left to its owner` : "not labelled queue: left to its owner");
  }
  return { skip: null, actions, alerts, notes, retryInMinutes };
}

/** The sticky alert comment for one PR, or null when there is nothing to say. Never mentions
 * anyone (no "@" outside code spans). `hadComment`: whether a keeper comment already exists. */
export function alertBody(alerts, { hadComment = false } = {}) {
  if (!alerts.length) return hadComment ? `${KEEPER_MARKER}\n**PR keeper**\n\nNo open alerts.` : null;
  return `${KEEPER_MARKER}\n**PR keeper**\n\nThis PR needs a person:\n\n${alerts.map((a) => `- **${a.kind}**: this PR ${a.text}`).join("\n")}\n\nThe keeper updates this comment on each run (scripts/pr-keeper.mjs).`;
}

/** An issue body without its timestamp: rewrite the issue only when this changes. */
export const alertTable = (text) => (text ?? "").replace(/as of \d{4}-\d\d-\d\dT[\d:.]+Z/, "as of");

/** The tracking issue body for every open alert across PRs ({ number, alerts }[]). */
export function issueBody(rows, now) {
  const open = rows.filter((r) => r.alerts.length);
  if (!open.length) return `${KEEPER_MARKER}\nNo open PR alerts as of ${now}.`;
  return `${KEEPER_MARKER}\nOpen PR alerts as of ${now} (updated by scripts/pr-keeper.mjs on every run):\n\n| PR | Alert | Detail |\n|---|---|---|\n${open.flatMap((r) => r.alerts.map((a) => `| #${r.number} | ${a.kind} | ${a.text.replace(/\|/g, "\\|")} |`)).join("\n")}\n`;
}

/** The bd create arguments for one repair request: P1, external ref per head (idempotent). */
export function repairBeadArgs(repo, { number, head, files, ref }) {
  const title = `Merge main into PR #${number} by hand: pr-doctor could not resolve the conflict`;
  const description = `The PR keeper filed this for whoever picks up repair beads (scripts/pr-keeper.mjs). PR https://github.com/${repo}/pull/${number} at head ${String(head).slice(0, 12)} conflicts with main in files pr-doctor does not resolve${files?.length ? `: ${files.join(", ")}` : ""}. Merge origin/main into the branch, resolve, run the focused checks, push, then run node scripts/dev/queue-pr.mjs ${number} --head <new head>.`;
  return [title, "-t", "bug", "-p", "1", "-l", ISSUE_LABEL, "--external-ref", ref, "-d", description,
    "--acceptance", `PR #${number} merges cleanly with main and pr-doctor or the keeper clears ${NEEDS_MERGE_LABEL}.`];
}

// --- I/O ------------------------------------------------------------------------

function sh(cmd, args, { cwd, check = true, env } = {}) {
  const result = spawnSync(cmd, args, { cwd, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, env: env ? { ...process.env, ...env } : process.env });
  if (result.error) throw result.error;
  if (check && result.status !== 0) throw new Error(`${cmd} ${args.slice(0, 3).join(" ")} exited ${result.status}: ${(result.stderr || result.stdout).trim().split("\n")[0]}`);
  return { code: result.status, stdout: result.stdout, stderr: result.stderr };
}
const gh = (args, opts) => sh("gh", args, opts).stdout;
const git = (args, opts) => sh("git", args, opts);

/** Clean merge with main? true, false, or null when it could not tell. Fetches objects only:
 * no ref, no FETCH_HEAD and no working tree change. */
export function mergeTreeClean(repo, snap) {
  try {
    const main = gh(["api", `repos/${repo}/commits/main`, "--jq", ".sha"]).trim();
    if (git(["fetch", "-q", "--no-tags", "--no-write-fetch-head", "origin", main, snap.headOid], { check: false }).code !== 0) {
      git(["fetch", "-q", "--no-tags", "--no-write-fetch-head", "origin", `+refs/pull/${snap.number}/head`, "+refs/heads/main"]);
    }
    const result = git(["merge-tree", "--write-tree", main, snap.headOid], { check: false });
    return result.code === 0 ? true : result.code === 1 ? false : null;
  } catch { return null; }
}

function stickyComment(repo, n, trusted) {
  const comments = JSON.parse(gh(["api", "--paginate", "--slurp", `repos/${repo}/issues/${n}/comments`])).flat();
  return trustedComment(comments, KEEPER_MARKER, trusted);
}

function upsertComment(repo, n, alerts, dryRun, trusted) {
  const existing = stickyComment(repo, n, trusted);
  const body = alertBody(alerts, { hadComment: !!existing });
  if (!body || existing?.body === body) return "unchanged";
  if (dryRun) return existing ? "would update alert comment" : "would post alert comment";
  if (existing) gh(["api", "-X", "PATCH", `repos/${repo}/issues/comments/${existing.id}`, "-f", `body=${body}`]);
  else gh(["api", "-X", "POST", `repos/${repo}/issues/${n}/comments`, "-f", `body=${body}`]);
  return existing ? "updated alert comment" : "posted alert comment";
}

function upsertIssue(repo, rows, now, dryRun) {
  const issues = JSON.parse(gh(["issue", "list", "--repo", repo, "--label", ISSUE_LABEL, "--state", "all", "--limit", "20", "--json", "number,title,state,body"]))
    .filter((i) => i.title === ISSUE_TITLE).sort((a, b) => b.number - a.number);
  const issue = issues[0] ?? null;
  const anyOpen = rows.some((r) => r.alerts.length);
  const body = issueBody(rows, now);
  if (!issue && !anyOpen) return "no issue needed";
  if (dryRun) return `would ${!issue ? "open" : anyOpen && issue.state !== "OPEN" ? "reopen" : !anyOpen && issue.state === "OPEN" ? "close" : "keep"} the tracking issue`;
  if (!issue) {
    gh(["label", "create", ISSUE_LABEL, "--repo", repo, "--color", "fbca04", "--description", "PR keeper: stalled PRs that need a person"], { check: false });
    const url = gh(["issue", "create", "--repo", repo, "--title", ISSUE_TITLE, "--label", ISSUE_LABEL, "--body", body]).trim();
    return `opened ${url}`;
  }
  if (alertTable(issue.body) !== alertTable(body)) gh(["issue", "edit", String(issue.number), "--repo", repo, "--body", body]);
  if (anyOpen && issue.state !== "OPEN") gh(["issue", "reopen", String(issue.number), "--repo", repo]);
  if (!anyOpen && issue.state === "OPEN") gh(["issue", "close", String(issue.number), "--repo", repo, "--reason", "completed"]);
  return `issue #${issue.number} ${anyOpen ? "open" : "closed"}`;
}

function runAction(repo, n, action, dryRun) {
  if (action.kind === "repair") return `repair requested (${action.ref})`;
  const what = action.kind === "unlabel" ? `removed ${action.label}` : describe(action.kind);
  if (dryRun) return `would have ${what}`;
  if (action.kind === "unlabel") gh(["pr", "edit", String(n), "--repo", repo, "--remove-label", action.label]);
  else if (action.kind === "queue") gh(["pr", "comment", String(n), "--repo", repo, "--body", QUEUE_COMMENT]);
  else if (action.kind === "refresh") gh(["pr", "comment", String(n), "--repo", repo, "--body", REFRESH_COMMENT]);
  else throw new Error(`unknown action ${action.kind}`);
  return what;
}

/** Seconds to wait before one more pass so a PR that just turned green is queued in this
 * run (the cron is unreliable here), or null. Capped at the green window plus a minute. */
export function settleSeconds(retries, greenMinutes = KEEPER_GREEN_MINUTES) {
  const due = retries.filter((m) => m != null);
  return due.length ? Math.ceil(Math.min(Math.max(...due), greenMinutes) * 60) + 60 : null;
}

const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

function keep({ repo, dryRun, settle = false, trusted = trustedAuthors(repo, { run: gh }) }) {
  const now = new Date().toISOString();
  const list = JSON.parse(gh(["pr", "list", "--repo", repo, "--base", "main", "--state", "open", "--limit", "100", "--json", "number"]));
  const rows = [], repairs = [];
  for (const { number } of list) {
    try {
      const snap = fetchSnapshot(repo, number);
      const clean = snap.labels.includes(NEEDS_MERGE_LABEL) && !skipReason(snap) ? mergeTreeClean(repo, snap) : null;
      const decision = decideKeeper(snap, { now, mergeTreeClean: clean, trusted });
      if (decision.skip) { rows.push({ number, alerts: [], log: [`skip: ${decision.skip}`] }); continue; }
      const log = decision.actions.map((a) => runAction(repo, number, a, dryRun));
      for (const a of decision.actions.filter((x) => x.kind === "repair")) repairs.push({ number, head: a.head, files: a.files, ref: a.ref });
      log.push(upsertComment(repo, number, decision.alerts, dryRun, trusted), ...decision.notes);
      rows.push({ number, alerts: decision.alerts, log, retry: decision.retryInMinutes });
    } catch (error) {
      console.log(`::warning title=PR keeper #${number}::${String(error.message).split("\n")[0]}`);
      rows.push({ number, alerts: [], log: [`error: ${String(error.message).split("\n")[0]}`], error: true });
    }
  }
  let issue = "";
  try { issue = upsertIssue(repo, rows.filter((r) => !r.error), now, dryRun); } catch (error) { issue = `issue error: ${error.message.split("\n")[0]}`; }
  const table = ["| PR | Alerts | Log |", "|---|---|---|", ...rows.map((r) => `| #${r.number} | ${r.alerts.map((a) => a.kind).join(", ") || "-"} | ${r.log.filter((l) => l && l !== "unchanged").join("; ").replace(/\|/g, "\\|")} |`)].join("\n");
  console.log(`${table}\n\nTracking issue: ${issue}${repairs.length ? `\nRepairs requested: ${repairs.map((r) => r.ref).join(", ")}` : ""}`);
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `## PR keeper${dryRun ? " (dry run)" : ""}\n\n${table}\n\nTracking issue: ${issue}\n`);
  const wait = settle && !dryRun ? settleSeconds(rows.map((r) => r.retry)) : null;
  if (wait !== null) {
    console.log(`waiting ${wait}s for PRs that just turned green, then one more pass`);
    sleep(wait * 1000);
    return [...repairs, ...keep({ repo, dryRun, settle: false, trusted })];
  }
  return repairs;
}

/** Which repair requests need a new bead, given the queue's beads ({ external_ref, status }):
 * none while the PR has an open repair bead (a mid-fix push makes a new head, not a new job),
 * and never twice for one head, whatever that bead's status. */
export function repairsToFile(requests, beads) {
  const refs = (beads ?? []).map((b) => ({ ref: b.external_ref ?? "", open: b.status !== "closed" }));
  const filed = [];
  for (const request of requests) {
    const prefix = `gh-pr-${request.number}-`;
    if (refs.some((b) => b.ref === request.ref) || refs.some((b) => b.open && b.ref.startsWith(prefix))) continue;
    filed.push(request);
    refs.push({ ref: request.ref, open: true });
  }
  return filed;
}

/** File the repair requests that need a bead, through scripts/dev/create-bead (pull, create, sync). */
function fileRepairs({ repo, file }) {
  const requests = JSON.parse(readFileSync(file, "utf8"));
  if (!requests.length) return;
  const ws = process.env.BRAUSER_QUEUE_WORKSPACE;
  if (!ws) throw new Error("BRAUSER_QUEUE_WORKSPACE is not set: no queue workspace to file repair beads in");
  const beads = JSON.parse(sh("bd", ["list", "--json", "--all", "--limit", "0"], { cwd: ws }).stdout);
  const toFile = repairsToFile(requests, beads);
  for (const request of requests) if (!toFile.includes(request)) console.log(`#${request.number}: a repair bead is already filed (${request.ref})`);
  for (const request of toFile) {
    sh("python3", ["scripts/dev/create-bead", ...repairBeadArgs(repo, request)], { env: { BD_NON_INTERACTIVE: "1" } });
    console.log(`#${request.number}: filed repair bead ${request.ref}`);
  }
}

function main(argv) {
  const dryRun = argv.includes("--dry-run");
  const at = argv.indexOf("--repo");
  const repo = at >= 0 ? argv[at + 1] : process.env.GITHUB_REPOSITORY;
  if (!repo) throw new Error("usage: node scripts/pr-keeper.mjs [--dry-run] [--repo owner/name] | --file-repairs <file>");
  const fileAt = argv.indexOf("--file-repairs");
  if (fileAt >= 0) return fileRepairs({ repo, file: argv[fileAt + 1] });
  const repairs = keep({ repo, dryRun, settle: process.env.PR_KEEPER_SETTLE === "1" });
  const unique = [...new Map(repairs.map((r) => [r.ref, r])).values()];
  if (process.env.PR_KEEPER_REPAIRS && !dryRun) writeFileSync(process.env.PR_KEEPER_REPAIRS, JSON.stringify(unique));
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `repairs=${dryRun ? 0 : unique.length}\n`);
}

function isEntryPoint() {
  try { return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href; } catch { return false; }
}
if (isEntryPoint()) {
  try { main(process.argv.slice(2)); } catch (error) { console.error(`pr-keeper: ${error.message}`); process.exit(1); }
}
