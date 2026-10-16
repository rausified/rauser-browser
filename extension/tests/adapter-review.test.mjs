import assert from "node:assert/strict";
import test from "node:test";

const {
  exampleUrl,
  newAdapter,
  overlapText,
  parseAdapterJson,
  previewDocument,
  probeUrls,
} = await import("../dist/adapter-review.js");

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
