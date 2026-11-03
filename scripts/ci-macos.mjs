// Does a change need the macOS build? (AGENTS.md "Local checks", ci.yml.)
//
// The macOS job is slow and its runners are often starved, so ci.yml runs it
// only for a change that can behave differently on macOS than on Windows.
// Windows runs the same Rust and extension tests for every code change, the
// ubuntu Accessibility job runs the axe and keyboard checks, and every push to
// main runs macOS as well, so main's own run catches what a PR skipped.
//
// A change needs macOS when it touches:
//  1. a path in MACOS_PATHS (toolchain, the native port, the smoke, the
//     macOS-only dev tools, the CI definition), or any crate's Cargo.toml;
//  2. host Rust that only Unix/macOS builds compile: a file inside a module
//     its parent gates (`#[cfg(unix)] pub mod service;` makes all of
//     host/src/service/ Unix-only), a file under an inner `#![cfg(unix)]`, or
//     changed lines (new version) or deleted lines (old version) inside an
//     item under a Unix-only `#[cfg(...)]`, or a line that adds such a cfg;
//  3. a file that Unix-only code embeds or names in a string literal
//     (`include_str!("../../SUMMARY.md")`, a fixture directory a service test
//     reads): Windows never runs those checks.
// Anything else is tested on Windows. An empty or failed diff runs macOS.
//
// The pure parts take a `repo` with:
//   read(path)     -> the file at the commit under test, or null if absent
//   readBase(path) -> the file at the base commit, or null if absent
//   hostRust()     -> every host .rs path at the commit under test
// and a `changes` map from scripts/ci-changes.mjs: path -> { added, deleted },
// new-version and old-version line numbers from `git diff -U0`.

import path from "node:path";

export const MACOS_PATHS = [
  // Build scripts, the Rust toolchain and dependencies.
  "host/build.rs", "Cargo.lock", "rust-toolchain.toml", ".cargo/",
  // The fuzz targets compile only on macOS (Unix/libFuzzer).
  "adapter-templates/fuzz/",
  // The extension's native port and its manifest (permissions, the native
  // host name): the boundary only the macOS smoke drives end to end. The wire
  // schema is checked on Windows by both sides' tests and generate.py --check.
  "extension/native.ts", "extension/manifest.json",
  // The smoke run itself.
  "scripts/smoke-macos.mjs",
  // macOS-only developer tools (LaunchAgent, native-host registration, live
  // deploy, index-lock watch); their tests skip off macOS.
  "scripts/dev/deploy-live", "scripts/dev/index-lock-watch",
  "scripts/dev-service.mjs", "scripts/register-dev-host.mjs",
  "scripts/tests/dev-service.test.mjs",
  "scripts/tests/test_deploy_live.py", "scripts/tests/test_index_lock_watch.py",
  // The CI definition and these classifiers: a change to them must prove the
  // macOS job still works.
  ".github/workflows/ci.yml", ".github/workflows/build-os.yml",
  "scripts/ci-changes.mjs", "scripts/ci-macos.mjs",
];

// --- cfg predicates ------------------------------------------------------------

/** Parses a cfg predicate ("all(test, unix)") into a tree; null if malformed. */
export function parseCfg(text) {
  let i = 0;
  const ws = () => { while (i < text.length && /\s/.test(text[i])) i++; };
  function node() {
    ws();
    const name = /^[A-Za-z_][A-Za-z0-9_]*/.exec(text.slice(i));
    if (!name) return null;
    i += name[0].length;
    ws();
    if (text[i] === "=") {
      i++; ws();
      const value = /^"((?:[^"\\]|\\.)*)"/.exec(text.slice(i));
      if (!value) return null;
      i += value[0].length;
      return { key: name[0], value: value[1] };
    }
    if (text[i] === "(") {
      i++;
      const args = [];
      for (;;) {
        ws();
        if (text[i] === ")") { i++; break; }
        const arg = node();
        if (!arg) return null;
        args.push(arg);
        ws();
        if (text[i] === ",") { i++; continue; }
        if (text[i] === ")") { i++; break; }
        return null;
      }
      return { op: name[0], args };
    }
    return { flag: name[0] };
  }
  const tree = node();
  ws();
  return tree && i === text.length ? tree : null;
}

/** True when code under this predicate is never compiled on Windows. */
export function excludesWindows(tree) {
  if (!tree) return false;
  if (tree.flag) return tree.flag === "unix";
  if (tree.key) {
    return (tree.key === "target_os" && tree.value !== "windows")
      || (tree.key === "target_family" && tree.value === "unix");
  }
  if (tree.op === "all") return tree.args.some(excludesWindows);
  if (tree.op === "any") return tree.args.length > 0 && tree.args.every(excludesWindows);
  if (tree.op === "not") return tree.args.length === 1 && isWindows(tree.args[0]);
  return false;
}

function isWindows(tree) {
  if (tree.flag) return tree.flag === "windows";
  if (tree.key) return (tree.key === "target_os" && tree.value === "windows") || (tree.key === "target_family" && tree.value === "windows");
  if (tree.op === "any") return tree.args.some(isWindows);
  if (tree.op === "all") return tree.args.length > 0 && tree.args.every(isWindows);
  return false;
}

/** The predicates of every `cfg(...)`/`cfg!(...)` on a line, balanced parens. */
export function cfgPredicates(line) {
  const out = [];
  const re = /\bcfg!?\s*\(/g;
  let m;
  while ((m = re.exec(line))) {
    let depth = 1, j = re.lastIndex;
    for (; j < line.length && depth > 0; j++) {
      if (line[j] === "(") depth++;
      else if (line[j] === ")") depth--;
    }
    if (depth === 0) out.push(line.slice(re.lastIndex, j - 1));
  }
  return out;
}

/** True when the line carries a cfg that excludes Windows. */
export function hasUnixOnlyCfg(line) {
  return cfgPredicates(line).some((p) => excludesWindows(parseCfg(p)));
}

const isOuterAttr = (line) => /^\s*#\[/.test(line);
const isInnerAttr = (line) => /^\s*#!\[/.test(line);

// --- Unix-only regions of one file -------------------------------------------

function stripLiterals(line) {
  return line.replace(/\/\/.*$/, "").replace(/"(?:[^"\\]|\\.)*"/g, '""').replace(/'(?:[^'\\]|\\.)'/g, "''");
}

/**
 * The 1-based lines of `text` that only Unix/macOS builds compile: the whole
 * file under an inner `#![cfg(unix)]`, else each item under a Unix-only
 * `#[cfg(...)]`, from the attribute to the item's closing brace, or, before a
 * block opens, its `;` (a use, a statement) or `,` (a field, a match arm)
 * outside parentheses and brackets. Brace counting skips strings, chars and
 * comments roughly, which is enough to bound an item.
 */
export function unixOnlyLines(text) {
  const lines = text.split("\n");
  const out = new Set();
  if (lines.slice(0, 60).some((line) => isInnerAttr(line) && hasUnixOnlyCfg(line))) {
    for (let n = 1; n <= lines.length; n++) out.add(n);
    return out;
  }
  for (let i = 0; i < lines.length; i++) {
    if (!isOuterAttr(lines[i]) || !hasUnixOnlyCfg(lines[i])) continue;
    let depth = 0, parens = 0, opened = false, end = lines.length - 1;
    scan: for (let j = i + 1; j < lines.length; j++) {
      const code = stripLiterals(lines[j]);
      if (!opened && isOuterAttr(code)) continue;
      for (const ch of code) {
        if (ch === "(" || ch === "[") parens++;
        else if (ch === ")" || ch === "]") parens--;
        else if (ch === "{") { depth++; opened = true; }
        else if (ch === "}") {
          depth--;
          if (opened && depth === 0) { end = j; break scan; }
          if (depth < 0) { end = j - 1; break scan; }
        } else if ((ch === ";" || ch === ",") && !opened && depth === 0 && parens <= 0) { end = j; break scan; }
      }
    }
    for (let k = i; k <= end; k++) out.add(k + 1);
  }
  return out;
}

// --- Repository-wide facts: gated modules and the files Unix code uses --------

/** The file or directory a module declared in `file` lives in. */
function moduleDir(file) {
  const base = path.posix.basename(file, ".rs");
  const dir = path.posix.dirname(file);
  return ["lib", "main", "mod"].includes(base) ? dir : `${dir}/${base}`;
}

/** Facts about the commit under test, computed once per repo. */
export function unixFacts(repo) {
  const files = repo.hostRust();
  const regions = new Map();
  const gatedModules = [];
  const inputs = new Set();
  for (const file of files) {
    const text = repo.read(file);
    if (text === null) continue;
    const gated = unixOnlyLines(text);
    regions.set(file, gated);
    const lines = text.split("\n");
    for (const n of gated) {
      const line = lines[n - 1] ?? "";
      const decl = /^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;/.exec(line);
      if (decl) {
        const dir = moduleDir(file);
        gatedModules.push(`${dir}/${decl[1]}.rs`, `${dir}/${decl[1]}/`);
      }
    }
  }
  // Every file inside a gated module is Unix-only throughout.
  const inGatedModule = (file) => gatedModules.some((entry) => (entry.endsWith("/") ? file.startsWith(entry) : file === entry));
  for (const file of files) {
    const text = repo.read(file);
    if (text === null) continue;
    const lines = text.split("\n");
    const gated = inGatedModule(file) ? null : regions.get(file);
    lines.forEach((line, index) => {
      if (gated && !gated.has(index + 1)) return;
      for (const literal of line.matchAll(/"((?:[^"\\]|\\.)*)"/g)) {
        for (const target of resolveLiteral(file, literal[1])) inputs.add(target);
      }
    });
  }
  return { regions, gatedModules, inGatedModule, inputs: [...inputs] };
}

/**
 * Repository paths a string literal in `file` may name: relative to the
 * file's directory (include_str!'s rule), or to host/ (CARGO_MANIFEST_DIR
 * joins, test fixture paths). Only literals that look like paths count.
 */
function resolveLiteral(file, literal) {
  if (!/[/.]/.test(literal) || /\s|[{}<>*?|]|^[a-z]+:/i.test(literal) || literal.length > 200) return [];
  if (!literal.includes("/") && !/\.(md|json|ts|txt|html|toml)$/.test(literal)) return [];
  const out = [];
  const fromFile = path.posix.normalize(path.posix.join(path.posix.dirname(file), literal));
  const fromHost = path.posix.normalize(path.posix.join("host", literal.replace(/^\/+/, "")));
  for (const candidate of [fromFile, fromHost]) {
    if (!candidate.startsWith("..") && candidate !== "." && candidate !== "host") out.push(candidate.replace(/\/$/, ""));
  }
  return out;
}

// --- The decision --------------------------------------------------------------

/** Why a change to `file` needs macOS, or null when it does not. */
export function macosReason(file, repo, changes, facts = unixFacts(repo)) {
  if (typeof file !== "string" || file === "") return "unknown path";
  if (file === "Cargo.toml" || file.endsWith("/Cargo.toml")) return "a crate manifest";
  if (MACOS_PATHS.some((entry) => (entry.endsWith("/") ? file.startsWith(entry) : file === entry))) return "a macOS path";
  if (facts.inputs.some((input) => file === input || file.startsWith(`${input}/`))) return "read by Unix-only code";
  if (!(file.startsWith("host/") && file.endsWith(".rs"))) return null;
  if (facts.inGatedModule(file)) return "inside a Unix-only module";
  const change = changes.get(file);
  const now = repo.read(file);
  const before = repo.readBase(file);
  if (!change) return "changed lines unknown";
  const nowLines = now === null ? [] : now.split("\n");
  const beforeLines = before === null ? [] : before.split("\n");
  const nowGated = now === null ? new Set() : (facts.regions.get(file) ?? unixOnlyLines(now));
  const beforeGated = before === null ? new Set() : unixOnlyLines(before);
  if (nowGated.size === nowLines.length && nowLines.length > 0) return "a Unix-only file";
  if (change.added.some((n) => nowGated.has(n) || hasUnixOnlyCfg(nowLines[n - 1] ?? ""))) return "changes Unix-only lines";
  if (change.deleted.some((n) => beforeGated.has(n) || hasUnixOnlyCfg(beforeLines[n - 1] ?? ""))) return "deletes Unix-only lines";
  return null;
}

/** Per path, why macOS is needed; an empty change list needs it. */
export function macosReasons(paths, repo, changes) {
  if (paths.length === 0) return [["(no changed paths)", "an empty or failed diff"]];
  const facts = unixFacts(repo);
  return paths.map((file) => [file, macosReason(file, repo, changes, facts)]).filter(([, why]) => why);
}

/** Changed line numbers per file from `git diff -U0`: new-version and old-version. */
export function parseChangedLines(diff) {
  const out = new Map();
  let oldFile = null, file = null;
  for (const line of diff.split("\n")) {
    if (line.startsWith("--- ")) { oldFile = line === "--- /dev/null" ? null : line.slice(6); continue; }
    if (line.startsWith("+++ ")) {
      file = line === "+++ /dev/null" ? oldFile : line.slice(6);
      if (file && !out.has(file)) out.set(file, { added: [], deleted: [] });
      continue;
    }
    const hunk = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(line);
    if (!hunk || !file) continue;
    const entry = out.get(file);
    const oldStart = Number(hunk[1]), oldCount = hunk[2] === undefined ? 1 : Number(hunk[2]);
    const newStart = Number(hunk[3]), newCount = hunk[4] === undefined ? 1 : Number(hunk[4]);
    for (let n = oldStart; n < oldStart + oldCount; n++) entry.deleted.push(n);
    for (let n = newStart; n < newStart + newCount; n++) entry.added.push(n);
  }
  return out;
}
