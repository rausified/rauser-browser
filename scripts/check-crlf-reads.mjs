#!/usr/bin/env node

// CRLF lint for tests that read the repository's own text (delay-fix rank 11,
// brwr-0524). Windows CI checks the repository out with core.autocrlf, so a
// test that reads a source file and looks for a `\n`-spelled marker passes on
// macOS and Linux and fails only on Windows, usually in the merge queue: #377's
// rename guard (host/src/fsutil.rs) read every host/src file with
// `fs::read_to_string` and searched for "#[cfg(test)]\nmod tests", so on
// Windows it never found a test module and reported every fixture's rename.
//
// The rule: in test code, a read of a file in the checkout either normalizes
// line endings in the same statement (`.replace("\r\n", "\n")`,
// `.replace(/\r\n/g, "\n")`, Rust's `.lines()`, which strips the `\r`) or
// carries a `crlf-ok: <reason>` comment on the read's line or the line above
// it (for a run of reads on consecutive lines, above the first). The reason
// says why line endings cannot matter there, for example a single-line
// `contains` check.
//
// What counts:
// - Test code: Rust files under a `tests/` directory, and every line from the
//   first `#[cfg(test)]` of any other .rs file; JavaScript `*.test.mjs` files
//   and any .mjs under a `tests/` directory. Python is not checked: its
//   `read_text()` and text-mode `open()` translate `\r\n` to `\n` already.
// - A read: `read_to_string(` or `include_str!(` in Rust, `readFileSync(` or
//   `readFile(` in JavaScript.
// - In the checkout: the statement names the repository (`CARGO_MANIFEST_DIR`,
//   `include_str!`, `import.meta`, `REPO`, `ROOT`, `repoRoot`, `__dirname`),
//   or, in Rust, the read sits within WALK_WINDOW lines after both
//   `CARGO_MANIFEST_DIR` and `read_dir(`: a test that walks the source tree,
//   as #377's did, reads paths that never name the repository themselves.
// - Exempt: a statement that parses structured data (JSON, TOML, `.parse()`),
//   names a .json or .toml file (parsed where it is used), names a
//   `fixtures/` path (host/tests/fixtures is checked out `-text`, byte-exact),
//   or, in JavaScript, reads bytes (no "utf8" encoding argument).
//
// Usage: node scripts/check-crlf-reads.mjs   (exit 1 and one line per finding)
// scripts/tests/check-crlf-reads.test.mjs runs it over the repository in
// `npm run test:scripts`, so every code CI path (and the README gate) fails a
// new un-normalized read.

import { execFileSync } from "node:child_process";
import { readFileSync, realpathSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const REPO = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

/** How far back (in lines) a Rust read looks for a source-tree walk. */
export const WALK_WINDOW = 120;
/** How many lines a statement may span (the read's line and the next ones). */
const STATEMENT_LINES = 3;

const READ = { rs: /\b(?:read_to_string|include_str!)\s*\(/, mjs: /\b(?:readFileSync|readFile)\s*\(/ };
const ANCHOR = /CARGO_MANIFEST_DIR|include_str!|import\.meta|\bREPO\b|\bROOT\b|\brepoRoot\b|__dirname/;
const NORMALIZED = /\\r|\.lines\(\)/;
const PARSED = /JSON\.parse|serde_json|toml::|\.parse\(\)|\.json\b|\.toml\b|fixtures\//;
const MARKER = /crlf-ok:\s*\S/;
const TEXT_ENCODING = /["'`]utf-?8["'`]|encoding\s*:/i;

/** "rs" or "mjs" when `file` holds test code this lint checks, else null. */
export function testKind(file) {
  const normalized = file.replaceAll("\\", "/");
  if (normalized.endsWith(".rs")) return "rs";
  if (normalized.endsWith(".mjs") && (/\.test\.mjs$/.test(normalized) || /(^|\/)tests\//.test(normalized))) return "mjs";
  return null;
}

/** The 0-based line where `file`'s test code starts, or -1 when it has none. */
function testStart(file, lines) {
  if (/(^|\/)tests\//.test(file.replaceAll("\\", "/")) || file.endsWith(".mjs")) return 0;
  return lines.findIndex((line) => /#\[cfg\((?:all\()?test\b/.test(line));
}

/** Findings `{ file, line, text }` (1-based line) for one file's text. */
export function scanText(text, file) {
  const kind = testKind(file);
  if (!kind) return [];
  // Split on either ending so a CRLF checkout scans the same lines.
  const lines = text.split(/\r?\n/);
  const start = testStart(file, lines);
  if (start < 0) return [];
  const findings = [];
  for (let i = start; i < lines.length; i++) {
    if (!READ[kind].test(lines[i])) continue;
    const statement = statementAt(lines, i);
    if (NORMALIZED.test(statement) || PARSED.test(statement)) continue;
    if (kind === "mjs" && !TEXT_ENCODING.test(statement)) continue;
    if (marked(lines, i, kind)) continue;
    if (!ANCHOR.test(statement) && !(kind === "rs" && walksSourceTree(lines, Math.max(start, i - WALK_WINDOW), i))) continue;
    findings.push({ file, line: i + 1, text: lines[i].trim() });
  }
  return findings;
}

/** The read's line and its continuation lines, up to the first that ends the statement. */
function statementAt(lines, index) {
  const parts = [];
  for (let j = index; j < Math.min(lines.length, index + STATEMENT_LINES); j++) {
    parts.push(lines[j]);
    if (/;\s*(?:\/\/.*)?$/.test(lines[j])) break;
  }
  return parts.join(" ");
}

/** A crlf-ok marker on the read's line, or above it or the first read of its run. */
function marked(lines, index, kind) {
  let first = index;
  while (first > 0 && READ[kind].test(lines[first - 1])) first--;
  return MARKER.test(lines[index]) || MARKER.test(lines[index - 1] ?? "") || MARKER.test(lines[first - 1] ?? "");
}

function walksSourceTree(lines, from, to) {
  const window = lines.slice(from, to).join("\n");
  return window.includes("CARGO_MANIFEST_DIR") && window.includes("read_dir(");
}

/** Every finding in the tracked files of `repo`. */
export function scanRepo(repo = REPO) {
  const files = execFileSync("git", ["ls-files", "-z", "--", "*.rs", "*.mjs"], {
    cwd: repo, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, env: { ...process.env, GIT_OPTIONAL_LOCKS: "0" },
  }).split("\0").filter((file) => file !== "" && testKind(file));
  return files.flatMap((file) => scanText(readFileSync(path.join(repo, file), "utf8"), file));
}

function isEntryPoint() {
  try { return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href; } catch { return false; }
}

if (isEntryPoint()) {
  const findings = scanRepo();
  for (const { file, line, text } of findings) {
    console.log(`${file}:${line}: reads checkout text without normalizing CRLF (add .replace("\\r\\n", "\\n") or a "crlf-ok: <reason>" comment): ${text}`);
  }
  if (findings.length) process.exit(1);
  console.log("check-crlf-reads: every test read of checkout text is CRLF-safe");
}
