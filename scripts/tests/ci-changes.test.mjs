import assert from "node:assert/strict";
import test from "node:test";

import { classify, isDocsOrQueuePath, parseNameList } from "../ci-changes.mjs";

test("root Markdown, docs/ and the beads queue are docs or queue files", () => {
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
    "docs/adapters/github.pr.md",
    "docs/superpowers/plans/2026-10-28-task-bar.md",
    ".beads/issues.jsonl",
    ".beads/config.yaml",
  ]) {
    assert.equal(isDocsOrQueuePath(path), true, path);
  }
});

test("everything else counts as code, including nested Markdown that tests or builds read", () => {
  for (const path of [
    "host/src/native.rs",
    "host/tests/fixtures/pre-home-layout/README.md",
    "protocol/README.md",
    "protocol/adapter-templates/README.md",
    "protocol/schema.json",
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
