#!/usr/bin/env node
// Flags new timing assumptions in test code: fixed sleeps, wall-clock bounds
// on elapsed time, and date literals. Each one passes on an idle machine and
// fails on a loaded runner or a later calendar day (brwr-1kgh, brwr-3wyt,
// brwr-gxag). Wait for a condition, inject a clock, or synchronize instead.
//
// Existing uses are a ratchet, not a rewrite: `scripts/test-timing-baseline.json`
// holds each file's count per rule, and a file may not go above it. A use
// that is the behaviour under test (a hang guard, a fixture date that never
// meets the clock) carries a `timing-ok: <reason>` comment on its line or the
// line above, and is not counted.
//
// The baseline only shrinks. The check compares it with the base revision's
// copy (origin/main, else the first parent of a merge commit, which is what
// CI's pull-request checkout is) and fails on a new entry or a higher count,
// unless the same change adds a reason for it under `justified` in the
// baseline: `"justified": { "<file>": { "<rule>": "<why>" } }`. A reason the
// base already holds does not cover a new rise. With no base copy (the file's
// first introduction, or a checkout with neither ref) the comparison is
// skipped and says so.
//
// Rust wall-clock bounds are found two ways: a comparison with an `elapsed`
// value or an `as_millis()`-style number anywhere in test code, and, inside
// an `assert!`-family call, a spaced comparison (`a < b`, as rustfmt writes
// it, unlike `Vec<T>`) that involves `duration_since`, `elapsed`, or a name
// the file binds with `let` to `Instant::now()`, an elapsed value or a
// `Duration`. A deadline (`let deadline = Instant::now() + WAIT`) is not such
// a name, so `assert!(Instant::now() < deadline)` in a wait loop, a hang
// guard, is not flagged; nor is a comparison of two constant durations.
// Limits: a measured time reached only through a struct field or a helper
// call, a name bound in one file and compared in another, a comparison
// outside an assertion, and an assertion whose comparison rustfmt did not
// space are not flagged.
//
// `scripts/smoke-macos.mjs` is scanned too, as text only: importing it runs
// the smoke.
//
//   node scripts/check-test-timing.mjs            check the repository
//   node scripts/check-test-timing.mjs --update   rewrite the baseline to today's counts
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const BASELINE = "scripts/test-timing-baseline.json";
// This check's own tests hold every pattern as sample text.
const SELF_TEST = "scripts/tests/check-test-timing.test.mjs";
// Test drivers outside a tests directory, read as text and never imported.
const SCRIPTED_JS = new Set(["scripts/smoke-macos.mjs"]);

const ISO_DATE = /["'`](?:19|20)\d{2}-\d{2}-\d{2}/;

/** Each rule, per language, as line patterns. */
export const RULES = {
  "fixed-sleep": {
    why: "a fixed sleep assumes how long the other side takes; wait until the condition holds",
    rust: [/\bthread::sleep\s*\(/],
    js: [
      // A promise settled by a timer, whatever the delay expression:
      // setTimeout(resolve, 20), setTimeout(() => resolve(), WAIT_MS + 200).
      /\bsetTimeout\(\s*(?:resolve|res|r|done|\(\)\s*=>\s*(?:resolve|res|r|done)\(\s*\))\s*,/,
      /\b(?:delay|sleep)\(\s*[\w.$]+(?:\s*[-+*]\s*[\w.$]+)*\s*\)/,
    ],
    py: [/\btime\.sleep\s*\(/],
  },
  "wall-clock-bound": {
    why: "a bound on elapsed time fails on a slow runner; assert the work done, or keep time only as a hang guard",
    rust: [/\belapsed(?:\(\))?\s*<=?\s*[\w:(]/, /[\w)]\s*>=?\s*[\w.]*\belapsed\b/, /\bas_(?:millis|secs|micros)\(\)\s*<=?\s*\d/],
    js: [/\b(?:Date|performance)\.now\(\)\s*-\s*[\w.]+\s*<=?\s*\d/, /\belapsed\w*\s*<=?\s*\d/],
    py: [/\btime\.(?:monotonic|time|perf_counter)\(\)\s*-\s*\w+\s*<=?\s*\d/, /\belapsed\w*\s*<=?\s*\d/],
  },
  "date-literal": {
    why: "a fixed date meets the real clock one day; derive it from an injected now",
    rust: [ISO_DATE],
    js: [ISO_DATE],
    py: [ISO_DATE],
  },
};

const ALLOW = /timing-ok:\s*\S/;

/** The language whose test code `file` holds, or null when it holds none. */
export function languageOf(file) {
  const parts = file.split("/");
  const name = parts.at(-1);
  if (file.endsWith(".rs")) return "rust";
  if (SCRIPTED_JS.has(file)) return "js";
  if (/\.(?:test\.)?m?js$/.test(name) && (name.includes(".test.") || parts.includes("tests"))) return "js";
  if (name.endsWith(".py") && parts.includes("tests")) return "py";
  return null;
}

/** The lines of `text` that are test code, as [line number, text] pairs.
 * A Rust file under a `tests` directory, or named `tests.rs`, is all test
 * code; any other Rust file is test code from the `#[cfg(test)]` on its
 * inline test module (`mod tests {`), not one on a single item above it. */
export function testLines(file, text) {
  const lines = text.replaceAll("\r\n", "\n").split("\n");
  let start = 0;
  if (languageOf(file) === "rust") {
    const parts = file.split("/");
    const whole = parts.slice(0, -1).includes("tests") || parts.at(-1) === "tests.rs";
    if (!whole) {
      start = lines.findIndex((line, index) => /^\s*#\[cfg\(test\)\]/.test(line) &&
        /^\s*(?:pub(?:\([\w:]+\))?\s+)?mod\s+\w+\s*\{/.test(lines.slice(index + 1).find((next) => !/^\s*(?:#\[|\/\/)/.test(next)) ?? ""));
      if (start < 0) return [];
    }
  }
  return lines.slice(start).map((line, index) => [start + index + 1, line]);
}

/** Every counted finding in one file's text: {rule, line, text}. */
export function scan(file, text) {
  const language = languageOf(file);
  if (!language) return [];
  const findings = [];
  const lines = testLines(file, text);
  const timed = language === "rust" ? timeNames(lines) : null;
  let assertDepth = 0;
  lines.forEach(([number, line], index) => {
    const previous = index > 0 ? lines[index - 1][1] : "";
    let inAssert = assertDepth > 0;
    if (language === "rust") {
      const start = line.search(/\b(?:debug_)?assert(?:_\w+)?!\s*\(/);
      if (start >= 0 && assertDepth === 0) {
        inAssert = true;
        assertDepth = depthChange(line.slice(start));
      } else if (assertDepth > 0) {
        assertDepth = Math.max(0, assertDepth + depthChange(line));
      }
    }
    if (ALLOW.test(line) || (ALLOW.test(previous) && /^\s*(?:\/\/|#)/.test(previous))) return;
    for (const [rule, { [language]: patterns }] of Object.entries(RULES)) {
      const hit = patterns.some((pattern) => pattern.test(line)) ||
        (rule === "wall-clock-bound" && inAssert && assertsTime(line, timed));
      if (hit) findings.push({ rule, line: number, text: line.trim() });
    }
  });
  return findings;
}

const TIME_VALUE = /Instant::now\(\)|\.elapsed\(\)|\bduration_since\(|\bDuration::from_\w+\(/;

/** The names a Rust test binds with `let` to an instant or a duration. */
function timeNames(lines) {
  const names = new Set();
  for (const [, line] of lines) {
    const binding = /\blet\s+(?:mut\s+)?([a-z_]\w*)\s*(?::\s*[\w:<>]+\s*)?=\s*([^;]*)/.exec(line);
    // `Instant::now() + WAIT` is a deadline: comparing now with it is a
    // hang guard in a wait loop, not a bound on how long the work took.
    if (binding && TIME_VALUE.test(binding[2]) && !/Instant::now\(\)\s*[-+]/.test(binding[2])) names.add(binding[1]);
  }
  return names;
}

/** Whether an assertion line compares a time: a spaced comparison operator
 * and a duration, an elapsed value, or a name bound to one. */
function assertsTime(line, names) {
  const code = line.replace(/"(?:[^"\\]|\\.)*"/g, '""');
  if (!/\s(?:<=?|>=?)\s/.test(code)) return false;
  if (/\bduration_since\(|\belapsed\b/.test(code)) return true;
  return [...(names ?? [])].some((name) => new RegExp(`\\b${name}\\b`).test(code));
}

function depthChange(text) {
  const code = text.replace(/"(?:[^"\\]|\\.)*"/g, '""');
  return (code.match(/\(/g) ?? []).length - (code.match(/\)/g) ?? []).length;
}

/** Baseline entries above the base revision's: a new file or rule, or a
 * higher count, that this change gives no reason for. A reason counts only
 * when it is not blank and differs from the base's for that file and rule. */
export function baselineGrowth(current, base) {
  const grown = [];
  for (const [file, rules] of Object.entries(current.files ?? {})) {
    for (const [rule, to] of Object.entries(rules)) {
      const from = base.files?.[file]?.[rule] ?? 0;
      if (to <= from) continue;
      const reason = current.justified?.[file]?.[rule];
      const justified = typeof reason === "string" && reason.trim() !== "" && reason !== base.justified?.[file]?.[rule];
      if (!justified) grown.push({ file, rule, from, to });
    }
  }
  return grown.sort((a, b) => a.file.localeCompare(b.file) || a.rule.localeCompare(b.rule));
}

/** The baseline as the base revision holds it, or null when there is no
 * base to compare with: origin/main, else HEAD's first parent when HEAD is a
 * merge commit (CI's pull-request checkout), else nothing. */
export function baseBaseline(root, file = BASELINE) {
  const git = (...args) => execFileSync("git", ["-C", root, ...args], {
    encoding: "utf8", env: { ...process.env, GIT_OPTIONAL_LOCKS: "0" }, stdio: ["ignore", "pipe", "ignore"],
  }).trim();
  const exists = (ref) => { try { git("rev-parse", "--verify", "--quiet", `${ref}^{commit}`); return true; } catch { return false; } };
  let base = null;
  if (exists("refs/remotes/origin/main")) base = "refs/remotes/origin/main";
  else if (exists("HEAD^2") && exists("HEAD^1")) base = "HEAD^1";
  if (!base) return null;
  try {
    return JSON.parse(git("show", `${base}:${file}`));
  } catch {
    return null;
  }
}

/** Counts per file per rule, only for files with a finding. */
export function count(findingsByFile) {
  const counts = {};
  for (const [file, findings] of Object.entries(findingsByFile)) {
    for (const { rule } of findings) {
      counts[file] ??= {};
      counts[file][rule] = (counts[file][rule] ?? 0) + 1;
    }
  }
  return counts;
}

/** Files and rules over their baseline: {file, rule, count, allowed, findings}. */
export function overBaseline(findingsByFile, baseline) {
  const counts = count(findingsByFile);
  const over = [];
  for (const [file, rules] of Object.entries(counts)) {
    for (const [rule, value] of Object.entries(rules)) {
      const allowed = baseline[file]?.[rule] ?? 0;
      if (value > allowed) {
        over.push({ file, rule, count: value, allowed, findings: findingsByFile[file].filter((f) => f.rule === rule) });
      }
    }
  }
  return over.sort((a, b) => a.file.localeCompare(b.file) || a.rule.localeCompare(b.rule));
}

/** The baseline as committed: sorted, one file per line, so parallel PRs
 * that touch different files do not conflict. */
export function formatBaseline(counts, justified = {}) {
  const block = (entries, value) => Object.keys(entries).sort().map((file) => {
    const rules = Object.keys(entries[file]).sort().map((rule) => `${JSON.stringify(rule)}: ${value(entries[file][rule])}`);
    return `    ${JSON.stringify(file)}: { ${rules.join(", ")} }`;
  }).join(",\n");
  const kept = Object.fromEntries(Object.entries(justified)
    .map(([file, rules]) => [file, Object.fromEntries(Object.entries(rules).filter(([rule]) => counts[file]?.[rule]))])
    .filter(([, rules]) => Object.keys(rules).length));
  const reasons = Object.keys(kept).length ? `,\n  "justified": {\n${block(kept, JSON.stringify)}\n  }` : "";
  return `{\n  "version": 1,\n  "files": {\n${block(counts, String)}\n  }${reasons}\n}\n`;
}

export function scanRepository(root) {
  // Read-only git calls must not take index.lock (AGENTS.md).
  // Untracked files too, so a new test file is checked before it is added.
  const listed = execFileSync("git", ["ls-files", "-z", "--cached", "--others", "--exclude-standard"], {
    cwd: root, encoding: "utf8", env: { ...process.env, GIT_OPTIONAL_LOCKS: "0" }, maxBuffer: 64 * 1024 * 1024,
  });
  const findingsByFile = {};
  for (const file of listed.split("\0").filter(Boolean)) {
    if (!languageOf(file) || file === SELF_TEST || file.startsWith("target/") || file.includes("node_modules/")) continue;
    let text;
    try { text = readFileSync(path.join(root, file), "utf8"); } catch { continue; } // Deleted in the worktree.
    const findings = scan(file, text);
    if (findings.length) findingsByFile[file] = findings;
  }
  return findingsByFile;
}

function main(argv) {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const findingsByFile = scanRepository(root);
  const stored = JSON.parse(readFileSync(path.join(root, BASELINE), "utf8"));
  if (argv.includes("--update")) {
    writeFileSync(path.join(root, BASELINE), formatBaseline(count(findingsByFile), stored.justified));
    console.log(`Wrote ${BASELINE}.`);
    return 0;
  }
  const base = baseBaseline(root);
  const grown = base === null ? [] : baselineGrowth(stored, base);
  if (base === null) console.log(`test timing: no base copy of ${BASELINE} to compare with; growth not checked.`);
  for (const { file, rule, from, to } of grown) {
    console.error(`${BASELINE}: ${file} ${rule} rose from ${from} to ${to} with no new reason under "justified".`);
  }
  const over = overBaseline(findingsByFile, stored.files);
  if (over.length === 0 && grown.length === 0) {
    console.log("test timing: no new fixed sleeps, wall-clock bounds or date literals in tests.");
    return 0;
  }
  for (const { file, rule, count: value, allowed, findings } of over) {
    console.error(`${file}: ${value} ${rule} line(s), ${allowed} allowed: ${RULES[rule].why}.`);
    for (const finding of findings) console.error(`  ${file}:${finding.line}: ${finding.text}`);
  }
  console.error(`Fix the new one, or mark a justified one with a \`timing-ok: <reason>\` comment on its line or the line above. ` +
    `Lower counts need no change; run \`node scripts/check-test-timing.mjs --update\` only to record a reviewed baseline.`);
  return 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = main(process.argv.slice(2));
}
