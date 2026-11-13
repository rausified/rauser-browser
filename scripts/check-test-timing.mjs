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
//   node scripts/check-test-timing.mjs            check the repository
//   node scripts/check-test-timing.mjs --update   rewrite the baseline to today's counts
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const BASELINE = "scripts/test-timing-baseline.json";
// This check's own tests hold every pattern as sample text.
const SELF_TEST = "scripts/tests/check-test-timing.test.mjs";

const ISO_DATE = /["'`](?:19|20)\d{2}-\d{2}-\d{2}/;

/** Each rule, per language, as line patterns. */
export const RULES = {
  "fixed-sleep": {
    why: "a fixed sleep assumes how long the other side takes; wait until the condition holds",
    rust: [/\bthread::sleep\s*\(/],
    js: [
      /setTimeout\(\s*\(?\s*\)?\s*(?:=>\s*)?[A-Za-z_$][\w$]*\s*\)?\s*,\s*\d/,
      /\b(?:delay|sleep|wait)\(\s*\d[\d_]*\s*\)/,
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
  lines.forEach(([number, line], index) => {
    const previous = index > 0 ? lines[index - 1][1] : "";
    if (ALLOW.test(line) || (ALLOW.test(previous) && /^\s*(?:\/\/|#)/.test(previous))) return;
    for (const [rule, { [language]: patterns }] of Object.entries(RULES)) {
      if (patterns.some((pattern) => pattern.test(line))) findings.push({ rule, line: number, text: line.trim() });
    }
  });
  return findings;
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
export function formatBaseline(counts) {
  const files = Object.keys(counts).sort();
  const body = files.map((file) => {
    const rules = Object.keys(counts[file]).sort().map((rule) => `${JSON.stringify(rule)}: ${counts[file][rule]}`);
    return `    ${JSON.stringify(file)}: { ${rules.join(", ")} }`;
  });
  return `{\n  "version": 1,\n  "files": {\n${body.join(",\n")}\n  }\n}\n`;
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
  if (argv.includes("--update")) {
    writeFileSync(path.join(root, BASELINE), formatBaseline(count(findingsByFile)));
    console.log(`Wrote ${BASELINE}.`);
    return 0;
  }
  const baseline = JSON.parse(readFileSync(path.join(root, BASELINE), "utf8")).files;
  const over = overBaseline(findingsByFile, baseline);
  if (over.length === 0) {
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
