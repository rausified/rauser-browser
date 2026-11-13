import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";

import {
  BASELINE, baseBaseline, baselineGrowth, count, formatBaseline, languageOf, overBaseline, scan, scanRepository,
  testLines,
} from "../check-test-timing.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const rules = (file, text) => scan(file, text).map(({ rule, line }) => [rule, line]);

test("the repository adds no timing assumption to tests beyond its baseline", () => {
  const baseline = JSON.parse(readFileSync(path.join(root, BASELINE), "utf8"));
  assert.equal(baseline.version, 1);
  const over = overBaseline(scanRepository(root), baseline.files);
  assert.deepEqual(over.map(({ file, rule, count: value, allowed, findings }) =>
    `${file}: ${value} ${rule} (${allowed} allowed): ${findings.map((f) => f.line).join(", ")}`), []);
});

test("the baseline grows only with a justification this change adds", (t) => {
  const baseline = JSON.parse(readFileSync(path.join(root, BASELINE), "utf8"));
  const base = baseBaseline(root);
  if (base === null) {
    t.diagnostic("no base revision holds the baseline (first introduction, or no origin/main and no merge parent)");
    return;
  }
  assert.deepEqual(baselineGrowth(baseline, base).map(({ file, rule, from, to }) => `${file} ${rule}: ${from} -> ${to}`), []);
});

test("only test code is scanned", () => {
  assert.equal(languageOf("host/src/service/claude.rs"), "rust");
  assert.equal(languageOf("extension/tests/panel/summarize.test.mjs"), "js");
  assert.equal(languageOf("extension/tests/panel/harness.mjs"), "js");
  assert.equal(languageOf("extension/panel.ts"), null);
  // The smoke runner holds the macOS smoke's waits; it is read as text only,
  // never imported, since importing it runs the smoke.
  assert.equal(languageOf("scripts/smoke-macos.mjs"), "js");
  assert.equal(languageOf("scripts/pr-watch.mjs"), null);
  assert.equal(languageOf("scripts/tests/test_work_queue.py"), "py");
  assert.equal(languageOf("scripts/work_queue.py"), null);
  // A Rust source file is test code only inside its inline test module; a
  // #[cfg(test)] on one item above it does not start it.
  const source = [
    "#[cfg(test)]", "const SEAM: u8 = 1;", "fn product() { thread::sleep(WAIT); }",
    "#[cfg(test)]", "mod tests {", "    fn waits() { thread::sleep(WAIT); }", "}",
  ].join("\r\n");
  assert.deepEqual(rules("host/src/x.rs", source), [["fixed-sleep", 6]]);
  assert.deepEqual(testLines("host/src/x.rs", "fn main() { thread::sleep(WAIT); }"), []);
  // A file under tests/, or a tests.rs, is test code throughout.
  assert.deepEqual(rules("host/tests/service.rs", "thread::sleep(POLL);"), [["fixed-sleep", 1]]);
  assert.deepEqual(rules("host/src/index/tests.rs", "thread::sleep(POLL);"), [["fixed-sleep", 1]]);
});

test("fixed sleeps, wall-clock bounds and date literals are found", () => {
  const js = [
    "await new Promise((resolve) => setTimeout(resolve, 20));",
    "await delay(100);",
    "assert.ok(performance.now() - started < 500);",
    "const at = Date.parse(\"2026-11-09T10:00:00Z\");",
    "await until(() => ready(), \"ready\");",
    "setTimeout(() => listener(reply), 0);",
    "await new Promise((resolve) => setTimeout(resolve, DIAG_FLUSH_MS + 200));",
    "await new Promise((resolve) => setTimeout(() => resolve(), 20));",
    "await delay(SETTLE_MS);",
  ].join("\n");
  assert.deepEqual(rules("extension/tests/x.test.mjs", js), [["fixed-sleep", 1], ["fixed-sleep", 2],
    ["wall-clock-bound", 3], ["date-literal", 4], ["fixed-sleep", 7], ["fixed-sleep", 8], ["fixed-sleep", 9]]);
  const rust = [
    "#[cfg(test)]", "mod tests {",
    "    assert!(started.elapsed() < Duration::from_secs(4));",
    "    assert!(Duration::from_secs(4) > started.elapsed());",
    "    let deadline = Instant::now() + HANG_GUARD;",
    "    let created = \"2026-11-09T10:00:00Z\";",
    "}",
  ].join("\n");
  assert.deepEqual(rules("host/src/y.rs", rust),
    [["wall-clock-bound", 3], ["wall-clock-bound", 4], ["date-literal", 6]]);
  assert.deepEqual(rules("scripts/tests/test_x.py", "time.sleep(0.5)\nassert elapsed < 2\n"),
    [["fixed-sleep", 1], ["wall-clock-bound", 2]]);
});

test("a timing-ok comment with a reason exempts its line or the next one", () => {
  const js = [
    "await delay(5); // timing-ok: the clock must tick once between the two writes",
    "// timing-ok: a fixture date that never meets the clock",
    "const at = \"2026-11-09\";",
    "await delay(5); // timing-ok:",
    "const later = \"2026-11-10\"; const unrelated = 1;",
  ].join("\n");
  assert.deepEqual(rules("extension/tests/x.test.mjs", js), [["fixed-sleep", 4], ["date-literal", 5]]);
});

test("a file may hold up to its baseline, and a new file none", () => {
  const findings = {
    "a.test.mjs": scan("extension/tests/a.test.mjs", "await delay(1);\nawait delay(2);"),
    "b.test.mjs": scan("extension/tests/b.test.mjs", "await delay(1);"),
  };
  const over = overBaseline(findings, { "a.test.mjs": { "fixed-sleep": 2 }, "c.test.mjs": { "fixed-sleep": 9 } });
  assert.deepEqual(over.map(({ file, count: value, allowed }) => [file, value, allowed]), [["b.test.mjs", 1, 0]]);
  // Fewer than the baseline passes: a fix needs no baseline edit.
  assert.deepEqual(overBaseline(findings, { "a.test.mjs": { "fixed-sleep": 5 }, "b.test.mjs": { "fixed-sleep": 1 } }), []);
});

test("the baseline is written one sorted file per line", () => {
  const text = formatBaseline(count({
    "z.rs": [{ rule: "fixed-sleep" }],
    "a.rs": [{ rule: "fixed-sleep" }, { rule: "date-literal" }, { rule: "fixed-sleep" }],
  }));
  assert.equal(text, [
    "{", "  \"version\": 1,", "  \"files\": {",
    "    \"a.rs\": { \"date-literal\": 1, \"fixed-sleep\": 2 },",
    "    \"z.rs\": { \"fixed-sleep\": 1 }",
    "  }", "}", "",
  ].join("\n"));
  assert.deepEqual(JSON.parse(text).files["a.rs"], { "date-literal": 1, "fixed-sleep": 2 });
});

test("the smoke runner's own waits are scanned from its text", () => {
  const text = readFileSync(path.join(root, "scripts/smoke-macos.mjs"), "utf8");
  const sleeps = scan("scripts/smoke-macos.mjs", text).filter(({ rule }) => rule === "fixed-sleep");
  assert.ok(sleeps.some((finding) => finding.text === "await delay(100);"), "the click retry's pause is seen");
});

test("Rust time comparisons in assertions are found under any name", () => {
  const rust = [
    "#[cfg(test)]", "mod tests {",
    "    let started = Instant::now();",
    "    let dt = Instant::now().duration_since(start);",
    "    assert!(dt < Duration::from_secs(1));",
    "    assert!(Duration::from_secs(1) > dt);",
    "    assert!(",
    "        started.elapsed() >= BACKOFF,",
    "        \"waited {dt:?}\"",
    "    );",
    "    let deadline = Instant::now() + HANG_GUARD;",
    "    while Instant::now() < deadline {",
    "    assert!(items.len() < 5);",
    "    let waits: Vec<Duration> = Vec::new();",
    "    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(()));",
    "    assert!(took <= limit, \"{took:?}\");",
    "    let took = run.elapsed();",
    "}",
  ].join("\n");
  assert.deepEqual(rules("host/src/z.rs", rust).filter(([rule]) => rule === "wall-clock-bound"),
    [["wall-clock-bound", 5], ["wall-clock-bound", 6], ["wall-clock-bound", 8], ["wall-clock-bound", 16]]);
});

test("a baseline may not gain an entry or a count unless this change justifies it", () => {
  const base = { files: { "a.rs": { "fixed-sleep": 2 } }, justified: { "c.rs": { "fixed-sleep": "old reason" } } };
  const grown = {
    files: { "a.rs": { "fixed-sleep": 3, "date-literal": 1 }, "b.rs": { "fixed-sleep": 1 }, "c.rs": { "fixed-sleep": 4 } },
    justified: { "b.rs": { "fixed-sleep": "a hang guard for a child that never exits" }, "c.rs": { "fixed-sleep": "old reason" } },
  };
  assert.deepEqual(baselineGrowth(grown, base).map(({ file, rule, from, to }) => [file, rule, from, to]), [
    ["a.rs", "date-literal", 0, 1],
    ["a.rs", "fixed-sleep", 2, 3],
    // A justification main already holds does not cover a new rise.
    ["c.rs", "fixed-sleep", 0, 4],
  ]);
  // Shrinking, or removing an entry, never needs a justification.
  assert.deepEqual(baselineGrowth({ files: { "a.rs": { "fixed-sleep": 1 } } }, base), []);
  // An empty reason is no justification.
  assert.equal(baselineGrowth({ files: { "b.rs": { "fixed-sleep": 1 } }, justified: { "b.rs": { "fixed-sleep": " " } } },
    { files: {} }).length, 1);
});

test("the base baseline comes from origin/main, else a merge commit's first parent, else none", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "timing-base-"));
  const git = (...args) => execFileSync("git", ["-C", dir, ...args], {
    encoding: "utf8", env: { ...process.env, GIT_OPTIONAL_LOCKS: "0", GIT_CONFIG_NOSYSTEM: "1", HOME: dir },
  }).trim();
  const write = (files) => writeFileSync(path.join(dir, "baseline.json"), JSON.stringify({ version: 1, files }));
  try {
    git("init", "-q", "-b", "main");
    git("config", "user.email", "t@example.invalid");
    git("config", "user.name", "t");
    git("config", "commit.gpgsign", "false");
    write({ "a.rs": { "fixed-sleep": 1 } });
    git("add", "baseline.json");
    git("commit", "-q", "-m", "base");
    // No origin/main and no merge parent: nothing to compare with.
    assert.equal(baseBaseline(dir, "baseline.json"), null);
    git("checkout", "-q", "-b", "pr");
    write({ "a.rs": { "fixed-sleep": 2 } });
    git("commit", "-q", "-am", "pr");
    git("checkout", "-q", "main");
    git("merge", "-q", "--no-ff", "-m", "merge", "pr");
    // CI's pull-request checkout is a merge commit whose first parent is main.
    assert.deepEqual(baseBaseline(dir, "baseline.json").files, { "a.rs": { "fixed-sleep": 1 } });
    git("update-ref", "refs/remotes/origin/main", git("rev-parse", "HEAD~1"));
    git("checkout", "-q", "pr");
    assert.deepEqual(baseBaseline(dir, "baseline.json").files, { "a.rs": { "fixed-sleep": 1 } });
    // A base without the file (its first introduction) compares with nothing.
    assert.equal(baseBaseline(dir, "missing.json"), null);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
