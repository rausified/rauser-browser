import assert from "node:assert/strict";
import test from "node:test";

const { captureOrigin, planCaptureSite } = await import("../dist/capture-site.js");
const { compileDocument, evaluate } = await import("../dist/adapter-templates.js");
const { starterOffers } = await import("../dist/starter-adapters.js");
const CONFIG = {
  storage: { root: "/vault" }, capture_enabled: false,
  adapters: { format: 1, artifacts: [] },
};
const BOTH = { organizations: "Acme-Corp", repositories: true, pullRequests: true, issues: true };
const plan = (url, config = CONFIG, choice = BOTH) => {
  const result = planCaptureSite(config, url, choice);
  assert.equal(result.ok, true, result.message);
  return result.plan;
};
const match = (plan, url) => {
  const result = compileDocument(plan.document);
  assert.equal(result.ok, true);
  return evaluate(result.doc, url);
};

test("Jira and OneDrive start from the current URL and retain starter identities", () => {
  for (const [url, id] of [
    ["https://acme.atlassian.net/browse/PROJ-42", "jira:acme.atlassian.net/PROJ-42"],
    ["https://acme.atlassian.net/jira/software/projects/PROJ/boards/7?selectedIssue=PROJ-42", "jira:acme.atlassian.net/PROJ-42"],
    ["https://acme.sharepoint.com/sites/work/_layouts/15/Doc.aspx?sourcedoc=%7B123e4567-e89b-12d3-a456-426614174000%7D", "sharepoint:acme.sharepoint.com/123e4567-e89b-12d3-a456-426614174000"],
    ["https://acme-my.sharepoint.com/personal/person/_layouts/15/Doc.aspx?sourcedoc=123e4567-e89b-12d3-a456-426614174000", "sharepoint:acme-my.sharepoint.com/123e4567-e89b-12d3-a456-426614174000"],
  ]) {
    const result = plan(url);
    assert.equal(result.current.result.artifact_id, id);
    assert.equal(result.startsCapture, true);
    assert.equal(result.probes.length, 27);
    assert.ok(result.probes.every((row) => row.probe && !row.result.match));
    assert.ok(result.examples.every((row) => !row.probe && row.result.match));
    assert.deepEqual(result.document.artifacts, starterOffers(new URL(url).origin, []).map((offer) => offer.adapter));
  }
});

test("one organization field scopes every checked GitHub type", () => {
  const result = plan("https://github.com/acme-corp/widget/pull/12/files", CONFIG,
    { organizations: "Acme-Corp", repositories: true, pullRequests: true, issues: true });
  assert.equal(result.current.result.mode, "review");
  for (const adapter of result.document.artifacts) assert.deepEqual(adapter.scope, { owner: ["acme-corp"] });
  assert.equal(match(result, "https://github.com/acme-corp/widget/pull/12").match, true);
  assert.equal(match(result, "https://github.com/acme-corp/widget/issues/12").match, true);
  assert.equal(match(result, "https://github.com/acme-corp/widget/tree/main").match, true);
  assert.equal(match(result, "https://github.com/other-org/widget/issues/12").match, false);
  assert.equal(match(result, "https://github.com/personal/widget/issues/12").match, false);
  assert.ok(result.examples.every((row) => row.result.match));
  const issueOnly = plan("https://github.com/acme-corp/widget/pull/12", CONFIG,
    { organizations: "acme-corp", repositories: false, pullRequests: false, issues: true });
  assert.equal(issueOnly.document.artifacts.length, 1);
  assert.equal(issueOnly.current.result.match, false);
});

test("capture plan puts PR and issue before repo", () => {
  const result = plan("https://github.com/acme-corp/widget/pull/12/files", CONFIG,
    { organizations: "acme-corp", repositories: true, pullRequests: true, issues: true });
  assert.deepEqual(result.document.artifacts.map((adapter) => adapter.type), ["github.pr", "github.issue", "github.repo"]);
  assert.equal(result.current.result.type, "github.pr");
  assert.deepEqual(result.current.result.within, { type: "github.repo", artifact_id: "github.repo:github.com/acme-corp/widget" });
  assert.equal(match(result, "https://github.com/acme-corp/widget/issues/3").type, "github.issue");
  assert.equal(match(result, "https://github.com/acme-corp/widget/tree/main").type, "github.repo");
  assert.ok(result.probes.every((row) => !row.result.match));
  assert.deepEqual(result.examples.map((row) => row.result.type), ["github.pr", "github.issue", "github.repo"]);
});

test("unchecking repositories leaves pull requests and issues unlinked", () => {
  const result = plan("https://github.com/acme-corp/widget/pull/12", CONFIG,
    { organizations: "acme-corp", repositories: false, pullRequests: true, issues: true });
  assert.deepEqual(result.document.artifacts.map((adapter) => adapter.type), ["github.pr", "github.issue"]);
  for (const adapter of result.document.artifacts) assert.equal(adapter.within, undefined);
  assert.equal(result.current.result.type, "github.pr");
  assert.equal(result.current.result.within ?? null, null);
});

test("empty, invalid, or unselected GitHub scope cannot produce a save", () => {
  for (const choice of [
    { organizations: "", repositories: true, pullRequests: false, issues: false },
    { organizations: "acme", repositories: false, pullRequests: false, issues: false },
    { organizations: "bad/org", repositories: false, pullRequests: true, issues: false },
    { organizations: "*", repositories: false, pullRequests: true, issues: false },
  ]) assert.equal(planCaptureSite(CONFIG, "https://github.com/acme/repo/pull/1", choice).ok, false);
});

test("unknown, insecure, lookalike and credential-bearing sites stay outside starter setup", () => {
  for (const url of [
    "https://work.example/items/3", "http://acme.atlassian.net/browse/X-1",
    "https://github.com.evil.example/acme/repo/pull/1", "chrome://newtab", "not a URL",
    "https://user:secret@acme.atlassian.net/browse/ABC-1",
  ]) {
    assert.equal(captureOrigin(url), null);
    assert.equal(planCaptureSite(CONFIG, url, BOTH).ok, false);
  }
});

test("existing site policy is never replaced or supplemented, including custom definitions", () => {
  const artifacts = [{ type: "custom.work", origin: "https://github.com", match: [{ path: "/items/{id:int}" }], id: "custom:{host}/{id}", unscoped: true }];
  const config = { ...CONFIG, adapters: { format: 1, artifacts } };
  assert.match(planCaptureSite(config, "https://github.com/acme/repo/issues/1", BOTH).message, /already has capture rules/);
  assert.deepEqual(config.adapters.artifacts, artifacts);
});

test("adding a site preserves other definitions and an intentional global pause", () => {
  const config = structuredClone(CONFIG);
  config.adapters.artifacts = starterOffers("https://first.atlassian.net", []).map((offer) => offer.adapter);
  const before = structuredClone(config);
  const result = plan("https://second.atlassian.net/browse/ABC-1", config);
  assert.equal(result.startsCapture, false);
  assert.deepEqual(result.document.artifacts[0], config.adapters.artifacts[0]);
  assert.equal(plan("https://second.atlassian.net/browse/ABC-1", { ...config, capture_enabled: true }).startsCapture, true);
  assert.deepEqual(config, before);
  assert.equal(planCaptureSite({ ...CONFIG, storage: null }, "https://second.atlassian.net", BOTH).ok, false);
});

test("a dashboard is explicitly skipped without preventing useful site setup", () => {
  const result = plan("https://acme.atlassian.net/dashboard");
  assert.equal(result.current.result.match, false);
  assert.ok(result.examples[0].result.match);
});

test("invalid future documents and the whole-document adapter limit refuse a plan", () => {
  const future = { ...CONFIG, adapters: { format: 999, artifacts: [] } };
  assert.equal(planCaptureSite(future, "https://acme.atlassian.net", BOTH).ok, false);
  const artifacts = Array.from({ length: 64 }, (_, n) => starterOffers(`https://work${n}.atlassian.net`, [])[0].adapter);
  assert.equal(planCaptureSite({ ...CONFIG, adapters: { format: 1, artifacts } }, "https://acme.atlassian.net", BOTH).ok, false);
});
