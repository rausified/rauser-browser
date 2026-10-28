import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const {
  exampleUrl,
  newAdapter,
  overlapText,
  parseAdapterJson,
  previewDocument,
  probeUrls,
  stripParamConflicts,
  stripsParam,
} = await import("../dist/adapter-review.js");

const catalog = JSON.parse(await readFile(new URL("../../protocol/capture-site/catalog.json", import.meta.url)));

const JIRA = {
  type: "jira.issue",
  origin: "https://acme.atlassian.net",
  match: [
    { path: "/browse/{key:jira_key}" },
    { path: "/jira/{*}", query: { selectedIssue: "{key:jira_key}" } },
  ],
  id: "jira:{host}/{key}",
  title: "[{key:jira_key}] {title} - Jira",
};

test("adapter review parses one adapter and gives actionable validation errors", () => {
  const valid = parseAdapterJson(JSON.stringify(JIRA));
  assert.equal(valid.ok, true);

  const syntax = parseAdapterJson("{");
  assert.equal(syntax.ok, false);
  assert.match(syntax.message, /not valid JSON/);

  const missing = parseAdapterJson(JSON.stringify({ type: "jira.issue" }));
  assert.equal(missing.ok, false);
  assert.match(missing.message, /missing a required setting/i);
  assert.match(missing.message, /adapter 1/i);
});

test("adapter review builds useful example URLs without exposing real activity", () => {
  assert.equal(exampleUrl(JIRA), "https://acme.atlassian.net/browse/ABC-123");
  assert.deepEqual(newAdapter("https://work.example"), {
    type: "work.item",
    origin: "https://work.example",
    match: [{ path: "/items/{id:int}" }],
    id: "work.item:{host}/{id}",
  });
});

test("generated examples satisfy bounded capture types", () => {
  const bounded = {
    type: "work.item",
    origin: "https://work.example",
    match: [{
      path: "/items/{id:token(20,64)}/{page:int(1,2)}/{owner:name(16)}/{project:jira_key(5,8)}/{tail:hex(32)@tail}",
    }],
    id: "work.item:{host}/{id}/{page}/{owner}/{project}/{tail}",
  };
  const url = exampleUrl(bounded);
  const preview = previewDocument({ format: 1, artifacts: [bounded] }, url, "", false);
  assert.equal(preview.ok, true);
  assert.equal(preview.rows[0].result.match, true, url);

  const gdoc = {
    type: "gdoc",
    origin: "https://docs.google.com",
    unscoped: "acknowledged",
    match: [{ path: "/document/d/{id:token(20,64)}/{mode?:(edit|view|preview)}" }],
    modes: { edit: "edit", view: "view", preview: "view" },
    id: "gdoc:{host}/{id}",
  };
  const gdocUrl = exampleUrl(gdoc);
  const gdocPreview = previewDocument({ format: 1, artifacts: [gdoc] }, gdocUrl, "", false);
  assert.equal(gdocPreview.ok, true);
  assert.equal(gdocPreview.rows[0].result.match, true, gdocUrl);
});

test("match preview reports IDs and flags a fixed non-artifact probe match", () => {
  const normal = previewDocument(
    { format: 1, artifacts: [JIRA] },
    "https://acme.atlassian.net/browse/proj-42",
    "[PROJ-42] Repair export - Jira",
    true,
    JIRA.origin,
  );
  assert.equal(normal.ok, true);
  const example = normal.rows.find((row) => !row.probe);
  assert.equal(example.result.match, true);
  assert.equal(example.result.artifact_id, "jira:acme.atlassian.net/PROJ-42");
  assert.equal(example.result.title, "Repair export");
  assert.equal(normal.rows.some((row) => row.probe && row.result.match), false);

  const searchAdapter = {
    type: "work.search",
    origin: "https://work.example",
    match: [{ path: "/search", query: { q: "{id:token}" } }],
    id: "work.search:{host}/{id}",
  };
  const risky = previewDocument(
    { format: 1, artifacts: [searchAdapter] }, "", "", true, searchAdapter.origin,
  );
  assert.equal(risky.ok, true);
  assert.equal(risky.rows.some((row) => row.url === "https://work.example/search?q=x" && row.result.match), true);
});

test("probe and overlap copy names the concrete review evidence", () => {
  assert.ok(probeUrls("https://work.example").includes("https://work.example/login"));
  assert.ok(probeUrls("https://work.example").includes("https://work.example/oauth"));
  assert.equal(overlapText(
    { first_adapter: 0, first_rule: 1, second_adapter: 1, second_rule: 0 },
    [JIRA, { ...JIRA, type: "jira.board" }],
  ), "jira.issue rule 2 may overlap jira.board rule 1; jira.issue wins.");
});

test("probeUrls matches the host catalog", () => {
  assert.deepEqual(
    probeUrls("https://x.test"),
    catalog.probe_paths.map((path) => `https://x.test${path}`),
  );
});

test("strip_params entries match query keys as the shared vectors say", async () => {
  const { readFile } = await import("node:fs/promises");
  const vectors = JSON.parse(await readFile(new URL("../../protocol/strip-params/vectors.json", import.meta.url)));
  assert.ok(vectors.cases.length > 0);
  for (const { entry, param, strips } of vectors.cases) {
    assert.equal(stripsParam(entry, param), strips, `${entry} vs ${param}`);
  }
});

test("strip_params conflicts name each entry an adapter's query or absent keys read", () => {
  const list = {
    type: "work.list", origin: "https://work.example",
    match: [{ path: "/lists/{id:int}", absent: ["q"] }], id: "list:{host}/{id}",
  };
  const document = { format: 1, artifacts: [JIRA, list, { match: "broken" }] };
  assert.deepEqual(stripParamConflicts(["utm_*", "fbclid"], document), []);
  assert.deepEqual(stripParamConflicts(["Q", "utm_*", "selected*"], document), [
    { entry: "Q", params: ["q"], adapters: ["work.list on https://work.example"] },
    { entry: "selected*", params: ["selectedIssue"], adapters: ["jira.issue on https://acme.atlassian.net"] },
  ]);
});
