// PR doctor (AGENTS.md "PR doctor"): finds open PRs into main that GitHub
// reports as conflicting, which run no CI and so stall the merge queue silently,
// and either repairs them or says so on the PR.
//
// Repair merges main into the PR branch with main's merge rules and main's
// copy of the queue tools (never the PR's), then resolves only:
//   - .beads/issues.jsonl and BOSUN_FEATURE_BACKLOG.md, which left git: the
//     queue lives in bd's Dolt database, synced through refs/dolt/data
//     (AGENTS.md "Work queue"). A PR that still edits them conflicts as
//     modify/delete. The doctor applies the PR's bead edits to the database as
//     field-level updates through bd and syncs (`work_queue.py adopt`, in the
//     bootstrapped workspace $BRAUSER_QUEUE_WORKSPACE), merges the PR's
//     hand-written backlog edits into docs/backlog/frame.md, and drops both
//     files from the PR. An edit that conflicts is left to a person, with
//     instructions;
//   - Markdown conflicts where both sides only inserted text at the same point
//     (a diff3 hunk with an empty base), keeping main's text first, then the PR's.
// Anything else (code, a changed line, a file added on both sides) is left to
// a person: the PR gets the `needs-merge` label and one sticky comment listing
// the files. A repair is pushed without force, and only when PR_DOCTOR_PUSH=1
// (a token whose pushes start CI; the workflow's own GITHUB_TOKEN cannot);
// otherwise the doctor reports what it would do.
//
// Usage: node scripts/pr-doctor.mjs --tools <dir> [--dry-run]
//   <dir> holds main's work_queue.py, render_backlog.py and
//   check_queue_consistency.py. Run from a full clone of the repo whose
//   `origin` is GitHub; GH_TOKEN and GITHUB_REPOSITORY must be set.

import { spawnSync } from "node:child_process";
import { appendFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export const MARKER = "<!-- pr-doctor -->";
export const LABEL = "needs-merge";
export const SKIP_LABEL = "pr-doctor:skip";
// The Mergify queue label (.mergify.yml): a PR waiting to merge.
const QUEUE_LABEL = "queue";
// worker-contract.json merge.comment: what actually queues a labelled PR.
export const QUEUE_COMMENT = "@Mergifyio queue";
export const QUEUE_PATH = ".beads/issues.jsonl";
export const BACKLOG_PATH = "BOSUN_FEATURE_BACKLOG.md";
export const QUEUE_FILES = Object.freeze([QUEUE_PATH, BACKLOG_PATH]);
export const FRAME_PATH = "docs/backlog/frame.md";
const QUEUE_HELP = "the work queue is no longer in git (it lives in bd's Dolt database): note the branch tip and its merge base with main, `git rm` both queue files and commit, `git merge -Xfind-renames=100% origin/main` (exact renames only, or git pairs the old backlog with the new frame), then run `scripts/dev/queue adopt --from <old tip> --base <merge base>`: it applies the PR's bead edits through bd and carries its backlog prose into docs/backlog/frame.md; commit the frame";

// --- Pure parts (scripts/tests/pr-doctor.test.mjs) ---------------------------

/** Resolves diff3 conflict hunks whose base is empty, i.e. both sides only
 * inserted text at the same point: theirs (main) first, then ours (the PR).
 * Any other hunk leaves the whole file unresolved and unchanged. */
export function resolveAppendOnly(text) {
  const lines = text.split(/(?<=\n)/);
  const out = [];
  let remaining = 0;
  for (let i = 0; i < lines.length; i++) {
    if (!lines[i].startsWith("<<<<<<< ")) {
      out.push(lines[i]);
      continue;
    }
    const ours = [], base = [], theirs = [];
    // A hunk is exactly ours, one base line, ours/theirs separator, theirs.
    // Any extra or nested marker-like line (say a Markdown setext underline,
    // "=======", inside a side) makes the split ambiguous, so it is left to
    // a person rather than guessed.
    let part = ours, j = i + 1, closed = false, hasBase = false, ambiguous = false, separated = false;
    for (; j < lines.length; j++) {
      const line = lines[j];
      if (line.startsWith("<<<<<<< ")) { ambiguous = true; part.push(line); continue; }
      if (line.startsWith("||||||| ") || line === "|||||||\n") {
        if (hasBase || separated) ambiguous = true;
        part = base; hasBase = true; continue;
      }
      if (line === "=======\n" || line === "=======") {
        if (separated || !hasBase) ambiguous = true;
        part = theirs; separated = true; continue;
      }
      if (line.startsWith(">>>>>>> ")) { closed = true; break; }
      part.push(line);
    }
    if (!closed || !hasBase || ambiguous || base.length > 0) {
      remaining++;
      out.push(...lines.slice(i, j + 1));
    } else {
      const lastTheirs = theirs.at(-1) ?? "";
      const firstOurs = ours[0] ?? "";
      const gap = theirs.length > 0 && ours.length > 0 && lastTheirs.trim() !== "" && firstOurs.startsWith("#")
        ? ["\n"] : [];
      out.push(...theirs, ...gap, ...ours);
    }
    i = j;
  }
  return remaining === 0
    ? { resolved: true, remaining: 0, text: out.join("") }
    : { resolved: false, remaining, text };
}

/** "skip", "pending" (GitHub has not computed mergeability), "conflicting" or "clean". */
export function classifyPr(pr) {
  if (pr.baseRefName !== "main" || pr.isDraft || pr.isCrossRepository) return "skip";
  if ((pr.labels ?? []).some((label) => label.name === SKIP_LABEL)) return "skip";
  if (pr.mergeable === "CONFLICTING") return "conflicting";
  if (pr.mergeable === "UNKNOWN" || !pr.mergeable) return "pending";
  return "clean";
}

const list = (files) => files.map((file) => `- \`${file}\``).join("\n");

/** The sticky comment for an outcome; never mentions anyone. */
export function commentBody({ outcome, files = [], sha = "", queued = false, resolvable = false, requeued = false }) {
  const head = `${MARKER}\n<!-- pr-doctor outcome=${outcome} -->\n**PR doctor**\n\n`;
  const waiting = queued ? " It is labeled `queue` and waits until this is fixed: a conflicting PR runs no CI and the Mergify queue cannot take it." : " A conflicting PR runs no CI.";
  switch (outcome) {
    case "repaired":
      return head + `Merged \`main\` into this branch as ${sha}. The conflicts were only in the work queue (its bead edits applied through bd, its backlog prose moved to \`${FRAME_PATH}\`, and the files dropped from this PR) or both-sides-appended Markdown, resolved with main's text first:\n\n${list(files)}\n\nCI runs on the new commit.${requeued ? ` The push cancelled this PR's queue request, so the doctor commented \`${QUEUE_COMMENT}\` again.` : ""} Pull before pushing again.`;
    case "needs-person":
      return head + `This PR conflicts with \`main\` in files the doctor does not resolve on its own.${waiting} The files:\n\n${list(files)}\n\nMerge \`main\` into the branch and resolve these by hand. For \`${QUEUE_PATH}\` or \`${BACKLOG_PATH}\`: ${QUEUE_HELP}. The \`${LABEL}\` label clears on the next run after the PR merges cleanly.`;
    case "report-only":
      return head + `This PR conflicts with \`main\`.${waiting}\n\n${resolvable
        ? `The doctor could resolve it (work-queue files or both-sides-appended Markdown only) but has no push token (\`PR_DOCTOR_TOKEN\`), so it did not:`
        : "Files needing a person:"}\n\n${list(files)}\n\nMerge \`main\` into the branch to unblock it.`;
    case "resolved":
      return head + "The conflict with `main` is gone.";
    default:
      throw new Error(`unknown outcome ${outcome}`);
  }
}

/** Whether the doctor must ask Mergify to queue a PR again after its push:
 * a push cancels a pending `@Mergifyio queue` request, whoever pushes. Only
 * the label marks a PR the merge queue should take (an armed auto-merge is
 * not a queue request). */
export function needsRequeue(pr, outcome) {
  return outcome === "repaired" && (pr.labels ?? []).some((item) => item.name === QUEUE_LABEL);
}

/** Comment QUEUE_COMMENT once GitHub reports `sha` as the PR's head, polling
 * every `stepMs` for at most `limitMs`: a comment Mergify reads before the
 * push lands is cancelled by it. Past the limit it comments anyway (as the
 * remote-worker driver's contract_requeue_after_push does): a late request
 * beats none. A failed read counts as not yet. */
export function requeueAfterPush({ sha, headOf, comment, wait, limitMs = 60000, stepMs = 5000 }) {
  let seen = null;
  for (let waited = 0; ; waited += stepMs) {
    try { seen = headOf(); } catch { seen = null; }
    if (seen === sha || waited >= limitMs) break;
    wait(stepMs);
  }
  comment(QUEUE_COMMENT);
  return { settled: seen === sha, seen };
}

// --- I/O ----------------------------------------------------------------------

function sh(cmd, args, { cwd, check = true, input } = {}) {
  const result = spawnSync(cmd, args, { cwd, input, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (check && result.status !== 0) {
    throw new Error(`${cmd} ${args.join(" ")} exited ${result.status}: ${(result.stderr || result.stdout).trim()}`);
  }
  return { code: result.status, stdout: result.stdout, stderr: result.stderr };
}
const git = (args, opts) => sh("git", args, opts);
const gh = (args, opts) => sh("gh", args, opts);
const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

const FIELDS = "number,title,url,headRefName,baseRefName,isDraft,isCrossRepository,mergeable,labels,autoMergeRequest";

function listPrs(repo) {
  return JSON.parse(gh(["pr", "list", "--repo", repo, "--base", "main", "--state", "open", "--limit", "100", "--json", FIELDS]).stdout);
}

function settle(repo, pr) {
  // GitHub computes mergeability lazily; asking for it starts the job.
  for (let attempt = 0; pr.mergeable === "UNKNOWN" && attempt < 4; attempt++) {
    sleep(5000);
    pr = JSON.parse(gh(["pr", "view", String(pr.number), "--repo", repo, "--json", FIELDS]).stdout);
  }
  return pr;
}

const unmerged = () => git(["diff", "--name-only", "--diff-filter=U"]).stdout.split("\n").filter(Boolean);

/** Main's copy of the queue tool, so no code from the PR branch runs here. */
function workQueue(tools, args) {
  const python = process.env.PR_DOCTOR_PYTHON || "python3";
  return sh(python, [join(tools, "work_queue.py"), "--repo", ".", ...args], { check: false });
}

/** `work_queue.py adopt` arguments: apply the PR's committed queue edits
 * (`from` against `base`) through bd and sync, or only check them (`push`
 * false, or `frame` given: a check against main's frame before the merge). */
export function adoptArgs({ from, base, push, frame = null }) {
  return ["adopt", "--from", from, "--base", base, ...(frame ? ["--frame", frame] : []), ...(push ? [] : ["--dry-run"])];
}

function adopt(tools, args) {
  const result = workQueue(tools, adoptArgs(args));
  const detail = (result.stdout + result.stderr).trim().split("\n").filter((line) => line && !line.startsWith("Warning:")).at(-1) ?? "";
  return { ok: result.code === 0, detail };
}

function stage(number, file) {
  const result = git(["show", `:${number}:${file}`], { check: false });
  return result.code === 0 ? result.stdout : null;
}

const exists = (spec) => git(["cat-file", "-e", spec], { check: false }).code === 0;

/** Merges main into the PR branch in this clone; { ok, files, sha? }.
 * Queue edits are synced to refs/dolt/data only when `canPush`. */
export function tryRepair(pr, tools, canPush) {
  const branch = pr.headRefName;
  git(["fetch", "-q", "--no-tags", "origin", "+refs/heads/main:refs/remotes/origin/main", `+refs/heads/${branch}:refs/remotes/origin/${branch}`]);
  git(["checkout", "-q", "-f", "-B", "pr-doctor/work", `origin/${branch}`]);
  git(["clean", "-qfd"]);
  // Main's merge rules apply even when the branch predates them.
  const gitDir = git(["rev-parse", "--git-common-dir"]).stdout.trim();
  mkdirSync(join(gitDir, "info"), { recursive: true });
  writeFileSync(join(gitDir, "info", "attributes"), git(["show", "origin/main:.gitattributes"], { check: false }).stdout);

  // A PR cut before the queue left git still edits the committed files.
  // Check its edits against the database and main's frame first, then drop
  // the files on the PR side before merging, so the merge never sees them.
  const tip = git(["rev-parse", "HEAD"]).stdout.trim();
  const base = git(["merge-base", "HEAD", "origin/main"]).stdout.trim();
  const queueMoved = !exists(`origin/main:${QUEUE_PATH}`) && exists(`origin/main:${FRAME_PATH}`);
  const carried = queueMoved ? QUEUE_FILES.filter((file) => exists(`HEAD:${file}`)) : [];
  const tmp = mkdtempSync(join(tmpdir(), "pr-doctor-"));
  try {
    if (carried.length > 0) {
      writeFileSync(join(tmp, "frame.md"), git(["show", `origin/main:${FRAME_PATH}`]).stdout);
      const checked = adopt(tools, { from: tip, base, push: false, frame: join(tmp, "frame.md") });
      if (!checked.ok) return { ok: false, files: carried.map((file) => `${file} (${checked.detail || "adopt failed"})`) };
      git(["rm", "-q", "--", ...carried]);
      git(["commit", "-q", "-m", "Drop the work-queue files (PR doctor)\n\nTheir edits are applied through bd in the merge that follows (AGENTS.md \"Work queue\")."]);
    }

    const message = `Merge main into ${branch} (PR doctor)\n\nAutomatic: work-queue and both-sides-appended Markdown conflicts only (scripts/pr-doctor.mjs).`;
    // Exact renames only: main's frame is most of the old backlog's text, and
    // git would otherwise pair them and conflict on the frame.
    const merge = git(["-c", "merge.conflictStyle=diff3", "merge", "-q", "--no-ff", "--no-commit", "-Xfind-renames=100%", "-m", message, "origin/main"], { check: false });
    const resolved = [];
    if (merge.code !== 0) {
      const files = unmerged();
      if (files.length === 0) {
        git(["merge", "--abort"], { check: false });
        return { ok: false, files: ["(git merge failed without conflicts)"] };
      }
      for (const file of files) {
        if (!file.endsWith(".md")) continue;
        const stages = [1, 2, 3].map((number) => stage(number, file));
        if (stages.some((text) => text === null)) continue;
        const [mergeBase, ours, theirs] = stages;
        for (const [name, text] of Object.entries({ ours, base: mergeBase, theirs })) writeFileSync(join(tmp, name), text);
        const merged = git(["merge-file", "-p", "--diff3", "-L", "ours", "-L", "base", "-L", "theirs",
          join(tmp, "ours"), join(tmp, "base"), join(tmp, "theirs")], { check: false }).stdout;
        const result = resolveAppendOnly(merged);
        if (!result.resolved) continue;
        writeFileSync(file, result.text);
        git(["add", "--", file]);
        if (!unmerged().includes(file)) resolved.push(file);
      }
      const left = unmerged();
      if (left.length > 0) {
        git(["merge", "--abort"], { check: false });
        return { ok: false, files: left };
      }
    }
    if (carried.length > 0) {
      // The merge tree has main's frame now; adopt merges the PR's backlog
      // prose into it and applies the bead edits through bd.
      const applied = adopt(tools, { from: tip, base, push: canPush });
      if (!applied.ok) {
        git(["merge", "--abort"], { check: false });
        return { ok: false, files: carried.map((file) => `${file} (${applied.detail || "adopt failed"})`) };
      }
      git(["add", "--", FRAME_PATH]);
      for (const file of carried) resolved.push(`${file} (edits applied through bd; prose in ${FRAME_PATH})`);
    }
    if (git(["rev-parse", "-q", "--verify", "MERGE_HEAD"], { check: false }).code === 0) git(["commit", "-q", "--no-edit"]);
    const kept = resolved.filter((file) => !/ \(/.test(file));
    const markers = kept.length === 0 ? "" : git(["grep", "-l", "-E", "^(<{7}|>{7}) (ours|theirs)", "--", ...kept], { check: false }).stdout.trim();
    if (markers) return { ok: false, files: markers.split("\n") };
    return { ok: true, files: resolved.length ? resolved : ["(merged cleanly)"], sha: git(["rev-parse", "--short=12", "HEAD"]).stdout.trim() };
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

function stickyComment(repo, number) {
  const comments = JSON.parse(gh(["api", "--paginate", "--slurp", `repos/${repo}/issues/${number}/comments`]).stdout).flat();
  return comments.find((comment) => (comment.body ?? "").startsWith(MARKER)) ?? null;
}

const outcomeOf = (comment) => /pr-doctor outcome=([\w-]+)/.exec(comment?.body ?? "")?.[1] ?? null;

function report(repo, pr, body, { dryRun, label }) {
  if (dryRun) return;
  const existing = stickyComment(repo, pr.number);
  if (!existing) gh(["api", "-X", "POST", `repos/${repo}/issues/${pr.number}/comments`, "-f", `body=${body}`]);
  else if (existing.body !== body) gh(["api", "-X", "PATCH", `repos/${repo}/issues/comments/${existing.id}`, "-f", `body=${body}`]);
  const labelled = (pr.labels ?? []).some((item) => item.name === LABEL);
  if (label && !labelled) gh(["pr", "edit", String(pr.number), "--repo", repo, "--add-label", LABEL]);
  if (!label && labelled) gh(["pr", "edit", String(pr.number), "--repo", repo, "--remove-label", LABEL]);
}

function main() {
  const args = process.argv.slice(2);
  const dryRun = args.includes("--dry-run");
  const toolsAt = args.indexOf("--tools");
  const tools = toolsAt >= 0 ? args[toolsAt + 1] : null;
  const repo = process.env.GITHUB_REPOSITORY;
  if (!tools || !repo) throw new Error("usage: GITHUB_REPOSITORY=owner/name node scripts/pr-doctor.mjs --tools <dir> [--dry-run]");
  for (const file of ["work_queue.py", "render_backlog.py", "check_queue_consistency.py"]) readFileSync(join(tools, file));
  const canPush = process.env.PR_DOCTOR_PUSH === "1" && !dryRun;
  if (!dryRun) {
    gh(["label", "create", LABEL, "--repo", repo, "--color", "d93f0b", "--force",
      "--description", "Conflicts with main; the PR doctor could not resolve it alone"], { check: false });
  }

  const rows = [];
  for (let pr of listPrs(repo)) {
    try {
      if (classifyPr(pr) === "pending") pr = settle(repo, pr);
      const kind = classifyPr(pr);
      const queued = pr.autoMergeRequest != null || (pr.labels ?? []).some((item) => item.name === QUEUE_LABEL);
      if (kind === "skip" || kind === "pending") {
        rows.push([pr.number, kind, ""]);
        continue;
      }
      if (kind === "clean") {
        const labelled = (pr.labels ?? []).some((item) => item.name === LABEL);
        const comment = labelled || !dryRun ? stickyComment(repo, pr.number) : null;
        const stale = labelled || ["needs-person", "report-only"].includes(outcomeOf(comment));
        if (stale) report(repo, pr, commentBody({ outcome: "resolved" }), { dryRun, label: false });
        rows.push([pr.number, stale ? "resolved" : "clean", ""]);
        continue;
      }
      const result = tryRepair(pr, tools, canPush);
      let outcome;
      if (!result.ok) outcome = canPush ? "needs-person" : "report-only";
      else if (!canPush) outcome = "report-only";
      else {
        const push = git(["push", "-q", "origin", `HEAD:refs/heads/${pr.headRefName}`], { check: false });
        if (push.code !== 0) {
          rows.push([pr.number, "skipped", "the branch moved during the repair; next run retries"]);
          continue;
        }
        outcome = "repaired";
      }
      const requeued = needsRequeue(pr, outcome);
      const body = commentBody({ outcome, files: result.files, sha: result.sha, queued, resolvable: result.ok, requeued });
      report(repo, pr, body, { dryRun, label: outcome !== "repaired" });
      let note = "";
      if (requeued) {
        const sha = git(["rev-parse", "HEAD"]).stdout.trim();
        const { settled, seen } = requeueAfterPush({
          sha,
          headOf: () => JSON.parse(gh(["api", `repos/${repo}/pulls/${pr.number}`]).stdout).head.sha,
          comment: (text) => gh(["api", "-X", "POST", `repos/${repo}/issues/${pr.number}/comments`, "-f", `body=${text}`]),
          wait: sleep,
        });
        if (!settled) console.log(`::warning title=PR doctor #${pr.number}::GitHub still showed ${seen ?? "no head"}, not ${sha.slice(0, 12)}, after 60s; requeued anyway`);
        note = " (requeued)";
      }
      rows.push([pr.number, outcome + note + (outcome === "report-only" && result.ok ? " (resolvable)" : ""), result.files.join(", ")]);
    } catch (error) {
      console.log(`::warning title=PR doctor #${pr.number}::${String(error.message).split("\n")[0]}`);
      rows.push([pr.number, "error", String(error.message).split("\n")[0]]);
      git(["merge", "--abort"], { check: false });
    }
  }
  git(["checkout", "-q", "-f", "--detach", "origin/main"], { check: false });

  const table = ["| PR | Outcome | Files |", "|---|---|---|", ...rows.map(([n, o, f]) => `| #${n} | ${o} | ${f} |`)].join("\n");
  console.log(table);
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `## PR doctor${dryRun ? " (dry run)" : canPush ? "" : " (report only)"}\n\n${table}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) main();
