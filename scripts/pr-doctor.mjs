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
//     (a diff3 hunk with an empty base), keeping main's text first, then the PR's;
//   - generated files (.gitattributes `merge=brauser-generated`, including
//     AGENTS.md's Current state table, generated from one file per row): main's copy
//     of scripts/merge-generated.mjs regenerates them from the merged sources
//     with main's generators and their --check, and after the merge every
//     output of a generator it ran is rewritten from that same regeneration,
//     so an output only one side changed cannot stay stale. A merge that
//     changes a generator, or conflicts in its inputs, is left to a person.
// Anything else (code, a changed line, a file added on both sides) is left to
// a person: the PR gets the `needs-merge` label and one sticky comment listing
// the files. A repair is pushed without force, and only when PR_DOCTOR_PUSH=1
// (a token whose pushes start CI; the workflow's own GITHUB_TOKEN cannot);
// otherwise the doctor reports what it would do.
//
// Usage: node scripts/pr-doctor.mjs --tools <dir> [--phase plan|repair|publish]
//          [--state <file>] [--dry-run]
//   <dir> holds main's work_queue.py, render_backlog.py,
//   check_queue_consistency.py, merge-generated.mjs, generate-starters.mjs,
//   generate-current-state.mjs and protocol/generate.py (as generate.py). Run from a full clone of the repo whose
//   `origin` is GitHub.
//
// Phases (brwr-p2a7), one workflow job each, so the merge and regeneration
// run in a job that has no secret and a GITHUB_TOKEN with no permissions
// (its runner still has network and the Actions runtime token), and so the
// job that pushes reads what that job leaves behind only as data:
//   plan     (token) lists the PRs, settles mergeability, updates PRs whose
//            conflict is gone, fetches each conflicting one, writes the state
//            and the list of PRs to repair (job outputs `repairs`, `prs`)
//            and, with --bundle-out, a git bundle of main and those branches.
//   repair   (no credential, no remote: it reads only the bundle) merges main
//            into each, regenerates, keeps each result in
//            refs/pr-doctor/<number>, checks queue edits against its copy of
//            the database, and writes the state and, with --bundle-out, a
//            bundle of those refs.
//   publish  (token, a fresh runner with its own checkout of main and main's
//            tools) takes the PR list from --prs (the plan job's output),
//            reads each PR's branch and head from the API and refuses main.
//            It pushes a repaired commit only when its parents are the PR
//            head (or a commit that only drops the queue files from it) and a
//            commit of main, and its tree is the merge publish derives itself
//            (`unexplainedPaths`: git merge-tree, the append-only Markdown
//            rule, and main's generators run with no credential). It
//            re-applies queue edits with main's tools in a fresh workspace
//            per PR (apply, compare the frame with the repaired one, sync),
//            pushes without force with hooks and global and system git config
//            off, and comments, labels and requeues. A PR the repair job left
//            no checked result for is labelled needs-merge.
// The phases hand over through --state (default <dir>/doctor-state.json).
// With no --phase, all three run in turn in one process.

import { spawnSync } from "node:child_process";
import { appendFileSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

import { GENERATORS, configure as configureMergeDrivers, driverCommand, regenerate } from "./merge-generated.mjs";
import { DOCTOR_MARKER, DOCTOR_SKIP_LABEL, NEEDS_MERGE_LABEL, QUEUE_COMMENT, QUEUE_LABEL } from "./pr-constants.mjs";
import { queueAfterPush } from "./dev/queue-pr.mjs";

export const MARKER = DOCTOR_MARKER;
export const LABEL = NEEDS_MERGE_LABEL;
export const SKIP_LABEL = DOCTOR_SKIP_LABEL;
// worker-contract.json merge.comment: what actually queues a labelled PR.
export { QUEUE_COMMENT };
export const QUEUE_PATH = ".beads/issues.jsonl";
export const BACKLOG_PATH = "BOSUN_FEATURE_BACKLOG.md";
export const QUEUE_FILES = Object.freeze([QUEUE_PATH, BACKLOG_PATH]);
export const FRAME_PATH = "docs/backlog/frame.md";
export const NOTES_DIR = "docs/backlog/notes";
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

/** Comment QUEUE_COMMENT once GitHub reports `sha` as the PR's head (queue-pr's
 * `queueAfterPush`, shared with scripts/dev/queue-pr.mjs): a comment Mergify reads
 * before the push lands is cancelled by it. Past the limit it comments anyway (as the
 * remote-worker driver's contract_requeue_after_push does): a late request beats none. */
export function requeueAfterPush({ sha, headOf, comment, wait, limitMs = 60000, stepMs = 5000 }) {
  const { settled, seen } = queueAfterPush({ sha, headOf, comment, wait, limitMs, stepMs, force: true });
  return { settled, seen };
}

// --- I/O ----------------------------------------------------------------------

function sh(cmd, args, { cwd, check = true, input, env } = {}) {
  const result = spawnSync(cmd, args, { cwd, input, env: env ? { ...process.env, ...env } : process.env, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
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
function workQueue(tools, args, repo = ".", env = undefined) {
  const python = process.env.PR_DOCTOR_PYTHON || "python3";
  return sh(python, [join(tools, "work_queue.py"), "--repo", repo, ...args], { check: false, env });
}

/** `work_queue.py adopt` arguments: apply the PR's committed queue edits
 * (`from` against `base`) through bd locally, for the publish phase to sync
 * (`push`; `--no-sync` makes no network call), or only check them (`push`
 * false, or `frame` given: a check against main's frame before the merge). */
export function adoptArgs({ from, base, push, frame = null }) {
  return ["adopt", "--from", from, "--base", base, ...(frame ? ["--frame", frame] : []), push ? "--no-sync" : "--dry-run"];
}

/** Main's queue tool `sync`: publishes the bead edits the repair phase
 * applied (it pulls, checks, and runs bd sync), checking the queue against
 * the frame and notes in `repo`. */
export function syncQueue(tools, repo = ".", env = undefined) {
  const result = workQueue(tools, ["sync"], repo, env);
  const detail = (result.stdout + result.stderr).trim().split("\n").filter((line) => line && !line.startsWith("Warning:")).at(-1) ?? "";
  return { ok: result.code === 0, detail };
}

function adopt(tools, args, env = undefined) {
  const result = workQueue(tools, adoptArgs(args), ".", env);
  const detail = (result.stdout + result.stderr).trim().split("\n").filter((line) => line && !line.startsWith("Warning:")).at(-1) ?? "";
  return { ok: result.code === 0, detail };
}

function stage(number, file) {
  const result = git(["show", `:${number}:${file}`], { check: false });
  return result.code === 0 ? result.stdout : null;
}

const isSymlink = (file) => {
  try {
    return lstatSync(file).isSymbolicLink();
  } catch {
    return false;
  }
};

const exists = (spec) => git(["cat-file", "-e", spec], { check: false }).code === 0;

/** After a merge, the files the generated-file driver resolved (its log),
 * each named with how. For every generator it regenerated, all of that
 * generator's outputs are written from the same regeneration and staged:
 * git takes an output only one side changed without asking the driver, and
 * that side's copy can be stale against the merged sources. */
function regeneratedFiles({ log, ours, other, tools, refused = [] }) {
  let lines = [];
  try {
    lines = readFileSync(log, "utf8").split("\n").filter(Boolean).map((line) => line.split("\t"));
  } catch {
    return [];
  }
  const out = lines.map(([path, how]) => `${path} (${how === "union" ? "union of both sides' ids" : "regenerated from the merged sources"})`);
  for (const generator of GENERATORS) {
    if (!lines.some(([path, how]) => how === "regenerated" && generator.outputs.includes(path))) continue;
    const result = regenerate(generator, { ours, other, tools });
    if (!result.ok) continue;
    for (const [path, text] of Object.entries(result.files)) {
      if (lines.some(([logged]) => logged === path)) continue;
      // Never write through a symlink the branch committed (#463 review I2).
      if (isSymlink(path)) {
        refused.push(path);
        continue;
      }
      writeFileSync(path, text);
      git(["add", "--", path]);
    }
  }
  return out;
}

/** Fetches main and the PR branch, for `tryRepair` to merge (the plan phase:
 * the repair phase has no credential to fetch with). */
export function fetchForRepair(pr) {
  const branch = pr.headRefName;
  git(["fetch", "-q", "--no-tags", "origin", "+refs/heads/main:refs/remotes/origin/main", `+refs/heads/${branch}:refs/remotes/origin/${branch}`]);
}

/** Merges main into the already-fetched PR branch in this clone, with no
 * network call; { ok, files, sha?, adopted? }. When `canPush`, queue edits
 * are applied to the local database (`adopted`, with the `carried` files)
 * for `syncQueue` to publish; otherwise they are only checked. */
export function tryRepair(pr, tools, canPush) {
  const branch = pr.headRefName;
  git(["checkout", "-q", "-f", "-B", "pr-doctor/work", `origin/${branch}`]);
  git(["clean", "-qfd"]);
  // Main's merge rules apply even when the branch predates them.
  const gitDir = git(["rev-parse", "--git-common-dir"]).stdout.trim();
  mkdirSync(join(gitDir, "info"), { recursive: true });
  writeFileSync(join(gitDir, "info", "attributes"), git(["show", "origin/main:.gitattributes"], { check: false }).stdout);
  // Main's generated-file merge driver, running main's generators only.
  configureMergeDrivers({ tools });

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

    const message = `Merge main into ${branch} (PR doctor)\n\nAutomatic: work-queue, both-sides-appended Markdown and regenerated generated-file conflicts only (scripts/pr-doctor.mjs).`;
    const ours = git(["rev-parse", "HEAD"]).stdout.trim();
    const other = git(["rev-parse", "origin/main"]).stdout.trim();
    const driverLog = join(tmp, "merge-driver.log");
    const driverEnv = { BRAUSER_MERGE_TOOLS: tools, BRAUSER_MERGE_LOG: driverLog, BRAUSER_MERGE_PYTHON: process.env.PR_DOCTOR_PYTHON || "python3" };
    // Exact renames only: main's frame is most of the old backlog's text, and
    // git would otherwise pair them and conflict on the frame.
    const merge = git(["-c", "merge.conflictStyle=diff3", "merge", "-q", "--no-ff", "--no-commit", "-Xfind-renames=100%", "-m", message, "origin/main"], { check: false, env: driverEnv });
    const refused = [];
    const resolved = regeneratedFiles({ log: driverLog, ours, other, tools, refused });
    if (refused.length) {
      git(["merge", "--abort"], { check: false });
      return { ok: false, files: refused.map((file) => `${file} (a symlink; the doctor never writes through one)`) };
    }
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
        // Never write through a symlink the branch committed (#463 review I2).
        if (!result.resolved || isSymlink(file)) continue;
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
    return {
      ok: true,
      files: resolved.length ? resolved : ["(merged cleanly)"],
      sha: git(["rev-parse", "--short=12", "HEAD"]).stdout.trim(),
      adopted: canPush && carried.length > 0,
      carried,
    };
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

const PHASES = Object.freeze(["plan", "repair", "publish"]);
const STATE_VERSION = 2;
const warn = (pr, error) => {
  const line = String(error.message).split("\n")[0];
  console.log(`::warning title=PR doctor #${pr.number}::${line}`);
  return line;
};
/** Git calls the publish phase makes over data the repair job produced: no
 * hooks, no global or system config (GIT_CONFIG_GLOBAL and
 * GIT_CONFIG_NOSYSTEM are set on the step too). */
const safeGit = (args, opts) => git(["-c", "core.hooksPath=/dev/null", ...args], opts);

function summary(title, rows) {
  const table = ["| PR | Outcome | Files |", "|---|---|---|", ...rows.map(([n, o, f]) => `| #${n} | ${o} | ${f} |`)].join("\n");
  console.log(table);
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `## ${title}\n\n${table}\n`);
}

/** Lists the PRs, settles each one's mergeability, updates a PR whose
 * conflict is gone, and fetches each conflicting one. One item per PR, in
 * list order: a finished `row`, or `repair: true` for the repair phase. */
function planPhase({ repo, dryRun }) {
  if (!dryRun) {
    gh(["label", "create", LABEL, "--repo", repo, "--color", "d93f0b", "--force",
      "--description", "Conflicts with main; the PR doctor could not resolve it alone"], { check: false });
  }
  const items = [];
  for (let pr of listPrs(repo)) {
    try {
      if (classifyPr(pr) === "pending") pr = settle(repo, pr);
      const kind = classifyPr(pr);
      if (kind === "skip" || kind === "pending") {
        items.push({ pr, row: [pr.number, kind, ""] });
        continue;
      }
      if (kind === "clean") {
        const labelled = (pr.labels ?? []).some((item) => item.name === LABEL);
        const comment = labelled || !dryRun ? stickyComment(repo, pr.number) : null;
        const stale = labelled || ["needs-person", "report-only"].includes(outcomeOf(comment));
        if (stale) report(repo, pr, commentBody({ outcome: "resolved" }), { dryRun, label: false });
        items.push({ pr, row: [pr.number, stale ? "resolved" : "clean", ""] });
        continue;
      }
      fetchForRepair(pr);
      items.push({ pr: { number: pr.number, headRefName: pr.headRefName }, repair: true });
    } catch (error) {
      items.push({ pr, row: [pr.number, "error", warn(pr, error)] });
    }
  }
  return items;
}

/** Merges main into each fetched PR and keeps the result in
 * refs/pr-doctor/<number>. Makes no network call and needs no credential. */
function repairPhase(items, { tools, canPush }) {
  for (const item of items) {
    if (!item.repair) continue;
    const { pr } = item;
    try {
      const result = tryRepair(pr, tools, canPush);
      if (result.ok) git(["update-ref", `refs/pr-doctor/${pr.number}`, "HEAD"]);
      item.result = { ok: result.ok, files: result.files };
    } catch (error) {
      item.repair = false;
      item.row = [pr.number, "error", warn(pr, error)];
      git(["merge", "--abort"], { check: false });
    }
  }
  git(["checkout", "-q", "-f", "--detach", "origin/main"], { check: false });
  return items;
}

/** A file name from the untrusted repair state, made safe to quote in a
 * comment: one line, no backticks or mentions, bounded. */
export function cleanFile(text) {
  return String(text).replace(/[\u0000-\u001f\u007f`@<>|]/g, " ").slice(0, 200);
}

/** A blob's text at `commit:file`, or null when there is none. */
function blobText(commit, file) {
  const result = safeGit(["cat-file", "blob", `${commit}:${file}`], { check: false });
  return result.code === 0 ? result.stdout : null;
}

/** What the doctor's append-only rule makes of a conflicted Markdown file,
 * from the same diff3 stages tryRepair used, or null when it leaves the file
 * to a person. Pure apart from reading blobs. */
function appendOnlyResolution(base, first, second, file) {
  const stages = [base, first, second].map((commit) => blobText(commit, file));
  if (stages.some((text) => text === null)) return null;
  const tmp = mkdtempSync(join(tmpdir(), "pr-doctor-md-"));
  try {
    const [mergeBase, ours, theirs] = stages;
    for (const [name, text] of Object.entries({ ours, base: mergeBase, theirs })) writeFileSync(join(tmp, name), text);
    const merged = safeGit(["merge-file", "-p", "--diff3", "-L", "ours", "-L", "base", "-L", "theirs",
      join(tmp, "ours"), join(tmp, "base"), join(tmp, "theirs")], { check: false }).stdout;
    const result = resolveAppendOnly(merged);
    return result.resolved ? result.text : null;
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

const CREDENTIAL_ENV = /^(GH_TOKEN|GITHUB_TOKEN|DOCTOR_GIT_TOKEN|PR_DOCTOR_TOKEN|GIT_CONFIG_(COUNT|KEY_\d+|VALUE_\d+)|GIT_ASKPASS|SSH_AUTH_SOCK)$/;

/** merge-generated.mjs `regenerate`, run the way the doctor's merge ran it:
 * its inner merge goes through main's copy of the driver (from `tools`), with
 * every token and git credential variable unset for the duration (the
 * generators themselves already get only `generatorEnv`). */
function regenerateWithoutCredentials(generator, { ours, other, tools }) {
  const saved = { ...process.env };
  try {
    for (const name of Object.keys(process.env)) if (CREDENTIAL_ENV.test(name)) delete process.env[name];
    Object.assign(process.env, {
      GIT_CONFIG_COUNT: "2",
      GIT_CONFIG_KEY_0: "merge.brauser-generated.name",
      GIT_CONFIG_VALUE_0: "Brauser generated files (PR doctor publish check)",
      GIT_CONFIG_KEY_1: "merge.brauser-generated.driver",
      GIT_CONFIG_VALUE_1: driverCommand({ tools }),
      BRAUSER_MERGE_TOOLS: tools,
      BRAUSER_MERGE_PYTHON: process.env.PR_DOCTOR_PYTHON || "python3",
    });
    return regenerate(generator, { ours, other, tools });
  } finally {
    for (const name of Object.keys(process.env)) if (!(name in saved)) delete process.env[name];
    Object.assign(process.env, saved);
  }
}

/** The paths where `commit` is not the merge publish itself derives from
 * (first, second) (#463 re-review I-new-1). Publish runs `git merge-tree`
 * (no merge driver is configured in its clone, hooks and global config are
 * off), then accepts a path that differs from that merge, or that merge-tree
 * left conflicted, only when it is:
 *   - a conflicted Markdown file equal to the append-only resolution;
 *   - a generated output equal to what main's generator makes of the merged
 *     sources (merge-generated.mjs `regenerate`, with main's tools and a
 *     credential-free environment);
 *   - the backlog frame of a PR whose queue files were dropped, which
 *     `adoptAndSync` checks against main's adopt before anything is pushed.
 * Anything else, a quiet revert of main's change included, is returned. */
export function unexplainedPaths({ commit, first, second, tools, carried = [] }) {
  const merge = safeGit(["merge-tree", "--write-tree", "--name-only", "--no-messages", first, second], { check: false });
  if (merge.code !== 0 && merge.code !== 1) return ["(git merge-tree failed)"];
  const [tree, ...conflictedList] = merge.stdout.split("\n").filter(Boolean);
  const conflicted = new Set(conflictedList);
  const differing = safeGit(["diff", "--name-only", "--no-renames", tree, commit]).stdout.split("\n").filter(Boolean);
  const base = safeGit(["merge-base", first, second]).stdout.trim();
  const regenerated = new Map();
  const unexplained = [];
  for (const file of new Set([...differing, ...conflicted])) {
    const actual = blobText(commit, file);
    if (carried.length > 0 && file === FRAME_PATH) continue;
    if (actual !== null && conflicted.has(file) && file.endsWith(".md") && appendOnlyResolution(base, first, second, file) === actual) continue;
    const generator = GENERATORS.find((candidate) => candidate.outputs.includes(file));
    if (generator && actual !== null) {
      if (!regenerated.has(generator.name)) regenerated.set(generator.name, regenerateWithoutCredentials(generator, { ours: first, other: second, tools }));
      const result = regenerated.get(generator.name);
      if (result.ok && result.files[file] === actual) continue;
    }
    unexplained.push(file);
  }
  return unexplained;
}

/** Whether `commit` (from the repair job) is the doctor's honest merge of
 * the PR head `head` and a commit of `main`: { ok: true, first, second,
 * carried } or { ok: false, moved?, reason }. The first parent is `head`, or
 * a commit whose only parent is `head` and which only deletes the queue
 * files (the drop commit); the tree is checked by `unexplainedPaths`.
 * `moved` marks a PR whose branch moved since the repair (retried next run). */
export function verifyRepair({ commit, head, main, tools }) {
  const parents = safeGit(["rev-list", "--parents", "-n", "1", commit]).stdout.trim().split(" ").slice(1);
  if (parents.length !== 2) return { ok: false, reason: "the repaired commit is not a two-parent merge" };
  const [first, second] = parents;
  if (safeGit(["merge-base", "--is-ancestor", second, main], { check: false }).code !== 0) {
    return { ok: false, reason: "the repaired commit's second parent is not a commit of main" };
  }
  let carried = [];
  if (first !== head) {
    const dropParents = safeGit(["rev-list", "--parents", "-n", "1", first]).stdout.trim().split(" ").slice(1);
    if (dropParents.length !== 1 || dropParents[0] !== head) {
      const known = safeGit(["merge-base", "--is-ancestor", first, head], { check: false }).code === 0
        || (dropParents.length === 1 && safeGit(["merge-base", "--is-ancestor", dropParents[0], head], { check: false }).code === 0);
      return known
        ? { ok: false, moved: true, reason: "the branch moved since the repair; next run retries" }
        : { ok: false, reason: "the repaired commit is not based on the PR's head" };
    }
    const changes = safeGit(["diff", "--name-status", "--no-renames", head, first]).stdout.split("\n").filter(Boolean);
    if (changes.length === 0 || !changes.every((line) => QUEUE_FILES.some((file) => line === `D\t${file}`))) {
      return { ok: false, reason: "the repaired commit's first parent changes more than the queue files" };
    }
    carried = changes.map((line) => line.slice(2));
  }
  const unexplained = unexplainedPaths({ commit, first, second, tools, carried });
  if (unexplained.length) {
    return { ok: false, reason: `the repaired commit differs from the doctor's own merge in ${unexplained.slice(0, 5).map(cleanFile).join(", ")}${unexplained.length > 5 ? ", ..." : ""}` };
  }
  return { ok: true, first, second, carried };
}

/** Re-applies the PR's queue edits with main's tools (pull, apply, check the
 * frame it computes against the repaired commit's, then sync against the
 * repaired frame and notes): { ok, detail }. Each call works in a fresh
 * clone of the job's workspace (#463 re-review M-new-1), so an adopt that
 * fails partway leaves nothing behind for the next PR's sync to publish.
 * The repaired files come out with `git show` into a no-checkout worktree,
 * so no PR file is checked out with the token in the environment. */
export function adoptAndSync({ tools, head, base, commit }) {
  const shared = process.env.BRAUSER_QUEUE_WORKSPACE;
  if (!shared) return { ok: false, detail: "no work-queue workspace" };
  const scratch = mkdtempSync(join(tmpdir(), "pr-doctor-adopt-"));
  const tree = join(scratch, "tree");
  const ws = join(scratch, "queue-ws");
  try {
    const url = safeGit(["-C", shared, "remote", "get-url", "origin"]).stdout.trim();
    safeGit(["clone", "-q", "--depth", "1", url, ws]);
    for (const key of ["user.name", "user.email"]) {
      const value = safeGit(["-C", shared, "config", "--get", key], { check: false }).stdout.trim();
      if (value) safeGit(["-C", ws, "config", key, value]);
    }
    const booted = sh("bd", ["bootstrap"], { cwd: ws, check: false });
    if (booted.code !== 0) return { ok: false, detail: `bd bootstrap failed: ${(booted.stderr || booted.stdout).trim().split("\n").at(-1)}` };
    const env = { BRAUSER_QUEUE_WORKSPACE: ws };

    safeGit(["worktree", "add", "-q", "--no-checkout", "--detach", tree, commit]);
    const write = (file, text) => {
      mkdirSync(join(tree, file, ".."), { recursive: true });
      writeFileSync(join(tree, file), text);
    };
    const repairedFrame = blobText(commit, FRAME_PATH);
    if (repairedFrame === null) return { ok: false, detail: `the repaired commit has no ${FRAME_PATH}` };
    write(FRAME_PATH, repairedFrame);
    const notes = safeGit(["ls-tree", "-z", "--name-only", `${commit}:${NOTES_DIR}`], { check: false });
    if (notes.code === 0) {
      for (const name of notes.stdout.split("\0").filter(Boolean)) {
        if (!/^[\w.-]+\.md$/.test(name)) continue;
        const text = blobText(commit, `${NOTES_DIR}/${name}`);
        if (text !== null) write(`${NOTES_DIR}/${name}`, text);
      }
    }
    const mainFrame = join(scratch, "frame.md");
    writeFileSync(mainFrame, safeGit(["show", `origin/main:${FRAME_PATH}`]).stdout);
    const applied = adopt(tools, { from: head, base, push: true, frame: mainFrame }, env);
    if (!applied.ok) return { ok: false, detail: applied.detail || "adopt failed" };
    if (readFileSync(mainFrame, "utf8") !== repairedFrame) {
      return { ok: false, detail: `the repaired ${FRAME_PATH} is not what main's adopt computes` };
    }
    const synced = syncQueue(tools, tree, env);
    return synced.ok ? { ok: true } : { ok: false, detail: synced.detail || "queue sync failed" };
  } finally {
    git(["worktree", "remove", "--force", tree], { check: false });
    rmSync(scratch, { recursive: true, force: true });
  }
}

/** The PR as the API reports it now, or a reason to leave it alone. */
function livePr(repo, number) {
  const pr = JSON.parse(gh(["api", `repos/${repo}/pulls/${number}`]).stdout);
  const headRefName = pr.head?.ref;
  if (typeof headRefName !== "string" || headRefName === "" || headRefName === "main") return { reason: "refused: the PR's head branch is main" };
  if (pr.base?.ref !== "main" || pr.head?.repo?.full_name !== repo || pr.state !== "open") return { reason: "skipped: no longer an open same-repo PR into main" };
  return {
    pr: {
      number: Number(number),
      headRefName,
      labels: (pr.labels ?? []).map((label) => ({ name: label.name })),
      autoMergeRequest: pr.auto_merge ?? null,
    },
    head: pr.head.sha,
  };
}

/** Pushes the repairs the repair job made, after checking each one, and
 * comments, labels and requeues. The PR list comes from the plan job's
 * state (`items`); from the repair job's (`results`, by PR number, or null
 * when the repair job left none) only each repair's ok and files are read,
 * and those files only as text. */
function publishPhase(items, results, { repo, tools, dryRun, canPush, bundle }) {
  const rows = [];
  const numbers = [...new Set(items.filter((item) => item?.repair).map((item) => Number(item.pr?.number)))]
    .filter((number) => Number.isInteger(number) && number > 0);
  for (const number of numbers) {
    const claimed = results === null
      ? { ok: false, files: ["(the repair job did not finish; see the PR doctor workflow run)"] }
      : results.get(number) ?? { ok: false, files: ["(the repair job reported nothing for this PR)"] };
    try {
      const live = livePr(repo, number);
      if (!live.pr) {
        rows.push([number, live.reason, ""]);
        continue;
      }
      const { pr, head } = live;
      const queued = pr.autoMergeRequest != null || pr.labels.some((item) => item.name === QUEUE_LABEL);
      let result = { ok: false, files: (Array.isArray(claimed.files) ? claimed.files : ["(no file list)"]).slice(0, 50).map(cleanFile) };
      let outcome;
      if (claimed.ok === true && bundle) {
        git(["fetch", "-q", "--no-tags", "origin", "+refs/heads/main:refs/remotes/origin/main", `+refs/heads/${pr.headRefName}:refs/remotes/origin/${pr.headRefName}`]);
        const ref = `refs/pr-doctor/${number}`;
        const fetched = safeGit(["fetch", "-q", "--no-tags", bundle, `+${ref}:${ref}`], { check: false });
        const commit = fetched.code === 0 ? safeGit(["rev-parse", "--verify", `${ref}^{commit}`]).stdout.trim() : null;
        const main = safeGit(["rev-parse", "origin/main"]).stdout.trim();
        const verified = commit === null
          ? { ok: false, reason: "the repair job's bundle has no commit for this PR" }
          : verifyRepair({ commit, head, main, tools });
        if (verified.moved) {
          rows.push([number, "skipped", verified.reason]);
          continue;
        }
        if (!verified.ok) {
          result = { ok: false, files: [cleanFile(verified.reason)] };
        } else {
          result = { ok: true, files: result.files, sha: commit.slice(0, 12), commit };
          if (canPush && verified.carried.length > 0) {
            const base = safeGit(["merge-base", head, verified.second]).stdout.trim();
            const synced = adoptAndSync({ tools, head, base, commit });
            if (!synced.ok) result = { ok: false, files: verified.carried.map((file) => `${file} (${cleanFile(synced.detail)})`) };
          }
        }
        if (result.ok && canPush) {
          const push = safeGit(["push", "-q", "origin", `${commit}:refs/heads/${pr.headRefName}`], { check: false });
          if (push.code !== 0) {
            rows.push([number, "skipped", "the branch moved during the repair; next run retries"]);
            continue;
          }
          outcome = "repaired";
        }
      }
      if (!outcome) outcome = !result.ok && canPush ? "needs-person" : "report-only";
      const requeued = needsRequeue(pr, outcome);
      const body = commentBody({ outcome, files: result.files, sha: result.sha, queued, resolvable: result.ok, requeued });
      report(repo, pr, body, { dryRun, label: outcome !== "repaired" });
      let note = "";
      if (requeued) {
        const sha = result.commit;
        const { settled, seen } = requeueAfterPush({
          sha,
          headOf: () => JSON.parse(gh(["api", `repos/${repo}/pulls/${number}`]).stdout).head.sha,
          comment: (text) => gh(["api", "-X", "POST", `repos/${repo}/issues/${number}/comments`, "-f", `body=${text}`]),
          wait: sleep,
        });
        if (!settled) console.log(`::warning title=PR doctor #${number}::GitHub still showed ${seen ?? "no head"}, not ${sha.slice(0, 12)}, after 60s; requeued anyway`);
        note = " (requeued)";
      }
      rows.push([number, outcome + note + (outcome === "report-only" && result.ok ? " (resolvable)" : ""), result.files.join(", ")]);
    } catch (error) {
      rows.push([number, "error", warn({ number }, error)]);
    }
  }
  return rows;
}

function readState(file) {
  const state = JSON.parse(readFileSync(file, "utf8"));
  if (state.version !== STATE_VERSION || !Array.isArray(state.items)) throw new Error(`${file}: not a version ${STATE_VERSION} doctor state`);
  return state;
}

function writeBundle(file, refs) {
  if (refs.length === 0) return;
  git(["bundle", "create", "-q", file, ...refs]);
}

function main() {
  const args = process.argv.slice(2);
  const option = (name) => {
    const at = args.indexOf(name);
    return at >= 0 ? args[at + 1] : null;
  };
  const tools = option("--tools");
  const phase = option("--phase");
  const repo = process.env.GITHUB_REPOSITORY;
  const usage = "usage: GITHUB_REPOSITORY=owner/name node scripts/pr-doctor.mjs --tools <dir> [--phase plan|repair|publish] [--state <file>] [--bundle-out <file>] [--bundle <file>] [--prs <n,n,...>] [--dry-run]";
  if (!tools || (phase !== null && !PHASES.includes(phase))) throw new Error(usage);
  if (phase !== "repair" && !repo) throw new Error(usage);
  const stateFile = option("--state") ?? join(tools, "doctor-state.json");
  const bundleOut = option("--bundle-out");
  if (phase === null || phase === "repair") {
    for (const file of ["work_queue.py", "render_backlog.py", "check_queue_consistency.py"]) readFileSync(join(tools, file));
  }
  const dryRun = args.includes("--dry-run");
  // The plan and publish phases each take these from their own job, never
  // from a state file another job wrote.
  const canPush = process.env.PR_DOCTOR_PUSH === "1" && !dryRun;

  let state;
  if (phase === null || phase === "plan") {
    state = { version: STATE_VERSION, canPush, items: planPhase({ repo, dryRun }) };
    const finished = state.items.filter((item) => !item.repair).map((item) => item.row);
    summary(`PR doctor: listed${dryRun ? " (dry run)" : ""}`, finished);
    if (process.env.GITHUB_OUTPUT) {
      const prs = state.items.filter((item) => item.repair).map((item) => item.pr.number);
      appendFileSync(process.env.GITHUB_OUTPUT, `repairs=${prs.length}\nprs=${prs.join(",")}\n`);
    }
    if (phase === "plan" && bundleOut) {
      const branches = state.items.filter((item) => item.repair).map((item) => `refs/remotes/origin/${item.pr.headRefName}`);
      writeBundle(bundleOut, branches.length ? ["refs/remotes/origin/main", ...branches] : []);
    }
  } else if (phase === "publish" && option("--prs") !== null && !existsSync(stateFile)) {
    state = null;
  } else {
    state = readState(stateFile);
  }
  if (phase === null || phase === "repair") {
    state.items = repairPhase(state.items, { tools, canPush: state.canPush === true });
    if (phase === "repair" && bundleOut) {
      writeBundle(bundleOut, state.items.filter((item) => item.repair && item.result?.ok).map((item) => `refs/pr-doctor/${item.pr.number}`));
    }
  }
  if (phase === null || phase === "publish") {
    // In the workflow, the PR list is the plan job's and only the results
    // are the repair job's (which may have left none).
    let items = state?.items ?? [];
    const results = state === null ? null : new Map(items.filter((item) => item?.repair).map((item) => [Number(item.pr?.number), item.result ?? {}]));
    // The workflow passes the plan job's list as a job output (--prs), which
    // the repair job cannot change, as it could an artifact.
    if (phase === "publish" && option("--prs") !== null) {
      items = option("--prs").split(",").filter(Boolean).map((number) => ({ pr: { number: Number(number) }, repair: true }));
    }
    const rows = publishPhase(items, results, { repo, tools, dryRun, canPush, bundle: phase === null ? "." : option("--bundle") });
    summary(`PR doctor${dryRun ? " (dry run)" : canPush ? "" : " (report only)"}`, rows);
  }
  if (phase === "plan" || phase === "repair") writeFileSync(stateFile, JSON.stringify(state, null, 2));
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) main();
