import assert from "node:assert/strict";
import test from "node:test";

import { scanRepo, scanText, testKind } from "../check-crlf-reads.mjs";

const lines = (findings) => findings.map((finding) => finding.line);

// The shape of host/src/fsutil.rs before d6ad99d (#377): a walk rooted at
// CARGO_MANIFEST_DIR, then a read whose own line never names the repository,
// searched for "\n"-spelled markers kept in a const.
const RENAME_GUARD = [
  "pub fn rename_or_copy() {}",
  "#[cfg(test)]",
  "mod tests {",
  "    #[test]",
  "    fn every_rename_goes_through_the_helper() {",
  "        let src_dir = Path::new(env!(\"CARGO_MANIFEST_DIR\")).join(\"src\");",
  "        visit(&src_dir);",
  "    }",
  "    fn visit(dir: &Path) {",
  "        for entry in fs::read_dir(dir).unwrap() {",
  "            check(&entry.unwrap().path());",
  "        }",
  "    }",
  "    const MARKERS: &[&str] = &[\"#[cfg(test)]\\nmod tests\"];",
  "    fn check(path: &Path) {",
  "        let text = fs::read_to_string(path).unwrap();",
  "        let cut = MARKERS.iter().filter_map(|m| text.find(m)).min();",
  "    }",
  "}",
].join("\n");

test("the pre-#377 rename guard is flagged (a source-tree walk, not a named path)", () => {
  assert.deepEqual(lines(scanText(RENAME_GUARD, "host/src/fsutil.rs")), [16]);
});

test("the #377 fix (normalize in the same statement) passes", () => {
  const fixed = RENAME_GUARD.replace("read_to_string(path).unwrap();", "read_to_string(path).unwrap().replace(\"\\r\\n\", \"\\n\");");
  assert.deepEqual(scanText(fixed, "host/src/fsutil.rs"), []);
});

test("a CRLF checkout of the file scans the same lines", () => {
  assert.deepEqual(lines(scanText(RENAME_GUARD.replaceAll("\n", "\r\n"), "host/src/fsutil.rs")), [16]);
});

test("production code before #[cfg(test)] is not test code", () => {
  const text = "const P: &str = include_str!(\"../../SUMMARY.md\");\n#[cfg(test)]\nmod tests {}\n";
  assert.deepEqual(scanText(text, "host/src/service/prompt.rs"), []);
  assert.deepEqual(scanText("const P: &str = include_str!(\"../x.rs\");\n", "host/src/lib.rs"), []);
});

test("#[cfg(all(test, unix))] also starts test code", () => {
  const text = "#[cfg(all(test, unix))]\nmod tests {\n    const S: &str = include_str!(\"../brand.ts\");\n}\n";
  assert.deepEqual(lines(scanText(text, "host/src/service/paths.rs")), [3]);
});

test("Rust: include_str! of source text is flagged; .lines() and a marker pass", () => {
  const file = "adapter-templates/tests/vectors.rs";
  assert.deepEqual(lines(scanText("const D: &str = include_str!(\"disputed.txt\");\n", file)), [1]);
  assert.deepEqual(scanText("let d: Vec<_> = include_str!(\"disputed.txt\").lines().collect();\n", file), []);
  assert.deepEqual(scanText("// crlf-ok: read with str::lines, which strips \\r\nconst D: &str = include_str!(\"disputed.txt\");\n", file), []);
  assert.deepEqual(scanText("const D: &str = include_str!(\"disputed.txt\"); // crlf-ok: single-line contains\n", file), []);
});

test("an empty crlf-ok marker does not count", () => {
  assert.deepEqual(lines(scanText("// crlf-ok:\nconst D: &str = include_str!(\"disputed.txt\");\n", "host/tests/x.rs")), [2]);
});

test("one marker covers a run of reads on consecutive lines", () => {
  const text = [
    "#[test]",
    "fn scan() {",
    "    // crlf-ok: substring checks for single-line identifiers",
    "    let modules = [",
    "        (\"a.rs\", include_str!(\"../a.rs\")),",
    "        (\"b.rs\", include_str!(\"../b.rs\")),",
    "    ];",
    "    let c = include_str!(\"../c.rs\");",
    "}",
  ].join("\n");
  // The marker sits above the array, not above the first read: only an
  // adjacent line or the line above a consecutive run counts.
  assert.deepEqual(lines(scanText(text, "host/src/index/tests/database.rs")), [5, 6, 8]);
  const above = text.replace("    let modules = [\n", "    let modules = [\n    // crlf-ok: substring checks for single-line identifiers\n");
  assert.deepEqual(lines(scanText(above, "host/src/index/tests/database.rs")), [9]);
});

test("structured data, fixtures and vault files are exempt", () => {
  const file = "host/src/capture.rs";
  const text = [
    "#[cfg(test)]",
    "mod tests {",
    "    const V: &str = include_str!(\"../../protocol/strip-params/vectors.json\");",
    "    let s: Json = serde_json::from_str(include_str!(\"../../protocol/schema.json\")).unwrap();",
    "    let f = include_str!(\"../tests/fixtures/notes/page.md\");",
    "    let t: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();",
    "    let log = fs::read_to_string(root.path().join(relative_path)).unwrap();",
    "}",
  ].join("\n");
  assert.deepEqual(scanText(text, file), []);
});

test("JavaScript: a text read of the checkout is flagged; normalized, parsed and byte reads pass", () => {
  const file = "extension/tests/options.test.mjs";
  assert.deepEqual(lines(scanText('const html = readFileSync(new URL("../options.html", import.meta.url), "utf8");\n', file)), [1]);
  assert.deepEqual(lines(scanText('const css = await readFile(path.join(REPO, "panel.css"), { encoding: "utf8" });\n', file)), [1]);
  assert.deepEqual(scanText('const html = readFileSync(new URL("../options.html", import.meta.url), "utf8").replace(/\\r\\n/g, "\\n");\n', file), []);
  assert.deepEqual(scanText('const v = JSON.parse(readFileSync(new URL("../v.json", import.meta.url), "utf8"));\n', file), []);
  assert.deepEqual(scanText('const body = await readFile(new URL("../../dist/x", import.meta.url));\n', file), []);
  assert.deepEqual(scanText('const out = readFileSync(path.join(tmp, "out.md"), "utf8");\n', file), []);
});

test("only Rust and JavaScript test files are checked", () => {
  assert.equal(testKind("host/src/fsutil.rs"), "rs");
  assert.equal(testKind("extension/tests/panel/harness.mjs"), "mjs");
  assert.equal(testKind("scripts/tests/ci-changes.test.mjs"), "mjs");
  assert.equal(testKind("scripts\\tests\\ci-changes.test.mjs"), "mjs");
  assert.equal(testKind("scripts/ci-changes.mjs"), null);
  assert.equal(testKind("scripts/tests/test_work_queue.py"), null);
});

test("every test read of checkout text in this repository is CRLF-safe", () => {
  const findings = scanRepo();
  assert.deepEqual(findings.map(({ file, line, text }) => `${file}:${line}: ${text}`), []);
});

test("a statement ends at its semicolon, so the next line's .json does not exempt it", () => {
  const text = 'const D: &str = include_str!("disputed.txt");\nconst C: &str = include_str!("../catalog.json");\n';
  assert.deepEqual(lines(scanText(text, "adapter-templates/tests/vectors.rs")), [1]);
});
