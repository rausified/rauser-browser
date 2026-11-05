// scripts/new-starter.mjs tests (docs/superpowers/specs/2026-11-03-starter-families-design.md
// §4 "Scaffolder"; spec §5 PR C step 10 "Acceptance"). Every test runs on a
// tmpRoot that copies protocol/, extension/starter-families.generated.ts and
// docs/adapters/ (the scaffolder and the generator both run from it with
// --root), then `git init`s and commits that copy once, so a later
// `git status --porcelain` names exactly what the scaffolder, and then the
// generator, wrote or changed.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test from "node:test";

import { coverageProblems, loadFamilies } from "../generate-starters.mjs";
import { ipcDenied } from "./ipc-guard.mjs";

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const SCAFFOLDER = path.join(REPO, "scripts", "new-starter.mjs");
const GENERATOR = path.join(REPO, "scripts", "generate-starters.mjs");
const GENERATED_TS = "extension/starter-families.generated.ts";
const LOCK_FILE = "protocol/starters/ids.lock.json";

// The six section headings scripts/new-starter.mjs writes (spec §4
// "Scaffolder"), each starting as a lone "TODO" body.
const DOC_HEADINGS = [
  "What each rule covers",
  "Titles",
  "Recorded samples",
  "What the draft does not record",
  "Known limits",
  "Live check still needed",
];

function git(root, args) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8" });
}

function tmpRoot(t) {
  const dir = mkdtempSync(path.join(os.tmpdir(), "new-starter-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  cpSync(path.join(REPO, "protocol"), path.join(dir, "protocol"), { recursive: true });
  cpSync(path.join(REPO, "docs", "adapters"), path.join(dir, "docs", "adapters"), { recursive: true });
  mkdirSync(path.join(dir, "extension"), { recursive: true });
  cpSync(path.join(REPO, GENERATED_TS), path.join(dir, GENERATED_TS));
  git(dir, ["init", "-q"]);
  git(dir, ["config", "user.email", "test@example.com"]);
  git(dir, ["config", "user.name", "Test"]);
  git(dir, ["add", "-A"]);
  git(dir, ["commit", "-q", "-m", "baseline"]);
  return dir;
}

/** `git status --porcelain`'s paths, sorted, status codes stripped. */
function gitStatus(root) {
  const output = git(root, ["status", "--porcelain"]);
  return output.split("\n").filter((line) => line !== "").map((line) => line.slice(3)).sort();
}

function runScaffold(args) {
  return spawnSync(process.execPath, [SCAFFOLDER, ...args], { encoding: "utf8" });
}

function runGenerator(args) {
  return spawnSync(process.execPath, [GENERATOR, ...args], { encoding: "utf8" });
}

function readFamily(root, name) {
  return JSON.parse(readFileSync(path.join(root, "protocol", "starters", `${name}.json`), "utf8"));
}

function writeFamily(root, name, data) {
  writeFileSync(path.join(root, "protocol", "starters", `${name}.json`), `${JSON.stringify(data, null, 2)}\n`);
}

function readLock(root) {
  return JSON.parse(readFileSync(path.join(root, LOCK_FILE), "utf8"));
}

/** `docs/adapters/<type>.md`'s text with every section's "TODO" body
 * replaced by `bodies[i]`, same heading order as `DOC_HEADINGS` — the
 * acceptance test's way of actually filling in the doc, not just its
 * `Status:` line. */
function filledDocText(type, bodies) {
  const sections = DOC_HEADINGS.map((heading, index) => `## ${heading}\n\n${bodies[index]}\n`).join("\n");
  return `# ${type} starter draft\n\nStatus: development draft.\n\n${sections}`;
}

test("scaffolds a family file and doc, nothing else", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const result = runScaffold(["aha", "aha.feature", "--subdomain", ".aha.io", "--root", root]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /types\[0\]\.adapter\.match must be a non-empty array/);
  assert.deepEqual(gitStatus(root), ["docs/adapters/aha.feature.md", "protocol/starters/aha.json"]);
});

test("acceptance: a filled-in family changes only its file, its doc and generated files (spec task 10)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const baselineLock = readLock(root);
  assert.equal(Object.keys(baselineLock).length, 8, "today's lock has 8 entries; this test pins the append as a pure extension of them");

  const scaffold = runScaffold(["aha", "aha.feature", "--subdomain", ".aha.io", "--root", root]);
  assert.equal(scaffold.status, 0, scaffold.stdout + scaffold.stderr);

  // Before any doc section is filled in, --check names every one of them
  // (Task 6 review, Important 2): the "one pass" list must cover what the
  // scaffolder itself just wrote as a placeholder, not only what a human
  // edits later.
  const docPath = path.join(root, "docs", "adapters", "aha.feature.md");
  const beforeDocFill = runGenerator(["--check", "--root", root]);
  assert.equal(beforeDocFill.status, 1);
  for (const heading of DOC_HEADINGS) {
    assert.match(beforeDocFill.stdout, new RegExp(`docs/adapters/aha\\.feature\\.md: section "${heading}" is still a placeholder`));
  }

  // The family is scope: null throughout (gap 1: a subdomain family can
  // never be unscoped, whatever the spec's "Aha! (subdomain, unscoped)"
  // example says), so review_prompt stays null and sample_scope stays [].
  const aha = readFamily(root, "aha");
  assert.equal(aha.scope, null);
  aha.display_name = "Aha!";
  aha.sample_origin = "https://acme.aha.io";
  const type = aha.types[0];
  type.label = "Aha! features";
  type.description = "Aha! feature pages become one artifact per feature key.";
  type.record_label = "features";
  type.adapter = {
    type: "aha.feature",
    match: [{ path: "/features/{key:jira_key}" }],
    id: "aha.feature:{host}/{key}",
    title: "{title} | Aha!",
  };
  type.samples = [{
    url: "https://acme.aha.io/features/ENG-42",
    title: "Ticket name | Aha!",
    shape: "/features/{key}",
    note: "Aha! feature",
  }];
  type.vectors = [
    {
      id: "eval.aha.feature.positive",
      kind: "evaluate",
      why: "the /features/<key> rule matches a feature page",
      impls: ["rust", "ts"],
      url: "https://acme.aha.io/features/ENG-42",
      expect: {
        match: true, adapter: 0, rule: 0, type: "aha.feature",
        captures: { key: "ENG-42" }, artifact_id: "aha.feature:acme.aha.io/ENG-42", mode: null,
      },
    },
    {
      id: "eval.aha.feature.negative",
      kind: "evaluate",
      why: "the ideas route is not a feature page",
      impls: ["rust", "ts"],
      url: "https://acme.aha.io/ideas/ENG-42",
      expect: { match: false, reason: "no_rule" },
    },
  ];
  writeFamily(root, "aha", aha);

  // Fill in the doc's six sections too, not only its already-valid
  // Status: line (Task 6 review, Important 2).
  writeFileSync(docPath, filledDocText("aha.feature", [
    "1. **Feature page**, `/features/<key>`. The key is the ticket's dashed reference.",
    "Feature tab titles read <ticket name> | Aha!.",
    "Synthetic equivalent of the recorded shape, on acme.aha.io: /features/ENG-42, tab title \"Ticket name | Aha!\", records ENG-42.",
    "The Aha! root, search and settings routes are not feature pages.",
    "Multi-segment dashed keys (such as PRJ-E-12) are not yet supported.",
    "Before this page's status line changes to release-certified, check on macOS with real Chrome and a real Aha! account.",
  ]));

  const result = runGenerator(["--root", root]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /appended lock entry aha\.feature → aha\.feature:\{host\}\/\{key\}/);
  assert.doesNotMatch(result.stdout, /is still a placeholder/);

  assert.deepEqual(gitStatus(root), [
    "docs/adapters/README.md",
    "docs/adapters/aha.feature.md",
    "extension/starter-families.generated.ts",
    "protocol/adapter-templates/starter-vectors.json",
    "protocol/capture-site/catalog.json",
    "protocol/capture-site/starter-shapes.json",
    "protocol/starters/aha.json",
    "protocol/starters/ids.lock.json",
  ]);

  // Open decision 1 / risk 3: the lock append is a pure extension of the
  // 8 entries that existed before this run, not a rewrite of them.
  const finalLock = readLock(root);
  assert.deepEqual(Object.entries(finalLock).slice(0, 8), Object.entries(baselineLock));
  assert.equal(finalLock["aha.feature"], "aha.feature:{host}/{key}");

  let engine;
  try {
    const built = await import(pathToFileURL(path.join(REPO, "extension", "dist", "adapter-templates.js")).href);
    engine = { compileDocument: built.compileDocument, evaluate: built.evaluate };
  } catch (error) {
    t.skip(`extension/dist/adapter-templates.js is not built (run npm run build:extension first): ${error.message}`);
    return;
  }
  const families = loadFamilies(root);
  const vectors = JSON.parse(readFileSync(path.join(root, "protocol", "adapter-templates", "vectors.json"), "utf8"));
  const starterVectors = JSON.parse(readFileSync(path.join(root, "protocol", "adapter-templates", "starter-vectors.json"), "utf8"));
  const problems = coverageProblems({ families, vectors, starterVectors }, engine);
  assert.deepEqual(problems, []);
});

test("scaffolder misuse: a type that already exists", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const result = runScaffold(["atlassian", "jira.issue", "--root", root]);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /jira\.issue.*already exists/);
  assert.deepEqual(gitStatus(root), []);
});

test("scaffolder misuse: origin flags on an existing family", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const result = runScaffold(["atlassian", "atlassian.newtype", "--exact", "https://example.test", "--root", root]);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /atlassian.*already exists/);
  assert.deepEqual(gitStatus(root), []);
});

test("scaffolder misuse: --scope on an existing family", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  // No --exact/--subdomain at all: isolates --scope as the only offending flag.
  const result = runScaffold(["atlassian", "atlassian.newtype2", "--scope", "unscoped", "--root", root]);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /atlassian.*already exists/);
  assert.deepEqual(gitStatus(root), []);
});

test("scaffolder misuse: a new family with no origin flag", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const result = runScaffold(["newfam", "newfam.item", "--root", root]);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /--exact or --subdomain/);
  assert.deepEqual(gitStatus(root), []);
});

test("scaffolder misuse: an underscore in the type", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const result = runScaffold(["newfam", "new_fam.item", "--subdomain", ".newfam.test", "--root", root]);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /type "new_fam\.item" must match/);
  assert.deepEqual(gitStatus(root), []);
});

test("scaffolder misuse: --scope unscoped with --subdomain", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const result = runScaffold(["newfam2", "newfam2.item", "--subdomain", ".newfam2.test", "--scope", "unscoped", "--root", root]);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /unscoped.*--exact/);
  assert.deepEqual(gitStatus(root), []);
});

test("scaffolder misuse: the doc page already exists", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const docPath = path.join(root, "docs", "adapters", "box.feature.md");
  writeFileSync(docPath, "precious hand-started notes\n");
  const result = runScaffold(["box", "box.feature", "--exact", "https://app.box.com", "--root", root]);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /docs\/adapters\/box\.feature\.md already exists/);
  assert.deepEqual(gitStatus(root), ["docs/adapters/box.feature.md"]);
  assert.equal(readFileSync(docPath, "utf8"), "precious hand-started notes\n");
});
