import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import {
  checkLock, familyClaims, loadFamilies, renderCatalogLine, renderTs, validateFamily,
} from "../generate-starters.mjs";
import { ORIGIN_CASES, STRICT_ORIGIN_CASES } from "../../extension/tests/starter-origin-cases.mjs";
import { EXAMPLE_STRICT_FAMILY } from "../../extension/tests/starter-fixtures.mjs";

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const SCRIPT = path.join(REPO, "scripts", "generate-starters.mjs");
const GENERATED_TS = "extension/starter-families.generated.ts";
const CATALOG_JSON = "protocol/capture-site/catalog.json";
const SHAPES_JSON = "protocol/capture-site/shapes.json";
const STARTER_SHAPES_JSON = "protocol/capture-site/starter-shapes.json";
const VECTORS_JSON = "protocol/adapter-templates/vectors.json";
const STARTER_VECTORS_JSON = "protocol/adapter-templates/starter-vectors.json";

const LEGACY_ORDER = ["jira.issue", "confluence.page", "sharepoint.doc", "github.pr", "github.issue", "github.repo", "miro.board"];

function tmpRoot(t) {
  const dir = mkdtempSync(path.join(os.tmpdir(), "generate-starters-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  cpSync(path.join(REPO, "protocol", "starters"), path.join(dir, "protocol", "starters"), { recursive: true });
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

test("--check is clean on the committed tree", () => {
  const result = run(["--check"]);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.stdout.trim(), "");
});

test("a stale generated file fails --check", (t) => {
  const root = tmpRoot(t);
  const tsPath = path.join(root, GENERATED_TS);
  writeFileSync(tsPath, readFileSync(tsPath, "utf8") + "\n// stale\n");
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout, new RegExp(`out of date: ${GENERATED_TS}`));
});

test("an edited ID template fails the lock", (t) => {
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  atlassian.types[0].adapter.id = "jira:{host}/{key}/changed";
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout, /id template changed for jira\.issue/);
});

test("a missing adapter.id names the file and field", (t) => {
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  delete atlassian.types[0].adapter.id;
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: types\[0\]\.adapter\.id is required/);
});

test("an unknown field names the file and field", (t) => {
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  atlassian.types[0].lable = "typo";
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json:.*unknown field "lable"/);
});

test("an invalid JSON file names the file and the parse error", (t) => {
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  writeFileSync(atlassianPath, readFileSync(atlassianPath, "utf8").slice(0, -1) + ",}");
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: invalid JSON: /);
});

test("a duplicate type or key across family files is caught", (t) => {
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

test("a subdomain family's key without {origin} names the file and field", (t) => {
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  atlassian.types[0].key = "jira";
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: types\[0\]\.key must contain "\{origin\}"/);
});

test("adapter.match must be a non-empty array", (t) => {
  const root = tmpRoot(t);
  const atlassianPath = path.join(root, "protocol", "starters", "atlassian.json");
  const atlassian = JSON.parse(readFileSync(atlassianPath, "utf8"));
  atlassian.types[0].adapter.match = [];
  writeFileSync(atlassianPath, JSON.stringify(atlassian, null, 2));
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: types\[0\]\.adapter\.match must be a non-empty array/);
});

test("status.checked_on must be a string or null", (t) => {
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

test("--check tolerates a CRLF checkout of the generated outputs", (t) => {
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

test("a missing samples names the file and field", (t) => {
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  delete miro.types[0].samples;
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/miro\.json: types\[0\]\.samples is required/);
});

test("a sample URL off the family's origin is refused", (t) => {
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

test("samples must be empty when samples_shape is false", (t) => {
  const root = tmpRoot(t);
  const github = readFamily(root, "github");
  github.types[2].samples.push({ url: "https://github.com/acme/widget", title: null, shape: "/acme/widget", note: "x" });
  writeFamily(root, "github", github);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/github\.json: types\[2\]\.samples must be empty when samples_shape is false/,
  );
});

test("samples must not be empty when samples_shape is true", (t) => {
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

test("a stale starter-shapes.json fails --check", (t) => {
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
    url: "https://miro.com/app/board/uXjVM0uuBLo=/",
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

test("origin.labels must be 1", (t) => {
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.origin.labels = 2;
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/atlassian\.json: origin\.labels must be 1/);
});

test("origin.exclude must be a list of lowercase DNS labels", (t) => {
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.origin.exclude = ["WWW"];
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/atlassian\.json: origin\.exclude must be a list of lowercase DNS labels/,
  );
});

test('origin unknown field "labels" on an exact origin', (t) => {
  const root = tmpRoot(t);
  const miro = readFamily(root, "miro");
  miro.origin.labels = 1;
  writeFamily(root, "miro", miro);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(result.stdout + result.stderr, /protocol\/starters\/miro\.json: origin unknown field "labels"/);
});

test("sample_scope must not be empty for a scoped family", (t) => {
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

test("sample_scope must be empty for an unscoped family", (t) => {
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

test("sample_origin must be on the family's own origin", (t) => {
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

test('overlaps[].expect must be "disjoint"', (t) => {
  const root = tmpRoot(t);
  const atlassian = readFamily(root, "atlassian");
  atlassian.overlaps[0].expect = "may_overlap";
  writeFamily(root, "atlassian", atlassian);
  const result = run(["--check", "--root", root]);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout + result.stderr,
    /protocol\/starters\/atlassian\.json: overlaps\[0\]\.expect must be "disjoint"/,
  );
});

test("overlaps[].a and .b must name a type in this family", (t) => {
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

test("every pair of types that is not a within container needs an overlaps entry", (t) => {
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

test("a within container is exempt from the pair-coverage check", (t) => {
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

test('a vector must not carry "doc"', (t) => {
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

test("a vector URL off the family's origin is refused", (t) => {
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

test("a vector id duplicating a legacy vectors.json case is caught", (t) => {
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
