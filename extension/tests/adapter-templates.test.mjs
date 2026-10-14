import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

// Run after build:extension. adapter-templates.js is pure: no chrome or DOM
// access. Runs every shared vector whose impls include "ts"
// (protocol/adapter-templates/README.md). Vector ids listed in
// adapter-templates-disputed.json are skipped and reported as disputed.
const {
  canonicalCapture, compileDocument, evaluate, indexOfChars, lastIndexOfChars, scanRefs, urlSegments, validateDocument,
} = await import("../dist/adapter-templates.js");

const readJson = (path) => JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));
const vectors = readJson("../../protocol/adapter-templates/vectors.json");
const disputed = new Set(readJson("./adapter-templates-disputed.json"));
const engineSource = readFileSync(new URL("../adapter-templates.ts", import.meta.url), "utf8");

function resolveDoc(doc) {
  if (doc && typeof doc === "object" && "$fixture" in doc) {
    const fixture = vectors.fixtures[doc.$fixture];
    assert.ok(fixture, `unknown fixture ${doc.$fixture}`);
    return fixture;
  }
  return doc;
}

/** A string builder: plain strings, {repeat, times}, and raw {utf16} units. */
function resolveString(value) {
  if (!value || typeof value !== "object" || !("$build" in value)) return value;
  return value.$build.map((part) => {
    if (typeof part === "string") return part;
    if ("repeat" in part) return part.repeat.repeat(part.times);
    return String.fromCharCode(...part.utf16);
  }).join("");
}

function resolveExpect(expect) {
  return "title" in expect ? { ...expect, title: resolveString(expect.title) } : expect;
}

function isCanonicalValue(value) {
  for (const char of value) {
    const code = char.codePointAt(0);
    if (code > 0x7e || code <= 0x20 || "/:{}%".includes(char)) return false;
  }
  return true;
}

function checkInvariants(result) {
  if (!result.match) return;
  for (const value of Object.values(result.captures)) assert.ok(isCanonicalValue(value), `canonical ${value}`);
  assert.ok(Buffer.byteLength(result.artifact_id, "utf8") <= 512);
  const colon = result.artifact_id.indexOf(":");
  assert.ok(colon > 0);
  const [host, ...parts] = result.artifact_id.slice(colon + 1).split("/");
  assert.ok(host.length > 0);
  for (const part of parts) assert.ok(part.length > 0 && isCanonicalValue(part), `id part ${part}`);
}

function run(testCase) {
  switch (testCase.kind) {
    case "validate":
      return validateDocument(resolveDoc(testCase.doc));
    case "capture":
      return { canonical: canonicalCapture(testCase.ctype, testCase.value) };
    case "segments":
      return { segments: urlSegments(resolveString(testCase.path)) };
    case "refs":
      return { values: scanRefs(testCase.find, resolveString(testCase.text)) };
    case "evaluate": {
      const compiled = compileDocument(resolveDoc(testCase.doc));
      assert.ok(compiled.ok, `fixture invalid: ${JSON.stringify(compiled.error)}`);
      const title = "title" in testCase ? resolveString(testCase.title) : undefined;
      const result = evaluate(compiled.doc, resolveString(testCase.url), title);
      checkInvariants(result);
      return result;
    }
    default:
      throw new Error(`unexpected kind ${testCase.kind}`);
  }
}

const tsCases = vectors.cases.filter((testCase) => testCase.impls.includes("ts"));

test("adapter_template_vectors_have_unique_ids_and_known_disputes", () => {
  const ids = new Set(vectors.cases.map((testCase) => testCase.id));
  assert.equal(ids.size, vectors.cases.length);
  for (const id of disputed) assert.ok(ids.has(id), `disputed id ${id} is not a vector`);
});

for (const testCase of tsCases) {
  if (testCase.pending || disputed.has(testCase.id)) {
    test(testCase.id, { skip: testCase.pending ? `pending ${testCase.pending}` : "disputed" }, () => {});
    continue;
  }
  test(testCase.id, () => {
    const started = performance.now();
    const result = run(testCase);
    const elapsed = performance.now() - started;
    assert.deepEqual(result, resolveExpect(testCase.expect), testCase.why);
    if (testCase.budget_ms !== undefined) {
      assert.ok(elapsed <= testCase.budget_ms, `${elapsed.toFixed(1)} ms over ${testCase.budget_ms} ms`);
    }
  });
}

test("adapter_templates_title_searches_are_linear", () => {
  const hay = Array.from({ length: 100_000 }, () => "a");
  const needle = [...Array.from({ length: 50_000 }, () => "a"), "b"];
  const started = performance.now();
  assert.equal(indexOfChars(hay, needle, 0), -1);
  assert.equal(lastIndexOfChars(hay, needle.toReversed(), hay.length), -1);
  // A naive scan does about 2.5 * 10^9 comparisons here.
  assert.ok(performance.now() - started < 500);
  const chars = [..."a - b - c"];
  assert.equal(indexOfChars(chars, [..." - "], 2), 5);
  assert.equal(lastIndexOfChars(chars, [..." - "], 8), 5);
  assert.equal(lastIndexOfChars(chars, [..." - "], 7), 1);
  assert.equal(lastIndexOfChars(chars, [..." - "], 2), -1);
  // A mismatch after a partial match restarts at the needle's border, not at
  // its start, so a self-overlapping needle is still found.
  assert.equal(indexOfChars([..."aaab"], [..."aab"], 0), 1);
  assert.equal(lastIndexOfChars([..."baaa"], [..."baa"], 4), 0);
  assert.equal(indexOfChars([..."abababc"], [..."ababc"], 0), 2);
});

test("adapter_templates_huge_id_template_costs_no_more_than_the_id_cap", () => {
  // About 1 MB of literal parts: the template validates, but every ID it
  // builds is over 512 bytes. Building the whole string took about 22 ms per
  // visit; stopping at the cap takes microseconds.
  const compiled = compileDocument({
    format: 1,
    artifacts: [{
      type: "acme.item", origin: "https://acme.example.com", match: [{ path: "/items/{id:int}" }],
      id: `acme:{host}/{id}${"/a".repeat(500_000)}`, title: "{title} {id:int}",
      alias: { from: "tab_title", capture: "id" },
    }],
  });
  assert.ok(compiled.ok);
  const started = performance.now();
  for (let i = 0; i < 50; i += 1) {
    assert.deepEqual(evaluate(compiled.doc, "https://acme.example.com/items/7", "x 8"),
      { match: false, reason: "id_too_long" });
  }
  assert.ok(performance.now() - started < 200);
});

/** Every regular-expression use in a source file: regex literals, any
 * `RegExp` identifier, and the string methods that build a RegExp from a
 * string argument. Walks the TypeScript AST, so comments, strings, and
 * division never count and no line position hides a literal. */
function regexUses(source, fileName) {
  const file = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true);
  const found = [];
  const visit = (node) => {
    const at = () => `${fileName}:${file.getLineAndCharacterOfPosition(node.getStart(file)).line + 1}`;
    if (node.kind === ts.SyntaxKind.RegularExpressionLiteral) found.push(`${at()} regex literal ${node.getText(file)}`);
    else if (ts.isIdentifier(node) && node.text === "RegExp") found.push(`${at()} RegExp`);
    else if (ts.isCallExpression(node)) {
      // Called, not read: `url.search` is a string, `s.search(x)` a regex.
      const callee = node.expression;
      const name = ts.isPropertyAccessExpression(callee) ? callee.name.text
        : ts.isElementAccessExpression(callee) && ts.isStringLiteralLike(callee.argumentExpression)
          ? callee.argumentExpression.text : null;
      if (name !== null && REGEX_METHODS.has(name)) found.push(`${at()} ${name}()`);
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  return found;
}

const REGEX_METHODS = new Set(["match", "matchAll", "search", "exec"]);

test("adapter_templates_regex_guard_flags_every_regex_form", () => {
  for (const line of [
    "const m = ok ? /^[a-z]+$/.exec(s) : null;",
    "if (a && /x/.exec(s)) {}",
    "text.split(sep || /\\//);",
    "function f(s) { return/x/.exec(s); }",
    "const o = { re: /a+/ };",
    "const r = new RegExp(\"a\");",
    "const m = s.match(\"a+\");",
    "const m = s[\"search\"](\"a\");",
  ]) {
    assert.notDeepEqual(regexUses(line, "sample.ts"), [], line);
  }
  for (const line of [
    "const x = a / b / c;",
    "// see /regex/ in a comment",
    "const s = \"/not/a/regex/\";",
    "const n = (a) / 2;",
    "const q = url.search;",
  ]) {
    assert.deepEqual(regexUses(line, "sample.ts"), [], line);
  }
});

test("adapter_templates_engine_uses_no_regular_expressions", () => {
  assert.deepEqual(regexUses(engineSource, "adapter-templates.ts"), []);
  const evalSource = readFileSync(new URL("../../scripts/adapter-templates-eval.mjs", import.meta.url), "utf8");
  assert.deepEqual(regexUses(evalSource, "adapter-templates-eval.mjs"), []);
});
