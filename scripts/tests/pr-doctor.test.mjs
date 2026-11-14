import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { ipcDenied } from "./ipc-guard.mjs";
import { quietGitEnv } from "./git-test-env.mjs";
import { spawnSync as spawnSyncTop } from "node:child_process";

import {
  MARKER, QUEUE_COMMENT, adoptArgs, classifyPr, commentBody, needsRequeue, requeueAfterPush,
  adoptAndSync, cleanFile, resolveAppendOnly, syncQueue, tryRepair, verifyRepair,
} from "../pr-doctor.mjs";

// --- resolveAppendOnly: diff3 hunks whose base is empty ----------------------

const hunk = (ours, base, theirs) =>
  `<<<<<<< ours\n${ours}||||||| base\n${base}=======\n${theirs}>>>>>>> theirs\n`;

test("both sides appending at the same point keeps both, main's (theirs) first", () => {
  const text = "# Log\n\n" + hunk("## B from the PR\n", "", "## A from main\n") + "tail\n";
  const result = resolveAppendOnly(text);
  assert.equal(result.resolved, true);
  assert.equal(result.text, "# Log\n\n## A from main\n\n## B from the PR\ntail\n");
});

test("list-item appends are joined without inserting a blank line", () => {
  const text = hunk("- +1 PR evidence\n", "", "- +1 main evidence\n");
  assert.equal(resolveAppendOnly(text).text, "- +1 main evidence\n- +1 PR evidence\n");
});

test("a hunk where either side changed base text is left for a person", () => {
  const text = "a\n" + hunk("changed by PR\n", "original\n", "changed by main\n") + "z\n";
  const result = resolveAppendOnly(text);
  assert.equal(result.resolved, false);
  assert.equal(result.remaining, 1);
  assert.equal(result.text, text, "an unresolvable file is returned unchanged");
});

test("one unresolvable hunk keeps the whole file unresolved", () => {
  const text = hunk("x\n", "", "y\n") + hunk("p\n", "q\n", "r\n");
  const result = resolveAppendOnly(text);
  assert.equal(result.resolved, false);
  assert.equal(result.remaining, 1);
});

test("two-way markers without a base section are never resolved", () => {
  const text = "<<<<<<< ours\nx\n=======\ny\n>>>>>>> theirs\n";
  assert.equal(resolveAppendOnly(text).resolved, false);
});

test("text with no markers is already resolved and unchanged", () => {
  assert.deepEqual(resolveAppendOnly("plain\n"), { resolved: true, remaining: 0, text: "plain\n" });
});

test("a truncated hunk is not resolved", () => {
  assert.equal(resolveAppendOnly("<<<<<<< ours\nx\n||||||| base\n=======\ny\n").resolved, false);
});

// --- classifyPr --------------------------------------------------------------

const pr = (over = {}) => ({
  number: 1, baseRefName: "main", isDraft: false, isCrossRepository: false,
  mergeable: "MERGEABLE", labels: [], ...over,
});

test("a conflicting same-repo PR into main is a candidate", () => {
  assert.equal(classifyPr(pr({ mergeable: "CONFLICTING" })), "conflicting");
});

test("drafts, forks, other bases and opted-out PRs are skipped", () => {
  assert.equal(classifyPr(pr({ mergeable: "CONFLICTING", isDraft: true })), "skip");
  assert.equal(classifyPr(pr({ mergeable: "CONFLICTING", isCrossRepository: true })), "skip");
  assert.equal(classifyPr(pr({ mergeable: "CONFLICTING", baseRefName: "release" })), "skip");
  assert.equal(classifyPr(pr({ mergeable: "CONFLICTING", labels: [{ name: "pr-doctor:skip" }] })), "skip");
});

test("unknown mergeability is pending and a mergeable PR is clean", () => {
  assert.equal(classifyPr(pr({ mergeable: "UNKNOWN" })), "pending");
  assert.equal(classifyPr(pr()), "clean");
});

// --- commentBody -------------------------------------------------------------

test("every comment carries the sticky marker", () => {
  for (const outcome of ["repaired", "needs-person", "report-only", "resolved"]) {
    assert.ok(commentBody({ outcome, files: ["a.md"], resolvable: true, sha: "abc1234" }).startsWith(MARKER));
  }
});

test("a needs-person comment lists the files and says the queued PR is waiting", () => {
  const body = commentBody({ outcome: "needs-person", files: ["host/src/x.rs", "DESIGN.md"], queued: true });
  assert.match(body, /host\/src\/x\.rs/);
  assert.match(body, /DESIGN\.md/);
  assert.match(body, /labeled `queue` and waits/);
  assert.doesNotMatch(body, /@\w/, "the comment never mentions anyone");
});

test("a repaired comment names the merge commit and the auto-resolved files", () => {
  const body = commentBody({ outcome: "repaired", files: ["docs/history.md"], sha: "abc1234" });
  assert.match(body, /abc1234/);
  assert.match(body, /docs\/history\.md/);
});

// --- Fail-closed edge cases (review of #258) ---------------------------------

test("CRLF marker lines are never half-resolved: the file stays unresolved and unchanged", () => {
  const text = "<<<<<<< ours\r\nx\r\n||||||| base\r\n=======\r\ny\r\n>>>>>>> theirs\r\n";
  const result = resolveAppendOnly(text);
  if (result.resolved) {
    assert.doesNotMatch(result.text, /^(<{7}|={7}|>{7}|\|{7})/m, "a resolution never keeps markers");
  } else {
    assert.equal(result.text, text);
  }
});

test("a final hunk with no trailing newline still resolves both sides", () => {
  const text = "<<<<<<< ours\nB\n||||||| base\n=======\nA\n>>>>>>> theirs";
  const result = resolveAppendOnly(text);
  assert.equal(result.resolved, true);
  assert.equal(result.text, "A\nB\n");
});

test("marker-looking text inside a side leaves the hunk to a person", () => {
  const nested = "<<<<<<< ours\n<<<<<<< not really\n||||||| base\n=======\ny\n>>>>>>> theirs\n";
  assert.equal(resolveAppendOnly(nested).resolved, false);
  // A Markdown setext heading underline inside a side looks like a separator.
  const setext = "<<<<<<< ours\nTitle\n||||||| base\n=======\nOther\n=======\n>>>>>>> theirs\n";
  assert.equal(resolveAppendOnly(setext).resolved, false);
});

// --- The work queue left git (AGENTS.md "Work queue") -------------------------

test("a needs-person comment explains how to carry queue edits into bd", () => {
  const body = commentBody({ outcome: "needs-person", files: [".beads/issues.jsonl (bead(s) brwr-a1 changed differently)"] });
  assert.match(body, /scripts\/dev\/queue adopt --from <old tip> --base <merge base>/);
  assert.match(body, /git rm/);
  assert.match(body, /docs\/backlog\/frame\.md/);
});

test("adopt applies without syncing only when the doctor may push, and checks against a given frame", () => {
  // The repair step has no credential (brwr-p2a7): it applies the edits with
  // --no-sync (no network) and the publish step syncs them (syncQueue).
  assert.deepEqual(adoptArgs({ from: "tip", base: "abc", push: true }), ["adopt", "--from", "tip", "--base", "abc", "--no-sync"]);
  assert.deepEqual(adoptArgs({ from: "tip", base: "abc", push: false, frame: "/t/frame.md" }),
    ["adopt", "--from", "tip", "--base", "abc", "--frame", "/t/frame.md", "--dry-run"]);
});

// A PR cut before the move still edits the committed queue: main deleted the
// files, so GitHub reports a modify/delete conflict. The doctor applies the
// edits through bd, moves the backlog prose into the frame, and drops the
// files. Needs bd; skipped where it is not installed.
test("tryRepair applies a PR's queue edits through bd and drops the files", { skip: process.platform === "win32" }, (t) => doctorRepair(t, { conflicting: false }));
test("tryRepair refuses, naming the bead and field, when the database changed a field the PR also changed", { skip: process.platform === "win32" }, (t) => doctorRepair(t, { conflicting: true }));

async function doctorRepair(t, { conflicting }) {
  if (await ipcDenied(t, "child-pipe")) return;
  const { mkdtempSync, mkdirSync, rmSync, writeFileSync, readFileSync, realpathSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join, dirname } = await import("node:path");
  const { spawnSync } = await import("node:child_process");
  const version = spawnSync("bd", ["version"], { encoding: "utf8" });
  if (version.error?.code === "ENOENT") {
    t.skip("needs bd (beads) installed");
    return;
  }
  if (version.error) throw version.error;
  assert.equal(version.status, 0, version.stdout + version.stderr);
  const { fileURLToPath } = await import("node:url");
  const scripts = join(dirname(fileURLToPath(import.meta.url)), "..");
  const root = realpathSync(mkdtempSync(join(tmpdir(), "pr-doctor-queue-")));
  // Background git maintenance off in every scratch repo (git-test-env.mjs).
  const identity = quietGitEnv({ GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@example.com", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@example.com", BD_NON_INTERACTIVE: "1" });
  const saved = { ...process.env };
  const cwd = process.cwd();
  const run = (cmd, dir, ...args) => {
    const r = spawnSync(cmd, args, { cwd: dir, encoding: "utf8", env: { ...process.env, ...identity } });
    assert.equal(r.status, 0, `${cmd} ${args.join(" ")}: ${r.stdout}${r.stderr}`);
    return r.stdout.trim();
  };
  const git = (dir, ...args) => run("git", dir, ...args);
  const bead = (id, extra = {}) => JSON.stringify({ id, title: `Title of ${id}`, status: "open", priority: 2, created_at: "2026-10-01T00:00:00Z", updated_at: "2026-10-01T00:00:00Z", ...extra });
  const block = (section, body = "") => `<!-- BEGIN GENERATED section=${section}: edit the beads, then run python3 scripts/render_backlog.py -->\n${body}<!-- END GENERATED section=${section} -->\n`;
  // `proposed` is hand-written Proposed prose: the queue check fails a closed
  // bead named there.
  const frame = (prose, proposed = "") => `# Backlog\n\n${prose}\n\n## Proposed\n\n${proposed}${block("new")}\n## In Progress\n\n${block("in-progress")}\n## Shipped\n\n${block("shipped")}`;
  const cites = "Follow-up to `brwr-a1`.\n\n";
  const backlog = (prose, ids, proposed = "") => frame(prose, proposed).replace(block("new"), block("new", ids.map((id) => `- [ ] \`${id}\` **Title of ${id}.** _(P2)_\n`).join("")));
  try {
    Object.assign(process.env, identity);
    delete process.env.BRAUSER_QUEUE_WORKSPACE;
    const origin = join(root, "origin.git");
    const url = `file://${origin}`;
    git(root, "init", "-q", "--bare", "-b", "main", origin);
    const seed = join(root, "seed");
    git(root, "init", "-q", "-b", "main", seed);
    mkdirSync(join(seed, ".beads"));
    writeFileSync(join(seed, ".beads/issues.jsonl"), `${bead("brwr-a1")}\n`);
    writeFileSync(join(seed, "BOSUN_FEATURE_BACKLOG.md"), backlog("Intro.", ["brwr-a1"], cites));
    git(seed, "add", ".");
    git(seed, "commit", "-qm", "base");
    // The PR: a bead edit, new prose and an ordinary change.
    git(seed, "checkout", "-q", "-b", "feat/old");
    writeFileSync(join(seed, ".beads/issues.jsonl"), `${bead("brwr-a1", { status: "closed", updated_at: "2026-10-02T00:00:00Z" })}\n${bead("brwr-p1")}\n`);
    writeFileSync(join(seed, "BOSUN_FEATURE_BACKLOG.md"), backlog("Intro.\n\nProse from the PR.", ["brwr-p1"]));
    writeFileSync(join(seed, "code.txt"), "pr\n");
    git(seed, "add", ".");
    git(seed, "commit", "-qm", "pr");
    // Main moves the queue into Dolt and keeps the frame.
    git(seed, "checkout", "-q", "main");
    git(seed, "rm", "-q", ".beads/issues.jsonl", "BOSUN_FEATURE_BACKLOG.md");
    mkdirSync(join(seed, "docs/backlog"), { recursive: true });
    writeFileSync(join(seed, "docs/backlog/frame.md"), frame("Intro.", cites));
    git(seed, "add", ".");
    git(seed, "commit", "-qm", "queue moved");
    git(seed, "push", "-q", origin, "main", "feat/old");
    const seedRows = join(root, "seed.jsonl");
    writeFileSync(seedRows, `${bead("brwr-a1")}\n`);
    const main = join(root, "main");
    git(root, "clone", "-q", origin, main);
    run("python3", main, join(scripts, "work_queue.py"), "--repo", main, "seed", "--remote", url, "--from", seedRows, "--yes");
    // The doctor's bootstrapped workspace.
    const ws = join(root, "ws");
    git(root, "clone", "-q", origin, ws);
    run("bd", ws, "bootstrap");
    process.env.BRAUSER_QUEUE_WORKSPACE = ws;
    if (conflicting) {
      // Since the PR was cut, the database set the field the PR also sets.
      run("bd", ws, "update", "brwr-a1", "--status", "blocked");
      run("python3", ws, join(scripts, "work_queue.py"), "--repo", ws, "sync");
    }

    const clone = join(root, "clone");
    git(root, "clone", "-q", origin, clone);
    process.chdir(clone);
    const result = tryRepair({ number: 9, headRefName: "feat/old" }, scripts, true);
    if (!conflicting) {
      assert.equal(result.adopted, true, JSON.stringify(result));
      assert.deepEqual(result.carried, [".beads/issues.jsonl", "BOSUN_FEATURE_BACKLOG.md"]);
      // Applied locally only; the publish phase syncs it, against the PR's
      // repaired frame (Codex P2): the repair phase leaves the tree on main,
      // whose frame still cites brwr-a1, which the PR closed.
      const head = git(clone, "rev-parse", "HEAD");
      const prHead = git(clone, "rev-parse", "origin/feat/old");
      const verified = verifyRepair({ commit: head, head: prHead, main: git(clone, "rev-parse", "origin/main") });
      assert.equal(verified.ok, true, JSON.stringify(verified));
      assert.deepEqual(verified.carried.sort(), [".beads/issues.jsonl", "BOSUN_FEATURE_BACKLOG.md"]);
      git(clone, "checkout", "-q", "-f", "--detach", "origin/main");
      assert.equal(syncQueue(scripts).ok, false, "main's frame fails the check");
      // The publish job: a fresh database, main's tools, the repaired commit
      // as data (#463 review C1, M1, M2).
      const base = git(clone, "merge-base", prHead, verified.second);
      const fresh = (name) => {
        const dir = join(root, name);
        git(root, "clone", "-q", origin, dir);
        run("bd", dir, "bootstrap");
        return dir;
      };
      const dolt = () => git(clone, "ls-remote", origin, "refs/dolt/data");
      // A repaired frame main's adopt would not compute is refused before any sync.
      git(clone, "checkout", "-q", "-f", head);
      writeFileSync(join(clone, "docs/backlog/frame.md"), "# Not the doctor's frame\n");
      git(clone, "commit", "-qam", "forged frame");
      const forged = git(clone, "rev-parse", "HEAD");
      git(clone, "checkout", "-q", "-f", "--detach", "origin/main");
      process.env.BRAUSER_QUEUE_WORKSPACE = fresh("ws-forged");
      const beforeForged = dolt();
      const refused = adoptAndSync({ tools: scripts, head: prHead, base, commit: forged });
      assert.equal(refused.ok, false);
      assert.match(refused.detail, /is not what main's adopt computes/);
      assert.equal(dolt(), beforeForged, "nothing synced");
      process.env.BRAUSER_QUEUE_WORKSPACE = fresh("ws-publish");
      const published = adoptAndSync({ tools: scripts, head: prHead, base, commit: head });
      assert.equal(published.ok, true, JSON.stringify(published));
      assert.equal(git(clone, "status", "--porcelain").split("\n").filter((line) => line && line !== "?? target/").join("\n"), "", "nothing checked out in the main tree");
      git(clone, "checkout", "-q", "-f", head);
    }
    if (conflicting) {
      assert.equal(result.ok, false, JSON.stringify(result));
      assert.match(result.files.join("\n"), /brwr-a1 status: database 'blocked', branch 'closed'/);
      assert.equal(commentBody({ outcome: "needs-person", files: result.files }).includes("brwr-a1 status"), true);
      const after = run("bd", ws, "export");
      assert.match(after, /"id":"brwr-a1"[^\n]*"status":"blocked"/);
      assert.doesNotMatch(after, /"id":"brwr-p1"/);
      return;
    }
    assert.equal(result.ok, true, JSON.stringify(result));
    const tree = git(clone, "ls-tree", "-r", "--name-only", "HEAD").split("\n");
    assert.ok(!tree.includes(".beads/issues.jsonl") && !tree.includes("BOSUN_FEATURE_BACKLOG.md"), tree.join(","));
    assert.ok(tree.includes("code.txt"));
    assert.match(git(clone, "show", "HEAD:docs/backlog/frame.md"), /Prose from the PR\./);
    // A fresh bootstrap sees the PR's bead edits.
    const fresh = join(root, "fresh");
    git(root, "clone", "-q", origin, fresh);
    run("bd", fresh, "bootstrap");
    const rows = run("bd", fresh, "export");
    assert.match(rows, /"id":"brwr-p1"/);
    assert.match(rows, /"id":"brwr-a1"[^\n]*"status":"closed"/);
    readFileSync(join(clone, "docs/backlog/frame.md"));
  } finally {
    process.chdir(cwd);
    for (const key of Object.keys(process.env)) if (!(key in saved)) delete process.env[key];
    Object.assign(process.env, saved);
    rmSync(root, { recursive: true, force: true });
  }
}

test("tryRepair regenerates a PR whose only conflicts are generated files", { skip: process.platform === "win32" }, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { cpSync, mkdirSync } = await import("node:fs");
  const { join } = await import("node:path");
  const { ENV, STARTER_OUTPUTS, addFamily, commitAll, genStarters, git, makeFixture } = await import("./generated-fixture.mjs");
  const { root, repo, cleanup } = makeFixture();
  const saved = { ...process.env };
  const cwd = process.cwd();
  try {
    // Main gained one family since the PR branched; the PR adds another.
    for (const [branch, name] of [["main-next", "widgeta"], ["feat/family", "widgetb"]]) {
      git(repo, ["checkout", "-q", "-b", branch, "main"]);
      addFamily(repo, name);
      assert.equal(genStarters(repo).code, 0);
      commitAll(repo, `add ${name}`);
    }
    git(repo, ["checkout", "-q", "main"]);
    git(repo, ["merge", "-q", "--ff-only", "main-next"]);
    const origin = join(root, "origin.git");
    git(root, ["clone", "-q", "--bare", repo, origin]);
    const clone = join(root, "clone");
    git(root, ["clone", "-q", origin, clone]);
    // Main's tools, as the workflow copies them.
    const tools = join(root, "tools");
    mkdirSync(tools);
    for (const [from, to] of [["scripts/merge-generated.mjs", "merge-generated.mjs"], ["scripts/generate-starters.mjs", "generate-starters.mjs"], ["scripts/generate-current-state.mjs", "generate-current-state.mjs"], ["protocol/generate.py", "generate.py"]]) {
      cpSync(join(repo, from), join(tools, to));
    }
    Object.assign(process.env, ENV);
    process.chdir(clone);
    const result = tryRepair({ number: 7, headRefName: "feat/family" }, tools, false);
    assert.equal(result.ok, true, JSON.stringify(result));
    assert.ok(result.files.includes("extension/starter-families.generated.ts (regenerated from the merged sources)"), result.files.join("\n"));
    assert.ok(result.files.includes("protocol/starters/ids.lock.json (regenerated from the merged sources)"), result.files.join("\n"));
    assert.equal(git(clone, ["rev-list", "--parents", "-n", "1", "HEAD"]).out.trim().split(" ").length, 3, "a merge commit");
    assert.equal(genStarters(clone, "--check").code, 0, genStarters(clone, "--check").out);
    assert.equal(git(clone, ["status", "--porcelain"]).out, "");
    for (const file of STARTER_OUTPUTS) assert.doesNotMatch(git(clone, ["show", `HEAD:${file}`]).out, /^<{7} /m, file);
  } finally {
    process.chdir(cwd);
    for (const key of Object.keys(process.env)) if (!(key in saved)) delete process.env[key];
    Object.assign(process.env, saved);
    cleanup();
  }
});

// brwr-p2a7 (#463 review C1): the repair job has no credential and no
// remote, only a bundle; the publish job, a fresh clone, believes nothing in
// the repair job's state or bundle it can check against the API and main.
test("repair runs from a bundle with no remote; publish checks the repaired commit before pushing", { skip: process.platform === "win32" }, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { chmodSync, cpSync, mkdirSync, readFileSync, writeFileSync } = await import("node:fs");
  const { join } = await import("node:path");
  const { spawnSync } = await import("node:child_process");
  const { fileURLToPath } = await import("node:url");
  const { ENV, addFamily, commitAll, genStarters, git, makeFixture } = await import("./generated-fixture.mjs");
  const scripts = join(fileURLToPath(import.meta.url), "..", "..");
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  for (const [branch, name] of [["main-next", "widgeta"], ["feat/family", "widgetb"]]) {
    git(repo, ["checkout", "-q", "-b", branch, "main"]);
    addFamily(repo, name);
    assert.equal(genStarters(repo).code, 0);
    commitAll(repo, `add ${name}`);
  }
  git(repo, ["checkout", "-q", "main"]);
  git(repo, ["merge", "-q", "--ff-only", "main-next"]);
  const origin = join(root, "origin.git");
  git(root, ["clone", "-q", "--bare", repo, origin]);
  const before = git(origin, ["rev-parse", "refs/heads/feat/family"]).out.trim();
  const mainSha = git(origin, ["rev-parse", "refs/heads/main"]).out.trim();
  const tools = join(root, "tools");
  mkdirSync(join(tools, "dev"), { recursive: true });
  for (const name of ["pr-doctor.mjs", "pr-constants.mjs", "work_queue.py", "render_backlog.py", "check_queue_consistency.py", "merge-generated.mjs", "generate-starters.mjs", "generate-current-state.mjs"]) {
    cpSync(join(scripts, name), join(tools, name));
  }
  cpSync(join(scripts, "dev", "queue-pr.mjs"), join(tools, "dev", "queue-pr.mjs"));
  cpSync(join(scripts, "..", "protocol", "generate.py"), join(tools, "generate.py"));
  const env = { ...ENV, PATH: process.env.PATH, HOME: process.env.HOME, TMPDIR: process.env.TMPDIR ?? "/tmp" };
  const doctor = (cwd, phase, extra, more = []) => spawnSync(process.execPath,
    [join(tools, "pr-doctor.mjs"), "--tools", tools, "--phase", phase, ...more], { cwd, encoding: "utf8", env: { ...env, ...extra } });

  // The plan job's hand-over: a bundle of main and the branch, and the state.
  const plan = join(root, "plan");
  git(root, ["clone", "-q", origin, plan]);
  const inBundle = join(root, "repo.bundle");
  git(plan, ["bundle", "create", "-q", inBundle, "refs/remotes/origin/main", "refs/remotes/origin/feat/family"]);
  const state = join(root, "state.json");
  writeFileSync(state, JSON.stringify({ version: 2, canPush: true, items: [{ pr: { number: 7, headRefName: "feat/family" }, repair: true }] }));

  // The repair job: a repository with no remote at all.
  const work = join(root, "repair");
  git(root, ["init", "-q", "-b", "doctor", work]);
  git(work, ["fetch", "-q", "--no-tags", inBundle, "+refs/remotes/origin/*:refs/remotes/origin/*"]);
  git(work, ["checkout", "-q", "--detach", "origin/main"]);
  const outBundle = join(root, "repaired.bundle");
  const repaired = doctor(work, "repair", {}, ["--state", state, "--bundle-out", outBundle]);
  assert.equal(repaired.status, 0, repaired.stdout + repaired.stderr);
  assert.equal(JSON.parse(readFileSync(state, "utf8")).items[0].result.ok, true);
  const commit = git(work, ["rev-parse", "refs/pr-doctor/7"]).out.trim();

  // The publish job: a fresh clone, and a fake gh whose PR view says what the API would.
  const bin = join(root, "bin");
  mkdirSync(bin);
  const calls = join(root, "gh-calls");
  writeFileSync(join(bin, "gh"), `#!/bin/sh
echo "$*" >> "${calls}"
case "$*" in
  "api repos/example/repo/pulls/7") printf '{"head":{"ref":"%s","sha":"%s","repo":{"full_name":"example/repo"}},"base":{"ref":"main"},"state":"open","labels":[]}' "$FAKE_HEAD_REF" "${before}";;
  *--paginate*) echo "[[]]";;
  *) echo "{}";;
esac
`);
  chmodSync(join(bin, "gh"), 0o755);
  const publish = (bundle, headRef = "feat/family") => {
    const dir = join(root, `publish-${Math.random().toString(36).slice(2)}`);
    git(root, ["clone", "-q", origin, dir]);
    return doctor(dir, "publish", { PATH: `${bin}:${process.env.PATH}`, GITHUB_REPOSITORY: "example/repo", GH_TOKEN: "not-a-real-token", PR_DOCTOR_PUSH: "1", FAKE_HEAD_REF: headRef },
      ["--state", state, "--bundle", bundle]);
  };
  const branchTip = () => git(origin, ["rev-parse", "refs/heads/feat/family"]).out.trim();
  const forge = (name, makeCommit) => {
    const sha = makeCommit();
    git(work, ["update-ref", "refs/pr-doctor/7", sha]);
    const file = join(root, `${name}.bundle`);
    git(work, ["bundle", "create", "-q", file, "refs/pr-doctor/7"]);
    git(work, ["update-ref", "refs/pr-doctor/7", commit]);
    return file;
  };

  // The API names main as the head branch: refused, whatever the state says.
  const toMain = publish(outBundle, "main");
  assert.equal(toMain.status, 0, toMain.stdout + toMain.stderr);
  assert.match(toMain.stdout, /\| #7 \| refused: the PR's head branch is main \|/);
  assert.equal(git(origin, ["rev-parse", "refs/heads/main"]).out.trim(), mainSha);

  // Parents that are not (the PR head, a commit of main).
  const tree = git(work, ["rev-parse", `${commit}^{tree}`]).out.trim();
  const wrongParents = forge("parents", () => {
    const stranger = git(work, ["commit-tree", tree, "-m", "not on main"]).out.trim();
    return git(work, ["commit-tree", tree, "-p", before, "-p", stranger, "-m", "forged"]).out.trim();
  });
  const parents = publish(wrongParents);
  assert.match(parents.stdout, /\| #7 \| skipped \| the repaired commit's second parent is not a commit of main \|/);
  assert.equal(branchTip(), before);

  // A file outside what the doctor resolves, changed from both parents.
  const extra = forge("extra", () => {
    git(work, ["checkout", "-q", "-f", "--detach", commit]);
    writeFileSync(join(work, "scripts/merge-generated.mjs"), `${readFileSync(join(work, "scripts/merge-generated.mjs"), "utf8")}// forged\n`);
    git(work, ["add", "scripts/merge-generated.mjs"]);
    const forgedTree = git(work, ["write-tree"]).out.trim();
    git(work, ["checkout", "-q", "-f", "--detach", "origin/main"]);
    return git(work, ["commit-tree", forgedTree, "-p", before, "-p", mainSha, "-m", "forged"]).out.trim();
  });
  const beyond = publish(extra);
  assert.match(beyond.stdout, /\| #7 \| skipped \| the repaired commit changes scripts\/merge-generated\.mjs beyond what the doctor resolves \|/);
  assert.equal(branchTip(), before);

  // The real repair passes the checks and is pushed.
  const published = publish(outBundle);
  assert.equal(published.status, 0, published.stdout + published.stderr);
  assert.equal(branchTip(), commit);
  assert.match(published.stdout, /\| #7 \| repaired \|/);
  assert.match(readFileSync(calls, "utf8"), /repos\/example\/repo\/issues\/7\/comments/);
});

test("cleanFile keeps an untrusted file name to one quiet line", () => {
  assert.equal(cleanFile("a`b@c\nd<e>|f"), "a b c d e  f");
  assert.equal(cleanFile("x".repeat(500)).length, 200);
});

test("the merge runs in its own job with no permissions and no secret; publish is a fresh job over main", () => {
  const yml = readFileSync(new URL("../../.github/workflows/pr-doctor.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const job = (name) => yml.split(`\n  ${name}:\n`)[1].split(/\n  [\w-]+:\n/)[0];
  const repair = job("doctor-repair");
  assert.match(repair, /\n    permissions: \{\}\n/);
  assert.doesNotMatch(repair, /secrets\.|github\.token|GH_TOKEN|GIT_CONFIG|pr-doctor-git-auth|actions\/checkout/);
  assert.match(repair, /--phase repair/);
  assert.match(repair, /download-artifact@[0-9a-f]{40}/);
  const plan = job("doctor-plan");
  assert.match(plan, /persist-credentials: false/);
  assert.match(plan, /--phase plan/);
  const publish = job("doctor");
  assert.match(publish, /needs: \[doctor-plan, doctor-repair\]/);
  assert.match(publish, /actions\/checkout@[0-9a-f]{40}[^\n]*\n\s+with:\n\s+ref: main/);
  assert.match(publish, /persist-credentials: false/);
  assert.match(publish, /GIT_CONFIG_GLOBAL: \/dev\/null/);
  assert.match(publish, /GIT_CONFIG_NOSYSTEM: "1"/);
  assert.match(publish, /--phase publish/);
  // Publish runs its own checkout's copies of the tools, never the artifact's.
  assert.match(publish, /cp scripts\/pr-doctor\.mjs [^\n]*"\$RUNNER_TEMP\/pr-doctor\/"|cp scripts\/pr-doctor\.mjs/);
  assert.doesNotMatch(publish, /doctor-input/);
  // No credential is ever written to a .git/config.
  assert.doesNotMatch(yml.split("\n  keeper:\n")[0], /git (-C \S+ )?config [^\n]*extraheader/);
  for (const name of ["doctor-plan", "doctor"]) {
    for (const text of job(name).split(/\n      - /).filter((text) => /GH_TOKEN|DOCTOR_GIT_TOKEN/.test(text))) {
      assert.match(text, /\. "\$RUNNER_TEMP\/(doctor-input\/tools|pr-doctor)\/pr-doctor-git-auth\.sh"|GH_TOKEN/, name);
    }
  }
});

test("pr-doctor-git-auth.sh adds the header through GIT_CONFIG_* and keeps existing entries", { skip: process.platform === "win32" }, async () => {
  const { spawnSync } = await import("node:child_process");
  const script = new URL("../pr-doctor-git-auth.sh", import.meta.url).pathname;
  const result = spawnSync("sh", ["-c", `. "${script}" && echo "$GIT_CONFIG_COUNT|$GIT_CONFIG_KEY_1|$GIT_CONFIG_VALUE_1"`],
    { encoding: "utf8", env: { PATH: process.env.PATH, DOCTOR_GIT_TOKEN: "tok", GIT_CONFIG_COUNT: "1", GIT_CONFIG_KEY_0: "a.b", GIT_CONFIG_VALUE_0: "c" } });
  assert.equal(result.status, 0, result.stderr);
  const [count, key, value] = result.stdout.trim().split("\n").at(-1).split("|");
  assert.deepEqual([count, key], ["2", "http.https://github.com/.extraheader"]);
  assert.equal(value, `AUTHORIZATION: basic ${Buffer.from("x-access-token:tok").toString("base64")}`);
  assert.match(result.stdout, /^::add-mask::/m);
  const empty = spawnSync("sh", ["-c", `. "${script}"`], { encoding: "utf8", env: { PATH: process.env.PATH } });
  assert.notEqual(empty.status, 0);
});

test("scratch-repo git env turns background maintenance off and keeps existing entries", () => {
  const env = quietGitEnv({ GIT_CONFIG_COUNT: "1", GIT_CONFIG_KEY_0: "credential.helper", GIT_CONFIG_VALUE_0: "!echo" });
  assert.equal(env.GIT_CONFIG_COUNT, "4");
  assert.equal(env.GIT_CONFIG_KEY_0, "credential.helper");
  const pairs = Object.fromEntries([1, 2, 3].map((i) => [env[`GIT_CONFIG_KEY_${i}`], env[`GIT_CONFIG_VALUE_${i}`]]));
  assert.deepEqual(pairs, { "maintenance.auto": "false", "gc.auto": "0", "gc.autoDetach": "false" });
});

// --- requeue after a repair push ---------------------------------------------
// A push after an `@Mergifyio queue` request cancels it, whoever pushes (#318:
// the doctor's merge push at 04:43Z on 2026-11-05 cancelled the 02:11Z request
// and nothing requeued it). So after pushing a repair to a queued PR the
// doctor comments again, once GitHub reports the pushed head.

test("only a repaired PR carrying the queue label is requeued", () => {
  const labelled = { labels: [{ name: "queue" }] };
  assert.equal(needsRequeue(labelled, "repaired"), true);
  assert.equal(needsRequeue(labelled, "needs-person"), false);
  assert.equal(needsRequeue(labelled, "report-only"), false);
  assert.equal(needsRequeue({ labels: [], autoMergeRequest: {} }, "repaired"), false);
  assert.equal(needsRequeue({}, "repaired"), false);
});

test("the requeue waits until GitHub shows the pushed head, then comments once", () => {
  const heads = ["old", "old", "new"];
  const comments = [];
  const waits = [];
  const result = requeueAfterPush({
    sha: "new",
    headOf: () => heads.shift(),
    comment: (body) => comments.push(body),
    wait: (ms) => waits.push(ms),
  });
  assert.deepEqual(comments, [QUEUE_COMMENT]);
  assert.equal(QUEUE_COMMENT, "@Mergifyio queue");
  assert.deepEqual(waits, [5000, 5000]);
  assert.deepEqual(result, { settled: true, seen: "new" });
});

test("the requeue poll is bounded and still comments when the head never shows", () => {
  let reads = 0;
  const comments = [];
  const result = requeueAfterPush({
    sha: "new",
    headOf: () => { reads++; if (reads === 2) throw new Error("gh: 502"); return "old"; },
    comment: (body) => comments.push(body),
    wait: () => {},
    limitMs: 20000,
    stepMs: 5000,
  });
  assert.equal(reads, 5);
  assert.deepEqual(comments, [QUEUE_COMMENT]);
  assert.equal(result.settled, false);
  assert.equal(result.seen, "old");
});

test("a repaired comment for a queued PR says the doctor requeued it", () => {
  const body = commentBody({ outcome: "repaired", files: ["docs/history.md"], sha: "abc123", requeued: true });
  assert.match(body, /`@Mergifyio queue`/);
  assert.doesNotMatch(commentBody({ outcome: "repaired", files: ["a"], sha: "abc123" }), /Mergifyio/);
});
