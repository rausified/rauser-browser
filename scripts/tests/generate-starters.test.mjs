import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdtempSync, readFileSync, renameSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import {
  appendLock, catalogTypeOrder, checkLock, checkPolicy, checkRegistry, familyClaims, loadFamilies, loadFamilyEntries,
  renderAdaptersIndex, renderCatalogLine, renderContainerFirstLine, renderStarterVectors, renderTs, validateFamily,
} from "../generate-starters.mjs";
import { ORIGIN_CASES, SHIPPED_STRICT_ORIGIN_CASES, STRICT_ORIGIN_CASES } from "../../extension/tests/starter-origin-cases.mjs";
import { EXAMPLE_CONTAINER_FIRST_FAMILY, EXAMPLE_STRICT_FAMILY } from "../../extension/tests/starter-fixtures.mjs";
import { ipcDenied } from "./ipc-guard.mjs";

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const SCRIPT = path.join(REPO, "scripts", "generate-starters.mjs");
const GENERATED_TS = "extension/starter-families.generated.ts";
const CATALOG_JSON = "protocol/capture-site/catalog.json";
const SHAPES_JSON = "protocol/capture-site/shapes.json";
const STARTER_SHAPES_JSON = "protocol/capture-site/starter-shapes.json";
const VECTORS_JSON = "protocol/adapter-templates/vectors.json";
const STARTER_VECTORS_JSON = "protocol/adapter-templates/starter-vectors.json";

const LEGACY_ORDER = ["jira.issue", "confluence.page", "sharepoint.doc", "github.pr", "github.issue", "github.repo", "miro.board"];
// The four families shipped before D1 (figma, google-workspace, vidcast);
// file-name order, which is also legacy catalog order for these four. A
// later family must not disturb this relative order (catalogTypeOrder keeps
// every existing type's position and only ever appends new ones).
const LEGACY_FAMILIES = ["atlassian", "github", "miro", "sharepoint"];

function tmpRoot(t) {
  const dir = mkdtempSync(path.join(os.tmpdir(), "generate-starters-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  cpSync(path.join(REPO, "protocol", "starters"), path.join(dir, "protocol", "starters"), { recursive: true });
  cpSync(path.join(REPO, "docs", "adapters"), path.join(dir, "docs", "adapters"), { recursive: true });
  cpSync(path.join(REPO, CATALOG_JSON), path.join(dir, CATALOG_JSON));
  cpSync(path.join(REPO, GENERATED_TS), path.join(dir, GENERATED_TS));
  cpSync(path.join(REPO, SHAPES_JSON), path.join(dir, SHAPES_JSON));
  cpSync(path.join(REPO, STARTER_SHAPES_JSON), path.join(dir, STARTER_SHAPES_JSON));
  cpSync(path.join(REPO, VECTORS_JSON), path.join(dir, VECTORS_JSON));
  cpSync(path.join(REPO, STARTER_VECTORS_JSON), path.join(dir, STARTER_VECTORS_JSON));
  return dir;
}

function run(args, cwd = REPO) {
  return spawnSync(process.execPath, [SCRIPT, ...args], { cwd, encoding: "utf8" });
}

test("loads the shipped families, with the four legacy families and their seven types ordered as before", () => {
  // Derived from the loaded families/catalog rather than pinned to today's
  // exact family list, so a new family (D1: google-workspace, figma,
  // vidcast) does not break this test; the four legacy families and their
  // seven types must still appear, in their original relative order, as a
  // subset (team lead's CI fix, PR #323).
  const families = loadFamilies(REPO);
  const familyNames = families.map((family) => family.family);
  assert.deepEqual(familyNames.filter((name) => LEGACY_FAMILIES.includes(name)), LEGACY_FAMILIES);
  // families is file-name order, which groups a family's types together but
  // does not itself follow catalog order (e.g. sharepoint's file sorts after
  // miro's), so only the presence of exactly the seven legacy types is
  // checked here; the catalog-order guarantee belongs to `current` below,
  // which is what catalog.json (and renderTs's STARTER_FAMILY_IDS) actually
  // orders by.
  const types = families.flatMap((family) => family.types.map((type) => type.type));
  const legacyTypesPresent = types.filter((type) => LEGACY_ORDER.includes(type));
  assert.deepEqual([...legacyTypesPresent].sort(), [...LEGACY_ORDER].sort());
  const current = JSON.parse(readFileSync(path.join(REPO, CATALOG_JSON), "utf8")).starter_types;
  // Find the line by content, not a hard-coded index: the index breaks
  // whenever catalog.json gains a line, and splitting on "\n" alone leaves a
  // trailing "\r" on a CRLF checkout (Windows CI).
  const catalogLines = readFileSync(path.join(REPO, CATALOG_JSON), "utf8").split(/\r?\n/);
  const line = catalogLines.find((candidate) => candidate.includes('"starter_types"'));
  assert.ok(line, "catalog.json has no starter_types line");
  assert.equal(renderCatalogLine(families, current), line);
  // catalogTypeOrder keeps every existing type's position and only appends
  // new ones at the end, so the legacy seven stay an ordered prefix of the
  // live catalog no matter how many types get added after them.
  assert.deepEqual(current.slice(0, LEGACY_ORDER.length), LEGACY_ORDER);
});

test("--check is clean on the committed tree", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const result = run(["--check"]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.stdout.trim(), "");
});

test("a stale generated file fails --check", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const tsPath = path.join(root, GENERATED_TS);
  writeFileSync(tsPath, readFileSync(tsPath, "utf8") + "\n// stale\n");
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout, new RegExp(`out of date: ${GENERATED_TS}`));
});

test("an edited ID template fails the lock", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  atlassian.types[0].adapter.id = "jira:{host}/{key}/changed";
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout, /id template changed for jira\.issue/);
});

test("a missing adapter.id names the file and field", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  delete atlassian.types[0].adapter.id;
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: types\[0\]\.adapter\.id is required/);
});

test("an unknown field names the file and field", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  atlassian.types[0].lable = "typo";
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json:.*unknown field "lable"/);
});

// brwr-p2a7: a type's `docs` is joined onto the root and read, so it must
// name a file directly inside docs/adapters/; `..`, an absolute path, a
// nested or backslashed path, or a non-.md name is refused before any read.
const ESCAPING_DOCS = [
  "../outside.md",
  "docs/adapters/../../outside.md",
  "docs/adapters/../secret.md",
  "/etc/outside.md",
  "docs/other/jira.issue.md",
  "docs/adapters/nested/jira.issue.md",
  "docs/adapters\\jira.issue.md",
  "docs/adapters/./jira.issue.md",
  "docs/adapters/jira.issue.txt",
  "docs/adapters/.md",
];

test("a docs path outside docs/adapters/ fails validation with the file and field (brwr-p2a7)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  // A file the escaping path names, whose placeholder section and Status
  // line would each add a problem if the generator read it.
  writeFileSync(path.join(root, "outside.md"), "# Outside\n\n## Research\n\nTODO\n\nStatus: nothing\n");
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  atlassian.types[0].docs = "docs/adapters/../../outside.md";
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  const output = result.stdout + result.stderr;
  assert.equal(result.status, 1);
  assert.match(output, /protocol\/starters\/atlassian\.json: types\[0\]\.docs must name a file docs\/adapters\/<name>\.md/);
  assert.doesNotMatch(output, /outside\.md: (section|first Status)/);
});

test("validateFamily refuses every escaping docs path and accepts the shipped form (brwr-p2a7)", () => {
  const family = loadFamilies(REPO).find((entry) => entry.types.some((type) => type.type === "jira.issue"));
  for (const docs of ESCAPING_DOCS) {
    const copy = structuredClone(family);
    copy.types[0].docs = docs;
    const problems = [];
    validateFamily(problems, "protocol/starters/atlassian.json", copy);
    assert.ok(
      problems.some((line) => line.includes("types[0].docs must name a file docs/adapters/<name>.md")),
      `${JSON.stringify(docs)} was accepted: ${problems.join("; ")}`,
    );
  }
  const problems = [];
  validateFamily(problems, "protocol/starters/atlassian.json", structuredClone(family));
  assert.deepEqual(problems.filter((line) => line.includes(".docs")), []);
});

test("checkRegistry never reads an escaping docs path (brwr-p2a7)", (t) => {
  const root = tmpRoot(t);
  writeFileSync(path.join(root, "outside.md"), "# Outside\n\n## Research\n\nTODO\n\nStatus: nothing\n");
  const entries = loadFamilyEntries(root);
  const atlassian = entries.find((entry) => entry.file.endsWith("atlassian.json"));
  for (const docs of ["../outside.md", path.join(root, "outside.md")]) {
    atlassian.family.types[0].docs = docs;
    const problems = checkRegistry(root, entries);
    assert.ok(problems.some((line) => line.includes("must name a file docs/adapters/<name>.md")), problems.join("; "));
    assert.ok(!problems.some((line) => /outside\.md: (section|first Status)/.test(line)), problems.join("; "));
  }
});

// #463 review I1: the path text can be fine and the file a symlink out.
test("a docs file that is a symlink, or under a symlinked docs/adapters, is never read (brwr-p2a7)", { skip: process.platform === "win32" }, (t) => {
  const root = tmpRoot(t);
  const outside = path.join(root, "outside.md");
  writeFileSync(outside, "# Outside\n\n## Research\n\nTODO\n\nStatus: nothing\n");
  const doc = path.join(root, "docs", "adapters", "jira.issue.md");
  rmSync(doc);
  symlinkSync(outside, doc);
  const entries = loadFamilyEntries(root);
  let problems = checkRegistry(root, entries);
  assert.ok(problems.some((line) => line.includes("docs/adapters/jira.issue.md must not be a symlink")), problems.join("; "));
  assert.ok(!problems.some((line) => /jira\.issue\.md: (section|first Status)/.test(line)), problems.join("; "));

  // The whole folder moved elsewhere behind a symlink.
  rmSync(doc);
  const adapters = path.join(root, "docs", "adapters");
  const moved = path.join(root, "elsewhere");
  renameSync(adapters, moved);
  symlinkSync(moved, adapters);
  problems = checkRegistry(root, entries);
  assert.ok(problems.some((line) => line.includes("docs/adapters must not be a symlink")), problems.join("; "));
});

test("write mode refuses to write a generated output through a symlink (brwr-p2a7)", { skip: process.platform === "win32" }, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const outside = path.join(root, "outside.txt");
  writeFileSync(outside, "keep me\n");
  const output = path.join(root, GENERATED_TS);
  rmSync(output);
  symlinkSync(outside, output);
  const result = run(["--root", root]);
  assert.equal(result.status, 1, result.stdout + result.stderr);
  assert.match(result.stdout + result.stderr, /extension\/starter-families\.generated\.ts is a symlink; refusing to write through it/);
  assert.equal(readFileSync(outside, "utf8"), "keep me\n");
});

test("an invalid JSON file names the file and the parse error", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  writeFileSync(atlassianPath, readFileSync(atlassianPath, "utf8").slice(0, -1) + ",}");
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: invalid JSON: /);
});

test("a duplicate type or key across family files is caught", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const githubPath = path.join(root, "protocol", "starters", "github.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  const github = JSON.parse(readFileSync(githubPath, "utf8"));
  atlassian.types[0].type = github.types[0].type;
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/github\.json: duplicate type "github\.pr" \(also in protocol\/starters\/atlassian\.json\)/);
});

test("a subdomain family's key without {origin} names the file and field", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  atlassian.types[0].key = "jira";
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: types\[0\]\.key must contain "\{origin\}"/);
});

// An "any" family whose members are all `kind: "exact"` still serves more
// than one site, so its key must name the origin the same way a subdomain
// family's does (#340 review, Minor 1). No shipped family is in this shape
// (box, the only "any" family, has a subdomain member), so this uses a
// minimal synthetic fixture rather than mutating a shipped one. Before this
// fix, `hasSubdomain` only looked for a `kind: "subdomain"` member, so this
// case passed the generator while `new-starter.mjs`'s `defaultKey` already
// required `{origin}` for any non-exact origin kind, including this one:
// the generator and the scaffolder disagreed.
function exampleAnyExactFamily() {
  return {
    family: "example-any-exact",
    display_name: "Example",
    review_prompt: "Choose what to remember.",
    origin: {
      kind: "any",
      of: [
        { kind: "exact", origin: "https://a.example.test" },
        { kind: "exact", origin: "https://b.example.test" },
      ],
    },
    scope: null,
    sample_origin: "https://a.example.test",
    sample_scope: [],
    overlaps: [],
    types: [
      {
        type: "example.item",
        key: "example.item",
        label: "Example items",
        label_variants: [],
        description: "An example item.",
        record_label: null,
        example_path: null,
        adapter: {
          type: "example.item",
          match: [{ path: "/item/{id:int}" }],
          id: "example.item:{host}/{id}",
        },
        history: [],
        status: { state: "development", method: null, checked_on: null, checked_by: null },
        docs: "docs/adapters/example.item.md",
        samples: [],
        samples_shape: false,
        vectors: [],
      },
    ],
  };
}

test("an 'any' family of only exact origins still requires {origin} in its key", () => {
  const family = exampleAnyExactFamily();
  const problems = [];
  validateFamily(problems, "<fixture>", family);
  assert.deepEqual(problems, [`<fixture>: types[0].key must contain "{origin}"`]);
});

test("the same 'any'-of-exact family passes once its key names {origin}, matching new-starter.mjs's defaultKey rule", () => {
  const family = exampleAnyExactFamily();
  family.types[0].key = "example.item:{origin}";
  const problems = [];
  validateFamily(problems, "<fixture>", family);
  assert.deepEqual(problems, []);
});

test("adapter.match must be a non-empty array", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  atlassian.types[0].adapter.match = [];
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: types\[0\]\.adapter\.match must be a non-empty array/);
});

test("status.checked_on must be a string or null", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  atlassian.types[0].status.checked_on = 20261102;
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: types\[0\]\.status\.checked_on must be a string or null/);
});

test("checkLock flags an edited id and leaves an untouched lock clean", () => {
  const families = loadFamilies(REPO);
  const lock = JSON.parse(readFileSync(path.join(REPO, "protocol", "starters", "ids.lock.json"), "utf8"));
  assert.deepEqual(checkLock(families, lock), []);
  const edited = JSON.parse(JSON.stringify(families));
  // Look jira.issue up by name: family files load in file-name order, so a
  // family sorting before atlassian.json (aha.json) moves it off index 0.
  edited.flatMap((family) => family.types).find((type) => type.type === "jira.issue").adapter.id = "changed";
  assert.deepEqual(checkLock(edited, lock), ["id template changed for jira.issue"]);
});

// Today's eight shipped lock entries (protocol/starters/README.md "Frozen
// IDs": the lock is append-only, so these must stay exactly as shipped no
// matter what gets added later).
const TODAYS_LOCK_ENTRIES = {
  "jira.issue": "jira:{host}/{key}",
  "confluence.page": "confluence.page:{host}/{id}",
  "sharepoint.doc": "sharepoint:{host}/{doc}",
  "github.pr": "github.pr:{host}/{owner}/{repo}/{number}",
  "github.issue": "github.issue:{host}/{owner}/{repo}/{number}",
  "github.repo": "github.repo:{host}/{owner}/{repo}",
  "miro.board": "miro.board:{host}/{board}",
  "github.repo#within": "github.repo:{host}/{owner}/{repo}",
};

test("today's eight lock entries are a literal subset of ids.lock.json", () => {
  const lock = JSON.parse(readFileSync(path.join(REPO, "protocol", "starters", "ids.lock.json"), "utf8"));
  for (const [key, id] of Object.entries(TODAYS_LOCK_ENTRIES)) {
    assert.equal(lock[key], id, `ids.lock.json["${key}"] must stay "${id}"`);
  }
});

test("checkLock reports a missing lock entry for a new type", () => {
  const families = loadFamilies(REPO);
  const lock = JSON.parse(readFileSync(path.join(REPO, "protocol", "starters", "ids.lock.json"), "utf8"));
  const { "jira.issue": _omitted, ...lockWithoutJiraIssue } = lock;
  assert.deepEqual(checkLock(families, lockWithoutJiraIssue), ["no lock entry for jira.issue"]);
});

test("renderTs starts with the generated-file header and is deterministic", () => {
  const families = loadFamilies(REPO);
  // The single-argument form (no explicit catalog order) still works and is
  // deterministic: PR A's renderTs(families) call. main() always passes the
  // catalog order explicitly (so the disk file's STARTER_FAMILY_IDS follows
  // catalog order, not file order), so the disk comparison below passes that
  // order explicitly too.
  const rendered = renderTs(families);
  assert.match(rendered, /^\/\/ Generated by scripts\/generate-starters\.mjs from protocol\/starters\/\*\.json\. Do not edit\.\n/);
  assert.equal(rendered, renderTs(loadFamilies(REPO)));
  // .gitattributes pins this file to LF, but normalise anyway: a CRLF
  // checkout (Windows CI) should not fail this on line endings alone.
  // Derive the catalog order from the live catalog.json (via the same
  // catalogTypeOrder main() calls) instead of the pinned legacy-only
  // LEGACY_ORDER: a new family's types extend the real catalog order on
  // disk, so comparing against LEGACY_ORDER alone would disagree with what
  // main() actually wrote (team lead's CI fix, PR #323).
  const current = JSON.parse(readFileSync(path.join(REPO, CATALOG_JSON), "utf8")).starter_types;
  const catalogOrdered = renderTs(families, catalogTypeOrder(families, current));
  assert.equal(catalogOrdered, readFileSync(path.join(REPO, GENERATED_TS), "utf8").replace(/\r\n/g, "\n"));
});

test("STARTER_FAMILY_IDS follows catalog order, with the legacy families as an ordered prefix", () => {
  // Ranking families by LEGACY_ORDER alone (rather than the live catalog)
  // isolates this from however many families ship today: any family with no
  // type in LEGACY_ORDER ranks last (familyCatalogOrder), so the four legacy
  // families must still come first, in their original order, no matter how
  // many later families (D1: google-workspace, figma, vidcast) got appended
  // after them (team lead's CI fix, PR #323).
  const families = loadFamilies(REPO);
  const rendered = renderTs(families, LEGACY_ORDER);
  const match = rendered.match(/export const STARTER_FAMILY_IDS = \[([^\]]*)\] as const;/);
  assert.ok(match, "STARTER_FAMILY_IDS line not found in rendered output");
  const ids = JSON.parse(`[${match[1]}]`);
  assert.deepEqual(ids.slice(0, LEGACY_FAMILIES.length), ["atlassian", "sharepoint", "github", "miro"]);
});

test("--check tolerates a CRLF checkout of the generated outputs", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const tsPath = path.join(root, GENERATED_TS);
  const catalogPath = path.join(root, CATALOG_JSON);
  // protocol/adapter-templates/starter-vectors.json shares readNormalised
  // with these two, but had no CRLF case of its own until .gitattributes
  // pinned it to LF alongside them (Minor 4).
  const starterVectorsPath = path.join(root, STARTER_VECTORS_JSON);
  writeFileSync(tsPath, readFileSync(tsPath, "utf8").replace(/\n/g, "\r\n"));
  writeFileSync(catalogPath, readFileSync(catalogPath, "utf8").replace(/\n/g, "\r\n"));
  writeFileSync(starterVectorsPath, readFileSync(starterVectorsPath, "utf8").replace(/\n/g, "\r\n"));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.stdout.trim(), "");
});

function readFamily(root, name) {
  return JSON.parse(readFileSync(path.join(root, "protocol", "starters", `${name}.json`), "utf8"));
}

function writeFamily(root, name, data) {
  writeFileSync(path.join(root, "protocol", "starters", `${name}.json`), JSON.stringify(data, null, 2));
}

test("display_name is required", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  delete atlassian.display_name;
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: display_name is required/);
});

test("review_prompt must be a string when a family has more than one type or a scope", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.review_prompt = null;
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr,
    /protocol\/starters\/atlassian\.json: review_prompt must be a string when a family has more than one type or a scope/);
});

test('scope.kind must be "github_org", "tenant_name" or "unscoped"', async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const github = readFamily(root, "github");
  github.scope = { kind: "oauth" };
  writeFamily(root, "github", github);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr,
    /protocol\/starters\/github\.json: scope\.kind must be "github_org", "tenant_name" or "unscoped"/);
});

test("scope.max must be an integer from 1 to 32", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.scope = { kind: "tenant_name", max: 0, label: "Workspaces", placeholder: "acme" };
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: scope\.max must be an integer from 1 to 32/);
});

test("an unscoped family's marker and {scope} example_path are both caught", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.scope = { kind: "unscoped", notice: "x" };
  miro.types[0].adapter.scope = { owner: { "$scope": true } };
  miro.types[0].example_path = "/{scope}";
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  const output = result.stdout + result.stderr;
  assert.match(output, /protocol\/starters\/miro\.json: types\[0\]\.adapter has a \{"\$scope": true\} marker but the family is unscoped/);
  assert.match(output, /protocol\/starters\/miro\.json: types\[0\]\.example_path uses \{scope\} but the family has no scope values/);
});

test('an unscoped family requires adapter.unscoped to be "acknowledged"', async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.scope = { kind: "unscoped", notice: "x" };
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: types\[0\]\.adapter\.unscoped must be "acknowledged" for an unscoped family/);
});

test('a scoped family requires a {"$scope": true} marker on every type', async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const github = readFamily(root, "github");
  delete github.types[0].adapter.scope;
  writeFamily(root, "github", github);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr,
    /protocol\/starters\/github\.json: types\[0\]\.adapter needs a \{"\$scope": true\} marker for a scoped family/);
});

test("a scoped family's adapter.scope must be a capture-keyed table, not the bare marker (Minor 2)", async (t) => {
  // A bare adapter.scope = {"$scope": true} (the old scaffolder's output
  // for a new type on a scoped family) carries the marker "somewhere in
  // the adapter", so the marker-anywhere check alone missed it; at
  // runtime it resolves to an array, not {"<capture>": [...]} (final
  // review, Minor 2).
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const github = readFamily(root, "github");
  github.types[0].adapter.scope = { "$scope": true };
  writeFamily(root, "github", github);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/github\.json: types\[0\]\.adapter\.scope must be an object whose values carry the \{"\$scope": true\} marker/,
  );
});

test('example_path must start with "/"', async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.types[0].example_path = "browse/PROJ-42";
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr,
    /protocol\/starters\/atlassian\.json: types\[0\]\.example_path must start with "\/"/);
});

test("a missing samples names the file and field", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  delete miro.types[0].samples;
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/miro\.json: types\[0\]\.samples is required/);
});

test("a sample URL off the family's origin is refused", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.types[0].samples[0].url = "https://example.com/app/board/x=/";
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: types\[0\]\.samples\[0\]\.url is not on this family's origin/,
  );
});

test("samples_shape false may still carry a titled sample, but renderStarterShapes leaves it out", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  // samples_shape controls only whether renderStarterShapes carries a type's
  // samples into starter-shapes.json, not whether the type may have samples
  // at all. A type the structural drafter cannot learn (a query-routed
  // ServiceNow-style incident form, reproduced here) still needs a titled
  // sample for coverageProblems' title check (Task 5 re-review, Important 2).
  const root = tmpRoot(t);
  const github = readFamily(root, "github");
  github.types[2].samples.push({
    url: "https://github.com/acme/widget",
    title: "acme/widget - GitHub",
    shape: "/acme/widget",
    note: "x",
  });
  writeFamily(root, "github", github);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 0, result.stdout + result.stderr);

  const families = loadFamilies(root);
  const github2 = families.find((family) => family.family === "github");
  assert.equal(github2.types[2].samples_shape, false);
  assert.equal(github2.types[2].samples.length, 1);

  const starterShapes = JSON.parse(readFileSync(path.join(root, STARTER_SHAPES_JSON), "utf8"));
  assert.ok(!starterShapes.types.includes("github.repo"));
  assert.ok(!starterShapes.accept.some((row) => row.url === "https://github.com/acme/widget"));
});

test("samples must not be empty when samples_shape is true", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.types[0].samples = [];
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: types\[0\]\.samples must not be empty when samples_shape is true/,
  );
});

test("a stale starter-shapes.json fails --check", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const starterShapesPath = path.join(root, STARTER_SHAPES_JSON);
  const starterShapes = JSON.parse(readFileSync(starterShapesPath, "utf8"));
  starterShapes.accept[0].shape = "/changed";
  writeFileSync(starterShapesPath, JSON.stringify(starterShapes, null, 2) + "\n");
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, new RegExp(`out of date: ${STARTER_SHAPES_JSON}`));
});

// Today's 15 starter shape rows, copied byte for byte from
// protocol/capture-site/shapes.json at 3d39a6d (before this task moved them
// into the family files). Permanent (design challenge e3): proves the move
// changed nothing but location, and today's total of 40 accept rows across
// the two files never shrinks.
const TODAY_STARTER_ROWS = [
  {
    note: "Jira canonical issue (docs/adapters/jira.issue.md)",
    url: "https://acme.atlassian.net/browse/PROJ-42",
    shape: "/browse/{key}",
  },
  {
    note: "Jira board modal (docs/adapters/jira.issue.md)",
    url: "https://acme.atlassian.net/jira/software/projects/PROJ/boards/7?selectedIssue=PROJ-42",
    shape: "/jira/software/projects/{slug}/boards/{num}?{param}={key}",
  },
  {
    note: "Confluence page (docs/adapters/confluence.page.md)",
    url: "https://acme.atlassian.net/wiki/spaces/ENG/pages/1000001/Release+Plan",
    shape: "/wiki/spaces/{slug}/pages/{num}/{slug}",
  },
  {
    note: "Confluence page without a slug (docs/adapters/confluence.page.md)",
    url: "https://acme.atlassian.net/wiki/spaces/ENG/pages/1000002",
    shape: "/wiki/spaces/{slug}/pages/{num}",
  },
  {
    note: "Confluence page in a personal space (docs/adapters/confluence.page.md)",
    url: "https://acme.atlassian.net/wiki/spaces/~0123456789abcdef0123456789abcdef012345/pages/1000003/Some+Page",
    shape: "/wiki/spaces/{slug}/pages/{num}/{slug}",
  },
  {
    note: "Confluence page version history (docs/adapters/confluence.page.md)",
    url: "https://acme.atlassian.net/wiki/spaces/ENG/history/1000004/Release+Plan",
    shape: "/wiki/spaces/{slug}/history/{num}/{slug}",
  },
  {
    note: "Miro board (docs/adapters/miro.board.md)",
    url: "https://miro.com/app/board/qK3nR8vDpZx=/",
    shape: "/app/board/{id}/",
  },
  {
    note: "GitHub issue (docs/adapters/github.issue.md)",
    url: "https://github.com/acme/widget/issues/42",
    shape: "/acme/widget/issues/{num}",
  },
  {
    note: "GitHub pull request overview (docs/adapters/github.pr.md)",
    url: "https://github.com/Acme/Widget/pull/42",
    shape: "/{slug}/{slug}/pull/{num}",
  },
  {
    note: "GitHub pull request files (docs/adapters/github.pr.md)",
    url: "https://github.com/acme/widget/pull/42/files",
    shape: "/acme/widget/pull/{num}/files",
  },
  {
    note: "GitHub pull request commits (docs/adapters/github.pr.md)",
    url: "https://github.com/acme/widget/pull/42/commits",
    shape: "/acme/widget/pull/{num}/commits",
  },
  {
    note: "GitHub pull request checks (docs/adapters/github.pr.md)",
    url: "https://github.com/acme/widget/pull/42/checks",
    shape: "/acme/widget/pull/{num}/checks",
  },
  {
    note: "SharePoint site document (docs/adapters/sharepoint.doc.md)",
    url: "https://acme.sharepoint.com/sites/Eng/_layouts/15/Doc.aspx?sourcedoc=%7B0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D%7D&file=Plan.docx&action=default&mobileredirect=true",
    shape: "/sites/{slug}/_layouts/{num}/{slug}?action={slug}&file={slug}&mobileredirect={slug}&sourcedoc={uuid}",
  },
  {
    note: "SharePoint personal document (docs/adapters/sharepoint.doc.md)",
    url: "https://acme-my.sharepoint.com/personal/jdoe_acme_com/_layouts/15/Doc.aspx?sourcedoc=0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d",
    shape: "/personal/{slug}/_layouts/{num}/{slug}?sourcedoc={uuid}",
  },
  {
    note: "SharePoint sharing form (docs/adapters/sharepoint.doc.md)",
    url: "https://acme.sharepoint.com/:w:/r/sites/Eng/_layouts/15/Doc.aspx?sourcedoc=%7B0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D%7D&file=Plan.docx&action=default&mobileredirect=true",
    shape: "/{slug}/r/sites/{slug}/_layouts/{num}/{slug}?action={slug}&file={slug}&mobileredirect={slug}&sourcedoc={uuid}",
  },
];

test("today's 15 starter shape rows are pinned", () => {
  assert.equal(TODAY_STARTER_ROWS.length, 15);
  const shapes = JSON.parse(readFileSync(path.join(REPO, SHAPES_JSON), "utf8"));
  const starterShapes = JSON.parse(readFileSync(path.join(REPO, STARTER_SHAPES_JSON), "utf8"));
  for (const row of TODAY_STARTER_ROWS) {
    const inStarterShapes = starterShapes.accept.filter((candidate) => JSON.stringify(candidate) === JSON.stringify(row));
    assert.equal(inStarterShapes.length, 1, `${row.note} appears exactly once in starter-shapes.json`);
    const inShapes = shapes.accept.some((candidate) => JSON.stringify(candidate) === JSON.stringify(row));
    assert.equal(inShapes, false, `${row.note} must not appear in shapes.json`);
  }
  assert.ok(shapes.accept.length + starterShapes.accept.length >= 40);
});

test("familyClaims agrees with starterKind on the 40 origins", () => {
  const families = loadFamilies(REPO);
  assert.equal(ORIGIN_CASES.length, 40);
  for (const [origin, expected] of ORIGIN_CASES) {
    const claimants = families.filter((family) => familyClaims(family.origin, origin));
    assert.ok(claimants.length <= 1, `${origin}: more than one family claims it: ${claimants.map((f) => f.family)}`);
    assert.equal(claimants[0]?.family ?? null, expected, origin);
  }
});

test("familyClaims agrees with starterKind on the strict-subdomain origins", () => {
  assert.equal(STRICT_ORIGIN_CASES.length, 10);
  for (const [origin, expected] of STRICT_ORIGIN_CASES) {
    const claims = familyClaims(EXAMPLE_STRICT_FAMILY.origin, origin);
    assert.equal(claims ? EXAMPLE_STRICT_FAMILY.family : null, expected, origin);
  }
});

test("familyClaims agrees with starterKind on the shipped strict-family origins", () => {
  const families = loadFamilies(REPO);
  for (const [origin, expected] of SHIPPED_STRICT_ORIGIN_CASES) {
    const claimants = families.filter((family) => familyClaims(family.origin, origin));
    assert.ok(claimants.length <= 1, `${origin}: more than one family claims it: ${claimants.map((f) => f.family)}`);
    assert.equal(claimants[0]?.family ?? null, expected, origin);
  }
});

test("an exclude entry ending in * skips every first label with that prefix", () => {
  const origin = { kind: "subdomain", suffix: ".example.com", labels: 1, exclude: ["www", "up-*"] };
  assert.equal(familyClaims(origin, "https://up-1.example.com"), false);
  assert.equal(familyClaims(origin, "https://up.example.com"), true);
  assert.equal(familyClaims(origin, "https://acme-up-1.example.com"), true);
  assert.equal(familyClaims(origin, "https://www.example.com"), false);
  const permissive = { kind: "subdomain", suffix: ".example.com", exclude: ["up-*"] };
  assert.equal(familyClaims(permissive, "https://up-1.x.example.com"), false); // first label only
  assert.equal(familyClaims(permissive, "https://x.up-1.example.com"), true);
});

for (const [bad, why] of [
  [["*"], "a bare *"],
  [["fupload-**"], "two stars"],
  [["f*upload"], "a star not at the end"],
  [["*fupload"], "a leading star"],
  [["FUPLOAD-*"], "upper case"],
  [["fup_load-*"], "a character no DNS label has"],
  [["-x*"], "a leading hyphen"],
]) {
  test(`origin.exclude refuses ${why}`, async (t) => {
    if (await ipcDenied(t, "child-pipe")) return;
    const root = tmpRoot(t);
    const atlassian = readFamily(root, "atlassian");
    atlassian.origin.exclude = bad;
    writeFamily(root, "atlassian", atlassian);
    const result = run(["--check", "--root", root]);
    assert.equal(result.status, 1);
    assert.match(
      result.stdout + result.stderr,
      /protocol\/starters\/atlassian\.json: origin\.exclude must be a list of lowercase DNS labels or label prefixes ending in a single "\*"/,
    );
  });
}

test("origin.labels must be 1", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.origin.labels = 2;
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: origin\.labels must be 1/);
});

test("origin.exclude must be a list of lowercase DNS labels", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.origin.exclude = ["WWW"];
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/atlassian\.json: origin\.exclude must be a list of lowercase DNS labels or label prefixes ending in a single "\*"/,
  );
});

test("origin.suffix without a leading dot is refused (Minor 1)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  // Without this check, a dotless suffix such as "service-now.com" claims
  // "evilservice-now.com" and the bare "service-now.com" through
  // familyClaims's suffix-match, and refuses "acme.service-now.com", the
  // one host a ServiceNow-style family actually wants (final review,
  // Minor 1).
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.origin.suffix = "atlassian.net";
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/atlassian\.json: origin\.suffix must start with "\." and have at least two labels/,
  );
});

test("origin.origin must be a canonical https origin (Minor 1)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  // A trailing slash (or any path/query/credentials) otherwise surfaces
  // only indirectly, as "sample_origin is not on this family's origin",
  // which points at the wrong field (final review, Minor 1).
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.origin.origin = "https://miro.com/";
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: origin\.origin must be a canonical https origin/,
  );
});

test('origin unknown field "labels" on an exact origin', async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.origin.labels = 1;
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/miro\.json: origin unknown field "labels"/);
});

test("sample_scope must not be empty for a scoped family", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const github = readFamily(root, "github");
  github.sample_scope = [];
  writeFamily(root, "github", github);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/github\.json: sample_scope must not be empty for a scoped family/,
  );
});

test("sample_scope must be empty for an unscoped family", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.sample_scope = ["acme"];
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: sample_scope must be empty for an unscoped family/,
  );
});

test("sample_origin must be on the family's own origin", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.sample_origin = "https://example.com";
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: sample_origin is not on this family's origin/,
  );
});

test('overlaps[].expect must be "disjoint" or "container_first"', async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.overlaps[0].expect = "may_overlap";
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/atlassian\.json: overlaps\[0\]\.expect must be "disjoint" or "container_first"/,
  );
});

test("overlaps[].a and .b must name a type in this family", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.overlaps[0].b = "jira.page";
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/atlassian\.json: overlaps\[0\]\.b "jira\.page" is not a type in this family/,
  );
});

test("every pair of types that is not a within container needs an overlaps entry", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.overlaps = [];
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/atlassian\.json: jira\.issue and confluence\.page have no overlaps entry/,
  );
});

test("a within container is exempt from the pair-coverage check", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const github = readFamily(root, "github");
  github.overlaps = [];
  writeFamily(root, "github", github);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  const output = result.stdout + result.stderr;
  assert.match(output, /protocol\/starters\/github\.json: github\.pr and github\.issue have no overlaps entry/);
  assert.doesNotMatch(output, /github\.pr and github\.repo have no overlaps entry/);
  assert.doesNotMatch(output, /github\.issue and github\.repo have no overlaps entry/);
});

// --- Declared container-first overlaps ---------------------------------------

/** The container-first fixture with `edit` applied to a deep copy, and the
 * problems `validateFamily` reports for it. */
function containerFirstProblems(edit) {
  const family = structuredClone(EXAMPLE_CONTAINER_FIRST_FAMILY);
  edit(family);
  const problems = [];
  validateFamily(problems, "nest.json", family);
  return problems;
}

test("a container_first pair passes validation", () => {
  assert.deepEqual(containerFirstProblems(() => {}), []);
});

test("a container_first pair becomes the family's expected overlap_report pairs, winner first", () => {
  const family = structuredClone(EXAMPLE_CONTAINER_FIRST_FAMILY);
  const vectors = JSON.parse(renderStarterVectors([family], family.types.map((type) => type.type)));
  const report = vectors.cases.find((entry) => entry.id === "overlap_report.starter.nest");
  assert.deepEqual(report.expect, { pairs: [{ first: [0, 1], second: [1, 0] }] });
  assert.match(report.why, /container_first/);
  // A family with only disjoint entries still expects no pair at all.
  const strict = JSON.parse(renderStarterVectors([EXAMPLE_STRICT_FAMILY], ["strict.item"]));
  assert.deepEqual(strict.cases.find((entry) => entry.id === "overlap_report.starter.strict").expect, { pairs: [] });
});

test("the catalog's container_first line lists each declared [container, contained] pair for the host", () => {
  const family = structuredClone(EXAMPLE_CONTAINER_FIRST_FAMILY);
  assert.equal(renderContainerFirstLine([family, EXAMPLE_STRICT_FAMILY]),
    '  "container_first": [{ "container": "nest.space", "contained": "nest.board", "container_label": "Nest spaces", "contained_label": "Nest boards" }],');
  assert.equal(renderContainerFirstLine([EXAMPLE_STRICT_FAMILY]), '  "container_first": [],');
  // The shipped catalog carries Splunk's pair.
  const catalog = JSON.parse(readFileSync(path.join(REPO, CATALOG_JSON), "utf8"));
  assert.deepEqual(catalog.container_first, [{
    container: "splunk.app", contained: "splunk.dashboard", container_label: "Splunk apps", contained_label: "Splunk dashboards",
  }]);
});

test("a container_first pair re-runs the contained type's own vectors with that type alone", () => {
  const family = structuredClone(EXAMPLE_CONTAINER_FIRST_FAMILY);
  const board = {
    id: "eval.nest.board.roadmap", kind: "evaluate", why: "a board", impls: ["rust", "ts"],
    url: "https://nest.example.test/s/acme/roadmap",
    expect: {
      match: true, adapter: 1, rule: 0, type: "nest.board", captures: { space: "acme", board: "roadmap" },
      artifact_id: "nest.board:nest.example.test/acme/roadmap", mode: null,
      within: { type: "nest.space", artifact_id: "nest.space:nest.example.test/s/acme" },
    },
  };
  const winner = {
    id: "eval.nest.space.settings", kind: "evaluate", why: "a space's built-in page", impls: ["rust", "ts"],
    url: "https://nest.example.test/s/acme/settings",
    expect: {
      match: true, adapter: 0, rule: 1, type: "nest.space", captures: { space: "acme" },
      artifact_id: "nest.space:nest.example.test/s/acme", mode: null,
    },
  };
  const miss = { id: "eval.nest.board.miss", kind: "evaluate", why: "no board", impls: ["rust", "ts"],
    url: "https://nest.example.test/x", expect: { match: false, reason: "no_rule" } };
  family.types[0].vectors = [winner];
  family.types[1].vectors = [board, miss];
  const vectors = JSON.parse(renderStarterVectors([family], family.types.map((type) => type.type)));
  // The fixture holds only the contained type, without its within: the
  // runtime drops a within whose container is not configured.
  assert.deepEqual(vectors.fixtures["starter:nest/nest.board"], {
    format: 1,
    artifacts: [{ type: "nest.board", origin: "https://nest.example.test", match: [{ path: "/s/{space:name}/{board:name}" }],
      id: "nest.board:{host}/{space}/{board}" }],
  });
  const alone = vectors.cases.filter((entry) => entry.doc?.$fixture === "starter:nest/nest.board");
  assert.deepEqual(alone.map((entry) => entry.id), ["eval.nest.board.roadmap.alone"]);
  const { within: _within, ...unlinked } = board.expect;
  assert.deepEqual(alone[0].expect, { ...unlinked, adapter: 0 });
  assert.deepEqual(alone[0].impls, ["rust", "ts"]);
  assert.match(alone[0].why, /without nest\.space/);
  // A family without a container_first entry gets no extra fixture.
  const strict = JSON.parse(renderStarterVectors([EXAMPLE_STRICT_FAMILY], ["strict.item"]));
  assert.deepEqual(Object.keys(strict.fixtures), ["starter:strict"]);
});

test("expected pairs are sorted by first, then second, and span several entries", () => {
  const family = structuredClone(EXAMPLE_CONTAINER_FIRST_FAMILY);
  family.types[0].adapter.match.push({ path: "/s/{space:name}/(trash)" });
  family.overlaps[0].pairs = [[2, 0], [1, 0]];
  const vectors = JSON.parse(renderStarterVectors([family], family.types.map((type) => type.type)));
  const report = vectors.cases.find((entry) => entry.id === "overlap_report.starter.nest");
  assert.deepEqual(report.expect.pairs, [{ first: [0, 1], second: [1, 0] }, { first: [0, 2], second: [1, 0] }]);
});

test('container_first needs b within a', () => {
  const problems = containerFirstProblems((family) => { delete family.types[1].adapter.within; });
  assert.ok(problems.includes('nest.json: overlaps[0].expect "container_first" needs nest.board within nest.space'), problems.join("\n"));
  // The contained type winning is today's within fall-through, never declared.
  const reversed = containerFirstProblems((family) => {
    family.overlaps[0] = { a: "nest.board", b: "nest.space", expect: "container_first", pairs: [[0, 1]] };
  });
  assert.ok(reversed.includes('nest.json: overlaps[0].expect "container_first" needs nest.space within nest.board'), reversed.join("\n"));
});

test("container_first needs b's within.id to be a's own id template", () => {
  const problems = containerFirstProblems((family) => { family.types[1].adapter.within.id = "nest.space:{host}/{space}"; });
  assert.ok(problems.includes('nest.json: overlaps[0].expect "container_first" needs nest.board\'s within.id to be nest.space\'s adapter.id'), problems.join("\n"));
});

test("container_first needs a before b", () => {
  const problems = containerFirstProblems((family) => { family.types.reverse(); });
  assert.ok(problems.includes('nest.json: overlaps[0].expect "container_first" needs nest.space before nest.board'), problems.join("\n"));
});

test("container_first rule pairs must name real rules", () => {
  const outOfRange = containerFirstProblems((family) => { family.overlaps[0].pairs = [[2, 0]]; });
  assert.ok(outOfRange.includes("nest.json: overlaps[0].pairs[0] names no rule"), outOfRange.join("\n"));
  const malformed = containerFirstProblems((family) => { family.overlaps[0].pairs = [[1]]; });
  assert.ok(malformed.includes("nest.json: overlaps[0].pairs[0] must be [<a rule>, <b rule>]"), malformed.join("\n"));
  const empty = containerFirstProblems((family) => { family.overlaps[0].pairs = []; });
  assert.ok(empty.includes('nest.json: overlaps[0].pairs must list at least one rule pair for "container_first"'), empty.join("\n"));
  const missing = containerFirstProblems((family) => { delete family.overlaps[0].pairs; });
  assert.ok(missing.includes('nest.json: overlaps[0].pairs must list at least one rule pair for "container_first"'), missing.join("\n"));
  const repeated = containerFirstProblems((family) => { family.overlaps[0].pairs = [[1, 0], [1, 0]]; });
  assert.ok(repeated.includes("nest.json: overlaps[0].pairs[1] repeats a rule pair"), repeated.join("\n"));
});

test('a disjoint entry may not carry pairs', () => {
  const problems = containerFirstProblems((family) => { family.overlaps[0].expect = "disjoint"; });
  assert.ok(problems.includes('nest.json: overlaps[0].pairs is only for "container_first"'), problems.join("\n"));
});

test("a type pair is declared at most once", () => {
  const problems = containerFirstProblems((family) => {
    family.overlaps.push({ a: "nest.board", b: "nest.space", expect: "disjoint" });
  });
  assert.ok(problems.includes("nest.json: overlaps[1] declares nest.board and nest.space again"), problems.join("\n"));
});

test("renderTs carries each family's overlaps for the runtime order and the settings warnings", () => {
  const family = structuredClone(EXAMPLE_CONTAINER_FIRST_FAMILY);
  const text = renderTs([family], family.types.map((type) => type.type));
  assert.match(text, /"overlaps": \[\s*\{\s*"a": "nest.space",\s*"b": "nest.board",\s*"expect": "container_first",\s*"pairs": \[/);
  assert.match(text, /overlaps\?: StarterOverlapData\[\];/);
});

test('a vector must not carry "doc"', async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.types[0].vectors.push({ id: "eval.miro.extra", doc: { "$fixture": "starters" } });
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: types\[0\]\.vectors\[0\] must not carry "doc" \(the family fixture is implied\)/,
  );
});

test("a vector URL off the family's origin is refused", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.types[0].vectors.push({ id: "eval.miro.extra", url: "https://example.com/app/board/x=/" });
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: types\[0\]\.vectors\[0\]\.url is not on this family's origin/,
  );
});

test("a vector missing id, kind, impls or expect is reported, field by field (deferred T2)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  // PR D authors write vectors by hand; a missing key otherwise serialises
  // with the key simply absent (undefined drops from JSON.stringify) and
  // the failure only surfaces inside the engine harness, far from the
  // family file (final review, deferred T2).
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.types[0].vectors.push({ why: "missing everything else", url: "https://miro.com/app/board/x=/" });
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  const output = result.stdout + result.stderr;
  assert.match(output, /protocol\/starters\/miro\.json: types\[0\]\.vectors\[0\]\.id is required/);
  assert.match(output, /protocol\/starters\/miro\.json: types\[0\]\.vectors\[0\]\.kind is required/);
  assert.match(output, /protocol\/starters\/miro\.json: types\[0\]\.vectors\[0\]\.impls is required/);
  assert.match(output, /protocol\/starters\/miro\.json: types\[0\]\.vectors\[0\]\.expect is required/);
});

test("a vector's kind and impls must be from the engines' own known values (deferred T2)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.types[0].vectors.push({
    id: "eval.miro.extra", kind: "bogus-kind", why: "x", impls: ["php"], expect: { match: true },
  });
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  const output = result.stdout + result.stderr;
  assert.match(output, /protocol\/starters\/miro\.json: types\[0\]\.vectors\[0\]\.kind must be one of validate, evaluate/);
  assert.match(output, /protocol\/starters\/miro\.json: types\[0\]\.vectors\[0\]\.impls must be a non-empty array of rust, ts/);
});

test("a vector id duplicating a legacy vectors.json case is caught", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.types[0].vectors.push({ id: "eval.miro.title" });
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: duplicate vector id "eval\.miro\.title" \(also in protocol\/adapter-templates\/vectors\.json\)/,
  );
});

test("the test-only tenant_name, unscoped and strict-subdomain fixtures pass the generator's own family validation", async () => {
  // extension/tests/starter-fixtures.mjs stands in for a protocol/starters/
  // family file in panel, gallery and capture-site tests. Running it through
  // the same validateFamily the generator applies to every real family file
  // means it cannot quietly drift out of the file format (as
  // EXAMPLE_UNSCOPED_FAMILY once did with a null review_prompt on a scoped
  // family) without a test catching it.
  const { EXAMPLE_TENANT_FAMILY, EXAMPLE_UNSCOPED_FAMILY, EXAMPLE_UNSCOPED_PAIR_FAMILY, EXAMPLE_STRICT_FAMILY: strictFixture } =
    await import("../../extension/tests/starter-fixtures.mjs");
  for (const fixture of [EXAMPLE_TENANT_FAMILY, EXAMPLE_UNSCOPED_FAMILY, EXAMPLE_UNSCOPED_PAIR_FAMILY, strictFixture]) {
    const problems = [];
    validateFamily(problems, "<fixture>", fixture);
    assert.deepEqual(problems, [], fixture.family);
  }
});

// --- Task 5: shared-origin policy (checks 6, 7, gaps 1, 6, 8) --------------

test('scope "unscoped" needs a shared exact origin (gap 1)', async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.review_prompt = "Choose whether to remember Miro boards.";
  miro.scope = { kind: "unscoped", notice: "Records every Miro board from every account in this Chrome profile." };
  miro.types[0].adapter.unscoped = "acknowledged";
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: scope "unscoped" needs an exact origin in protocol\/capture-site\/catalog\.json shared_origins/,
  );
});

test("a suffix nested inside another suffix always collides (check 6)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const sharepoint = readFamily(root, "sharepoint");
  const onedrive = {
    ...sharepoint,
    family: "onedrive",
    display_name: "OneDrive",
    origin: { kind: "subdomain", suffix: ".my.sharepoint.com" },
    sample_origin: "https://acme.my.sharepoint.com",
    types: sharepoint.types.map((type) => ({
      ...type,
      type: "onedrive.doc",
      key: "onedrive:{origin}",
      docs: "docs/adapters/onedrive.doc.md",
      adapter: { ...type.adapter, type: "onedrive.doc", id: "onedrive:{host}/{doc}" },
      samples: [],
      samples_shape: false,
    })),
  };
  writeFamily(root, "onedrive", onedrive);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/sharepoint\.json: origin overlaps protocol\/starters\/onedrive\.json/,
  );
});

/** A one-type development family on `origin`, for the collision tests below. */
function originFamily(name, origin, sampleOrigin) {
  return {
    family: name,
    display_name: name,
    review_prompt: null,
    origin,
    scope: null,
    sample_origin: sampleOrigin,
    sample_scope: [],
    overlaps: [],
    types: [{
      type: `${name}.page`,
      key: `${name}.page:{origin}`,
      label: "Pages",
      label_variants: [],
      description: "An example page.",
      record_label: null,
      example_path: null,
      adapter: { type: `${name}.page`, match: [{ path: "/p/{id:int}" }], id: `${name}.page:{host}/{id}` },
      history: [],
      status: { state: "development", method: null, checked_on: null, checked_by: null },
      docs: `docs/adapters/${name}.page.md`,
      samples: [],
      samples_shape: false,
      vectors: [],
    }],
  };
}

/** checkPolicy's collision problems for two synthetic families. */
function collisions(a, b) {
  return checkPolicy([{ file: "a.json", family: a }, { file: "b.json", family: b }], { shared_origins: {} })
    .filter((problem) => problem.includes("origin overlaps"));
}

test("a labels: 1 family coexists with a family on a deeper suffix (check 6)", () => {
  const shallow = originFamily("shallow", { kind: "subdomain", suffix: ".example.com", labels: 1 }, "https://acme.example.com");
  const deep = originFamily("deep", { kind: "subdomain", suffix: ".ideas.example.com", labels: 1 }, "https://acme.ideas.example.com");
  assert.deepEqual(collisions(shallow, deep), []);
  assert.deepEqual(collisions(deep, shallow), []);
  // The deeper family need not itself be strict: every host it claims has
  // at least two labels before the shallow suffix.
  const deepPermissive = { ...deep, origin: { kind: "subdomain", suffix: ".ideas.example.com" } };
  assert.deepEqual(collisions(shallow, deepPermissive), []);
});

test("a permissive family still collides with a deeper suffix, whatever exclude says (check 6)", () => {
  const shallow = originFamily("shallow", { kind: "subdomain", suffix: ".example.com", exclude: ["ideas"] }, "https://acme.example.com");
  const deep = originFamily("deep", { kind: "subdomain", suffix: ".ideas.example.com", labels: 1 }, "https://acme.ideas.example.com");
  assert.deepEqual(collisions(shallow, deep), ["b.json: origin overlaps a.json"]);
  // labels: 1 only on the deeper family proves nothing about the shallow one.
  assert.deepEqual(collisions(deep, shallow), ["b.json: origin overlaps a.json"]);
});

test("two families on the same suffix always collide, even both labels: 1 (check 6)", () => {
  const one = originFamily("one", { kind: "subdomain", suffix: ".example.com", labels: 1, exclude: ["www"] }, "https://acme.example.com");
  const two = originFamily("two", { kind: "subdomain", suffix: ".example.com", labels: 1 }, "https://acme.example.com");
  assert.deepEqual(collisions(one, two), ["b.json: origin overlaps a.json"]);
});

test("a multi-origin family collides through any of its members (check 6)", () => {
  const multi = originFamily("multi", {
    kind: "any",
    of: [
      { kind: "exact", origin: "https://app.example.com" },
      { kind: "subdomain", suffix: ".ent.example.com", labels: 1 },
    ],
  }, "https://app.example.com");
  const exact = originFamily("exact", { kind: "exact", origin: "https://app.example.com" }, "https://app.example.com");
  assert.deepEqual(collisions(multi, exact), ["b.json: origin overlaps a.json"]);
  const nested = originFamily("nested", { kind: "subdomain", suffix: ".x.ent.example.com" }, "https://acme.x.ent.example.com");
  assert.deepEqual(collisions(multi, nested), []); // labels: 1 on the member proves it
  const unrelated = originFamily("unrelated", { kind: "subdomain", suffix: ".other.example" }, "https://acme.other.example");
  assert.deepEqual(collisions(multi, unrelated), []);
});

test("a multi-origin family claims an origin any member claims", () => {
  const origin = {
    kind: "any",
    of: [
      { kind: "exact", origin: "https://app.example.com" },
      { kind: "subdomain", suffix: ".app.example.com", labels: 1, exclude: ["up-*"] },
    ],
  };
  assert.equal(familyClaims(origin, "https://app.example.com"), true);
  assert.equal(familyClaims(origin, "https://acme.app.example.com"), true);
  assert.equal(familyClaims(origin, "https://up-1.app.example.com"), false);
  assert.equal(familyClaims(origin, "https://example.com"), false);
});

for (const [origin, message, why] of [
  [{ kind: "any", of: [{ kind: "exact", origin: "https://a.example.com" }] }, /origin\.of must list 2 or more origins/, "one member"],
  [{ kind: "any", of: "x" }, /origin\.of must list 2 or more origins/, "a non-array"],
  [{ kind: "any" }, /origin\.of is required/, "no of"],
  [{ kind: "any", of: [{ kind: "exact", origin: "https://a.example.com" }, { kind: "any", of: [] }] }, /origin\.of\[1\]\.kind must be "exact" or "subdomain"/, "a nested any"],
  [{ kind: "any", of: [{ kind: "exact", origin: "https://a.example.com" }, { kind: "subdomain", suffix: "bad" }] }, /origin\.of\[1\]\.suffix must start with/, "a bad member"],
  [{ kind: "any", of: [{ kind: "subdomain", suffix: ".example.com" }, { kind: "subdomain", suffix: ".a.example.com", labels: 1 }] }, /origin\.of\[1\] overlaps origin\.of\[0\]/, "overlapping members"],
  [{ kind: "any", of: [{ kind: "exact", origin: "https://a.example.com" }, { kind: "exact", origin: "https://b.example.com" }], extra: 1 }, /origin unknown field "extra"/, "an unknown key"],
]) {
  test(`a multi-origin family refuses ${why}`, async (t) => {
    if (await ipcDenied(t, "child-pipe")) return;
    const root = tmpRoot(t);
    const atlassian = readFamily(root, "atlassian");
    atlassian.origin = origin;
    writeFamily(root, "atlassian", atlassian);
    const result = run(["--check", "--root", root]);
    assert.equal(result.status, 1);
    assert.match(result.stdout + result.stderr, new RegExp(`protocol/starters/atlassian\\.json: ${message.source}`));
  });
}

test("an exact origin collides with a subdomain family that would claim it (check 6)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const extra = {
    family: "zzzacme",
    display_name: "Acme",
    review_prompt: null,
    origin: { kind: "exact", origin: "https://acme.atlassian.net" },
    scope: null,
    sample_origin: "https://acme.atlassian.net",
    sample_scope: [],
    overlaps: [],
    types: [{
      type: "zzzacme.page",
      key: "zzzacme.page",
      label: "Acme pages",
      label_variants: [],
      description: "An example page.",
      record_label: null,
      example_path: null,
      adapter: { type: "zzzacme.page", match: [{ path: "/p/{id:int}" }], id: "zzzacme.page:{host}/{id}" },
      history: [],
      status: { state: "development", method: null, checked_on: null, checked_by: null },
      docs: "docs/adapters/zzzacme.page.md",
      samples: [],
      samples_shape: false,
      vectors: [],
    }],
  };
  writeFamily(root, "zzzacme", extra);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/zzzacme\.json: origin overlaps protocol\/starters\/atlassian\.json/,
  );
});

test('scope "github_org" is only for https://github.com (gap 8)', async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.review_prompt = "Choose whether to remember Miro boards.";
  miro.scope = { kind: "github_org" };
  miro.sample_scope = ["acme"];
  miro.types[0].adapter.scope = { owner: { "$scope": true } };
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: scope "github_org" is only for https:\/\/github\.com/,
  );
});

test("a tenant_name family's adapter.scope key must be the host's own tenant capture (gap 6)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const notion = {
    family: "notionteam",
    display_name: "Notion",
    review_prompt: "Choose what to remember on Notion.",
    origin: { kind: "exact", origin: "https://www.notion.so" },
    scope: { kind: "tenant_name", max: 4, label: "Workspaces", placeholder: "acme" },
    sample_origin: "https://www.notion.so",
    sample_scope: ["acme"],
    overlaps: [],
    types: [{
      type: "notionteam.page",
      key: "notionteam.page:{host}/{id}",
      label: "Notion pages",
      label_variants: [],
      description: "A Notion page.",
      record_label: null,
      example_path: null,
      adapter: {
        type: "notionteam.page",
        match: [{ path: "/{team:name|lower}/{id:token(32)}" }],
        scope: { team: { "$scope": true } },
        id: "notionteam.page:{host}/{id}",
      },
      history: [],
      status: { state: "development", method: null, checked_on: null, checked_by: null },
      docs: "docs/adapters/notionteam.page.md",
      samples: [],
      samples_shape: false,
      vectors: [],
    }],
  };
  writeFamily(root, "notionteam", notion);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/notionteam\.json: types\[0\]\.adapter\.scope must use "workspace", the host's tenant capture for https:\/\/www\.notion\.so/,
  );
});

test('"tenant_name" cannot be used where URLs name no tenant (gap 6)', async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const gdocsteam = {
    family: "gdocsteam",
    display_name: "Google Docs",
    review_prompt: "Choose what to remember on Google Docs.",
    origin: { kind: "exact", origin: "https://docs.google.com" },
    scope: { kind: "tenant_name", max: 4, label: "Workspaces", placeholder: "acme" },
    sample_origin: "https://docs.google.com",
    sample_scope: ["acme"],
    overlaps: [],
    types: [{
      type: "gdocsteam.doc",
      key: "gdocsteam.doc:{host}/{id}",
      label: "Google Docs",
      label_variants: [],
      description: "A Google Doc.",
      record_label: null,
      example_path: null,
      adapter: {
        type: "gdocsteam.doc",
        match: [{ path: "/document/d/{id:token(20,64)}" }],
        scope: { workspace: { "$scope": true } },
        id: "gdocsteam.doc:{host}/{id}",
      },
      history: [],
      status: { state: "development", method: null, checked_on: null, checked_by: null },
      docs: "docs/adapters/gdocsteam.doc.md",
      samples: [],
      samples_shape: false,
      vectors: [],
    }],
  };
  writeFamily(root, "gdocsteam", gdocsteam);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/gdocsteam\.json: scope "tenant_name" cannot be used on https:\/\/docs\.google\.com, whose URLs name no tenant/,
  );
});

test("a shared origin always needs account values (check 7)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const linearteam = {
    family: "linearteam",
    display_name: "Linear",
    review_prompt: null,
    origin: { kind: "exact", origin: "https://linear.app" },
    scope: null,
    sample_origin: "https://linear.app",
    sample_scope: [],
    overlaps: [],
    types: [{
      type: "linearteam.issue",
      key: "linearteam.issue:{host}/{id}",
      label: "Linear issues",
      label_variants: [],
      description: "A Linear issue.",
      record_label: null,
      example_path: null,
      adapter: {
        type: "linearteam.issue",
        match: [{ path: "/issue/{id:token(10)}" }],
        id: "linearteam.issue:{host}/{id}",
      },
      history: [],
      status: { state: "development", method: null, checked_on: null, checked_by: null },
      docs: "docs/adapters/linearteam.issue.md",
      samples: [],
      samples_shape: false,
      vectors: [],
    }],
  };
  writeFamily(root, "linearteam", linearteam);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/linearteam\.json: origin https:\/\/linear\.app is a shared origin, so scope must not be null/,
  );
});

test("a subdomain family whose suffix covers a shared origin still gets check 7 (Minor 7)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  // .google.com claims docs.google.com (a real shared_origins entry) just
  // as readily as it claims mail.google.com. Before Minor 7's fix, check 7
  // only ever looked at shared_origins[exactOrigin], which is always
  // undefined for a subdomain family, so this family's scope: null
  // silently escaped the "a shared origin always needs account values"
  // rule entirely.
  const root = tmpRoot(t);
  const allgoogle = {
    family: "allgoogle",
    display_name: "All Google",
    review_prompt: null,
    origin: { kind: "subdomain", suffix: ".google.com" },
    scope: null,
    sample_origin: "https://mail.google.com",
    sample_scope: [],
    overlaps: [],
    types: [],
  };
  writeFamily(root, "allgoogle", allgoogle);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/allgoogle\.json: origin https:\/\/docs\.google\.com is a shared origin, so scope must not be null/,
  );
});

// --- Task 5: registry cross-checks (spec §4 check 2 reverse, check 3) -----

test("a stray docs/adapters/*.md file names no family type (check 2 reverse)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  writeFileSync(path.join(root, "docs", "adapters", "stray.md"), "# Stray\n\nStatus: development draft.\n");
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /docs\/adapters\/stray\.md: no family type/);
});

test("a shapes.json row naming a starter doc belongs in a family file's samples (check 2 reverse)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const shapesPath = path.join(root, SHAPES_JSON);
  const shapes = JSON.parse(readFileSync(shapesPath, "utf8"));
  shapes.accept.push({
    note: "Jira canonical issue (docs/adapters/jira.issue.md)",
    url: "https://acme.atlassian.net/browse/PROJ-1",
    shape: "/browse/{key}",
  });
  writeFileSync(shapesPath, JSON.stringify(shapes, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/capture-site\/shapes\.json: row "Jira canonical issue \(docs\/adapters\/jira\.issue\.md\)" names a starter doc; it belongs in a family file's samples/,
  );
});

test("a type's missing doc page names the file and field (check 3)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  rmSync(path.join(root, "docs", "adapters", "miro.board.md"));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: types\[0\]\.docs docs\/adapters\/miro\.board\.md is missing/,
  );
});

test("a certified doc's Status line disagreeing with its checked_on is caught (check 3)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const docPath = path.join(root, "docs", "adapters", "jira.issue.md");
  writeFileSync(docPath, readFileSync(docPath, "utf8").replace("2026-11-02", "2026-11-03"));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /docs\/adapters\/jira\.issue\.md: first Status line does not match status \(expected "Status: release-certified 2026-11-02\."\)/,
  );
});

test("a development type's doc wrongly claiming certification is caught (check 3)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const docPath = path.join(root, "docs", "adapters", "confluence.page.md");
  const updated = readFileSync(docPath, "utf8").replace(/^Status:.*$/m, "Status: release-certified 2026-11-02.");
  writeFileSync(docPath, updated);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /docs\/adapters\/confluence\.page\.md: first Status line does not match status \(expected "Status: development draft\."\)/,
  );
});

// --- Task 5: one pass -------------------------------------------------------

test("validation, policy and registry problems are all reported in one pass", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);

  // A family validation problem (unknown field).
  const miro = readFamily(root, "miro");
  miro.types[0].lable = "typo";
  writeFamily(root, "miro", miro);

  // A policy problem (gap 8) on a different, unrelated family.
  const extragithub = {
    family: "extragithub",
    display_name: "Extra",
    review_prompt: "Choose what to remember on Extra.",
    origin: { kind: "exact", origin: "https://example.test" },
    scope: { kind: "github_org" },
    sample_origin: "https://example.test",
    sample_scope: ["acme"],
    overlaps: [],
    types: [{
      type: "extragithub.item",
      key: "extragithub.item",
      label: "Extra items",
      label_variants: [],
      description: "An example item.",
      record_label: null,
      example_path: null,
      adapter: {
        type: "extragithub.item",
        match: [{ path: "/{owner:name|lower}/item/{id:int}" }],
        scope: { owner: { "$scope": true } },
        id: "extragithub.item:{host}/{owner}/{id}",
      },
      history: [],
      status: { state: "development", method: null, checked_on: null, checked_by: null },
      docs: "docs/adapters/extragithub.item.md",
      samples: [],
      samples_shape: false,
      vectors: [],
    }],
  };
  writeFamily(root, "extragithub", extragithub);
  writeFileSync(path.join(root, "docs", "adapters", "extragithub.item.md"), "# Extra item\n\nStatus: development draft.\n");

  // A registry problem (check 3, a stale doc), on a third, unrelated file.
  const docPath = path.join(root, "docs", "adapters", "jira.issue.md");
  writeFileSync(docPath, readFileSync(docPath, "utf8").replace("2026-11-02", "2026-11-03"));

  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  const output = result.stdout + result.stderr;
  assert.match(output, /protocol\/starters\/miro\.json:.*unknown field "lable"/);
  assert.match(output, /protocol\/starters\/extragithub\.json: scope "github_org" is only for https:\/\/github\.com/);
  assert.match(
    output,
    /docs\/adapters\/jira\.issue\.md: first Status line does not match status \(expected "Status: release-certified 2026-11-02\."\)/,
  );
});

// --- Task 5: fixtures do not collide with real families (gap 5) -----------

/** Whether `value` is shaped enough like a `StarterFamilyData` to feed to
 * `checkPolicy` (a `family` name and an `origin` object) — not every
 * `starter-fixtures.mjs` export necessarily is one, but every one checked
 * for collisions here must be, or `originsCollide` would crash on it. */
function looksLikeFamily(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    && typeof value.family === "string"
    && typeof value.origin === "object" && value.origin !== null;
}

test("fixtures do not collide with real families (gap 5)", async () => {
  // Iterates every starter-fixtures.mjs export, rather than naming four by
  // hand, so a fixture added later is covered automatically instead of
  // silently skipping this check (final review, Minor 5).
  const fixtures = await import("../../extension/tests/starter-fixtures.mjs");
  const catalog = JSON.parse(readFileSync(path.join(REPO, CATALOG_JSON), "utf8"));
  const realEntries = loadFamilyEntries(REPO);
  const fixtureEntries = Object.entries(fixtures)
    .filter(([, value]) => looksLikeFamily(value))
    .map(([name, family]) => ({ file: `<fixture ${name}>`, family }));
  assert.ok(fixtureEntries.length >= 4, `expected at least today's 4 family fixtures, got ${fixtureEntries.length}`);
  assert.deepEqual(checkPolicy([...realEntries, ...fixtureEntries], catalog), []);
});

test("an unmoved www.figma.com fixture would have collided with a Figma starter family (gap 5 regression bite)", () => {
  // Proves checkPolicy's check 6 has teeth: before Task 5 moved it,
  // EXAMPLE_UNSCOPED_FAMILY sat on https://www.figma.com, which PR D's real
  // Figma family will claim. This manufactures that old shape locally,
  // without touching the real (already-moved) fixture, and a PR D-shaped
  // family claiming the same origin, to show the collision is caught.
  const figmaFamily = {
    family: "figma",
    origin: { kind: "exact", origin: "https://www.figma.com" },
  };
  const unmovedFixture = {
    family: "everyone",
    origin: { kind: "exact", origin: "https://www.figma.com" },
  };
  const entries = [
    { file: "protocol/starters/figma.json", family: figmaFamily },
    { file: "<fixture EXAMPLE_UNSCOPED_FAMILY>", family: unmovedFixture },
  ];
  const catalog = JSON.parse(readFileSync(path.join(REPO, CATALOG_JSON), "utf8"));
  assert.deepEqual(checkPolicy(entries, catalog), [
    "protocol/starters/figma.json: origin overlaps <fixture EXAMPLE_UNSCOPED_FAMILY>",
  ]);
});

// --- Task 5 review fix round 1 ---------------------------------------------

test("a starter-shapes.json row naming no family type is caught (check 2 reverse, Minor 1)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const starterShapesPath = path.join(root, STARTER_SHAPES_JSON);
  const starterShapes = JSON.parse(readFileSync(starterShapesPath, "utf8"));
  starterShapes.accept.push({
    note: "Nonexistent sample (docs/adapters/nosuch.type.md)",
    url: "https://example.test/x",
    shape: "/x",
  });
  writeFileSync(starterShapesPath, `${JSON.stringify(starterShapes, null, 2)}\n`);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  const output = result.stdout + result.stderr;
  assert.match(
    output,
    /protocol\/capture-site\/starter-shapes\.json: row "Nonexistent sample \(docs\/adapters\/nosuch\.type\.md\)" names no family type/,
  );
  assert.match(output, new RegExp(`out of date: ${STARTER_SHAPES_JSON}`));
});

test("a missing starter-shapes.json regenerates in write mode instead of crashing (Important 1)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const starterShapesPath = path.join(root, STARTER_SHAPES_JSON);
  rmSync(starterShapesPath);

  // --check still reports it as out of date (main's own readNormalised
  // already treats a missing generated file that way).
  const checked = run(["--check", "--root", root]);
  assert.equal(checked.status, 1);
  assert.match(checked.stdout + checked.stderr, new RegExp(`out of date: ${STARTER_SHAPES_JSON}`));

  // Write mode regenerates it cleanly rather than crashing on the missing
  // file (the review's exact regression: base regenerates it, head threw
  // ENOENT).
  const written = run(["--root", root]);
  assert.equal(written.status, 0, written.stdout + written.stderr);
  const regenerated = readFileSync(starterShapesPath, "utf8");
  assert.ok(regenerated.length > 0);
  const recheck = run(["--check", "--root", root]);
  assert.equal(recheck.status, 0, recheck.stdout + recheck.stderr);
});

test("write mode clears a stale starter-shapes.json row left by a removed type (Codex P2 round)", async (t) => {
  // Before this fix, checkRegistry validated the on-disk starter-shapes.json
  // even in write mode: removing a type (and its doc) left a stale row
  // there that write mode refused to run past, so it could never write the
  // very regeneration that would have cleared the row — a deadlock.
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const docPath = path.join(root, "docs", "adapters", "miro.extra.md");

  // Add a second miro type with a sample, so it gets a starter-shapes.json row.
  const miro = readFamily(root, "miro");
  miro.review_prompt = "Choose what to remember on this Miro board.";
  miro.overlaps = [{ a: "miro.board", b: "miro.extra", expect: "disjoint" }];
  miro.types.push({
    type: "miro.extra", key: "miro.extra", label: "Extra", label_variants: [],
    description: "An extra Miro type, test-only.", record_label: null, example_path: null,
    adapter: { type: "miro.extra", match: [{ path: "/extra/{id:token(8)}" }], id: "miro.extra:{host}/{id}" },
    history: [], status: { state: "development", method: null, checked_on: null, checked_by: null },
    docs: "docs/adapters/miro.extra.md",
    samples: [{ note: "Miro extra", url: "https://miro.com/extra/abc12345", title: null, shape: "/extra/{id}" }],
    samples_shape: true,
    vectors: [],
  });
  writeFamily(root, "miro", miro);
  writeFileSync(docPath, "# miro.extra starter draft\n\nStatus: development draft.\n");

  const added = run(["--root", root]);
  assert.equal(added.status, 0, added.stdout + added.stderr);
  const starterShapesPath = path.join(root, STARTER_SHAPES_JSON);
  const afterAdd = JSON.parse(readFileSync(starterShapesPath, "utf8"));
  assert.ok(afterAdd.accept.some((row) => row.url.includes("/extra/")), "expected the new row after adding the type");

  // Remove the type (and its doc), as an unmerged type would be dropped.
  const miro2 = readFamily(root, "miro");
  miro2.review_prompt = null;
  miro2.overlaps = [];
  miro2.types = miro2.types.filter((type) => type.type !== "miro.extra");
  writeFamily(root, "miro", miro2);
  rmSync(docPath);

  const removed = run(["--root", root]);
  assert.equal(removed.status, 0, removed.stdout + removed.stderr);
  const afterRemove = JSON.parse(readFileSync(starterShapesPath, "utf8"));
  assert.ok(!afterRemove.accept.some((row) => row.url.includes("/extra/")), "expected the stale row to be gone");

  // --check still passes against the now-clean, regenerated file.
  const checked = run(["--check", "--root", root]);
  assert.equal(checked.status, 0, checked.stdout + checked.stderr);
});

test("--check still catches a hand-edited stale starter-shapes.json row (Codex P2 round)", async (t) => {
  // The write-mode fix above must not weaken --check's own ability to
  // catch a real hand edit to the committed generated file.
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const starterShapesPath = path.join(root, STARTER_SHAPES_JSON);
  const starterShapes = JSON.parse(readFileSync(starterShapesPath, "utf8"));
  starterShapes.types.push("bogus.type");
  starterShapes.accept.push({ note: "Bogus row (docs/adapters/bogus.type.md)", url: "https://example.test/x", shape: "/x" });
  writeFileSync(starterShapesPath, `${JSON.stringify(starterShapes, null, 2)}\n`);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/capture-site\/starter-shapes\.json: row "Bogus row \(docs\/adapters\/bogus\.type\.md\)" names no family type/,
  );
});

test("an unparsable starter-shapes.json is treated as no rows, not a crash (Important 1)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const starterShapesPath = path.join(root, STARTER_SHAPES_JSON);
  writeFileSync(starterShapesPath, "not json");
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  const output = result.stdout + result.stderr;
  assert.match(output, new RegExp(`out of date: ${STARTER_SHAPES_JSON}`));
  assert.doesNotMatch(output, /SyntaxError/);
});

test("a subdomain family missing its suffix reports the validation line without crashing (Important 2)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const sharepoint = readFamily(root, "sharepoint");
  delete sharepoint.origin.suffix;
  writeFamily(root, "sharepoint", sharepoint);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  const output = result.stdout + result.stderr;
  assert.match(output, /protocol\/starters\/sharepoint\.json: origin\.suffix is required/);
  assert.doesNotMatch(result.stderr, /TypeError/);
  assert.doesNotMatch(result.stderr, /at originsCollide/);
});

// --- Task 5 review fix round 2 ---------------------------------------------

test("a family with an unrelated validation problem still gets its origin-collision and shared-origin checks (re-review)", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  // An unrelated top-level problem (an unknown field) that has nothing to
  // do with origin or scope.
  miro.bogus = "unrelated";
  // An exact origin that collides with github.json's, and is also a
  // shared origin (catalog.json shared_origins), while miro's scope stays
  // null: this should still report the collision and the shared-origin
  // scope message, not be silently skipped because of the unrelated
  // problem above (fix round 1 over-corrected by excluding the whole file
  // from every check once it had any problem at all).
  miro.origin = { kind: "exact", origin: "https://github.com" };
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  const output = result.stdout + result.stderr;
  assert.match(output, /protocol\/starters\/miro\.json: unknown field "bogus"/);
  assert.match(output, /protocol\/starters\/miro\.json: origin overlaps protocol\/starters\/github\.json/);
  assert.match(
    output,
    /protocol\/starters\/miro\.json: origin https:\/\/github\.com is a shared origin, so scope must not be null/,
  );
  assert.doesNotMatch(result.stderr, /TypeError/);
});

// --- Task 6: docs/adapters/README.md index, lock append, placeholders -----

test("renderAdaptersIndex gives the jira row and a development row for confluence.page", () => {
  const families = loadFamilies(REPO);
  const current = JSON.parse(readFileSync(path.join(REPO, CATALOG_JSON), "utf8")).starter_types;
  const order = catalogTypeOrder(families, current);
  const rendered = renderAdaptersIndex(families, order);
  assert.match(
    rendered,
    /^<!-- Generated by scripts\/generate-starters\.mjs from protocol\/starters\/\*\.json\. Do not edit\. -->\n# Starter adapters\n/,
  );
  assert.match(rendered, /\| \[jira\.issue\]\(jira\.issue\.md\) \| atlassian \| certified \| live \| 2026-11-02 \| John \|/);
  assert.match(rendered, /\| \[confluence\.page\]\(confluence\.page\.md\) \| atlassian \| development \| — \| — \| — \|/);
});

// A fully-filled-in, one-type, unscoped family on an origin no shipped
// family or test fixture claims, used only to isolate the lock behaviour
// below from every other validation, policy and registry check: the only
// thing missing from a fresh copy of it is its ids.lock.json entry.
const LOCK_TEST_FAMILY = {
  family: "ahalock",
  display_name: "Aha Lock Test",
  review_prompt: null,
  origin: { kind: "exact", origin: "https://ahalock.example" },
  scope: null,
  sample_origin: "https://ahalock.example",
  sample_scope: [],
  overlaps: [],
  types: [
    {
      type: "ahalock.feature",
      key: "ahalock.feature",
      label: "Aha feature",
      label_variants: [],
      description: "An example feature page.",
      record_label: null,
      example_path: null,
      adapter: {
        type: "ahalock.feature",
        match: [{ path: "/features/{id:int}" }],
        id: "ahalock.feature:{host}/{id}",
      },
      history: [],
      status: { state: "development", method: null, checked_on: null, checked_by: null },
      docs: "docs/adapters/ahalock.feature.md",
      samples: [{ note: "Aha feature", url: "https://ahalock.example/features/1", title: null, shape: "/features/{id}" }],
      samples_shape: true,
      vectors: [],
    },
  ],
};

function installLockTestFamily(root) {
  writeFamily(root, "ahalock", LOCK_TEST_FAMILY);
  writeFileSync(path.join(root, "docs", "adapters", "ahalock.feature.md"), "# ahalock.feature starter draft\n\nStatus: development draft.\n");
}

test("write mode appends a missing lock entry and prints it; --check still refuses it", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  installLockTestFamily(root);

  const checkResult = run(["--check", "--root", root]);
  assert.equal(checkResult.status, 1);
  assert.match(checkResult.stdout, /no lock entry for ahalock\.feature/);

  const writeResult = run(["--root", root]);
  assert.equal(writeResult.status, 0, writeResult.stdout + writeResult.stderr);
  assert.match(writeResult.stdout, /appended lock entry ahalock\.feature → ahalock\.feature:\{host\}\/\{id\}/);

  const lock = JSON.parse(readFileSync(path.join(root, "protocol", "starters", "ids.lock.json"), "utf8"));
  assert.equal(lock["ahalock.feature"], "ahalock.feature:{host}/{id}");

  const recheck = run(["--check", "--root", root]);
  assert.equal(recheck.status, 0, recheck.stdout + recheck.stderr);
});

test("an edited lock entry still fails in write mode, not only --check", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  installLockTestFamily(root);
  const lockPath = path.join(root, "protocol", "starters", "ids.lock.json");
  const lock = JSON.parse(readFileSync(lockPath, "utf8"));
  lock["ahalock.feature"] = "ahalock.feature:{host}/{id}/changed";
  writeFileSync(lockPath, JSON.stringify(lock, null, 2));

  const result = run(["--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout, /id template changed for ahalock\.feature/);
  assert.doesNotMatch(result.stdout, /appended lock entry/);
});

test("a \"TODO\" placeholder anywhere in a family file is reported by field path", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  atlassian.types[0].adapter.id = "TODO";
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout, /protocol\/starters\/atlassian\.json: types\[0\]\.adapter\.id is still a placeholder/);
});
