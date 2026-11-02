// Unit tests for extension/site-coverage.ts, from dist (npm run build:extension).
import assert from "node:assert/strict";
import test from "node:test";

const { coverageSentence, joinRecordedTypes, nearMiss, siteCoverage } = await import("../dist/site-coverage.js");
const { compileDocument, evaluate } = await import("../dist/adapter-templates.js");
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

// Near-miss detection runs only after the full document returned no_rule.
const GUID = "%7B8BAF1543-CDBF-4003-A48F-D4A939774FC0%7D";
const SOURCEDOC = { sourcedoc: "{doc:uuid}" };
const SHARE_RULE = { path: "/(:w:|:x:|:p:)/(r|g)/(sites|personal)/{_}/_layouts/15/Doc.aspx", query: SOURCEDOC };
const PERSONAL_RULE = { path: "/personal/{_}/_layouts/15/Doc.aspx", query: SOURCEDOC };
const staleOneDrive = (origin = "https://cisco-my.sharepoint.com") => ({
  type: "sharepoint.doc", origin, path_case: "insensitive",
  match: [
    { path: "/sites/{_}/_layouts/15/Doc.aspx", query: SOURCEDOC },
    { path: "/:w:/r/sites/{_}/_layouts/15/Doc.aspx", query: SOURCEDOC },
    PERSONAL_RULE,
  ],
  id: "sharepoint:{host}/{doc}",
});
const narrowed = () => ({ ...staleOneDrive("https://acme.sharepoint.com"), match: [staleOneDrive().match[0]] });
const CUSTOM_DOC = {
  type: "example.doc", origin: "https://docs.example", match: [{ path: "/a/Doc.aspx", query: SOURCEDOC }], id: "example.doc:{host}/{doc}",
};

function noRule(artifacts, url) {
  const compiled = compileDocument({ format: 1, artifacts });
  assert.equal(compiled.ok, true);
  assert.equal(evaluate(compiled.doc, url).reason, "no_rule", url);
  return nearMiss(artifacts, url);
}

test("a stale OneDrive starter is one current rule short of a sharing link", () => {
  const other = { type: "work.item", origin: "https://a.com", match: [{ path: "/i/{id:int}" }], id: "work.item:{host}/{id}" };
  const url = `https://cisco-my.sharepoint.com/:w:/r/personal/u/_layouts/15/Doc.aspx?sourcedoc=${GUID}&file=x.docx`;
  assert.deepEqual(noRule([other, staleOneDrive()], url),
    { kind: "starter_rule", adapter: 1, label: "OneDrive for work documents", rule: SHARE_RULE });
});

test("a hand-narrowed adapter is offered exactly the starter rule for the page", () => {
  const url = `https://acme.sharepoint.com/personal/u/_layouts/15/Doc.aspx?sourcedoc=${GUID}`;
  assert.deepEqual(noRule([narrowed()], url),
    { kind: "starter_rule", adapter: 0, label: "SharePoint documents", rule: PERSONAL_RULE });
});

test("a starter rule the adapter's own scope rejects is not a near-miss", () => {
  const [pr] = starterOffers("https://github.com", ["acme"]);
  const handWritten = {
    type: "github.pr", origin: "https://github.com", scope: { owner: ["acme"] },
    match: [{ path: "/{owner:name|lower}/{repo:name|lower}/pull/{number:int}" }],
    id: "github.pr:{host}/{owner}/{repo}/{number}",
  };
  assert.deepEqual(noRule([handWritten], "https://github.com/acme/widget/pull/4/files"),
    { kind: "starter_rule", adapter: 0, label: "GitHub pull requests", rule: pr.adapter.match[0] });
  assert.equal(noRule([handWritten], "https://github.com/other/widget/pull/4/files"), null);
});

test("a fixed-shape query identifier on another route is explained, with no rule", () => {
  assert.deepEqual(noRule([CUSTOM_DOC], `https://docs.example/b/Doc.aspx?sourcedoc=${GUID}`),
    { kind: "identifier", adapter: 0, label: "example.doc", key: "sourcedoc" });
  const hex = { ...CUSTOM_DOC, match: [{ path: "/a", query: { id: "{doc:hex(32)}" } }] };
  assert.deepEqual(noRule([hex], `https://docs.example/b?id=${"a".repeat(32)}`),
    { kind: "identifier", adapter: 0, label: "example.doc", key: "id" });
  // A short or variable hex, or a plain token, is not a fixed shape.
  for (const ctype of ["hex(8)", "hex(32,40)", "token", "int"]) {
    const loose = { ...CUSTOM_DOC, match: [{ path: "/a", query: { id: `{doc:${ctype}}` } }] };
    assert.equal(noRule([loose], `https://docs.example/b?id=${"a".repeat(32)}`), null, ctype);
  }
});

test("pages with no, a bad, or two identifiers, and path-only pages, are not near-misses", () => {
  for (const url of [
    "https://acme.sharepoint.com/personal/u/_layouts/15/Doc.aspx",
    "https://acme.sharepoint.com/personal/u/_layouts/15/Doc.aspx?sourcedoc=not-a-guid",
    "https://acme.sharepoint.com/personal/u/_layouts/15/Doc.aspx?file=x.docx",
    `https://acme.sharepoint.com/personal/u/_layouts/15/Doc.aspx?sourcedoc=${GUID}&sourcedoc=${GUID}`,
  ]) assert.equal(noRule([narrowed()], url), null, url);
  for (const url of [
    "https://docs.example/b/Doc.aspx",
    "https://docs.example/b/Doc.aspx?sourcedoc=nope",
    `https://docs.example/b/Doc.aspx?sourcedoc=${GUID}&sourcedoc=${GUID}`,
  ]) assert.equal(noRule([CUSTOM_DOC], url), null, url);
  assert.equal(noRule(GITHUB, "https://github.com/search?q=work"), null, "a single-segment GitHub page");
  assert.equal(nearMiss(GITHUB, "https://example.com/"), null, "another origin");
});
