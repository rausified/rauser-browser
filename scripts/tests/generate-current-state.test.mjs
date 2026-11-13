import assert from "node:assert/strict";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { BEGIN, END, ROWS_DIR, parseRow, run, withBlockOf, withTable } from "../generate-current-state.mjs";
import { REPO } from "./generated-fixture.mjs";

const ROW = "# Widget\n\nBuilt: one;\ntwo.\n\nPointer: DESIGN.md §1\n";

test("a row file becomes one table row, its state lines joined by spaces", () => {
  assert.deepEqual(parseRow(ROW), { ok: true, row: "| Widget | Built: one; two. | DESIGN.md §1 |" });
  assert.deepEqual(parseRow(ROW.replace(/\n/g, "\r\n")), { ok: true, row: "| Widget | Built: one; two. | DESIGN.md §1 |" });
});

test("a malformed row file is refused with the reason", () => {
  for (const [text, reason] of [
    ["Widget\n\nBuilt.\n\nPointer: x\n", /first line/],
    ["# Widget\nBuilt.\n\nPointer: x\n", /blank line must follow/],
    ["# Widget\n\nBuilt.\nPointer: x\n", /last line/],
    ["# Widget\n\nBuilt.\n\nPointer: x", /one newline/],
    ["# Widget\n\nBuilt.\n\nPointer: x\n\n", /one newline/],
    ["# Widget\n\n  Built.\n\nPointer: x\n", /unindented/],
    ["# Widget\n\nBuilt | not.\n\nPointer: x\n", /'\|'/],
    ["# Widget\n\n\nPointer: x\n", /one paragraph/],
  ]) {
    const parsed = parseRow(text);
    assert.equal(parsed.ok, false, text);
    assert.match(parsed.reason, reason, text);
  }
});

test("the table replaces only what lies between the markers", () => {
  const doc = `intro\n${BEGIN}\nold\n${END}\noutro\n`;
  assert.equal(withTable(doc, "| new |"), `intro\n${BEGIN}\n| new |\n${END}\noutro\n`);
  assert.equal(withTable(doc.replace(/\n/g, "\r\n"), "| new |"), `intro\n${BEGIN}\n| new |\n${END}\noutro\n`.replace(/\n/g, "\r\n"));
  assert.equal(withTable("intro\n", "| new |"), null);
  assert.equal(withTable(`${END}\n${BEGIN}\n`, "| new |"), null);
  assert.equal(withTable(`${BEGIN}\n${END}\n${BEGIN}\n${END}\n`, "| new |"), null);
  assert.equal(withBlockOf(doc, `x\n${BEGIN}\n| base |\n${END}\n`), `intro\n${BEGIN}\n| base |\n${END}\noutro\n`);
});

test("this repository's AGENTS.md table is what its row files generate", () => {
  assert.deepEqual(run({ root: REPO, check: true }), { code: 0, messages: [] });
});

/** A scratch root holding this repository's AGENTS.md and row files, every
 * line ending `eol`, the way a checkout with that line ending has them. */
function fixture(t, eol) {
  const root = mkdtempSync(join(tmpdir(), "current-state-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const withEol = (text) => text.replace(/\r\n/g, "\n").replace(/\n/g, eol);
  writeFileSync(join(root, "AGENTS.md"), withEol(readFileSync(join(REPO, "AGENTS.md"), "utf8"))); // crlf-ok: withEol drops every \r\n before writing the chosen line ending
  cpSync(join(REPO, ROWS_DIR), join(root, ROWS_DIR), { recursive: true });
  for (const name of readdirSync(join(root, ROWS_DIR))) {
    const file = join(root, ROWS_DIR, name);
    writeFileSync(file, withEol(readFileSync(file, "utf8")));
  }
  return root;
}

const lf = (text) => text.replace(/\r\n/g, "\n");
const agents = (root) => readFileSync(join(root, "AGENTS.md"), "utf8");

for (const [label, eol] of [["LF", "\n"], ["CRLF", "\r\n"]]) {
  test(`--check passes a correct ${label} checkout, and writing a new row keeps every line ${label}`, (t) => {
    const root = fixture(t, eol);
    assert.deepEqual(run({ root, check: true }), { code: 0, messages: [] });
    const before = agents(root);

    writeFileSync(join(root, ROWS_DIR, "999-widget.md"), ROW.replace(/\n/g, eol));
    const stale = run({ root, check: true });
    assert.equal(stale.code, 1);
    assert.match(stale.messages[0], /not what docs\/agents\/current-state\/ generates/);
    assert.equal(agents(root), before, "--check writes nothing");

    assert.equal(run({ root, check: false }).code, 0);
    const after = agents(root);
    assert.equal(after.split(eol).join("").includes("\n"), false, `no line ending other than ${label}`);
    assert.equal(eol === "\n" && after.includes("\r"), false, "no CR in an LF checkout");
    assert.match(lf(after), /^\| Widget \| Built: one; two\. \| DESIGN\.md §1 \|\n<!-- END GENERATED current-state -->$/m);
    assert.equal(lf(after).replace("| Widget | Built: one; two. | DESIGN.md §1 |\n", ""), lf(before), "only the table changed");
    assert.deepEqual(run({ root, check: true }), { code: 0, messages: [] });
  });
}

test("--check compares with line endings normalized, and a correct table is never rewritten", (t) => {
  // A checkout whose table lines alone are CRLF: the text is right, so
  // neither --check nor a write touches it.
  const root = fixture(t, "\n");
  const mixed = agents(root).replace(/^(\|.*)$/gm, "$1\r");
  writeFileSync(join(root, "AGENTS.md"), mixed);
  assert.deepEqual(run({ root, check: true }), { code: 0, messages: [] });
  assert.deepEqual(run({ root, check: false }), { code: 0, messages: [] });
  assert.equal(agents(root), mixed);
});

test("--check reports a bad row", (t) => {
  const root = fixture(t, "\n");
  mkdirSync(join(root, ROWS_DIR, "nested"));
  writeFileSync(join(root, ROWS_DIR, "998-bad.md"), "no heading\n");
  const bad = run({ root, check: true });
  assert.equal(bad.code, 1);
  assert.ok(bad.messages.some((message) => /998-bad\.md: first line/.test(message)), bad.messages.join("\n"));
  assert.ok(bad.messages.some((message) => /nested: not a row file/.test(message)), bad.messages.join("\n"));
});
