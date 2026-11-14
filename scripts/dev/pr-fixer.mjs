#!/usr/bin/env node
// PR fixer (docs/agents/pr-fixer.md): the agent half of the keeper/fixer split.
// The PR keeper (scripts/pr-keeper.mjs) files a P1 repair bead, external ref
// gh-pr-<n>-<head>, when pr-doctor cannot resolve a PR's conflict with main.
// This runner takes those beads with no person involved:
//
//   1. one pass per run, under a host lock; pulls the queue and lists the
//      open gh-pr-* beads;
//   2. closes a bead whose PR merged (citing the merge commit) or closed;
//      skips held, pr-doctor:skip, fork, exhausted and other actors' beads;
//   3. claims at most one still-conflicting PR as actor `pr-fixer`, counts the
//      attempt, syncs;
//   4. in a fresh detached worktree at the PR head, starts `git merge` of main
//      (with main's merge drivers) and runs one agent (codex exec, or claude -p)
//      on scripts/dev/pr-fixer/prompt.md, bounded in time, its process group
//      killed afterwards, with no GitHub credential in its environment;
//   5. verifies the result itself: nothing unmerged, a clean tree, a new HEAD
//      that descends from the PR head and main, no conflict markers, a clean
//      merge with main's current tip, the privacy scan, the agent's report of
//      its focused checks, and `codex exec review` with no P1;
//   6. pushes only if the PR head has not moved, never forced; notes the
//      repair in the PR body (the Owner line stays, checked by checkPrOwner);
//      runs scripts/dev/queue-pr.mjs <n> --head <sha>;
//   7. records the outcome on the bead: pushed (stays in_progress until the PR
//      merges, AGENTS.md "Close only what is on main"), or failed (released,
//      with the reason; after MAX_ATTEMPTS, labelled fixer-exhausted and left
//      open at P1 for a worker).
//
// The runner, never the agent, holds the push credential and makes every
// GitHub and queue write. It never merges, never force-pushes and never runs
// `gh pr merge`. Run it from a checkout of main: it uses that checkout's
// copies of the queue, privacy and queue-pr tools, never a PR's.
//
// Usage: node scripts/dev/pr-fixer.mjs [--repo owner/name] [--dry-run]
//          [--agent codex|claude] [--max-attempts 2] [--timeout-minutes 50]
//          [--work-root DIR]
// Environment: BRAUSER_QUEUE_WORKSPACE (the bootstrapped bd workspace), and
// for gh and git push whatever credential the host provides (GH_TOKEN, a git
// credential helper); docs/agents/pr-fixer.md lists the names, never values.
// Exit codes: 0 a pass ran (any outcome; the receipt says which), 1 error,
// 2 usage.

import { spawn, spawnSync } from "node:child_process";
import { appendFileSync, closeSync, existsSync, mkdirSync, openSync, readFileSync, realpathSync, statSync, unlinkSync, writeFileSync, writeSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

import { DOCTOR_SKIP_LABEL, HELD_LABEL, NEEDS_MERGE_LABEL, QUEUE_LABEL } from "../pr-constants.mjs";
import { checkPrOwner } from "../pr-owner.mjs";

export const FIXER_ACTOR = "pr-fixer";
export const EXHAUSTED_LABEL = "fixer-exhausted";
export const MAX_ATTEMPTS = 2;
export const NOTE_MARKER = "<!-- pr-fixer -->";
export const NOTE_END = "<!-- /pr-fixer -->";
const PROMPT_FILE = join(dirname(fileURLToPath(import.meta.url)), "pr-fixer", "prompt.md");
const PR_FIELDS = "number,state,headRefName,headRefOid,baseRefName,isCrossRepository,mergeable,labels,mergeCommit,body,title,url";

// --- Decisions (pure; scripts/tests/pr-fixer.test.mjs) -------------------------

/** The keeper's repair ref (scripts/pr-keeper.mjs repairRef) -> { number, head }, else null. */
export function parseRepairRef(ref) {
  const m = /^gh-pr-(\d+)-([0-9a-f]{7,40})$/.exec(ref ?? "");
  return m ? { number: Number(m[1]), head: m[2] } : null;
}

const metadataOf = (bead) => {
  if (bead.metadata && typeof bead.metadata === "object") return bead.metadata;
  try { return JSON.parse(bead.metadata ?? "{}") ?? {}; } catch { return {}; }
};
export const attemptsOf = (bead) => Number(metadataOf(bead).fixer_attempts ?? 0) || 0;

/** One repair bead and its PR (gh pr view, labels as names) ->
 * { action: "close" | "repair" | "queue" | "wait" | "skip", reason }. "queue" retries the queue
 * request for the fixer's own pushed head when queue-pr did not succeed after the push. */
export function decideBead(bead, pr, { maxAttempts = MAX_ATTEMPTS } = {}) {
  const ref = parseRepairRef(bead.external_ref);
  if (!ref) return { action: "skip", reason: "not a repair ref" };
  if (!["open", "in_progress"].includes(bead.status)) return { action: "skip", reason: `status ${bead.status}` };
  if (!pr) return { action: "skip", reason: `PR #${ref.number} not found` };
  if (pr.number !== ref.number) return { action: "skip", reason: `bead names PR #${ref.number}, got #${pr.number}` };
  if (pr.state === "MERGED") return { action: "close", reason: `PR #${pr.number} merged into main as ${pr.mergeCommit?.oid ?? "unknown"}` };
  if (pr.state === "CLOSED") return { action: "close", reason: `PR #${pr.number} closed unmerged; nothing to repair` };
  if (bead.status === "in_progress" && bead.assignee && bead.assignee !== FIXER_ACTOR) return { action: "skip", reason: `claimed by ${bead.assignee}` };
  if ((bead.labels ?? []).includes(EXHAUSTED_LABEL) || attemptsOf(bead) >= maxAttempts) return { action: "skip", reason: `exhausted after ${attemptsOf(bead)} attempt(s)` };
  if (pr.isCrossRepository) return { action: "skip", reason: "fork" };
  if (pr.baseRefName !== "main") return { action: "skip", reason: `base ${pr.baseRefName}` };
  for (const label of [HELD_LABEL, DOCTOR_SKIP_LABEL]) if (pr.labels.includes(label)) return { action: "skip", reason: `labelled ${label}` };
  // Removing the label takes a PR out of the queue (AGENTS.md): never push to it or queue it again.
  if (!pr.labels.includes(QUEUE_LABEL)) return { action: "skip", reason: `not queued (no ${QUEUE_LABEL} label): its owner took it out` };
  const meta = metadataOf(bead);
  if (bead.status === "in_progress" && bead.assignee === FIXER_ACTOR && meta.fixer_pushed_sha === pr.headRefOid
    && meta.fixer_queue_status !== undefined && String(meta.fixer_queue_status) !== "0" && pr.mergeable !== "CONFLICTING") {
    return { action: "queue", reason: `queue-pr exited ${meta.fixer_queue_status} after the push of ${pr.headRefOid.slice(0, 12)}; asking again` };
  }
  if (pr.mergeable === "CONFLICTING") return { action: "repair", reason: `PR #${pr.number} conflicts with main at ${pr.headRefOid.slice(0, 12)}` };
  if (pr.mergeable === "MERGEABLE") return { action: "wait", reason: `no conflict with main at ${pr.headRefOid.slice(0, 12)}` };
  return { action: "wait", reason: "GitHub has not computed mergeability" };
}

const CREDENTIAL_VARS = ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN", "PR_DOCTOR_TOKEN", "GIT_ASKPASS", "SSH_ASKPASS", "SSH_AUTH_SOCK"];

/** The agent's environment: no GitHub token, askpass, SSH agent or git credential
 * helper (GIT_CONFIG_* entries renumbered without them), and git never prompts. */
export function agentEnv(env) {
  const out = {};
  for (const [k, v] of Object.entries(env)) if (!CREDENTIAL_VARS.includes(k) && !/^GIT_CONFIG_(COUNT|KEY_\d+|VALUE_\d+)$/.test(k)) out[k] = v;
  let n = 0;
  for (let i = 0; i < Number(env.GIT_CONFIG_COUNT || 0); i++) {
    const key = env[`GIT_CONFIG_KEY_${i}`];
    if (key === undefined || /^credential(\.|$)/i.test(key) || /^url\..*insteadof$/i.test(key)) continue;
    out[`GIT_CONFIG_KEY_${n}`] = key;
    out[`GIT_CONFIG_VALUE_${n}`] = env[`GIT_CONFIG_VALUE_${i}`] ?? "";
    n++;
  }
  if (n) out.GIT_CONFIG_COUNT = String(n);
  out.GIT_TERMINAL_PROMPT = "0";
  return out;
}

/** Where the host keeps credentials the agent must never read: the fixer's env file, gh's
 * login, git's credential stores, SSH keys and netrc. The agent runs as the same user, so
 * stripping its environment is not enough; the sandbox hides these paths. */
export const defaultSecretPaths = (home) => [".config/brauser-pr-fixer", ".config/gh", ".git-credentials", ".config/git/credentials", ".ssh", ".netrc"].map((p) => join(home, p));

/** bubblewrap mounts that hide each existing secret: an empty tmpfs over a directory,
 * an empty file over a file. `kindOf(path)` -> "dir" | "file" | null (absent). */
export function maskArgs(paths, kindOf) {
  return paths.flatMap((p) => ({ dir: ["--tmpfs", p], file: ["--ro-bind", "/dev/null", p] })[kindOf(p)] ?? []);
}
const kindOfPath = (p) => { try { return statSync(p).isDirectory() ? "dir" : "file"; } catch { return null; } };

// Run inside the sandbox: exit 0 only when every path given is absent, empty or unreadable.
const PROBE = `const fs=require("fs");for(const p of process.argv.slice(1)){let leak=false;try{const st=fs.statSync(p);leak=st.isDirectory()?fs.readdirSync(p).length>0:fs.readFileSync(p).length>0}catch{}if(leak){console.error("readable: "+p);process.exit(1)}}`;

/** 1-based lines holding a leftover git conflict marker. */
export function conflictMarkers(text) {
  return text.split(/\r?\n/).flatMap((line, i) => (/^(<{7}|>{7})( |$)/.test(line) ? [i + 1] : []));
}

/** codex review output -> { ok, p1 }: any [P1] finding fails, and so does no output. */
export function reviewVerdict(text) {
  const p1 = String(text ?? "").split(/\r?\n/).filter((l) => /\[P1\]/.test(l)).map((l) => l.trim());
  if (!String(text ?? "").trim()) return { ok: false, p1, reason: "codex review printed nothing" };
  return p1.length ? { ok: false, p1, reason: `codex review found P1: ${p1.join("; ")}` } : { ok: true, p1 };
}

/** The agent's report (JSON it wrote) -> { ok, reason }. */
export function reportVerdict(report) {
  if (!report || typeof report !== "object") return { ok: false, reason: "the agent wrote no report" };
  if (report.resolved !== true) return { ok: false, reason: `the agent did not resolve the conflict: ${clean(report.reason ?? "no reason given")}` };
  const checks = Array.isArray(report.checks) ? report.checks : [];
  if (!checks.length) return { ok: false, reason: "the agent ran no focused check" };
  const failed = checks.filter((c) => c?.passed !== true);
  if (failed.length) return { ok: false, reason: `focused checks failed: ${failed.map((c) => clean(c?.command ?? "?")).join("; ")}` };
  return { ok: true };
}

/** Untrusted text made safe for a bead note or a PR body: one line, no mentions or code spans, capped. */
export function clean(text, max = 300) {
  return String(text).replace(/[\u0000-\u001f\u007f]+/g, " ").replace(/[`@]/g, "_").trim().slice(0, max);
}

/** The PR body with one marked fixer block, added at the end or replacing the last one. */
export function upsertFixerNote(body, note) {
  const block = `${NOTE_MARKER}\n${note}\n${NOTE_END}`;
  const text = String(body ?? "");
  const start = text.indexOf(NOTE_MARKER);
  const end = text.indexOf(NOTE_END, start);
  if (start >= 0 && end > start) return `${text.slice(0, start)}${block}${text.slice(end + NOTE_END.length)}`;
  return `${text.replace(/\s*$/, "")}\n\n${block}\n`;
}

/** Fill {{name}} placeholders; an unknown one is a bug in the template, not a blank. */
export function renderPrompt(template, vars) {
  return template.replace(/\{\{(\w+)\}\}/g, (_, name) => {
    if (!(name in vars)) throw new Error(`prompt placeholder {{${name}}} has no value`);
    return String(vars[name]);
  });
}

// --- I/O ------------------------------------------------------------------------

function defaultRun(cmd, args, { cwd, env, input } = {}) {
  const r = spawnSync(cmd, args, { cwd, env, input, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  return { status: r.error ? 127 : r.status, stdout: r.stdout ?? "", stderr: r.error ? String(r.error.message) : r.stderr ?? "" };
}

/** Run with a time limit in its own process group, killed (with any children it left) afterwards. */
export function runBounded(cmd, args, { cwd, env, input = "", timeoutMs, logFile }) {
  return new Promise((resolve) => {
    const fd = openSync(logFile, "a");
    let out = "", timedOut = false, done = false;
    const child = spawn(cmd, args, { cwd, env, detached: true, stdio: ["pipe", "pipe", fd] });
    const killGroup = () => { try { process.kill(-child.pid, "SIGKILL"); } catch { /* already gone */ } };
    const timer = setTimeout(() => { timedOut = true; killGroup(); }, timeoutMs); // timing-ok: the agent's hard time limit
    child.stdout.on("data", (chunk) => { writeSync(fd, chunk); if (out.length < 4 * 1024 * 1024) out += chunk; });
    const finish = (status) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      killGroup();
      closeSync(fd);
      resolve({ status, timedOut, stdout: out });
    };
    child.on("error", () => finish(127));
    child.on("close", (code) => finish(code ?? 1));
    child.stdin.on("error", () => {});
    child.stdin.end(input);
  });
}

function takeLock(path) {
  for (let i = 0; i < 2; i++) {
    try {
      const fd = openSync(path, "wx");
      writeSync(fd, String(process.pid));
      closeSync(fd);
      return true;
    } catch (error) {
      if (error.code !== "EEXIST") throw error;
      const pid = Number(readFileSync(path, "utf8").trim()); // crlf-ok: one pid, parsed
      try { if (pid > 0) { process.kill(pid, 0); return false; } } catch { /* stale: its process is gone */ }
      unlinkSync(path);
    }
  }
  return false;
}

/** One pass. Returns { outcome, reason?, decisions?, bead?, sha? }; outcome is one of
 * locked, dry-run, idle, closed, queued, refused, claim-refused, pushed, failed. */
export async function runPass(opts) {
  const {
    repo, toolsRoot, queueWorkspace, dryRun = false, agent = "codex", maxAttempts = MAX_ATTEMPTS,
    timeoutMs = 50 * 60_000, reviewTimeoutMs = 15 * 60_000, env = process.env, run = defaultRun,
    log = (line) => console.log(`pr-fixer: ${line}`), now = () => new Date().toISOString(),
  } = opts;
  const workRoot = opts.workRoot ?? join(env.XDG_STATE_HOME || join(homedir(), ".local", "state"), "brauser-pr-fixer");
  const commands = {
    sync: [join(toolsRoot, "scripts", "dev", "queue"), ["sync"]],
    privacy: [process.execPath, [join(toolsRoot, "scripts", "privacy-scan.mjs")]],
    queuePr: [process.execPath, [join(toolsRoot, "scripts", "dev", "queue-pr.mjs")]],
    mergeDrivers: [process.execPath, [join(toolsRoot, "scripts", "dev", "setup-merge-drivers"), "--tools", join(toolsRoot, "scripts")]],
    // The agent and the review run in a bubblewrap sandbox that hides the host's credentials.
    sandbox: ["bwrap", ["--dev-bind", "/", "/", "--die-with-parent"]],
    ...opts.commands,
  };
  const secretPaths = opts.secretPaths ?? defaultSecretPaths(env.HOME || homedir());
  const masks = maskArgs(secretPaths, kindOfPath);
  const wrap = (cmd, args, cwd) => [commands.sandbox[0], [...commands.sandbox[1], ...masks, "--chdir", cwd, "--", cmd, ...args]];
  mkdirSync(workRoot, { recursive: true });
  const lock = join(workRoot, "pr-fixer.lock");
  if (!takeLock(lock)) { log("another fixer holds the lock"); return { outcome: "locked" }; }
  const receipt = (r) => { appendFileSync(join(workRoot, "receipts.jsonl"), `${JSON.stringify({ at: now(), ...r })}\n`); return r; };
  try {
    const qenv = { ...env, BD_NON_INTERACTIVE: "1", BRAUSER_QUEUE_WORKSPACE: queueWorkspace };
    const bd = (...args) => run("bd", ["--actor", FIXER_ACTOR, ...args], { cwd: queueWorkspace, env: qenv });
    const sync = () => {
      const r = run(commands.sync[0], [...commands.sync[1]], { cwd: toolsRoot, env: qenv });
      if (r.status !== 0) log(`queue sync exited ${r.status}: ${clean(r.stderr || r.stdout)}`);
      return r.status;
    };
    const viewPr = (n) => {
      const r = run("gh", ["pr", "view", String(n), "--repo", repo, "--json", PR_FIELDS], { env });
      if (r.status !== 0) return null;
      const pr = JSON.parse(r.stdout);
      return { ...pr, labels: (pr.labels ?? []).map((l) => (typeof l === "string" ? l : l.name)) };
    };

    if (bd("dolt", "pull").status !== 0) log("bd dolt pull failed; deciding from the local queue");
    const listed = bd("list", "--json", "--all", "--limit", "0", "--external-contains", "gh-pr-");
    if (listed.status !== 0) throw new Error(`bd list failed: ${clean(listed.stderr)}`);
    const beads = JSON.parse(listed.stdout || "[]")
      .filter((b) => parseRepairRef(b.external_ref) && b.status !== "closed")
      .sort((a, b) => parseRepairRef(a.external_ref).number - parseRepairRef(b.external_ref).number);
    const decisions = beads.map((b) => {
      const pr = viewPr(parseRepairRef(b.external_ref).number);
      return { bead: b, pr, ...decideBead(b, pr, { maxAttempts }) };
    });
    for (const d of decisions) log(`${d.bead.id} (${d.bead.external_ref}): ${d.action}: ${d.reason}`);
    if (dryRun) return { outcome: "dry-run", decisions: decisions.map(({ bead, action, reason }) => ({ id: bead.id, action, reason })) };

    let closed = 0, queued = 0;
    for (const d of decisions.filter((x) => x.action === "close")) {
      if (bd("close", d.bead.id, "--reason", d.reason).status === 0) closed++;
    }
    for (const d of decisions.filter((x) => x.action === "queue")) {
      const q = run(commands.queuePr[0], [...commands.queuePr[1], String(d.pr.number), "--head", d.pr.headRefOid, "--repo", repo], { cwd: toolsRoot, env });
      bd("update", d.bead.id, "--set-metadata", `fixer_queue_status=${q.status}`, "--append-notes", `${now()} ${FIXER_ACTOR}: queue-pr retried for ${d.pr.headRefOid.slice(0, 12)}, exited ${q.status}.`);
      if (q.status === 0) queued++;
    }
    if (closed || decisions.some((x) => x.action === "queue")) sync();
    const target = decisions.find((d) => d.action === "repair");
    if (!target) return receipt({ outcome: queued ? "queued" : closed ? "closed" : "idle", closed, queued });

    // Fail closed before claiming: the agent must not be able to read the host's credentials.
    const probeDir = join(workRoot, "probe");
    mkdirSync(probeDir, { recursive: true });
    const [pcmd, pargs] = wrap(process.execPath, ["-e", PROBE, ...secretPaths], probeDir);
    const probe = run(pcmd, pargs, { cwd: probeDir, env: agentEnv(env) });
    if (probe.status !== 0) return receipt({ outcome: "refused", reason: `the agent sandbox (${commands.sandbox[0]}) does not hide the host's credentials: ${clean(probe.stderr || probe.stdout || `exit ${probe.status}`)}` });

    const { bead, pr } = target;
    const attempt = attemptsOf(bead) + 1;
    if (bd("update", bead.id, "--claim").status !== 0) return receipt({ outcome: "claim-refused", bead: bead.id });
    bd("update", bead.id, "--set-metadata", `fixer_attempts=${attempt}`);
    if (sync() !== 0) {
      bd("update", bead.id, "--status", "open", "--assignee", "", "--set-metadata", `fixer_attempts=${attempt - 1}`);
      return receipt({ outcome: "claim-refused", bead: bead.id, reason: "queue sync failed; the claim was not published" });
    }
    log(`${bead.id}: claimed; attempt ${attempt} of ${maxAttempts} on PR #${pr.number}`);

    const result = await repair({ repo, pr, bead, attempt, toolsRoot, workRoot, agent, timeoutMs, reviewTimeoutMs, env, run, commands, wrap, log, now, viewPr });
    const stamp = `${now()} ${FIXER_ACTOR} attempt ${attempt}`;
    if (result.ok) {
      bd("update", bead.id, "--status", "in_progress", "--append-notes",
        `${stamp}: pushed ${result.sha} (main ${result.base.slice(0, 12)} merged into PR #${pr.number}), resolving ${result.files.join(", ") || "no conflicted file"}; checks: ${result.checks.join("; ")}; codex review: no P1; queue-pr exited ${result.queueStatus}. The bead closes when the PR merges.`,
        "--set-metadata", "fixer_last_outcome=pushed", "--set-metadata", `fixer_pushed_sha=${result.sha}`, "--set-metadata", `fixer_queue_status=${result.queueStatus}`);
      sync();
      return receipt({ outcome: "pushed", bead: bead.id, pr: pr.number, sha: result.sha, queueStatus: result.queueStatus });
    }
    const exhausted = attempt >= maxAttempts;
    bd("update", bead.id, "--status", "open", "--assignee", "", "--append-notes",
      `${stamp}: failed: ${clean(result.reason, 1000)}${exhausted ? ` The fixer stops after ${maxAttempts} attempts (${EXHAUSTED_LABEL}); a worker or session must take it.` : ""}`,
      "--set-metadata", "fixer_last_outcome=failed", ...(exhausted ? ["--add-label", EXHAUSTED_LABEL] : []));
    sync();
    log(`${bead.id}: failed: ${result.reason}`);
    return receipt({ outcome: "failed", bead: bead.id, pr: pr.number, reason: result.reason });
  } finally {
    try { unlinkSync(lock); } catch { /* already gone */ }
  }
}

/** The repair of one PR in its own worktree -> { ok, reason } or { ok, sha, base, files, checks, queueStatus }. */
async function repair({ repo, pr, bead, attempt, toolsRoot, workRoot, agent, timeoutMs, reviewTimeoutMs, env, run, commands, wrap, log, now, viewPr }) {
  const stamp = now().replace(/[^0-9TZ]/g, "");
  const runDir = join(workRoot, "runs", `${bead.id}-${attempt}-${stamp}`);
  const wt = join(runDir, "worktree");
  mkdirSync(runDir, { recursive: true });
  const logFile = join(runDir, "run.log");
  const git = (cwd, ...args) => run("git", args, { cwd, env });
  const must = (r, what) => { if (r.status !== 0) throw new Error(`${what} failed: ${clean(r.stderr || r.stdout)}`); return r.stdout.trim(); };
  const identity = { GIT_AUTHOR_NAME: "Brauser PR fixer", GIT_AUTHOR_EMAIL: "pr-fixer@users.noreply.github.com", GIT_COMMITTER_NAME: "Brauser PR fixer", GIT_COMMITTER_EMAIL: "pr-fixer@users.noreply.github.com" };
  let added = false;
  try {
    if (git(toolsRoot, "check-ref-format", "--branch", pr.headRefName).status !== 0 || pr.headRefName.startsWith("-")) return { ok: false, reason: `unsafe branch name ${clean(pr.headRefName)}` };
    must(git(toolsRoot, "fetch", "--no-tags", "--quiet", "origin", "refs/heads/main"), "fetch main");
    const base = must(git(toolsRoot, "rev-parse", "FETCH_HEAD"), "read main");
    must(git(toolsRoot, "fetch", "--no-tags", "--quiet", "origin", `refs/heads/${pr.headRefName}`), "fetch the PR branch");
    const head = must(git(toolsRoot, "rev-parse", "FETCH_HEAD"), "read the PR head");
    if (head !== pr.headRefOid) return { ok: false, reason: `the PR branch is at ${head.slice(0, 12)}, GitHub reports ${pr.headRefOid.slice(0, 12)}; it moved` };
    run(commands.mergeDrivers[0], [...commands.mergeDrivers[1]], { cwd: toolsRoot, env });
    must(git(toolsRoot, "worktree", "add", "--quiet", "--detach", wt, head), "add the worktree");
    added = true;
    const merge = run("git", ["merge", "--no-ff", "--no-edit", base], { cwd: wt, env: { ...env, ...identity } });
    const files = must(git(wt, "diff", "--name-only", "--diff-filter=U"), "list conflicts").split("\n").filter(Boolean);
    if (merge.status !== 0 && !files.length) return { ok: false, reason: `git merge failed without conflicts: ${clean(merge.stderr || merge.stdout)}` };
    log(`${bead.id}: merging main ${base.slice(0, 12)} into ${head.slice(0, 12)}: ${files.length ? `conflicts in ${files.join(", ")}` : "merged cleanly"}`);

    const reportPath = join(runDir, "report.json");
    const prompt = renderPrompt(readFileSync(PROMPT_FILE, "utf8"), { // crlf-ok: a prompt; line endings do not matter to the agent
      number: pr.number, url: pr.url ?? `https://github.com/${repo}/pull/${pr.number}`, branch: pr.headRefName, bead: bead.id,
      base, head, files: files.length ? files.map((f) => `- ${f}`).join("\n") : "- (none: git merged it cleanly; check and commit if needed)", report: reportPath,
    });
    const aenv = { ...agentEnv(env), ...identity, PR_FIXER_REPORT: reportPath };
    const [cmd, args] = agent === "claude"
      ? ["claude", ["-p", "--dangerously-skip-permissions", "--output-format", "text"]]
      : ["codex", ["exec", "-s", "danger-full-access", "-C", wt, "-o", join(runDir, "agent-last-message.txt"), "-"]];
    log(`${bead.id}: running ${agent} (limit ${Math.round(timeoutMs / 60000)} min); log ${logFile}`);
    const ran = await runBounded(...wrap(cmd, args, wt), { cwd: wt, env: aenv, input: prompt, timeoutMs, logFile });
    if (ran.timedOut) return { ok: false, reason: `the agent ran past ${Math.round(timeoutMs / 60000)} minutes and was killed` };
    let report = null;
    try { report = JSON.parse(readFileSync(reportPath, "utf8")); } catch { /* reportVerdict says so */ }
    const verdict = reportVerdict(report);
    if (!verdict.ok) return { ok: false, reason: verdict.reason };

    // The runner's own checks: it believes no claim the agent makes about git.
    if (must(git(wt, "diff", "--name-only", "--diff-filter=U"), "list conflicts")) return { ok: false, reason: "unmerged paths remain" };
    if (existsSync(join(must(git(wt, "rev-parse", "--absolute-git-dir"), "find the git dir"), "MERGE_HEAD"))) return { ok: false, reason: "the merge was left uncommitted" };
    const dirty = must(git(wt, "status", "--porcelain"), "read status");
    if (dirty) return { ok: false, reason: `the worktree has uncommitted changes: ${clean(dirty, 200)}` };
    const sha = must(git(wt, "rev-parse", "HEAD"), "read HEAD");
    if (sha === head) return { ok: false, reason: "the agent made no commit" };
    for (const [ancestor, name] of [[head, "the PR head"], [base, "main"]]) {
      if (git(wt, "merge-base", "--is-ancestor", ancestor, sha).status !== 0) return { ok: false, reason: `the new HEAD does not descend from ${name}; history was rewritten` };
    }
    const changed = must(git(wt, "diff", "--name-only", head, sha), "list changed files").split("\n").filter(Boolean);
    const marked = changed.filter((f) => existsSync(join(wt, f)) && conflictMarkers(readFileSync(join(wt, f), "latin1")).length); // crlf-ok: conflictMarkers splits on \r?\n
    if (marked.length) return { ok: false, reason: `conflict markers remain in ${marked.join(", ")}` };
    must(git(toolsRoot, "fetch", "--no-tags", "--quiet", "origin", "refs/heads/main"), "refetch main");
    const tip = must(git(toolsRoot, "rev-parse", "FETCH_HEAD"), "read main");
    if (git(wt, "merge-tree", "--write-tree", tip, sha).status !== 0) return { ok: false, reason: `main moved to ${tip.slice(0, 12)} and conflicts with the repair again` };
    const privacy = run(commands.privacy[0], [...commands.privacy[1]], { cwd: wt, env });
    if (privacy.status !== 0) return { ok: false, reason: `the privacy scan failed: ${clean(privacy.stdout + privacy.stderr)}` };

    const reviewPrompt = `Review only how merge commit ${sha} resolved the merge of main into this PR branch${files.length ? `, conflicting in ${files.join(", ")}` : ""}. Report [P1] when a resolution drops or breaks a change from either side (the PR or main), leaves a generated file out of step with its sources, or leaves a conflict marker. The PR's own changes were reviewed before it was queued: treat them as context, not as findings.`;
    const reviewed = await runBounded(...wrap("codex", ["exec", "review", "--base", tip, reviewPrompt], wt), { cwd: wt, env: agentEnv(env), timeoutMs: reviewTimeoutMs, logFile });
    if (reviewed.timedOut || reviewed.status !== 0) return { ok: false, reason: `codex review did not finish (exit ${reviewed.status}${reviewed.timedOut ? ", timed out" : ""})` };
    const review = reviewVerdict(reviewed.stdout);
    if (!review.ok) return { ok: false, reason: review.reason };

    const checks = report.checks.slice(0, 10).map((c) => clean(c.command, 200));
    const note = `**Conflict repair** (scripts/dev/pr-fixer.mjs, repair bead ${bead.id}, ${now()}): merged main ${base.slice(0, 12)} into this branch as ${sha.slice(0, 12)}${files.length ? `, resolving ${files.map((f) => `\`${clean(f, 200)}\``).join(", ")}` : ""}. The agent's focused checks: ${checks.map((c) => `\`${c}\``).join(", ")}. codex review: no P1. The Owner above still owns this PR; the fixer only resolved the conflict.`;
    const current = viewPr(pr.number);
    if (!current || current.headRefOid !== head || current.state !== "OPEN") return { ok: false, reason: `the PR moved during the repair (now ${current ? `${current.state} at ${current.headRefOid.slice(0, 12)}` : "unreadable"}); its owner's work wins` };
    if (!current.labels.includes(QUEUE_LABEL)) return { ok: false, reason: `the PR left the queue during the repair (no ${QUEUE_LABEL} label); its owner took it out, so nothing is pushed` };
    const body = upsertFixerNote(current.body, note);
    const owner = checkPrOwner({ body, title: current.title, head: { ref: current.headRefName } });
    if (!owner.ok) return { ok: false, reason: `the PR body would fail the PR owner check: ${owner.reason}` };
    must(git(wt, "push", "--quiet", "origin", `HEAD:refs/heads/${pr.headRefName}`), "push (never forced)");
    log(`${bead.id}: pushed ${sha} to ${pr.headRefName}`);
    const bodyFile = join(runDir, "body.md");
    writeFileSync(bodyFile, body);
    if (run("gh", ["pr", "edit", String(pr.number), "--repo", repo, "--body-file", bodyFile], { env }).status !== 0) log(`${bead.id}: could not update the PR body`);
    // AGENTS.md "Keep watching until it merges": whoever fixes the conflict removes needs-merge
    // (verified clean against main's tip above), or queue-pr refuses the exclusion label.
    if (current.labels.includes(NEEDS_MERGE_LABEL) && run("gh", ["pr", "edit", String(pr.number), "--repo", repo, "--remove-label", NEEDS_MERGE_LABEL], { env }).status !== 0) log(`${bead.id}: could not remove ${NEEDS_MERGE_LABEL}`);
    const queued = run(commands.queuePr[0], [...commands.queuePr[1], String(pr.number), "--head", sha, "--repo", repo], { cwd: toolsRoot, env });
    log(`${bead.id}: queue-pr exited ${queued.status}: ${clean(queued.stdout.split("\n").filter(Boolean).pop() ?? "")}`);
    return { ok: true, sha, base, files, checks, queueStatus: queued.status };
  } catch (error) {
    return { ok: false, reason: error.message };
  } finally {
    if (added) git(toolsRoot, "worktree", "remove", "--force", wt);
    git(toolsRoot, "worktree", "prune");
  }
}

async function main(argv) {
  const { values } = parseArgs({
    args: argv,
    options: {
      repo: { type: "string" }, "dry-run": { type: "boolean" }, agent: { type: "string", default: "codex" },
      "max-attempts": { type: "string", default: String(MAX_ATTEMPTS) }, "timeout-minutes": { type: "string", default: "50" },
      "work-root": { type: "string" },
    },
  });
  const repo = values.repo ?? process.env.GITHUB_REPOSITORY;
  const queueWorkspace = process.env.BRAUSER_QUEUE_WORKSPACE;
  if (!repo || !queueWorkspace || !["codex", "claude"].includes(values.agent)) {
    console.error("usage: BRAUSER_QUEUE_WORKSPACE=<bd workspace> node scripts/dev/pr-fixer.mjs --repo owner/name [--dry-run] [--agent codex|claude] [--max-attempts 2] [--timeout-minutes 50] [--work-root DIR]");
    return 2;
  }
  const toolsRoot = spawnSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8" }).stdout.trim();
  const result = await runPass({
    repo, toolsRoot, queueWorkspace, dryRun: values["dry-run"], agent: values.agent, workRoot: values["work-root"],
    maxAttempts: Number(values["max-attempts"]), timeoutMs: Number(values["timeout-minutes"]) * 60_000,
  });
  console.log(`pr-fixer: ${result.outcome}${result.reason ? `: ${result.reason}` : ""}`);
  return 0;
}

function isEntryPoint() {
  try { return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href; } catch { return false; }
}
if (isEntryPoint()) {
  main(process.argv.slice(2)).then((code) => process.exit(code), (error) => { console.error(`pr-fixer: ${error.message}`); process.exit(1); });
}
