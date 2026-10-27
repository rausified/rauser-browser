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
// Neutralized body text must render with no <code> or <pre> at all, and its
// raw form must hold no backtick, `$`, tab, run of three `~` or run of four
// spaces, since Obsidian hands code spans, fences, indented code and math to
// Mermaid, Dataview and math engines. Each renderer must also decode the
// numeric entities the neutralizer writes to their plain characters. Each
// input is also rendered as a single-line field (`markdown::inline`) and as a
// wikilink (`markdown::wikilink`, as a viewer without wikilinks sees it):
// those must render with no <code> or <pre> either, and hold no raw backtick
// or `$`. Each input is also rendered as every agent field of a summary file
// (`summary::render_summary`, both link styles): its body must render with
// no <code> or <pre> and hold no raw backtick or `$`, and its front matter
// is checked as front matter.
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

// Inert text elements and the attributes the renderers give them, for front
// matter rendered as body text (`yaml_string` output keeps backticks, so code
// spans and fences can form there).
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
  // markdown-it writes a GFM column alignment as a style; only that exact
  // style is allowed (see ALIGNMENT_STYLE).
  ["th", ["align", "style"]],
  ["td", ["align", "style"]],
  ["hr", []],
  ["br", []],
  ["del", []],
  // markdown-it writes GFM strikethrough as <s>.
  ["s", []],
  ["input", ["type", "checked", "disabled"]],
]);

// Neutralized body text: no code span, fenced block or indented code block
// can form, because Obsidian hands their contents to Mermaid, Dataview, math
// and plugin engines (DESIGN.md §5.2 step 6). So <code> and <pre> are not
// allowed at all.
const ALLOWED_BODY = new Map([...ALLOWED].filter(([tag]) => tag !== "code" && tag !== "pre"));

// The only `style` value allowed, and only on <th> and <td>.
const ALIGNMENT_STYLE = /^text-align:(left|right|center)$/;

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
export function problems(html, allowedElements = ALLOWED) {
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
    const allowed = allowedElements.get(name);
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
    if (attributes.has("style") && !ALIGNMENT_STYLE.test(attributes.get("style"))) {
      found.push(`style ${JSON.stringify(attributes.get("style"))} on <${name}>`);
    }
    if (name === "input" && (attributes.get("type") !== "checkbox" || !attributes.has("disabled"))) {
      found.push("an <input> that is not a disabled checkbox");
    }
  }
  return found;
}

/**
 * Every problem in raw neutralized body text: characters that could open a
 * code span, fence, indented code block or math, as short descriptions.
 */
export function rawProblems(text) {
  const found = [];
  if (text.includes("`")) {
    found.push("a raw backtick");
  }
  if (text.includes("$")) {
    found.push("a raw '$'");
  }
  if (/~{3}/.test(text)) {
    found.push("a run of three or more raw '~'");
  }
  if (text.includes("\t")) {
    found.push("a raw tab");
  }
  if (/ {4}/.test(text)) {
    found.push("a run of four or more raw spaces");
  }
  return found;
}

/**
 * Every problem in a raw single-line field or wikilink: characters that could
 * open a code span or math. A wikilink starts with `[[`, so it cannot open a
 * fence or indented code block; a single-line field (`atLineStart`) may be
 * placed at a line start, so it must hold no `~` and no run of four spaces.
 */
export function lineRawProblems(text, atLineStart = false) {
  const found = [];
  if (atLineStart && text.includes("~")) {
    found.push("a raw '~'");
  }
  if (atLineStart && / {4}/.test(text)) {
    found.push("a run of four or more raw spaces");
  }
  if (text.includes("`")) {
    found.push("a raw backtick");
  }
  if (text.includes("$")) {
    found.push("a raw '$'");
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
// check cannot pass vacuously. The body list adds inputs that are only live
// in an Obsidian engine: code spans, fences and indented code.
const LIVE = [
  "a <img src=x> b",
  "![x](https://e.example/a.png)",
  "<p style=x>y</p>",
  "<table><tr><td style=\"background:url(https://e.example/a.png)\">x</td></tr></table>",
  "<table><tr><td style=\"text-align:left;background:url(https://e.example/a.png)\">x</td></tr></table>",
];

const LIVE_BODY = [
  ...LIVE,
  "`= this.file.name`",
  "```mermaid\nA@{ img: \"https://e.example/m.png\" }\n```",
  "~~~dataviewjs\ndv.el(\"img\")\n~~~",
  "    = this.file.name",
];

// The neutralizer's numeric entities, and the text each renderer must show;
// a doubled backslash before an entity shows one backslash.
const ENTITIES = "&#96;x&#96; &#36;y&#36; &#126;&#126;&#126; a&#9;b   &#32;c d\\\\&#9;e";
const DECODED = "`x` $y$ ~~~ a\tb    c d\\\te";

function selfCheck() {
  for (const [renderer, render] of RENDERERS) {
    const html = render(ENTITIES);
    if (html.trim() !== `<p>${DECODED}</p>`) {
      throw new Error(`self-check: ${renderer} rendered ${JSON.stringify(ENTITIES)} as ${JSON.stringify(html)}`);
    }
  }
  for (const [list, allowed] of [
    [LIVE, ALLOWED],
    [LIVE_BODY, ALLOWED_BODY],
  ]) {
    for (const text of list) {
      for (const [renderer, render] of RENDERERS) {
        if (problems(render(text), allowed).length === 0) {
          throw new Error(`self-check: ${renderer} rendering of ${JSON.stringify(text)} was not flagged`);
        }
      }
    }
  }
  for (const text of LIVE_BODY.slice(LIVE.length)) {
    if (rawProblems(text).length === 0) {
      throw new Error(`self-check: raw ${JSON.stringify(text)} was not flagged`);
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
    // matter support would; `inline` and `wikilink` (when present) are the
    // input as a single-line field and as a wikilink; each `joinedN` is the
    // output followed by another neutralized fragment; `summary` and
    // `summaryWikilink` are the body of a summary file (render_summary) with
    // every agent field set to the input, in each link style, and
    // `summaryQuery*` the same with the input also in the page URL's query;
    // `frontmatter_summary*` that file's front matter.
    for (const [kind, document] of Object.entries(documents)) {
      // Everything but front matter must render with no code or math.
      const body = !kind.startsWith("frontmatter");
      // A summary body joins single-line fields and wikilinks, placed after
      // host text, with neutralized text at line starts: its raw form must
      // hold no backtick or `$`, and its rendering is checked like the rest.
      const line = kind === "inline" || kind === "wikilink" || kind.startsWith("summary");
      if (body) {
        const found = line ? lineRawProblems(document, kind === "inline") : rawProblems(document);
        if (found.length > 0) {
          failures += 1;
          console.error(
            `FAIL raw (${kind})\n  input:    ${JSON.stringify(input)}\n  rendered: ${JSON.stringify(document)}\n  ${found.join("\n  ")}`,
          );
        }
      }
      for (const [renderer, render] of RENDERERS) {
        checked += 1;
        const html = render(document);
        const found = problems(html, body ? ALLOWED_BODY : ALLOWED);
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
