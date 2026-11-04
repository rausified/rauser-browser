import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { checkLock, loadFamilies, renderCatalogLine, renderTs, validateFamily } from "../generate-starters.mjs";
import { ipcDenied } from "./ipc-guard.mjs";

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const SCRIPT = path.join(REPO, "scripts", "generate-starters.mjs");
const GENERATED_TS = "extension/starter-families.generated.ts";
const CATALOG_JSON = "protocol/capture-site/catalog.json";

const LEGACY_ORDER = ["jira.issue", "confluence.page", "sharepoint.doc", "github.pr", "github.issue", "github.repo", "miro.board"];

function tmpRoot(t) {
  const dir = mkdtempSync(path.join(os.tmpdir(), "generate-starters-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  cpSync(path.join(REPO, "protocol", "starters"), path.join(dir, "protocol", "starters"), { recursive: true });
  cpSync(path.join(REPO, CATALOG_JSON), path.join(dir, CATALOG_JSON));
  cpSync(path.join(REPO, GENERATED_TS), path.join(dir, GENERATED_TS));
  return dir;
}

function run(args, cwd = REPO) {
  return spawnSync(process.execPath, [SCRIPT, ...args], { cwd, encoding: "utf8" });
}

test("loads the four shipped families with seven types in legacy catalog order", () => {
  const families = loadFamilies(REPO);
  assert.deepEqual(families.map((family) => family.family), ["atlassian", "github", "miro", "sharepoint"]);
  const types = families.flatMap((family) => family.types.map((type) => type.type));
  assert.equal(types.length, 7);
  const current = JSON.parse(readFileSync(path.join(REPO, CATALOG_JSON), "utf8")).starter_types;
  // Find the line by content, not a hard-coded index: the index breaks
  // whenever catalog.json gains a line, and splitting on "\n" alone leaves a
  // trailing "\r" on a CRLF checkout (Windows CI).
  const catalogLines = readFileSync(path.join(REPO, CATALOG_JSON), "utf8").split(/\r?\n/);
  const line = catalogLines.find((candidate) => candidate.includes('"starter_types"'));
  assert.ok(line, "catalog.json has no starter_types line");
  assert.equal(renderCatalogLine(families, current), line);
  assert.deepEqual(current, LEGACY_ORDER);
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
  edited[0].types[0].adapter.id = "changed";
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
  const catalogOrdered = renderTs(families, LEGACY_ORDER);
  assert.equal(catalogOrdered, readFileSync(path.join(REPO, GENERATED_TS), "utf8").replace(/\r\n/g, "\n"));
});

test("STARTER_FAMILY_IDS follows catalog order", () => {
  const families = loadFamilies(REPO);
  const rendered = renderTs(families, LEGACY_ORDER);
  assert.match(rendered, /export const STARTER_FAMILY_IDS = \["atlassian", "sharepoint", "github", "miro"\] as const;/);
});

test("--check tolerates a CRLF checkout of the generated outputs", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const root = tmpRoot(t);
  const tsPath = path.join(root, GENERATED_TS);
  const catalogPath = path.join(root, CATALOG_JSON);
  writeFileSync(tsPath, readFileSync(tsPath, "utf8").replace(/\n/g, "\r\n"));
  writeFileSync(catalogPath, readFileSync(catalogPath, "utf8").replace(/\n/g, "\r\n"));
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

test("display_name is required", (t) => {
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  delete atlassian.display_name;
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: display_name is required/);
});

test("review_prompt must be a string when a family has more than one type or a scope", (t) => {
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.review_prompt = null;
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr,
    /protocol\/starters\/atlassian\.json: review_prompt must be a string when a family has more than one type or a scope/);
});

test('scope.kind must be "github_org", "tenant_name" or "unscoped"', (t) => {
  const root = tmpRoot(t);
  const github = readFamily(root, "github");
  github.scope = { kind: "oauth" };
  writeFamily(root, "github", github);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr,
    /protocol\/starters\/github\.json: scope\.kind must be "github_org", "tenant_name" or "unscoped"/);
});

test("scope.max must be an integer from 1 to 32", (t) => {
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.scope = { kind: "tenant_name", max: 0, label: "Workspaces", placeholder: "acme" };
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: scope\.max must be an integer from 1 to 32/);
});

test("an unscoped family's marker and {scope} example_path are both caught", (t) => {
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

test('an unscoped family requires adapter.unscoped to be "acknowledged"', (t) => {
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.scope = { kind: "unscoped", notice: "x" };
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr,
    /protocol\/starters\/miro\.json: types\[0\]\.adapter\.unscoped must be "acknowledged" for an unscoped family/);
});

test('a scoped family requires a {"$scope": true} marker on every type', (t) => {
  const root = tmpRoot(t);
  const github = readFamily(root, "github");
  delete github.types[0].adapter.scope;
  writeFamily(root, "github", github);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr,
    /protocol\/starters\/github\.json: types\[0\]\.adapter needs a \{"\$scope": true\} marker for a scoped family/);
});

test('example_path must start with "/"', (t) => {
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.types[0].example_path = "browse/PROJ-42";
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr,
    /protocol\/starters\/atlassian\.json: types\[0\]\.example_path must start with "\/"/);
});

test("the test-only tenant_name and unscoped fixtures pass the generator's own family validation", async () => {
  // extension/tests/starter-fixtures.mjs stands in for a protocol/starters/
  // family file in panel, gallery and capture-site tests. Running it through
  // the same validateFamily the generator applies to every real family file
  // means it cannot quietly drift out of the file format (as
  // EXAMPLE_UNSCOPED_FAMILY once did with a null review_prompt on a scoped
  // family) without a test catching it.
  const { EXAMPLE_TENANT_FAMILY, EXAMPLE_UNSCOPED_FAMILY } =
    await import("../../extension/tests/starter-fixtures.mjs");
  for (const fixture of [EXAMPLE_TENANT_FAMILY, EXAMPLE_UNSCOPED_FAMILY]) {
    const problems = [];
    validateFamily(problems, "<fixture>", fixture);
    assert.deepEqual(problems, [], fixture.family);
  }
});
