// Unit tests for extension/site-coverage.ts, from dist (npm run build:extension).
import assert from "node:assert/strict";
import test from "node:test";

const { coverageSentence, joinRecordedTypes, siteCoverage } = await import("../dist/site-coverage.js");
const { starterOffers } = await import("../dist/starter-adapters.js");

const adaptersOf = (origin, scope = []) => starterOffers(origin, scope).map((offer) => offer.adapter);
const GITHUB = adaptersOf("https://github.com", ["acme"]);

test("joinRecordedTypes joins without an Oxford comma", () => {
  assert.equal(joinRecordedTypes([]), "");
  assert.equal(joinRecordedTypes(["a"]), "a");
  assert.equal(joinRecordedTypes(["a", "b"]), "a and b");
  assert.equal(joinRecordedTypes(["a", "b", "c"]), "a, b and c");
});

test("GitHub coverage names the record types in record order and the scoped accounts", () => {
  const coverage = siteCoverage(GITHUB, "https://github.com/search?q=work");
  assert.deepEqual(coverage, {
    host: "github.com", types: ["repositories", "pull requests", "issues"], accounts: ["acme"], moreAccounts: 0,
  });
  assert.equal(coverageSentence(coverage, "no_rule"),
    "github.com captures repositories, pull requests and issues for acme; this page isn't one of them.");
  assert.equal(coverageSentence(coverage, "out_of_scope"),
    "github.com captures repositories, pull requests and issues for acme; this page belongs to another account.");
  // Only the adapters present are named.
  const prOnly = siteCoverage(GITHUB.filter((adapter) => adapter.type === "github.pr"), "https://github.com/");
  assert.deepEqual(prOnly.types, ["pull requests"]);
});

test("starter adapters use their starter label; custom adapters use their type", () => {
  const jira = siteCoverage(adaptersOf("https://acme.atlassian.net"), "https://acme.atlassian.net/jira/your-work");
  assert.deepEqual(jira, { host: "acme.atlassian.net", types: ["Jira issues"], accounts: [], moreAccounts: 0 });
  assert.equal(coverageSentence(jira, "no_rule"), "acme.atlassian.net captures Jira issues; this page isn't one of them.");
  assert.equal(coverageSentence(jira, "out_of_scope"), null, "no accounts: keep today's sentence");
  assert.deepEqual(siteCoverage(adaptersOf("https://acme-my.sharepoint.com"), "https://acme-my.sharepoint.com/").types,
    ["OneDrive for work documents"]);
  assert.deepEqual(siteCoverage(adaptersOf("https://acme.sharepoint.com"), "https://acme.sharepoint.com/").types,
    ["SharePoint documents"]);
  assert.deepEqual(siteCoverage(adaptersOf("https://miro.com"), "https://miro.com/app/dashboard/").types, ["Miro boards"]);
  const custom = [
    { type: "example.page", origin: "https://example.com", match: [{ path: "/p/{id:int}" }], id: "example.page:{host}/{id}" },
    { type: "example.page", origin: "https://example.com", match: [{ path: "/q/{id:int}" }], id: "example.q:{host}/{id}" },
  ];
  assert.deepEqual(siteCoverage(custom, "https://example.com/about").types, ["example.page"], "deduplicated");
  // A starter-typed adapter whose ID is not the starter's is custom.
  const [jiraAdapter] = adaptersOf("https://acme.atlassian.net");
  assert.deepEqual(siteCoverage([{ ...jiraAdapter, id: "mine:{host}/{key}" }], "https://acme.atlassian.net/").types,
    ["jira.issue"]);
});

test("coverage reads only adapters on the page's origin", () => {
  assert.equal(siteCoverage(GITHUB, "https://example.com/"), null);
  assert.equal(siteCoverage([], "https://github.com/"), null);
  assert.equal(siteCoverage(GITHUB, "not a url"), null);
  assert.equal(siteCoverage([{ origin: "https://github.com" }], "https://github.com/"), null, "no type, no coverage");
});

test("more than three accounts show the first three and a count", () => {
  const coverage = siteCoverage(adaptersOf("https://github.com", ["a", "b", "c", "d", "e"]), "https://github.com/");
  assert.deepEqual(coverage.accounts, ["a", "b", "c"]);
  assert.equal(coverage.moreAccounts, 2);
  assert.equal(coverageSentence(coverage, "no_rule"),
    "github.com captures repositories, pull requests and issues for a, b, c and 2 more; this page isn't one of them.");
});

test("accounts union across scoped adapters and are dropped when any adapter is unscoped", () => {
  const [pr, issue, repo] = GITHUB;
  const union = siteCoverage([pr, { ...issue, scope: { owner: ["Beta", "ACME"] } }, repo], "https://github.com/");
  assert.deepEqual(union.accounts, ["acme", "beta"]);
  const mixed = [pr, { ...issue, scope: undefined, unscoped: "acknowledged" }, repo];
  const coverage = siteCoverage(mixed, "https://github.com/");
  assert.deepEqual(coverage.accounts, []);
  assert.equal(coverageSentence(coverage, "no_rule"),
    "github.com captures repositories, pull requests and issues; this page isn't one of them.");
  assert.equal(coverageSentence(coverage, "out_of_scope"), null);
});
