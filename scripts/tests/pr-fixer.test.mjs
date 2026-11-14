// Tests for scripts/dev/pr-fixer.mjs: the pure decisions, then whole passes on
// temporary git repositories with fake bd, gh, agent, review, queue sync,
// privacy scan and queue-pr commands. Nothing here reaches GitHub, bd's real
// database or a real agent.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  EXHAUSTED_LABEL, FIXER_ACTOR, NOTE_END, NOTE_MARKER, agentEnv, attemptsOf, conflictMarkers, decideBead,
  SANDBOX_ARGS, defaultSecretPaths, maskArgs, parseRepairRef, renderPrompt, reportVerdict, reviewVerdict, runPass, upsertFixerNote,
} from "../dev/pr-fixer.mjs";
import { quietGitEnv } from "./git-test-env.mjs";

const HEAD = "0123456789abcdef0123456789abcdef01234567";
const bead = (over = {}) => ({ id: "brwr-x1", status: "open", assignee: "", labels: ["pr-keeper"], external_ref: `gh-pr-7-${HEAD.slice(0, 12)}`, ...over });
const pr = (over = {}) => ({
  number: 7, state: "OPEN", headRefName: "feature", headRefOid: HEAD, baseRefName: "main", isCrossRepository: false,
  mergeable: "CONFLICTING", labels: ["queue", "needs-merge"], mergeCommit: null, body: "Owner: claude session abc\n", title: "feat: x", ...over,
});

test("parseRepairRef reads the keeper's ref and nothing else", () => {
  assert.deepEqual(parseRepairRef(`gh-pr-384-${HEAD.slice(0, 12)}`), { number: 384, head: HEAD.slice(0, 12) });
  for (const bad of ["", null, "gh-pr-x-abc", "gh-pr-12-", "gh-pr-12-XYZ1234", "jira-12", "gh-pr-12-abc12345;rm"]) assert.equal(parseRepairRef(bad), null, String(bad));
});

test("attemptsOf reads metadata as an object or a JSON string", () => {
  assert.equal(attemptsOf(bead()), 0);
  assert.equal(attemptsOf(bead({ metadata: { fixer_attempts: "2" } })), 2);
  assert.equal(attemptsOf(bead({ metadata: JSON.stringify({ fixer_attempts: 1 }) })), 1);
  assert.equal(attemptsOf(bead({ metadata: "not json" })), 0);
});

test("decideBead closes on a merged or closed PR, citing the merge commit", () => {
  assert.deepEqual(decideBead(bead(), pr({ state: "MERGED", mergeCommit: { oid: "abc123" } })), { action: "close", reason: "PR #7 merged into main as abc123" });
  assert.equal(decideBead(bead({ status: "in_progress", assignee: FIXER_ACTOR }), pr({ state: "CLOSED" })).action, "close");
  // Merged somewhere else is not on main (AGENTS.md "Close only what is on main").
  assert.equal(decideBead(bead(), pr({ state: "MERGED", baseRefName: "dev", mergeCommit: { oid: "abc" } })).action, "skip");
});

test("decideBead repairs only a conflicting PR it may own", () => {
  assert.equal(decideBead(bead(), pr()).action, "repair");
  // Its own earlier claim (a crash, or main moved again after its push) is repairable again.
  assert.equal(decideBead(bead({ status: "in_progress", assignee: FIXER_ACTOR }), pr()).action, "repair");
  const skips = [
    [bead({ status: "in_progress", assignee: "claude session 1" }), pr(), /claimed by claude session 1/],
    [bead({ status: "blocked" }), pr(), /status blocked/],
    [bead({ labels: [EXHAUSTED_LABEL] }), pr(), /exhausted/],
    [bead({ metadata: { fixer_attempts: 2 } }), pr(), /exhausted/],
    [bead(), pr({ isCrossRepository: true }), /fork/],
    [bead(), pr({ baseRefName: "dev" }), /base dev/],
    [bead(), pr({ labels: ["held"] }), /held/],
    [bead(), pr({ labels: ["pr-doctor:skip"] }), /pr-doctor:skip/],
    [bead({ external_ref: "gh-pr-8-abc1234" }), pr(), /names PR #8/],
    [bead(), null, /not found/],
    [bead({ external_ref: "elsewhere" }), pr(), /not a repair ref/],
  ];
  for (const [b, p, why] of skips) {
    const d = decideBead(b, p);
    assert.equal(d.action, "skip", JSON.stringify(d));
    assert.match(d.reason, why);
  }
  assert.equal(decideBead(bead(), pr({ mergeable: "MERGEABLE" })).action, "wait");
  // The owner took the PR out of the queue: the fixer leaves it alone (AGENTS.md "Remove the `queue` label ... to take a PR out").
  assert.match(decideBead(bead(), pr({ labels: ["needs-merge"] })).reason, /not queued/);
  assert.equal(decideBead(bead(), pr({ mergeable: "UNKNOWN" })).action, "wait");
});

test("decideBead retries the queue request for its own pushed head until queue-pr succeeded", () => {
  const mine = (meta) => bead({ status: "in_progress", assignee: FIXER_ACTOR, metadata: { fixer_attempts: "1", fixer_pushed_sha: HEAD, ...meta } });
  assert.equal(decideBead(mine({ fixer_queue_status: "3" }), pr({ mergeable: "MERGEABLE" })).action, "queue");
  assert.equal(decideBead(mine({ fixer_queue_status: "0" }), pr({ mergeable: "MERGEABLE" })).action, "wait");
  assert.equal(decideBead(mine({ fixer_queue_status: "3" }), pr({ mergeable: "MERGEABLE", headRefOid: "f".repeat(40) })).action, "wait");
  assert.equal(decideBead(mine({ fixer_queue_status: "3" }), pr({ mergeable: "MERGEABLE", labels: [] })).action, "skip");
  // The attempt budget limits repairs, not the queue request for a head already pushed.
  assert.equal(decideBead(mine({ fixer_attempts: "2", fixer_queue_status: "3" }), pr({ mergeable: "MERGEABLE" })).action, "queue");
});

test("the sandbox has its own PID namespace and procfs, so the runner's environ is out of reach", () => {
  for (const flag of ["--unshare-pid", "--proc", "--die-with-parent"]) assert.ok(SANDBOX_ARGS.includes(flag), flag);
  assert.equal(SANDBOX_ARGS[SANDBOX_ARGS.indexOf("--proc") + 1], "/proc");
});

test("defaultSecretPaths hides the whole config directory, not a list of known tools", () => {
  // Other tools' tokens live there too (a worker's GitHub token, a beacon's keys): mask it all.
  const paths = defaultSecretPaths("/h");
  for (const p of ["/h/.config", "/h/.ssh", "/h/.git-credentials", "/h/.netrc"]) assert.ok(paths.includes(p), p);
  assert.ok(defaultSecretPaths("/h", { XDG_CONFIG_HOME: "/x/conf" }).includes("/x/conf"));
  assert.ok(defaultSecretPaths("/h", {}, ["/opt/w/token"]).includes("/opt/w/token"), "--secret-path adds a host-specific path");
});

test("maskArgs hides secret directories and files from the agent's sandbox", () => {
  const kinds = { "/h/.config/gh": "dir", "/h/.git-credentials": "file", "/h/.ssh": null };
  assert.deepEqual(maskArgs(Object.keys(kinds), (p) => kinds[p]), ["--tmpfs", "/h/.config/gh", "--ro-bind", "/dev/null", "/h/.git-credentials"]);
});

test("agentEnv removes every GitHub credential and credential helper", () => {
  const env = agentEnv({
    PATH: "/bin", HOME: "/h", GH_TOKEN: "t", GITHUB_TOKEN: "t", GH_ENTERPRISE_TOKEN: "t", PR_DOCTOR_TOKEN: "t", GIT_ASKPASS: "/x", SSH_AUTH_SOCK: "/s",
    GIT_CONFIG_COUNT: "3", GIT_CONFIG_KEY_0: "credential.helper", GIT_CONFIG_VALUE_0: "!x", GIT_CONFIG_KEY_1: "gc.auto", GIT_CONFIG_VALUE_1: "0",
    GIT_CONFIG_KEY_2: "credential.https://github.com.helper", GIT_CONFIG_VALUE_2: "!y",
  });
  for (const k of ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "PR_DOCTOR_TOKEN", "GIT_ASKPASS", "SSH_AUTH_SOCK"]) assert.equal(env[k], undefined, k);
  assert.equal(env.GIT_CONFIG_COUNT, "1");
  assert.deepEqual([env.GIT_CONFIG_KEY_0, env.GIT_CONFIG_VALUE_0], ["gc.auto", "0"]);
  assert.equal(env.GIT_CONFIG_KEY_1, undefined);
  assert.equal(env.GIT_TERMINAL_PROMPT, "0");
  assert.equal(env.PATH, "/bin");
});

test("conflictMarkers finds leftover markers, not ordinary rules", () => {
  assert.deepEqual(conflictMarkers("a\n<<<<<<< HEAD\nb\n=======\nc\n>>>>>>> main\n"), [2, 6]);
  assert.deepEqual(conflictMarkers("Title\n=======\n<<<<<<<< not a marker\n"), []);
  assert.deepEqual(conflictMarkers("x\r\n<<<<<<< ours\r\n"), [2]);
});

test("reviewVerdict fails on a P1 and on an empty review", () => {
  assert.deepEqual(reviewVerdict("- [P2] Nit — a.js:3\n"), { ok: true, p1: [] });
  const bad = reviewVerdict("Findings:\n- [P1] Drops main's guard — a.js:9\n- [P3] x\n");
  assert.equal(bad.ok, false);
  assert.deepEqual(bad.p1, ["- [P1] Drops main's guard — a.js:9"]);
  assert.equal(reviewVerdict("   ").ok, false);
});

test("reportVerdict needs resolved true and every listed check passed", () => {
  assert.equal(reportVerdict({ resolved: true, checks: [{ command: "node --test x", passed: true }] }).ok, true);
  assert.match(reportVerdict(null).reason, /no report/);
  assert.match(reportVerdict({ resolved: false, reason: "needs the author" }).reason, /needs the author/);
  assert.match(reportVerdict({ resolved: true, checks: [] }).reason, /no focused check/);
  assert.match(reportVerdict({ resolved: true, checks: [{ command: "cargo test", passed: false }] }).reason, /cargo test/);
});

test("upsertFixerNote adds one marked block, replaces it, and keeps the Owner line", () => {
  const once = upsertFixerNote("Owner: claude session abc\n\n## Summary\n", "first");
  assert.match(once, /^Owner: claude session abc/);
  assert.equal(once.split(NOTE_MARKER).length, 2);
  const twice = upsertFixerNote(once, "second");
  assert.equal(twice.split(NOTE_MARKER).length, 2);
  assert.match(twice, /second/);
  assert.doesNotMatch(twice, /first/);
  assert.ok(twice.trimEnd().endsWith(NOTE_END));
});

test("renderPrompt fills every placeholder and refuses an unknown one", () => {
  assert.equal(renderPrompt("PR {{number}} on {{branch}}", { number: 7, branch: "f" }), "PR 7 on f");
  assert.throws(() => renderPrompt("{{nope}}", {}), /nope/);
});

// --- Whole passes ------------------------------------------------------------
// The fixer runs on a Linux or macOS host: these use shebang fakes on PATH and
// POSIX process groups, so they skip on Windows (the pure tests above still run).
const POSIX = { skip: process.platform === "win32" && "the PR fixer is a POSIX host tool" };

function script(dir, name, body) {
  const path = join(dir, name);
  writeFileSync(path, `#!/usr/bin/env node\n${body}\n`);
  chmodSync(path, 0o755);
  return path;
}

/** Temporary origin with a PR branch that conflicts with main, a tools clone, and fakes. */
function world({ review = "No findings.\n", agentResolves = true } = {}) {
  const root = mkdtempSync(join(tmpdir(), "pr-fixer-"));
  const env = quietGitEnv({
    ...process.env, GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@example.invalid", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@example.invalid",
    GH_TOKEN: "secret-for-test", FAKE_ROOT: root, FAKE_REVIEW: review, FAKE_RESOLVES: agentResolves ? "1" : "0",
  });
  const git = (cwd, ...args) => {
    const r = spawnSync("git", args, { cwd, env, encoding: "utf8" });
    assert.equal(r.status, 0, `git ${args.join(" ")}: ${r.stderr}`);
    return r.stdout.trim();
  };
  const seed = join(root, "seed");
  mkdirSync(seed);
  git(root, "init", "-q", "--bare", "-b", "main", join(root, "origin.git"));
  git(root, "init", "-q", "-b", "main", seed);
  writeFileSync(join(seed, "a.txt"), "base\n");
  git(seed, "add", ".");
  git(seed, "commit", "-qm", "base");
  git(seed, "checkout", "-qb", "feature");
  writeFileSync(join(seed, "a.txt"), "pr side\n");
  git(seed, "commit", "-qam", "pr");
  git(seed, "checkout", "-q", "main");
  writeFileSync(join(seed, "a.txt"), "main side\n");
  git(seed, "commit", "-qam", "main");
  git(seed, "remote", "add", "origin", join(root, "origin.git"));
  git(seed, "push", "-q", "origin", "main", "feature");
  const prHead = git(seed, "rev-parse", "feature");
  const tools = join(root, "tools");
  git(root, "clone", "-q", join(root, "origin.git"), tools);

  const bin = join(root, "bin");
  mkdirSync(bin);
  const state = (name, value) => writeFileSync(join(root, `${name}.json`), JSON.stringify(value));
  state("beads", [bead({ external_ref: `gh-pr-7-${prHead.slice(0, 12)}` })]);
  state("pr", pr({ headRefOid: prHead }));
  state("calls", []);
  const common = `const fs=require("fs"),p=require("path"),R=process.env.FAKE_ROOT;const rd=n=>JSON.parse(fs.readFileSync(p.join(R,n+".json"),"utf8"));const wr=(n,v)=>fs.writeFileSync(p.join(R,n+".json"),JSON.stringify(v));const log=(...a)=>{const c=rd("calls");c.push(a);wr("calls",c)};const A=process.argv.slice(2);`;
  script(bin, "bd", `${common}
let a=[...A];if(a[0]==="--actor")a=a.slice(2);log("bd",...a);
if(a[0]==="dolt")process.exit(0);
const beads=rd("beads");
if(a[0]==="list"){process.stdout.write(JSON.stringify(beads));process.exit(0)}
const b=beads.find(x=>x.id===a[1]);if(!b)process.exit(1);
if(a[0]==="close"){b.status="closed";b.close_reason=a[a.indexOf("--reason")+1];wr("beads",beads);process.exit(0)}
b.metadata=b.metadata||{};
for(let i=2;i<a.length;i++){const f=a[i];
if(f==="--claim"){if(b.assignee&&b.assignee!=="${FIXER_ACTOR}")process.exit(1);b.assignee="${FIXER_ACTOR}";b.status="in_progress"}
else if(f==="--status")b.status=a[++i];else if(f==="--assignee")b.assignee=a[++i];
else if(f==="--append-notes")b.notes=(b.notes?b.notes+"\\n":"")+a[++i];
else if(f==="--set-metadata"){const[k,v]=a[++i].split("=");b.metadata[k]=v}
else if(f==="--add-label")b.labels=[...b.labels,a[++i]];}
wr("beads",beads);`);
  script(bin, "gh", `${common}log("gh",...A);
const s=rd("pr");
if(A[0]==="pr"&&A[1]==="view"){process.stdout.write(JSON.stringify({...s,labels:s.labels.map(name=>({name}))}));process.exit(0)}
if(A[0]==="pr"&&A[1]==="edit"&&A.includes("--remove-label")){s.labels=s.labels.filter(l=>l!==A[A.indexOf("--remove-label")+1]);wr("pr",s);process.exit(0)}
if(A[0]==="pr"&&A[1]==="edit"){s.body=fs.readFileSync(A[A.indexOf("--body-file")+1],"utf8");wr("pr",s);process.exit(0)}
process.exit(1);`);
  // The agent and the review are one fake `codex`: `codex exec review ...` and `codex exec ... -`.
  script(bin, "codex", `${common}const cp=require("child_process");
if(A[1]==="review"){log("review",...A);process.stdout.write(process.env.FAKE_REVIEW);process.exit(0)}
log("agent",process.env.GH_TOKEN?"token-leaked":"no-token");
const prompt=fs.readFileSync(0,"utf8");const report=process.env.PR_FIXER_REPORT;if(!prompt.includes(report))process.exit(3);
if(process.env.FAKE_RESOLVES!=="1"){fs.writeFileSync(report,JSON.stringify({resolved:false,checks:[],reason:"needs the PR author's judgment"}));process.exit(0)}
fs.writeFileSync("a.txt","pr side\\nmain side\\n");
cp.execFileSync("git",["add","a.txt"]);cp.execFileSync("git",["commit","-q","--no-edit"]);
fs.writeFileSync(report,JSON.stringify({resolved:true,checks:[{command:"node --test a",passed:true}],summary:"kept both lines"}));`);
  const ok = (name) => [process.execPath, [script(bin, `fake-${name}`, `${common}log("${name}",...A);`)]];
  // A pass-through sandbox: it runs what follows "--" in --chdir's directory and masks nothing.
  const sandbox = script(bin, "fake-sandbox", `${common}log("sandbox");const i=A.indexOf("--"),c=A.indexOf("--chdir");
const r=require("child_process").spawnSync(A[i+1],A.slice(i+2),{cwd:A[c+1],stdio:"inherit"});process.exit(r.status??1);`);
  const commands = { sync: ok("sync"), privacy: ok("privacy"), queuePr: ok("queuePr"), mergeDrivers: ok("mergeDrivers"), sandbox: [sandbox, []] };
  env.PATH = `${bin}:${env.PATH}`;
  const read = (name) => JSON.parse(readFileSync(join(root, `${name}.json`), "utf8"));
  const originHead = () => git(tools, "ls-remote", "origin", "refs/heads/feature").split(/\s/)[0];
  return { root, env, tools, prHead, commands, read, git, state, originHead, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

const pass = (w, over = {}) => runPass({
  repo: "owner/name", toolsRoot: w.tools, workRoot: join(w.root, "work"), queueWorkspace: w.root, env: w.env,
  // timing-ok: the injected clock only names run folders and stamps notes; nothing compares it with the real time
  commands: w.commands, secretPaths: [], procProbe: [], log: () => {}, now: () => "2026-11-13T00:00:00Z", ...over,
});

test("a pass repairs a conflicting PR, pushes a merge and records it on the bead", POSIX, async (t) => {
  const w = world();
  t.after(w.cleanup);
  const result = await pass(w);
  assert.equal(result.outcome, "pushed", JSON.stringify(result));
  const pushed = w.originHead();
  assert.notEqual(pushed, w.prHead);
  assert.equal(w.git(w.tools, "fetch", "-q", "origin"), "");
  assert.equal(w.git(w.tools, "rev-list", "--parents", "-n1", pushed).split(" ").length, 3, "a merge commit");
  const [b] = w.read("beads");
  assert.equal(b.status, "in_progress");
  assert.equal(b.assignee, FIXER_ACTOR);
  assert.equal(b.metadata.fixer_pushed_sha, pushed);
  assert.equal(b.metadata.fixer_attempts, "1");
  assert.match(b.notes, /closes when the PR merges/);
  const calls = w.read("calls");
  assert.deepEqual(calls.find((c) => c[0] === "agent"), ["agent", "no-token"], "the agent never sees GH_TOKEN");
  assert.ok(calls.some((c) => c[0] === "review"), "codex review ran");
  const kinds = calls.map((c) => c[0]).filter((k) => ["sandbox", "agent", "review"].includes(k));
  assert.deepEqual(kinds, ["sandbox", "sandbox", "agent", "sandbox", "review"], "probe, agent and review each run in the sandbox");
  assert.deepEqual(calls.find((c) => c[0] === "queuePr").slice(1), ["7", "--head", pushed, "--repo", "owner/name"]);
  const body = w.read("pr").body;
  assert.ok(!w.read("pr").labels.includes("needs-merge"), "needs-merge removed after the push");
  const order = calls.map((c) => (c[0] === "gh" && c.includes("--remove-label") ? "unlabel" : c[0])).filter((k) => k === "unlabel" || k === "queuePr");
  assert.deepEqual(order, ["unlabel", "queuePr"], "unlabelled before queueing");
  assert.match(body, /^Owner: claude session abc/);
  assert.match(body, new RegExp(pushed.slice(0, 12)));
  assert.equal(spawnSync("git", ["worktree", "list"], { cwd: w.tools, encoding: "utf8" }).stdout.trim().split("\n").length, 1, "worktree removed");

  // Once GitHub reports the merge, the next pass closes the bead with the merge commit.
  w.state("pr", { ...w.read("pr"), state: "MERGED", mergeCommit: { oid: "feedface" } });
  assert.equal((await pass(w)).outcome, "closed");
  assert.match(w.read("beads")[0].close_reason, /feedface/);
});

test("a P1 from the review pushes nothing and releases the bead with the reason", POSIX, async (t) => {
  const w = world({ review: "- [P1] Drops main's line — a.txt:1\n" });
  t.after(w.cleanup);
  const result = await pass(w);
  assert.equal(result.outcome, "failed");
  assert.equal(w.originHead(), w.prHead, "nothing pushed");
  const [b] = w.read("beads");
  assert.equal(b.status, "open");
  assert.equal(b.assignee, "");
  assert.match(b.notes, /P1/);
  assert.ok(!b.labels.includes(EXHAUSTED_LABEL));
  // A second failure exhausts the bead; a third pass leaves it alone.
  assert.equal((await pass(w)).outcome, "failed");
  assert.ok(w.read("beads")[0].labels.includes(EXHAUSTED_LABEL));
  assert.equal((await pass(w)).outcome, "idle");
});

test("an agent that cannot resolve leaves the PR untouched", POSIX, async (t) => {
  const w = world({ agentResolves: false });
  t.after(w.cleanup);
  const result = await pass(w);
  assert.equal(result.outcome, "failed");
  assert.match(result.reason, /needs the PR author's judgment/);
  assert.equal(w.originHead(), w.prHead);
  assert.ok(!w.read("calls").some((c) => c[0] === "review" || c[0] === "queuePr"));
});

test("a PR head that moves during the repair wins over the fixer", POSIX, async (t) => {
  const w = world();
  t.after(w.cleanup);
  // The fake gh reports a new head on the second view (the pre-push check).
  const real = w.commands.privacy;
  w.commands.privacy = [process.execPath, ["-e", `const fs=require("fs"),p=require("path");const f=p.join(process.env.FAKE_ROOT,"pr.json");const s=JSON.parse(fs.readFileSync(f,"utf8"));s.headRefOid="${"f".repeat(40)}";fs.writeFileSync(f,JSON.stringify(s));`]];
  const result = await pass(w);
  w.commands.privacy = real;
  assert.equal(result.outcome, "failed");
  assert.match(result.reason, /moved/);
  assert.equal(w.originHead(), w.prHead);
});

test("dry run decides without claiming, and a second runner waits on the lock", POSIX, async (t) => {
  const w = world();
  t.after(w.cleanup);
  const dry = await pass(w, { dryRun: true });
  assert.equal(dry.outcome, "dry-run");
  assert.equal(dry.decisions[0].action, "repair");
  assert.ok(!w.read("calls").some((c) => c[0] === "bd" && c[1] === "update"));
  mkdirSync(join(w.root, "work"), { recursive: true });
  writeFileSync(join(w.root, "work", "pr-fixer.lock"), String(process.pid));
  assert.equal((await pass(w)).outcome, "locked");
});

test("a sandbox that lets the agent read a secret refuses the pass before any claim", POSIX, async (t) => {
  const w = world();
  t.after(w.cleanup);
  const secrets = join(w.root, "secrets");
  mkdirSync(secrets);
  writeFileSync(join(secrets, "env"), "GH_TOKEN=not-a-real-token\n");
  const result = await pass(w, { secretPaths: [secrets] });
  assert.equal(result.outcome, "refused");
  assert.match(result.reason, /sandbox/);
  // The same for the runner's own environment seen through procfs (here a stand-in file).
  writeFileSync(join(w.root, "environ"), "GH_TOKEN=not-a-real-token");
  assert.equal((await pass(w, { procProbe: [join(w.root, "environ")] })).outcome, "refused");
  assert.ok(!w.read("calls").some((c) => c[0] === "bd" && c[1] === "update"), "nothing claimed");
  assert.equal(w.originHead(), w.prHead);
});

test("an owner who takes the PR out of the queue mid-repair keeps it out", POSIX, async (t) => {
  const w = world();
  t.after(w.cleanup);
  w.commands.privacy = [process.execPath, ["-e", `const fs=require("fs"),p=require("path");const f=p.join(process.env.FAKE_ROOT,"pr.json");const s=JSON.parse(fs.readFileSync(f,"utf8"));s.labels=s.labels.filter(l=>l!=="queue");fs.writeFileSync(f,JSON.stringify(s));`]];
  const result = await pass(w);
  assert.equal(result.outcome, "failed");
  assert.match(result.reason, /queue/);
  assert.equal(w.originHead(), w.prHead);
  assert.ok(!w.read("calls").some((c) => c[0] === "queuePr"));
});

test("an opt-out label added mid-repair stops the push", POSIX, async (t) => {
  const w = world();
  t.after(w.cleanup);
  w.commands.privacy = [process.execPath, ["-e", `const fs=require("fs"),p=require("path");const f=p.join(process.env.FAKE_ROOT,"pr.json");const s=JSON.parse(fs.readFileSync(f,"utf8"));s.labels.push("held");fs.writeFileSync(f,JSON.stringify(s));`]];
  const result = await pass(w);
  assert.equal(result.outcome, "failed");
  assert.match(result.reason, /held/);
  assert.equal(w.originHead(), w.prHead);
});

test("a queue request that failed after the push is retried on the next pass", POSIX, async (t) => {
  const w = world();
  t.after(w.cleanup);
  const good = w.commands.queuePr;
  w.commands.queuePr = [process.execPath, ["-e", "process.exit(3)"]];
  assert.equal((await pass(w)).outcome, "pushed");
  assert.equal(w.read("beads")[0].metadata.fixer_queue_status, "3");
  w.commands.queuePr = good;
  // The label removal failed too (it is back): the retry clears it before queueing.
  w.state("pr", { ...w.read("pr"), headRefOid: w.originHead(), mergeable: "MERGEABLE", labels: ["queue", "needs-merge"] });
  assert.equal((await pass(w)).outcome, "queued");
  assert.ok(!w.read("pr").labels.includes("needs-merge"));
  assert.equal(w.read("beads")[0].metadata.fixer_queue_status, "0");
  assert.equal(w.read("calls").filter((c) => c[0] === "queuePr").length, 1);
});
