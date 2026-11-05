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
  const scaffold = runScaffold(["aha", "aha.feature", "--subdomain", ".aha.io", "--root", root]);
  assert.equal(scaffold.status, 0, scaffold.stdout + scaffold.stderr);

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

  // The doc's Status: line is already valid (the scaffolder wrote
  // "Status: development draft." and status.state is still "development").
  const docPath = path.join(root, "docs", "adapters", "aha.feature.md");
  assert.match(readFileSync(docPath, "utf8"), /^# aha\.feature starter draft\n\nStatus: development draft\.\n/);

  const result = runGenerator(["--root", root]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /appended lock entry aha\.feature → aha\.feature:\{host\}\/\{key\}/);

  assert.deepEqual(gitStatus(root), [
    "aha.feature.md",
    "aha.json",
    "catalog.json",
    "ids.lock.json",
    "starter-families.generated.ts",
    "starter-shapes.json",
    "starter-vectors.json",
  ].map((name) => {
    if (name === "aha.feature.md") return "docs/adapters/aha.feature.md";
    if (name === "aha.json") return "protocol/starters/aha.json";
    if (name === "catalog.json") return "protocol/capture-site/catalog.json";
    if (name === "ids.lock.json") return "protocol/starters/ids.lock.json";
    if (name === "starter-families.generated.ts") return GENERATED_TS;
    if (name === "starter-shapes.json") return "protocol/capture-site/starter-shapes.json";
    return "protocol/adapter-templates/starter-vectors.json";
  }).concat(["docs/adapters/README.md"]).sort());

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
