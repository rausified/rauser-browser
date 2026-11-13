import assert from "node:assert/strict";
import test from "node:test";

import { classify, devToolsReasons, isDocsOrQueuePath, parseNameList } from "../ci-changes.mjs";

test("root Markdown, docs/ and .beads/ are docs paths (check_current_state.py refuses a committed queue file)", () => {
  for (const path of [
    "AGENTS.md",
    "BOSUN_FEATURE_BACKLOG.md",
    "CookieFeature.md",
    "DESIGN.md",
    "PENDING_AMOS_DOC_UPDATES.md",
    "README.md",
    "TESTING.md",
    "adapter-templates.md",
    "docs/history.md",
    "docs/superpowers/plans/2026-10-28-task-bar.md",
    ".beads/issues.jsonl",
    ".beads/config.yaml",
  ]) {
    assert.equal(isDocsOrQueuePath(path), true, path);
  }
});

test("everything else counts as code, including SUMMARY.md and nested Markdown that tests or builds read", () => {
  for (const path of [
    "host/src/native.rs",
    "SUMMARY.md",
    "host/tests/fixtures/pre-home-layout/README.md",
    "protocol/README.md",
    "protocol/adapter-templates/README.md",
    "protocol/schema.json",
    // Open decision 7 (starter families PR C): a starter doc's Status: line
    // is read by the generator's check 3 and the registry test, both code.
    "docs/adapters/jira.issue.md",
    "docs/adapters/README.md",
    ".github/workflows/ci.yml",
    "scripts/check_queue_consistency.py",
    "package.json",
    "Cargo.lock",
    "LICENSE",
    "README.md.orig",
    "docs",
    "docsx/a.md",
    ".beads",
    ".beadsx/issues.jsonl",
    "",
  ]) {
    assert.equal(isDocsOrQueuePath(path), false, JSON.stringify(path));
  }
});

test("a change is docs-only only when every path is a docs or queue file", () => {
  assert.equal(classify(["AGENTS.md", ".beads/issues.jsonl", "docs/history.md"]), true);
  assert.equal(classify(["AGENTS.md", "host/src/native.rs"]), false);
});

test("docs/adapters/ is carved out of docs/ (Open decision 7): a doc edit always runs the code builds", () => {
  assert.equal(classify(["docs/adapters/jira.issue.md"]), false);
  assert.equal(classify(["docs/history.md"]), true);
});

test("an empty change list is not docs-only, so a failed diff never skips the builds", () => {
  assert.equal(classify([]), false);
});

test("NUL-separated git output keeps unusual names intact and drops the trailing terminator", () => {
  assert.deepEqual(parseNameList("AGENTS.md\0docs/café notes.md\0"), ["AGENTS.md", "docs/café notes.md"]);
  assert.deepEqual(parseNameList(""), []);
});

test("a path with a newline in its name is one path, not two", () => {
  // `docs/a\nhost/src/x.rs` is a single file under docs/, not a docs file plus a host file.
  assert.deepEqual(parseNameList("docs/a\nhost/src/x.rs\0"), ["docs/a\nhost/src/x.rs"]);
});

// The Python dev-tools tests (build-os.yml "Test the queue tools and dev
// scripts", npm run test:dev-tools) run only when something they read
// changed (delay-fix rank 12, brwr-f8mm; c0e86b2d F6: 9 minutes locally for
// a change that touched no Python).
test("Python dev-tool sources, their tests and the files they read need the dev-tools tests", () => {
  for (const path of [
    "scripts/work_queue.py",
    "scripts/check_current_state.py",
    "scripts/tests/test_work_queue.py",
    "scripts/tests/git_test_env.py",
    "scripts/tests/fixtures/mergify-comments.json",
    "scripts/dev/queue",
    "scripts/dev/deploy-live",
    "scripts/dev/worktree-hygiene",
    "scripts/dev/queue-seed/brwr-9eio.jsonl",
    "worker-contract.json",
    "worker-contract.schema.json",
    ".gitattributes",
    "BRAUSER_IMPROVEMENT_FEED.md",
    "docs/improvement-feed/2026-11-12-some-entry.md",
    "docs/backlog/frame.md",
    "docs/backlog/notes/2026-11-12-note.md",
    ".beads/metadata.json",
    // The definitions that run them, and this classifier itself.
    ".github/workflows/ci.yml",
    ".github/workflows/build-os.yml",
    ".github/workflows/pr-hygiene.yml",
    "scripts/ci-changes.mjs",
    "package.json",
  ]) {
    assert.equal(devToolsReasons([path]).length, 1, path);
  }
});

test("a change that touches no Python dev tool skips the dev-tools tests", () => {
  assert.deepEqual(devToolsReasons([
    "host/src/native.rs",
    "extension/panel.ts",
    "scripts/dev/pr-watch.mjs",
    "scripts/tests/pr-watch.test.mjs",
    "scripts/ci-macos.mjs",
    "scripts/smoke-macos.mjs",
    "docs/history.md",
    "README.md",
    "Cargo.lock",
  ]), []);
});

test("each dev-tools reason names the path that needs the tests", () => {
  assert.deepEqual(devToolsReasons(["extension/panel.ts", "scripts/work_queue.py", "worker-contract.json"]), [
    ["scripts/work_queue.py", "a Python dev tool or its tests"],
    ["worker-contract.json", "read by the dev-tools tests"],
  ]);
});

test("an empty change list runs the dev-tools tests, so a failed diff never skips them", () => {
  assert.deepEqual(devToolsReasons([]), [["(no changed paths)", "an empty or failed diff runs everything"]]);
});
