// Registry tests that need the built engine (docs/superpowers/specs/
// 2026-11-03-starter-families-design.md §4): check 4 (every starter rule has
// a positive and a negative evaluate vector, and a sample whose title the
// title template accepts) and check 8 (the generator's own --check is
// clean). Every other registry and shared-origin policy check is pure JSON
// analysis and lives in scripts/generate-starters.mjs's own checkPolicy and
// checkRegistry, tested in scripts/tests/generate-starters.test.mjs.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { coverageProblems, loadFamilies } from "../../scripts/generate-starters.mjs";

const { compileDocument, evaluate } = await import("../dist/adapter-templates.js");

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const SCRIPT = path.join(REPO, "scripts", "generate-starters.mjs");
const VECTORS_JSON = path.join(REPO, "protocol", "adapter-templates", "vectors.json");
const STARTER_VECTORS_JSON = path.join(REPO, "protocol", "adapter-templates", "starter-vectors.json");

const engine = { compileDocument, evaluate };

function loadInputs() {
  return {
    families: loadFamilies(REPO),
    vectors: JSON.parse(readFileSync(VECTORS_JSON, "utf8")),
    starterVectors: JSON.parse(readFileSync(STARTER_VECTORS_JSON, "utf8")),
  };
}

test("every starter rule has positive and negative vectors and a title sample", () => {
  const problems = coverageProblems(loadInputs(), engine);
  assert.deepEqual(problems, []);
});

test("the generator check is clean", () => {
  const result = spawnSync(process.execPath, [SCRIPT, "--check"], { cwd: REPO, encoding: "utf8" });
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.stdout.trim(), "");
});

test("a match rule with no positive evaluate vector is named (check 4)", () => {
  const inputs = loadInputs();
  inputs.vectors = structuredClone(inputs.vectors);
  // Every jira.issue case whose expect is a positive match on rule 1 (the
  // board-modal rule): removing all of them leaves rule 1 with no positive
  // coverage, while every other type and rule keeps its own.
  inputs.vectors.cases = inputs.vectors.cases.filter((vectorCase) => !(
    vectorCase.kind === "evaluate"
    && vectorCase.doc?.["$fixture"] === "starters"
    && vectorCase.expect?.match === true
    && vectorCase.expect?.type === "jira.issue"
    && vectorCase.expect?.rule === 1
  ));
  const problems = coverageProblems(inputs, engine);
  assert.deepEqual(problems, ["jira.issue: match rule 1 has no positive evaluate vector"]);
});

test("a type with no negative evaluate vector is named (check 4)", () => {
  const inputs = loadInputs();
  inputs.vectors = structuredClone(inputs.vectors);
  // Every evaluate case on miro.com (miro.board's only origin) whose expect
  // is a non-match: removing all of them leaves miro.board with only
  // positive cases, so no case proves the matcher correctly rejects a
  // miro.com URL.
  inputs.vectors.cases = inputs.vectors.cases.filter((vectorCase) => !(
    vectorCase.kind === "evaluate"
    && vectorCase.doc?.["$fixture"] === "starters"
    && vectorCase.expect?.match === false
    && typeof vectorCase.url === "string"
    && vectorCase.url.startsWith("https://miro.com")
  ));
  const problems = coverageProblems(inputs, engine);
  assert.deepEqual(problems, ["miro.board: no negative evaluate vector"]);
});

test("a sibling type's positive vector on a shared origin is not counted as this type's negative (check 4, Minor 3)", () => {
  // jira.issue and confluence.page share one origin (acme.atlassian.net).
  // Removing every genuine no-match case on that origin leaves both types
  // with only positives. Before Minor 3's fix, each type's positive cases
  // counted as the *other* type's negative, so this family passed "no
  // negative evaluate vector" for both types with zero real no-match
  // vectors between them; both must now be reported.
  const inputs = loadInputs();
  inputs.vectors = structuredClone(inputs.vectors);
  // new URL("blob:https://acme.atlassian.net/...").origin is also
  // "https://acme.atlassian.net" (two legacy cases, eval.url.blob_*), so
  // this filters by parsed origin, not a string prefix, to remove every
  // genuine negative on this origin, blob URLs included.
  const originOf = (url) => { try { return new URL(url).origin; } catch { return null; } };
  inputs.vectors.cases = inputs.vectors.cases.filter((vectorCase) => !(
    vectorCase.kind === "evaluate"
    && vectorCase.doc?.["$fixture"] === "starters"
    && vectorCase.expect?.match === false
    && originOf(vectorCase.url) === "https://acme.atlassian.net"
  ));
  const problems = coverageProblems(inputs, engine);
  assert.deepEqual(problems.sort(), ["confluence.page: no negative evaluate vector", "jira.issue: no negative evaluate vector"]);
});

test("a type with no sample whose title the title template accepts is named (check 4)", () => {
  const inputs = loadInputs();
  inputs.families = structuredClone(inputs.families);
  const miro = inputs.families.find((family) => family.family === "miro");
  assert.ok(miro, "expected a miro family");
  for (const type of miro.types) {
    for (const sample of type.samples) sample.title = null;
  }
  const problems = coverageProblems(inputs, engine);
  assert.deepEqual(problems, ["miro.board: no sample whose title the title template accepts"]);
});

test("a family whose synthesized fixture fails to compile is named, not silently skipped (check 4, Important 3)", () => {
  // Before the fix, `coverageProblems`'s title check did
  // `if (!compiled.ok) continue;`: a family whose synthesized fixture
  // doesn't compile reported nothing at all, which is exactly the vacuous
  // pass this check exists to prevent (CODING.md "no silent fallbacks").
  const inputs = loadInputs();
  inputs.families = structuredClone(inputs.families);
  const miro = inputs.families.find((family) => family.family === "miro");
  assert.ok(miro, "expected a miro family");
  // An adapter must declare at least one match rule (adapter-templates.ts
  // `no_rules`); emptying it makes compileDocument(synthesizeFixture(miro))
  // fail for the whole family.
  miro.types[0].adapter.match = [];
  const problems = coverageProblems(inputs, engine);
  assert.deepEqual(problems, ["miro: synthesized fixture does not compile (no_rules)"]);
});
