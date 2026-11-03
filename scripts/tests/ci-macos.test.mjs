import assert from "node:assert/strict";
import test from "node:test";

import { excludesWindows, hasUnixOnlyCfg, macosReasons, parseCfg, parseChangedLines, unixOnlyLines } from "../ci-macos.mjs";

const LIB = ["pub mod native;", "#[cfg(unix)]", "pub mod service;", "#[cfg(test)]", "mod corpus;"].join("\n");
const LINK = [
  "use std::path::Path;",                  // 1
  "#[cfg(unix)]",                          // 2
  "use std::os::unix::fs::PermissionsExt;", // 3
  "pub struct Link<'a> {",                 // 4
  "    #[cfg(unix)]",                      // 5
  "    pub socket: Option<&'a str>,",      // 6
  "    pub name: String,",                 // 7
  "}",                                     // 8
  "fn shared() -> u32 {",                  // 9
  "    let s = \"{ not a brace\";",        // 10
  "    1",                                 // 11
  "}",                                     // 12
  "#[cfg(all(test, unix))]",               // 13
  "fn only_unix() {",                      // 14
  "    if true { shared(); }",             // 15
  "}",                                     // 16
  "fn after() {}",                         // 17
].join("\n");
const PROMPT = 'const SUMMARY_MD: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../SUMMARY.md"));\nconst ANSWERS: &str = "tests/fixtures/site-draft-answers/";\n';

function repo(head, base = head) {
  return {
    read: (file) => head[file] ?? null,
    readBase: (file) => base[file] ?? null,
    hostRust: () => Object.keys(head).filter((file) => file.startsWith("host/") && file.endsWith(".rs")),
  };
}
const HEAD = { "host/src/lib.rs": LIB, "host/src/link.rs": LINK, "host/src/service/prompt.rs": PROMPT, "host/src/native.rs": "fn a() {}\n" };
const lines = (added, deleted = []) => ({ added, deleted });
const needs = (paths, changes = new Map(), head = HEAD, base = head) => macosReasons(paths, repo(head, base), changes).length > 0;

test("cfg predicates exclude Windows in any argument order (Codex P2)", () => {
  for (const pred of ["unix", "all(test, unix)", 'all(feature = "scripted-dialogs", unix)', 'target_os = "macos"', "not(windows)", 'not(target_os = "windows")', 'any(target_os = "macos", target_os = "linux")']) {
    assert.equal(excludesWindows(parseCfg(pred)), true, pred);
  }
  for (const pred of ["test", "windows", "not(unix)", "any(unix, windows)", 'feature = "scripted-dialogs"', "any(test, unix)"]) {
    assert.equal(excludesWindows(parseCfg(pred)), false, pred);
  }
  assert.equal(hasUnixOnlyCfg('#![cfg(all(feature = "scripted-dialogs", unix))]'), true);
  assert.equal(hasUnixOnlyCfg("    if cfg!(unix) {"), true);
});

test("unixOnlyLines bounds each Unix-only item: a use, a struct field, a function", () => {
  assert.deepEqual([...unixOnlyLines(LINK)].sort((a, b) => a - b), [2, 3, 5, 6, 13, 14, 15, 16]);
  assert.equal(unixOnlyLines("#![cfg(unix)]\nfn a() {}\nfn b() {}").size, 3, "an inner attribute gates the whole file");
  assert.equal(unixOnlyLines("#[cfg(not(unix))]\nfn windows_only() {}").size, 0);
});

test("a file in a module its parent gates is Unix-only, with no attribute of its own (Codex P1)", () => {
  const head = { ...HEAD, "host/src/service/codex.rs": "pub fn run() {}\n" };
  assert.equal(needs(["host/src/service/codex.rs"], new Map([["host/src/service/codex.rs", lines([1])]]), head), true);
  assert.equal(needs(["host/src/native.rs"], new Map([["host/src/native.rs", lines([1])]]), head), false);
});

test("host Rust needs macOS only for changed lines in Unix-only code or a new Unix cfg", () => {
  const change = (added, deleted) => new Map([["host/src/link.rs", lines(added, deleted)]]);
  assert.equal(needs(["host/src/link.rs"], change([10, 11])), false, "shared code: Windows tests it");
  assert.equal(needs(["host/src/link.rs"], change([7])), false, "a shared field");
  assert.equal(needs(["host/src/link.rs"], change([6])), true, "a Unix-only field");
  assert.equal(needs(["host/src/link.rs"], change([15])), true, "inside a Unix-only function");
  assert.equal(needs(["host/src/link.rs"], new Map()), true, "unknown changed lines count as all");
});

test("deleting a whole Unix-only item is judged on the base version (Codex P2)", () => {
  const head = { ...HEAD, "host/src/link.rs": LINK.split("\n").slice(0, 12).concat(["fn after() {}"]).join("\n") };
  // Lines 13-16 (the Unix-only function) were deleted; its new neighbours are shared code.
  assert.equal(needs(["host/src/link.rs"], new Map([["host/src/link.rs", lines([], [13, 14, 15, 16])]]), head, HEAD), true);
  assert.equal(needs(["host/src/link.rs"], new Map([["host/src/link.rs", lines([], [10])]]), head, HEAD), false);
});

test("files Unix-only code embeds or names need macOS: SUMMARY.md, service fixtures (Codex P2)", () => {
  assert.equal(needs(["SUMMARY.md"]), true);
  assert.equal(needs(["host/tests/fixtures/site-draft-answers/ok.json"]), true);
  assert.equal(needs(["host/tests/fixtures/capture-site-structural-response.json"]), false, "read by shared code only");
});

test("toolchain, manifests, the native port, smoke, macOS dev tools and CI need macOS; the rest skips it", () => {
  for (const file of ["Cargo.lock", "host/Cargo.toml", "adapter-templates/Cargo.toml", "rust-toolchain.toml", "host/build.rs",
    "adapter-templates/fuzz/fuzz_targets/match.rs", "extension/native.ts", "extension/manifest.json", "scripts/smoke-macos.mjs",
    "scripts/dev/deploy-live", "scripts/dev-service.mjs", "scripts/register-dev-host.mjs", "scripts/tests/dev-service.test.mjs",
    ".github/workflows/ci.yml", ".github/workflows/build-os.yml", "scripts/ci-changes.mjs", "scripts/ci-macos.mjs"]) {
    assert.equal(needs([file]), true, file);
  }
  for (const file of ["extension/starter-adapters.ts", "extension/panel.ts", "extension/panel.html", "extension/worker.ts",
    "extension/build.mjs", "protocol/schema.json", "protocol/adapter-templates/vectors.json", "protocol/capture-site/catalog.json",
    "adapter-templates/src/lib.rs", "package.json", "package-lock.json", "scripts/gate-adapter.mjs", "scripts/dev/pr-watch.mjs"]) {
    assert.equal(needs([file]), false, file);
  }
  assert.equal(needs([]), true, "an empty or failed diff runs macOS");
});

test("parseChangedLines reads new and old line numbers from git diff -U0", () => {
  const diff = [
    "diff --git a/host/src/a.rs b/host/src/a.rs", "--- a/host/src/a.rs", "+++ b/host/src/a.rs",
    "@@ -10,2 +10,3 @@ fn x() {", "-a", "-b", "+a", "+b", "+c",
    "@@ -40,3 +41,0 @@", "-gone",
    "@@ -60 +60 @@", "-x", "+y",
    "diff --git a/host/src/b.rs b/host/src/b.rs", "--- a/host/src/b.rs", "+++ /dev/null", "@@ -1,3 +0,0 @@", "-x",
  ].join("\n");
  const out = parseChangedLines(diff);
  assert.deepEqual(out.get("host/src/a.rs"), { added: [10, 11, 12, 60], deleted: [10, 11, 40, 41, 42, 60] });
  assert.deepEqual(out.get("host/src/b.rs"), { added: [], deleted: [1, 2, 3] }, "a deleted file keeps its old lines");
});
