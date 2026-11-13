#!/usr/bin/env node

// One sentence per line for Markdown instruction files (AGENTS.md).
//
// Two pull requests that each change a different sentence of one long line
// conflict, because git merges whole lines: #396 and #377 did, on AGENTS.md's
// one-line, 4,800-character CI paragraph. With one sentence per line they
// touch different lines and merge cleanly.
//
// `reflow(text)` puts each sentence of every paragraph and list item on its
// own line, continuing a list item at its content column, and changes only
// whitespace: `normalize(reflow(text)) === normalize(text)` always holds, and
// `--compare` proves it for two files. It leaves headings, tables, fenced
// code, HTML comments and blank lines alone, never splits inside a code span,
// after a common abbreviation, or where the next line would start a new
// Markdown block (a list marker, heading, quote or table row).
//
// Usage:
//   node scripts/sentence-lines.mjs --check <file>...    exit 1 naming each file not one sentence per line
//   node scripts/sentence-lines.mjs --write <file>...    reflow in place
//   node scripts/sentence-lines.mjs --compare <a> <b>    exit 1 unless both normalize to the same text

import { readFileSync, realpathSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

/** The text with every whitespace run made one space: what Markdown renders
 * a paragraph's soft line breaks as, and what a reflow must keep. */
export function normalize(text) {
  return text.split(/\s+/).filter(Boolean).join(" ");
}

const ABBREVIATIONS = new Set(["e.g.", "i.e.", "etc.", "vs.", "cf.", "approx.", "no.", "fig.", "al."]);
// A sentence ends at . ! or ?, optionally closed by quotes, brackets or emphasis.
const END = /[.!?]["'”’)\]*_]*$/;
// What may start the next sentence.
const START = /^["'“‘(\[*_`]*[A-Z0-9`]/;
// A line starting with one of these would begin a new block, not continue.
const BLOCK_START = /^(?:[-+*]\s|\d+[.)]\s|#|>|\||=+\s*$|`{3}|~{3})/;

/** The sentences of one paragraph's text (already on one line). */
export function sentences(text) {
  const out = [];
  let start = 0;
  let inCode = false;
  for (let i = 0; i < text.length; i += 1) {
    const char = text[i];
    if (char === "`") {
      let run = 1;
      while (text[i + run] === "`") run += 1;
      inCode = !inCode;
      i += run - 1;
      continue;
    }
    if (inCode || char !== " ") continue;
    const before = text.slice(start, i);
    const after = text.slice(i + 1);
    if (!END.test(before) || !START.test(after) || BLOCK_START.test(after)) continue;
    const lastWord = before.split(" ").pop().replace(/["'”’)\]*_]+$/, "").toLowerCase();
    if (ABBREVIATIONS.has(lastWord)) continue;
    out.push(before);
    start = i + 1;
  }
  out.push(text.slice(start));
  return out.filter((sentence) => sentence.length > 0);
}

const LIST_ITEM = /^(\s*)([-+*]|\d+[.)])(\s+)/;

function isProse(line) {
  return line.trim() !== "" && !/^\s*(#|\||<!--|```|~~~)/.test(line);
}

/** `text` with one sentence per line; see the file comment. */
export function reflow(text) {
  const crlf = text.includes("\r\n");
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const out = [];
  let fence = null;
  let comment = false;
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const fenceMark = /^\s*(```|~~~)/.exec(line);
    if (fence) {
      if (fenceMark && fenceMark[1] === fence) fence = null;
      out.push(line);
      i += 1;
      continue;
    }
    if (fenceMark) {
      fence = fenceMark[1];
      out.push(line);
      i += 1;
      continue;
    }
    if (comment || /^\s*<!--/.test(line)) {
      comment = !line.includes("-->");
      out.push(line);
      i += 1;
      continue;
    }
    if (!isProse(line)) {
      out.push(line);
      i += 1;
      continue;
    }
    // One paragraph or list item: this line plus its continuation lines.
    const item = LIST_ITEM.exec(line);
    const lead = item ? item[0] : /^\s*/.exec(line)[0];
    const indent = " ".repeat(lead.length);
    const parts = [line.slice(lead.length)];
    i += 1;
    while (i < lines.length && isProse(lines[i]) && !LIST_ITEM.test(lines[i]) && !/^\s*(```|~~~)/.test(lines[i])) {
      parts.push(lines[i].trim());
      i += 1;
    }
    const joined = parts.map((part) => part.trim()).join(" ");
    sentences(joined).forEach((sentence, n) => out.push(`${n === 0 ? lead : indent}${sentence}`));
  }
  const result = out.join("\n");
  return crlf ? result.replace(/\n/g, "\r\n") : result;
}

function isEntryPoint() {
  try {
    return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href;
  } catch {
    return false;
  }
}

if (isEntryPoint()) {
  const [mode, ...files] = process.argv.slice(2);
  const read = (file) => readFileSync(file, "utf8");
  if (mode === "--compare" && files.length === 2) {
    const same = normalize(read(files[0])) === normalize(read(files[1]));
    console.log(same ? `same normalized text: ${files[0]} and ${files[1]}` : `normalized text differs: ${files[0]} and ${files[1]}`);
    process.exit(same ? 0 : 1);
  } else if ((mode === "--check" || mode === "--write") && files.length > 0) {
    let code = 0;
    for (const file of files) {
      const text = read(file);
      const flowed = reflow(text);
      if (normalize(flowed) !== normalize(text)) throw new Error(`${file}: reflow would change the text; this is a bug in sentence-lines.mjs`);
      // Line endings normalized: a CRLF checkout of a reflowed file passes.
      if (flowed.replace(/\r\n/g, "\n") === text.replace(/\r\n/g, "\n")) continue;
      if (mode === "--write") {
        writeFileSync(file, flowed);
        console.log(`reflowed ${file}`);
      } else {
        console.error(`${file} is not one sentence per line; run node scripts/sentence-lines.mjs --write ${file}`);
        code = 1;
      }
    }
    process.exit(code);
  } else {
    console.error("usage: sentence-lines.mjs --check|--write <file>... | --compare <a> <b>");
    process.exit(2);
  }
}
