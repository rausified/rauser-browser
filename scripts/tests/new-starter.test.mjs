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
  // "acmetrack", not a real tool's name (Codex P2 round): the shipped
  // registry can gain a real family of almost any plausible name (PR D),
  // and this test must not start colliding with it and refusing with
  // "already exists" for an unrelated reason.
  const result = runScaffold(["acmetrack", "acmetrack.ticket", "--subdomain", ".acmetrack.io", "--root", root]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /types\[0\]\.adapter\.match must be a non-empty array/);
  assert.deepEqual(gitStatus(root), ["docs/adapters/acmetrack.ticket.md", "protocol/starters/acmetrack.json"]);
});

test("a new type on a scoped family gets the family's own scope marker key (Minor 2)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const result = runScaffold(["github", "github.discussion", "--root", root]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  // Not "adapter.scope must be an object whose values carry the {\"$scope\":
  // true} marker": the old scaffolder wrote the bare marker directly as
  // adapter.scope, which resolves at runtime to an array, not a
  // capture-keyed object (final review, Minor 2). The fix infers "owner"
  // from GitHub's other types instead of writing the marker itself.
  assert.doesNotMatch(result.stdout + result.stderr, /adapter\.scope must be an object/);
  const github = readFamily(root, "github");
  const discussion = github.types.find((type) => type.type === "github.discussion");
  assert.deepEqual(discussion.adapter.scope, { owner: { "$scope": true } });
});

test("a new type on a multi-origin family gets an origin-qualified key", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const result = runScaffold(["box", "box.hub", "--root", root]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.doesNotMatch(result.stdout + result.stderr, /key must contain/);
  const hub = readFamily(root, "box").types.find((type) => type.type === "box.hub");
  assert.equal(hub.key, "box.hub:{origin}");
});

test("acceptance: a filled-in family changes only its file, its doc and generated files (spec task 10)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const baselineLock = readLock(root);
  // Derived from the copied registry, not a literal count (Codex P2
  // round): the shipped ids.lock.json gains entries over time (PR D and
  // later), so pinning "8" here would break on every such PR without this
  // test itself being wrong.
  const baselineCount = Object.keys(baselineLock).length;
  assert.ok(baselineCount > 0, "expected at least one existing lock entry to extend");

  // "acmetrack"/"acmetrack.ticket", not a real tool's name (Codex P2
  // round): the shipped registry can gain a real family of almost any
  // plausible name (PR D), and this test must not start colliding with it
  // and refusing with "already exists" for an unrelated reason.
  const scaffold = runScaffold(["acmetrack", "acmetrack.ticket", "--subdomain", ".acmetrack.io", "--root", root]);
  assert.equal(scaffold.status, 0, scaffold.stdout + scaffold.stderr);

  // Before any doc section is filled in, --check names every one of them
  // (Task 6 review, Important 2): the "one pass" list must cover what the
  // scaffolder itself just wrote as a placeholder, not only what a human
  // edits later.
  const docPath = path.join(root, "docs", "adapters", "acmetrack.ticket.md");
  const beforeDocFill = runGenerator(["--check", "--root", root]);
  assert.equal(beforeDocFill.status, 1);
  for (const heading of DOC_HEADINGS) {
    assert.match(beforeDocFill.stdout, new RegExp(`docs/adapters/acmetrack\\.ticket\\.md: section "${heading}" is still a placeholder`));
  }

  // The family is scope: null throughout (gap 1: a subdomain family can
  // never be unscoped, whatever the spec's "Aha! (subdomain, unscoped)"
  // example says), so review_prompt stays null and sample_scope stays [].
  const acmetrack = readFamily(root, "acmetrack");
  assert.equal(acmetrack.scope, null);
  acmetrack.display_name = "AcmeTrack";
  acmetrack.sample_origin = "https://acme.acmetrack.io";
  const type = acmetrack.types[0];
  type.label = "AcmeTrack tickets";
  type.description = "AcmeTrack ticket pages become one artifact per ticket key.";
  type.record_label = "tickets";
  type.adapter = {
    type: "acmetrack.ticket",
    match: [{ path: "/tickets/{key:jira_key}" }],
    id: "acmetrack.ticket:{host}/{key}",
    title: "{title} | AcmeTrack",
  };
  type.samples = [{
    url: "https://acme.acmetrack.io/tickets/ENG-42",
    title: "Ticket name | AcmeTrack",
    shape: "/tickets/{key}",
    note: "AcmeTrack ticket",
  }];
  type.vectors = [
    {
      id: "eval.acmetrack.ticket.positive",
      kind: "evaluate",
      why: "the /tickets/<key> rule matches a ticket page",
      impls: ["rust", "ts"],
      url: "https://acme.acmetrack.io/tickets/ENG-42",
      expect: {
        match: true, adapter: 0, rule: 0, type: "acmetrack.ticket",
        captures: { key: "ENG-42" }, artifact_id: "acmetrack.ticket:acme.acmetrack.io/ENG-42", mode: null,
      },
    },
    {
      id: "eval.acmetrack.ticket.negative",
      kind: "evaluate",
      why: "the projects route is not a ticket page",
      impls: ["rust", "ts"],
      url: "https://acme.acmetrack.io/projects/ENG-42",
      expect: { match: false, reason: "no_rule" },
    },
  ];
  writeFamily(root, "acmetrack", acmetrack);

  // Fill in the doc's six sections too, not only its already-valid
  // Status: line (Task 6 review, Important 2).
  writeFileSync(docPath, filledDocText("acmetrack.ticket", [
    "1. **Ticket page**, `/tickets/<key>`. The key is the ticket's dashed reference.",
    "Ticket tab titles read <ticket name> | AcmeTrack.",
    "Synthetic equivalent of the recorded shape, on acme.acmetrack.io: /tickets/ENG-42, tab title \"Ticket name | AcmeTrack\", records ENG-42.",
    "The AcmeTrack root, search and settings routes are not ticket pages.",
    "Multi-segment dashed keys (such as PRJ-E-12) are not yet supported.",
    "Before this page's status line changes to release-certified, check on macOS with real Chrome and a real AcmeTrack account.",
  ]));

  const result = runGenerator(["--root", root]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(result.stdout, /appended lock entry acmetrack\.ticket → acmetrack\.ticket:\{host\}\/\{key\}/);
  assert.doesNotMatch(result.stdout, /is still a placeholder/);

  assert.deepEqual(gitStatus(root), [
    "docs/adapters/README.md",
    "docs/adapters/acmetrack.ticket.md",
    "extension/starter-families.generated.ts",
    "protocol/adapter-templates/starter-vectors.json",
    "protocol/capture-site/catalog.json",
    "protocol/capture-site/starter-shapes.json",
    "protocol/starters/acmetrack.json",
    "protocol/starters/ids.lock.json",
  ]);

  // Open decision 1 / risk 3: the lock append is a pure extension of the
  // entries that existed before this run, not a rewrite of them.
  const finalLock = readLock(root);
  assert.deepEqual(Object.entries(finalLock).slice(0, baselineCount), Object.entries(baselineLock));
  assert.equal(finalLock["acmetrack.ticket"], "acmetrack.ticket:{host}/{key}");

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

test("scaffolder misuse: a dotless --subdomain", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const result = runScaffold(["servicenow", "servicenow.incident", "--subdomain", "service-now.com", "--root", root]);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /--subdomain "service-now\.com" must start with "\." and have at least two labels/);
  assert.deepEqual(gitStatus(root), []);
});

test("scaffolder misuse: a non-canonical --exact origin (trailing slash)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const result = runScaffold(["docsgoogle", "docsgoogle.doc", "--exact", "https://docs.google.com/", "--root", root]);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /--exact "https:\/\/docs\.google\.com\/" must be a canonical https origin/);
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
