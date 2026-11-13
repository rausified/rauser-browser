import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";

import { normalize, reflow, sentences } from "../sentence-lines.mjs";
import { REPO } from "./generated-fixture.mjs";

test("AGENTS.md is one sentence per line", () => {
  const text = readFileSync(join(REPO, "AGENTS.md"), "utf8").replace(/\r\n/g, "\n");
  assert.equal(reflow(text), text, "run node scripts/sentence-lines.mjs --write AGENTS.md");
});

test("reflowing changes only whitespace, on every root Markdown file", () => {
  for (const file of ["AGENTS.md", "DESIGN.md", "TESTING.md", "README.md", "docs/history.md"]) {
    const text = readFileSync(join(REPO, file), "utf8"); // crlf-ok: compared only after normalize(), which drops all whitespace
    assert.equal(normalize(reflow(text)), normalize(text), file);
  }
});

test("sentences split after . ! or ? before a capital, a digit or code", () => {
  assert.deepEqual(sentences("One. Two! 3 things? `four` five."), ["One.", "Two!", "3 things?", "`four` five."]);
  assert.deepEqual(sentences("A **bold.** Next (in parens.) Then"), ["A **bold.**", "Next (in parens.)", "Then"]);
});

test("no split inside code, after an abbreviation, before lowercase or a new block", () => {
  assert.deepEqual(sentences("Run `a. B` now."), ["Run `a. B` now."]);
  assert.deepEqual(sentences("Use one, e.g. Codex, here."), ["Use one, e.g. Codex, here."]);
  assert.deepEqual(sentences("Version 1.2. then more"), ["Version 1.2. then more"]);
  assert.deepEqual(sentences("Ends here. - not a list"), ["Ends here. - not a list"]);
  assert.deepEqual(sentences("Ends here. 2. not a list"), ["Ends here. 2. not a list"]);
  assert.deepEqual(sentences("Ends here. # not a heading"), ["Ends here. # not a heading"]);
});

test("list items continue at their content column; other blocks are left alone", () => {
  const text = [
    "# Title. Two",
    "",
    "- Item one. Item two.",
    "  - Nested one. Nested two.",
    "1. Numbered. Next.",
    "",
    "| a. B | c. D |",
    "<!-- note. Two -->",
    "```",
    "code. Here",
    "```",
    "Para one",
    "wrapped. Para two.",
  ].join("\n");
  assert.equal(reflow(text), [
    "# Title. Two",
    "",
    "- Item one.",
    "  Item two.",
    "  - Nested one.",
    "    Nested two.",
    "1. Numbered.",
    "   Next.",
    "",
    "| a. B | c. D |",
    "<!-- note. Two -->",
    "```",
    "code. Here",
    "```",
    "Para one wrapped.",
    "Para two.",
  ].join("\n"));
  assert.equal(reflow(text.replace(/\n/g, "\r\n")), reflow(text).replace(/\n/g, "\r\n"));
});
