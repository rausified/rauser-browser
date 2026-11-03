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

test("everything else counts as code, including SUMMARY.md and nested Markdown that tests or builds read", () => {
  for (const path of [
    "host/src/native.rs",
    "SUMMARY.md",
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

test("toolchain, platform-only modules, the native port, smoke and CI changes need macOS", async () => {
  const { needsMacos, needsMacosPath } = await import("../ci-changes.mjs");
  for (const path of [
    "Cargo.lock", "Cargo.toml", "adapter-templates/Cargo.toml", "host/Cargo.toml", "rust-toolchain.toml", "host/build.rs",
    "adapter-templates/fuzz/fuzz_targets/match.rs", "host/src/service/unix_socket.rs", "host/src/macos/launch_agent.rs",
    "extension/native.ts", "extension/manifest.json", "scripts/smoke-macos.mjs", "scripts/dev/deploy-live",
    ".github/workflows/ci.yml", ".github/workflows/build-os.yml", "scripts/ci-changes.mjs",
  ]) {
    assert.equal(needsMacosPath(path, () => ""), true, path);
  }
  assert.equal(needsMacos([]), true, "an empty or failed diff runs macOS");
  assert.equal(needsMacosPath("host/src/gone.rs", () => null), true, "a deleted host file may have held Unix-only code");
});

test("adapter, starter, extension UI, engine, protocol and tooling changes skip macOS (PR #243's and #264's shapes)", async () => {
  const { needsMacos, needsMacosPath } = await import("../ci-changes.mjs");
  for (const path of [
    "extension/starter-adapters.ts", "extension/capture-site.ts", "extension/panel.ts", "extension/panel.html",
    "extension/worker.ts", "extension/build.mjs", "extension/tests/starter-adapters.test.mjs",
    "protocol/schema.json", "protocol/adapter-templates/vectors.json", "protocol/capture-site/catalog.json",
    "adapter-templates/src/lib.rs", "adapter-templates/tests/structural.rs", "package.json", "package-lock.json",
    "scripts/gate-adapter.mjs", "scripts/dev/pr-watch.mjs", "scripts/pr-doctor.mjs", "SUMMARY.md",
    "host/tests/fixtures/capture-site-structural-response.json",
  ]) {
    assert.equal(needsMacosPath(path, () => ""), false, path);
  }
  assert.equal(needsMacos(["extension/starter-adapters.ts", "protocol/adapter-templates/vectors.json",
    "docs/adapters/confluence.page.md", "host/tests/fixtures/capture-site-structural-response.json"]), false);
});

const RUST = [
  "use std::path::Path;",              // 1
  "#[cfg(unix)]",                      // 2
  "use std::os::unix::fs::PermissionsExt;", // 3
  "pub struct Link<'a> {",             // 4
  "    #[cfg(unix)]",                  // 5
  "    pub socket: Option<&'a str>,",  // 6
  "    pub name: String,",             // 7
  "}",                                 // 8
  "fn shared() -> u32 {",              // 9
  "    let s = \"{ not a brace\";",    // 10
  "    1",                             // 11
  "}",                                 // 12
  "#[cfg(unix)]",                      // 13
  "fn only_unix() {",                  // 14
  "    if true { shared(); }",         // 15
  "}",                                 // 16
  "fn after() {}",                     // 17
].join("\n");

test("unixOnlyLines bounds each Unix-only item: a use, a struct field, a function", async () => {
  const { unixOnlyLines } = await import("../ci-changes.mjs");
  assert.deepEqual([...unixOnlyLines(RUST)].sort((a, b) => a - b), [2, 3, 5, 6, 13, 14, 15, 16]);
  assert.equal(unixOnlyLines("#![cfg(unix)]\nfn a() {}\nfn b() {}").size, 3, "an inner attribute gates the whole file");
  assert.equal(unixOnlyLines("#[cfg(not(unix))]\nfn windows_only() {}").size, 0, "Windows-only code is tested on Windows");
});

test("a host Rust change needs macOS only when a changed line is Unix-only or adds a Unix cfg", async () => {
  const { needsMacosPath } = await import("../ci-changes.mjs");
  const read = () => RUST;
  assert.equal(needsMacosPath("host/src/link.rs", read, () => [10, 11]), false, "shared code: Windows tests it");
  assert.equal(needsMacosPath("host/src/link.rs", read, () => [7]), false, "a shared field");
  assert.equal(needsMacosPath("host/src/link.rs", read, () => [6]), true, "a Unix-only field");
  assert.equal(needsMacosPath("host/src/link.rs", read, () => [15]), true, "inside a Unix-only function");
  assert.equal(needsMacosPath("host/src/link.rs", read, () => null), true, "unknown changed lines count as all");
  assert.equal(needsMacosPath("host/src/plain.rs", () => "fn a() {}\n", () => null), false, "no Unix-only code at all");
});

test("parseChangedLines reads new-file line numbers from git diff -U0, including pure deletions", async () => {
  const { parseChangedLines } = await import("../ci-changes.mjs");
  const diff = [
    "diff --git a/host/src/a.rs b/host/src/a.rs", "--- a/host/src/a.rs", "+++ b/host/src/a.rs",
    "@@ -10,2 +10,3 @@ fn x() {", "-a", "-b", "+a", "+b", "+c",
    "@@ -40,3 +41,0 @@", "-gone",
    "@@ -60 +60 @@", "-x", "+y",
    "diff --git a/host/src/b.rs b/host/src/b.rs", "--- a/host/src/b.rs", "+++ /dev/null", "@@ -1,3 +0,0 @@", "-x",
  ].join("\n");
  const out = parseChangedLines(diff);
  assert.deepEqual(out.get("host/src/a.rs"), [10, 11, 12, 41, 42, 60]);
  assert.equal(out.has("host/src/b.rs"), false, "a deleted file has no new-file lines");
});
