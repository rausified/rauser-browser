import assert from "node:assert/strict";
import test from "node:test";

import {
  MARKER, adoptArgs, classifyPr, commentBody, resolveAppendOnly, tryRepair,
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

test("a needs-person comment lists the files and says auto-merge is waiting", () => {
  const body = commentBody({ outcome: "needs-person", files: ["host/src/x.rs", "DESIGN.md"], autoMerge: true });
  assert.match(body, /host\/src\/x\.rs/);
  assert.match(body, /DESIGN\.md/);
  assert.match(body, /auto-merge/i);
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

// --- The work queue left main (AGENTS.md "Work queue") -------------------------

test("a needs-person comment explains how to move queue edits to the beads branch", () => {
  const body = commentBody({ outcome: "needs-person", files: [".beads/issues.jsonl (bead(s) brwr-a1 changed differently)"] });
  assert.match(body, /scripts\/dev\/queue adopt --from origin\/<this branch>/);
  assert.match(body, /git rm/);
});

test("adopt pushes only when the doctor may push", () => {
  assert.deepEqual(adoptArgs({ number: 7, base: "abc", push: true }),
    ["adopt", "--from", "HEAD", "--base", "abc", "-m", "queue: carry bead edits from #7 (PR doctor)"]);
  assert.equal(adoptArgs({ number: 7, base: "abc", push: false }).at(-1), "--dry-run");
});

// A PR cut before the move still edits the queue: main deleted the files, so
// GitHub reports a modify/delete conflict. The doctor carries the edits to
// the beads branch and drops the files from the PR.
test("tryRepair moves a PR's queue edits to the beads branch and drops the files", { skip: process.platform === "win32" }, async () => {
  const { mkdtempSync, mkdirSync, rmSync, writeFileSync, realpathSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join, dirname } = await import("node:path");
  const { spawnSync } = await import("node:child_process");
  const { fileURLToPath } = await import("node:url");
  const scripts = join(dirname(fileURLToPath(import.meta.url)), "..");
  const root = realpathSync(mkdtempSync(join(tmpdir(), "pr-doctor-queue-")));
  const identity = { GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@example.com", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@example.com" };
  const saved = { ...process.env };
  const cwd = process.cwd();
  const run = (dir, ...args) => {
    const r = spawnSync("git", args, { cwd: dir, encoding: "utf8", env: { ...process.env, ...identity } });
    assert.equal(r.status, 0, `git ${args.join(" ")}: ${r.stderr}`);
    return r.stdout.trim();
  };
  const bead = (id, extra = {}) => JSON.stringify({ id, title: `Title of ${id}`, status: "open", priority: 2, created_at: "2026-10-01T00:00:00Z", ...extra });
  const backlog = (ids) => {
    const block = (section, body = "") => `<!-- BEGIN GENERATED section=${section}: edit the beads, then run python3 scripts/render_backlog.py -->\n${body}<!-- END GENERATED section=${section} -->\n`;
    const items = ids.map((id) => `- [ ] \`${id}\` **Title of ${id}.** _(P2)_\n`).join("");
    return `# Backlog\n\n## Proposed\n\n${block("new", items)}\n## In Progress\n\n${block("in-progress")}\n## Shipped\n\n${block("shipped")}`;
  };
  try {
    Object.assign(process.env, identity);
    delete process.env.BRAUSER_QUEUE_DIR;
    const origin = join(root, "origin.git");
    run(root, "init", "-q", "--bare", "-b", "main", origin);
    const seed = join(root, "seed");
    run(root, "init", "-q", "-b", "main", seed);
    mkdirSync(join(seed, ".beads"));
    writeFileSync(join(seed, ".beads/issues.jsonl"), `${bead("brwr-a1")}\n`);
    writeFileSync(join(seed, "BOSUN_FEATURE_BACKLOG.md"), backlog(["brwr-a1"]));
    writeFileSync(join(seed, "notes.md"), "# Notes\n");
    run(seed, "add", ".");
    run(seed, "commit", "-qm", "base");
    // The PR: a bead edit plus an ordinary change.
    run(seed, "checkout", "-q", "-b", "feat/old");
    writeFileSync(join(seed, ".beads/issues.jsonl"), `${bead("brwr-a1")}\n${bead("brwr-p1")}\n`);
    writeFileSync(join(seed, "BOSUN_FEATURE_BACKLOG.md"), backlog(["brwr-a1", "brwr-p1"]));
    writeFileSync(join(seed, "code.txt"), "pr\n");
    run(seed, "add", ".");
    run(seed, "commit", "-qm", "pr");
    // Main moves the queue off.
    run(seed, "checkout", "-q", "main");
    run(seed, "rm", "-q", ".beads/issues.jsonl", "BOSUN_FEATURE_BACKLOG.md");
    run(seed, "commit", "-qm", "queue moved");
    run(seed, "push", "-q", origin, "main", "feat/old");
    run(seed, "checkout", "-q", "--orphan", "beads");
    run(seed, "rm", "-rqf", ".");
    mkdirSync(join(seed, ".beads"), { recursive: true });
    writeFileSync(join(seed, ".beads/issues.jsonl"), `${bead("brwr-a1")}\n${bead("brwr-q1")}\n`);
    writeFileSync(join(seed, "BOSUN_FEATURE_BACKLOG.md"), backlog(["brwr-a1", "brwr-q1"]));
    run(seed, "add", ".");
    run(seed, "commit", "-qm", "beads");
    run(seed, "push", "-q", origin, "beads");

    const clone = join(root, "clone");
    run(root, "clone", "-q", origin, clone);
    process.chdir(clone);
    const result = tryRepair({ number: 9, headRefName: "feat/old" }, scripts, true);
    assert.equal(result.ok, true, JSON.stringify(result));
    assert.deepEqual(result.files.sort(), [
      ".beads/issues.jsonl (bead edits moved to the beads branch)",
      "BOSUN_FEATURE_BACKLOG.md (bead edits moved to the beads branch)",
    ]);
    const tree = run(clone, "ls-tree", "-r", "--name-only", "HEAD").split("\n");
    assert.ok(!tree.includes(".beads/issues.jsonl") && !tree.includes("BOSUN_FEATURE_BACKLOG.md"), tree.join(","));
    assert.ok(tree.includes("code.txt"));
    const queue = run(origin, "show", "beads:.beads/issues.jsonl");
    for (const id of ["brwr-a1", "brwr-p1", "brwr-q1"]) assert.match(queue, new RegExp(`"${id}"`));
    assert.match(run(origin, "log", "-1", "--format=%s", "beads"), /carry bead edits from #9/);
  } finally {
    process.chdir(cwd);
    for (const key of Object.keys(process.env)) if (!(key in saved)) delete process.env[key];
    Object.assign(process.env, saved);
    rmSync(root, { recursive: true, force: true });
  }
});
