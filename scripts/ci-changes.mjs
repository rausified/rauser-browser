// CI change classifier (AGENTS.md "Review and merge cadence"): decides whether a
// pull request or merge group touches only docs and work-queue files, so
// .github/workflows/ci.yml can skip the macOS and Windows builds and run the
// cheap docs/queue checks instead. Nothing builds, tests or ships from these
// paths, so skipping the builds for them is safe:
//
// - Markdown at the repository root (AGENTS.md, DESIGN.md, TESTING.md,
//   README.md, BOSUN_FEATURE_BACKLOG.md, CookieFeature.md,
//   PENDING_AMOS_DOC_UPDATES.md, adapter-templates.md, ...). README.md and
//   the status lines are checked by scripts/check_current_state.py, which the
//   docs job still runs.
// - Anything under docs/ (plans, specs, history, starter-adapter notes).
// - Anything under .beads/ (the work queue).
//
// Everything else is code, including SUMMARY.md at the root (the native host
// compiles it in as the page-summary prompt, so its tests must run) and
// Markdown below the root (fixtures under
// host/tests/, protocol/README.md and protocol/adapter-templates/README.md
// are read by tests or define the protocol). An empty change list is never
// docs-only, so a diff that found nothing (or failed) runs the full builds.
//
// It also decides whether the change needs the macOS build (MACOS_PATHS below).
//
// Usage: node scripts/ci-changes.mjs <base> <head>
// Prints the changed paths, `docs_only=true|false` and `macos=true|false`,
// and appends both to $GITHUB_OUTPUT when it is set. Exits 1 if git fails.

import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync, realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";

const DOCS_DIRS = ["docs/", ".beads/"];
/** Root Markdown that a build reads, so it is code. */
const BUILT_ROOT_MARKDOWN = ["SUMMARY.md"];

/** True when nothing in CI builds or tests from `path`. */
export function isDocsOrQueuePath(path) {
  if (typeof path !== "string" || path === "") return false;
  if (DOCS_DIRS.some((dir) => path.startsWith(dir) && path.length > dir.length)) return true;
  if (BUILT_ROOT_MARKDOWN.includes(path)) return false;
  return !path.includes("/") && path.endsWith(".md") && path.length > ".md".length;
}

// Paths whose change can behave differently on macOS than on Windows, so
// they need the slow, often runner-starved macOS build (about 15 minutes when
// a runner is free). Everything else is covered by the Windows build (the
// same Rust and extension tests) plus the Linux accessibility job, and every
// push to main still runs macOS, so a regression a PR skipped is caught right
// after it merges. Keep this list short: each entry costs every PR that
// touches it a macOS run.
export const MACOS_PATHS = [
  // Build scripts and the Rust toolchain and dependencies.
  "host/build.rs", "Cargo.lock", "rust-toolchain.toml", ".cargo/",
  // The fuzz targets compile only on macOS (Unix/libFuzzer).
  "adapter-templates/fuzz/",
  // The extension's native port and its manifest (permissions, the native
  // host name): the boundary only the macOS smoke drives end to end. The
  // wire schema itself is checked on Windows by both sides' tests and
  // protocol/generate.py --check.
  "extension/native.ts", "extension/manifest.json",
  // The smoke run itself.
  "scripts/smoke-macos.mjs",
  // macOS-only developer tools; their tests skip off macOS.
  "scripts/dev/deploy-live", "scripts/dev/index-lock-watch",
  "scripts/tests/test_deploy_live.py", "scripts/tests/test_index_lock_watch.py",
  // The CI definition and this classifier: a change to them must prove the
  // macOS job still works.
  ".github/workflows/ci.yml", ".github/workflows/build-os.yml", "scripts/ci-changes.mjs",
];

// Rust compiled only off Windows (Unix or macOS). Windows never builds or
// tests it, so a host change whose changed lines touch it needs macOS; any
// other host change is tested by Windows.
export const UNIX_ONLY_CFG = /cfg!?\((?:all\(|any\()?\s*(?:unix|target_os\s*=\s*"macos"|not\(\s*windows\s*\)|not\(\s*target_os\s*=\s*"windows"\s*\)|target_family\s*=\s*"unix")/;
const UNIX_ONLY_ATTR = new RegExp(`#!?\\[\\s*${UNIX_ONLY_CFG.source}`);

/**
 * The 1-based lines of `text` that only Unix/macOS builds compile: the whole
 * file under an inner `#![cfg(unix)]`, else each item under a `#[cfg(unix)]`
 * (or macOS / not-Windows) attribute, from the attribute to the item's
 * closing brace or semicolon. Brace counting skips strings, chars and
 * comments only roughly, which is enough to bound an item.
 */
export function unixOnlyLines(text) {
  const lines = text.split("\n");
  const out = new Set();
  const inner = lines.slice(0, 40).some((line) => /^\s*#!\[/.test(line) && UNIX_ONLY_ATTR.test(line));
  if (inner) { for (let i = 1; i <= lines.length; i++) out.add(i); return out; }
  for (let i = 0; i < lines.length; i++) {
    if (!/^\s*#\[/.test(lines[i]) || !UNIX_ONLY_ATTR.test(lines[i])) continue;
    // An item ends at its block's closing brace, or, before any block opens,
    // at a `;` (a use, a statement) or a `,` (a struct field, a match arm)
    // outside parentheses and brackets.
    let depth = 0, parens = 0, opened = false, end = lines.length - 1;
    scan: for (let j = i + 1; j < lines.length; j++) {
      const code = lines[j].replace(/\/\/.*$/, "").replace(/"(?:[^"\\]|\\.)*"/g, '""').replace(/'(?:[^'\\]|\\.)'/g, "''");
      if (!opened && /^\s*#\[/.test(code)) continue;
      for (const ch of code) {
        if (ch === "(" || ch === "[") parens++;
        else if (ch === ")" || ch === "]") parens--;
        else if (ch === "{") { depth++; opened = true; }
        else if (ch === "}") {
          depth--;
          if (opened && depth === 0) { end = j; break scan; }
          // The enclosing block closed first: the attribute's item ended above.
          if (depth < 0) { end = j - 1; break scan; }
        } else if ((ch === ";" || ch === ",") && !opened && depth === 0 && parens <= 0) { end = j; break scan; }
      }
    }
    for (let k = i; k <= end; k++) out.add(k + 1);
  }
  return out;
}

/** Reads a changed file at the commit under test; null when it was deleted. */
function readWorkingTree(path) {
  try { return readFileSync(path, "utf8"); } catch { return null; }
}

/**
 * True when a change to `path` needs the macOS build. For a host Rust file,
 * `readFile(path)` gives its content at the commit under test (null if
 * deleted) and `changed(path)` its changed line numbers in that version
 * (null if unknown, which counts as every line).
 */
export function needsMacosPath(path, readFile = readWorkingTree, changed = () => null) {
  if (typeof path !== "string" || path === "") return true;
  // Any crate's manifest (features, dependencies, cfg-gated deps).
  if (path === "Cargo.toml" || path.endsWith("/Cargo.toml")) return true;
  if (MACOS_PATHS.some((entry) => (entry.endsWith("/") ? path.startsWith(entry) : path === entry))) return true;
  if (path.startsWith("host/") && path.endsWith(".rs")) {
    // A module named for the platform is compiled only there.
    if (/(^|[/_.-])(unix|macos|darwin)([/_.-]|$)/.test(path)) return true;
    const text = readFile(path);
    // A deleted file may have held Unix-only code: build it on macOS.
    if (text === null) return true;
    const gated = unixOnlyLines(text);
    if (gated.size === 0) return false;
    const lines = changed(path);
    if (lines === null) return true;
    const all = text.split("\n");
    return lines.some((n) => gated.has(n) || UNIX_ONLY_CFG.test(all[n - 1] ?? ""));
  }
  return false;
}

/** Changed line numbers per file in `head`, from `git diff -U0 base head`. */
export function parseChangedLines(diff) {
  const out = new Map();
  let file = null;
  for (const line of diff.split("\n")) {
    if (line.startsWith("+++ ")) { file = line === "+++ /dev/null" ? null : line.slice(6); if (file) out.set(file, out.get(file) ?? []); continue; }
    const hunk = /^@@ -\d+(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(line);
    if (hunk && file) {
      const start = Number(hunk[2]), count = hunk[3] === undefined ? 1 : Number(hunk[3]);
      const lines = out.get(file);
      // A pure deletion (count 0) sits between `start` and `start + 1`.
      if (count === 0) lines.push(start, start + 1);
      else for (let n = start; n < start + count; n++) lines.push(n);
    }
  }
  return out;
}

/** True when the macOS build must run; an empty or failed diff always runs it. */
export function needsMacos(paths, readFile = readWorkingTree, changed = () => null) {
  return paths.length === 0 || paths.some((path) => needsMacosPath(path, readFile, changed));
}

/** True when every changed path is a docs or queue file and there is at least one. */
export function classify(paths) {
  return paths.length > 0 && paths.every(isDocsOrQueuePath);
}

/** Split `git diff --name-only -z` output; NUL is the only separator. */
export function parseNameList(output) {
  return output.split("\0").filter((path) => path !== "");
}

function main(argv) {
  const [base, head] = argv;
  if (!base || !head) throw new Error("usage: node scripts/ci-changes.mjs <base> <head>");
  // --no-renames: a file moved from host/ into docs/ shows both paths, so it
  // still counts as a code change.
  const output = execFileSync("git", ["diff", "--name-only", "-z", "--no-renames", base, head], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const paths = parseNameList(output);
  for (const path of paths) console.log(`${isDocsOrQueuePath(path) ? "docs " : "code "} ${JSON.stringify(path)}`);
  const docsOnly = classify(paths);
  const rustDiff = execFileSync("git", ["diff", "-U0", "--no-renames", "--no-color", base, head, "--", "host/*.rs", "host/**/*.rs"], {
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
  });
  const changedLines = parseChangedLines(rustDiff);
  const changed = (path) => changedLines.get(path) ?? null;
  const macos = !docsOnly && needsMacos(paths, readWorkingTree, changed);
  for (const path of paths.filter((p) => needsMacosPath(p, readWorkingTree, changed))) console.log(`macos ${JSON.stringify(path)}`);
  console.log(`docs_only=${docsOnly}`);
  console.log(`macos=${macos}`);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `docs_only=${docsOnly}\nmacos=${macos}\n`);
}

// Compare real paths: argv[1] may go through a symlink or contain characters
// a file URL escapes (spaces, Windows drive letters).
function isEntryPoint() {
  try { return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href; } catch { return false; }
}

if (isEntryPoint()) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
