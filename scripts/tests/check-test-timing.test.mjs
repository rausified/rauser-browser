import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  BASELINE, count, formatBaseline, languageOf, overBaseline, scan, scanRepository, testLines,
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

test("only test code is scanned", () => {
  assert.equal(languageOf("host/src/service/claude.rs"), "rust");
  assert.equal(languageOf("extension/tests/panel/summarize.test.mjs"), "js");
  assert.equal(languageOf("extension/tests/panel/harness.mjs"), "js");
  assert.equal(languageOf("extension/panel.ts"), null);
  assert.equal(languageOf("scripts/smoke-macos.mjs"), null);
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
