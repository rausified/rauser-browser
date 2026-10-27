#!/usr/bin/env node

// Real-renderer regression check for the host's remote-resource neutralizer
// (host/src/markdown.rs `neutralize_remote`, DESIGN.md §5.2 step 6).
//
// The host's corpus test (host/src/markdown_corpus.rs) neutralizes every
// input in host/tests/fixtures/markdown-neutralize-corpus.json and, with
// BRAUSER_NEUTRALIZE_CORPUS_OUT set, writes the {input, output} pairs to that
// file. This script runs that test, renders each output with markdown-it
// (html, linkify), micromark with GFM (dangerous HTML allowed) and marked
// (GFM), and fails if any rendered HTML holds an element or attribute outside
// a small allowlist of inert text markup. Every `<` a renderer emits is
// markup (text `<` is always written as `&lt;`), so each one is checked.
//
//   npm run test:markdown-neutralizer
//   node scripts/check-markdown-neutralizer.mjs --outputs pairs.json
//
// --outputs renders an existing pairs file instead of running cargo, for
// example one written by another version of markdown.rs.

import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import MarkdownIt from "markdown-it";
import { marked } from "marked";
import { micromark } from "micromark";
import { gfm, gfmHtml } from "micromark-extension-gfm";

const REPO = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const OUTPUT_ENV = "BRAUSER_NEUTRALIZE_CORPUS_OUT";

// Inert text elements and the attributes the renderers give them.
const ALLOWED = new Map([
  ["p", []],
  ["a", ["href", "title"]],
  ["em", []],
  ["strong", []],
  ["code", ["class"]],
  ["pre", []],
  ["ul", []],
  ["ol", ["start"]],
  ["li", []],
  ["blockquote", []],
  ...["h1", "h2", "h3", "h4", "h5", "h6"].map((tag) => [tag, []]),
  ["table", []],
  ["thead", []],
  ["tbody", []],
  ["tr", []],
  ["th", ["align"]],
  ["td", ["align"]],
  ["hr", []],
  ["br", []],
  ["del", []],
  // markdown-it writes GFM strikethrough as <s>.
  ["s", []],
  ["input", ["type", "checked", "disabled"]],
]);

const markdownIt = new MarkdownIt({ html: true, linkify: true });
const RENDERERS = [
  ["markdown-it", (text) => markdownIt.render(text)],
  [
    "micromark+gfm",
    (text) =>
      micromark(text, {
        allowDangerousHtml: true,
        extensions: [gfm()],
        htmlExtensions: [gfmHtml()],
      }),
  ],
  ["marked", (text) => marked.parse(text, { gfm: true, async: false })],
];

// A start or end tag at the current position, attributes quoted or not.
const TAG =
  /<(\/?)([A-Za-z][A-Za-z0-9-]*)((?:\s+[^\s"'>/=]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'=<>`]+))?)*)\s*(\/?)>/y;
const ATTRIBUTE = /([^\s"'>/=]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/g;

/** Every problem in rendered `html`, as short descriptions. */
export function problems(html) {
  const found = [];
  for (let at = html.indexOf("<"); at !== -1; at = html.indexOf("<", at + 1)) {
    TAG.lastIndex = at;
    const match = TAG.exec(html);
    if (!match) {
      found.push(`markup that is not a plain tag: ${JSON.stringify(html.slice(at, at + 40))}`);
      continue;
    }
    const [, closing, rawName, rawAttributes] = match;
    const name = rawName.toLowerCase();
    const allowed = ALLOWED.get(name);
    if (!allowed) {
      found.push(`element <${name}>`);
      continue;
    }
    if (closing) {
      continue;
    }
    const attributes = new Map();
    for (const [, key, double, single, bare] of rawAttributes.matchAll(ATTRIBUTE)) {
      attributes.set(key.toLowerCase(), double ?? single ?? bare ?? "");
    }
    for (const key of attributes.keys()) {
      if (!allowed.includes(key)) {
        found.push(`attribute ${key} on <${name}>`);
      }
    }
    if (name === "input" && (attributes.get("type") !== "checkbox" || !attributes.has("disabled"))) {
      found.push("an <input> that is not a disabled checkbox");
    }
  }
  return found;
}

function corpusPairs(outputs) {
  if (outputs) {
    return { pairs: JSON.parse(readFileSync(outputs, "utf8")), cargoFailed: false };
  }
  const scratch = mkdtempSync(path.join(tmpdir(), "brauser-neutralizer-"));
  const file = path.join(scratch, "pairs.json");
  try {
    const result = spawnSync(
      "cargo",
      [
        "test",
        "--locked",
        "-p",
        "brauser",
        "--lib",
        "markdown_corpus::the_corpus_comes_out_inert",
        "--",
        "--exact",
      ],
      { cwd: REPO, env: { ...process.env, [OUTPUT_ENV]: file }, stdio: "inherit" },
    );
    if (result.error) {
      throw result.error;
    }
    // The pairs are written before the test asserts anything, so a failing
    // neutralizer is still rendered.
    return { pairs: JSON.parse(readFileSync(file, "utf8")), cargoFailed: result.status !== 0 };
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

// Unneutralized inputs every renderer must be caught rendering live, so the
// check cannot pass vacuously.
const LIVE = ["a <img src=x> b", "![x](https://e.example/a.png)", "<p style=x>y</p>"];

function selfCheck() {
  for (const text of LIVE) {
    for (const [renderer, render] of RENDERERS) {
      if (problems(render(text)).length === 0) {
        throw new Error(`self-check: ${renderer} rendering of ${JSON.stringify(text)} was not flagged`);
      }
    }
  }
}

function main() {
  selfCheck();
  const { values } = parseArgs({ options: { outputs: { type: "string" } } });
  const { pairs, cargoFailed } = corpusPairs(values.outputs);
  if (!Array.isArray(pairs) || pairs.length === 0) {
    throw new Error("no corpus outputs to render");
  }
  let failures = 0;
  let checked = 0;
  for (const { input, ...documents } of pairs) {
    // `output` is the neutralized body text; `frontmatter` (when present) is
    // the input as a front matter value, rendered as a viewer without front
    // matter support would.
    for (const [kind, document] of Object.entries(documents)) {
      for (const [renderer, render] of RENDERERS) {
        checked += 1;
        const html = render(document);
        const found = problems(html);
        if (found.length > 0) {
          failures += 1;
          console.error(
            `FAIL ${renderer} (${kind})\n  input:    ${JSON.stringify(input)}\n  rendered: ${JSON.stringify(document)}\n  html:     ${JSON.stringify(html)}\n  ${found.join("\n  ")}`,
          );
        }
      }
    }
  }
  if (failures > 0 || cargoFailed) {
    console.error(
      `${failures} of ${checked} renderings hold live markup${cargoFailed ? "; the host corpus test failed" : ""}`,
    );
    process.exit(1);
  }
  console.log(`${pairs.length} corpus inputs: ${checked} renderings by ${RENDERERS.length} renderers, all inert`);
}

main();
