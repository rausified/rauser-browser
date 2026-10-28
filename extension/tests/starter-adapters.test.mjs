import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const { compileDocument, evaluate } = await import("../dist/adapter-templates.js");
const {
  githubOwnerFromUrl,
  hasAdapter,
  orderContainedFirst,
  parseGitHubOrganizations,
  starterKind,
  starterOffers,
  withContainerLinks,
  containerLinks,
} = await import("../dist/starter-adapters.js");

const vectors = JSON.parse(await readFile(new URL("../../protocol/adapter-templates/vectors.json", import.meta.url)));
const recorded = vectors.fixtures.starters.artifacts;
const catalog = JSON.parse(await readFile(new URL("../../protocol/capture-site/catalog.json", import.meta.url)));

function compiled(offers) {
  const artifacts = offers.map((offer) => offer.adapter);
  const result = compileDocument({ format: 1, artifacts });
  assert.equal(result.ok, true, result.ok ? "" : JSON.stringify(result.error));
  return result.doc;
}

test("generated starters stay identical to the shared cross-engine fixtures", () => {
  const generated = [
    ...starterOffers("https://acme.atlassian.net", []),
    ...starterOffers("https://github.com", ["acme-corp"]),
    ...starterOffers("https://acme.sharepoint.com", []),
    ...starterOffers("https://acme-my.sharepoint.com", []),
  ].map((offer) => offer.adapter);
  const expectedTypes = new Set(["jira.issue", "github.pr", "github.issue", "github.repo", "sharepoint.doc"]);
  const expected = recorded.filter((adapter) => expectedTypes.has(adapter.type));
  assert.deepEqual(generated, expected);
});

test("GitHub offers pull requests, issues, and repositories, each inside its repository", () => {
  const offers = starterOffers("https://github.com", ["acme-corp"]);
  assert.deepEqual(offers.map((offer) => offer.key), ["github.pr", "github.issue", "github.repo"]);
  const repo = offers[2];
  assert.equal(repo.label, "GitHub repositories");
  assert.equal(repo.description, "Every page inside a repository counts toward it; pull requests and issues count toward both.");
  assert.equal(repo.needs_scope, true);
  assert.equal("title" in repo.adapter, false);
  assert.equal("refs" in repo.adapter, false);
  for (const offer of offers.slice(0, 2)) {
    assert.deepEqual(offer.adapter.within, { type: "github.repo", id: repo.adapter.id });
  }
  assert.equal(starterOffers("https://github.com", [])[2].adapter, null);
  const doc = compiled(offers);
  const page = evaluate(doc, "https://github.com/acme-corp/widgets/blob/main/README.md");
  assert.equal(page.artifact_id, "github.repo:github.com/acme-corp/widgets");
  const pr = evaluate(doc, "https://github.com/acme-corp/widgets/pull/7");
  assert.deepEqual(pr.within, { type: "github.repo", artifact_id: page.artifact_id });
});

const adapter = (type, origin = "https://github.com", within = undefined) =>
  ({ type, origin, ...(within ? { within: { type: within, id: `${within}:{host}` } } : {}) });

test("orderContainedFirst moves a later PR adapter ahead of the repo catch-all", () => {
  const jira = adapter("jira.issue", "https://acme.atlassian.net");
  const repo = adapter("github.repo");
  const pr = adapter("github.pr", "https://github.com", "github.repo");
  const issue = adapter("github.issue", "https://github.com", "github.repo");
  assert.deepEqual(orderContainedFirst([jira, repo, pr]), [jira, pr, repo]);
  assert.deepEqual(orderContainedFirst([repo, issue, pr]), [issue, pr, repo]);
  // Nested containers pull their whole chain ahead of the outermost one.
  const org = adapter("github.org");
  const nestedRepo = adapter("github.repo", "https://github.com", "github.org");
  assert.deepEqual(orderContainedFirst([org, nestedRepo, pr]), [pr, nestedRepo, org]);
});

test("orderContainedFirst is stable and leaves other origins alone", () => {
  const repo = adapter("github.repo");
  const pr = adapter("github.pr", "https://github.com", "github.repo");
  const custom = adapter("custom.page");
  const elsewhereRepo = adapter("github.repo", "https://github.example.com");
  const elsewherePr = adapter("github.pr", "https://github.example.com", "github.repo");
  const ordered = [pr, custom, repo, elsewhereRepo, elsewherePr];
  assert.deepEqual(orderContainedFirst(ordered), [pr, custom, repo, elsewherePr, elsewhereRepo]);
  // A PR on another origin never moves ahead of this origin's repo.
  assert.deepEqual(orderContainedFirst([repo, custom, elsewherePr]), [repo, custom, elsewherePr]);
  const inOrder = [pr, repo, custom];
  const result = orderContainedFirst(inOrder);
  assert.deepEqual(result, inOrder);
  assert.notEqual(result, inOrder);
  // A within cycle cannot be satisfied; its members keep their order.
  const a = adapter("x.a", "https://github.com", "x.b");
  const b = adapter("x.b", "https://github.com", "x.a");
  assert.deepEqual(orderContainedFirst([a, b]), [a, b]);
  assert.deepEqual(orderContainedFirst([]), []);
});

test("starter origins are classified narrowly", () => {
  assert.equal(starterKind("https://acme.atlassian.net"), "jira");
  assert.equal(starterKind("https://atlassian.net"), null);
  assert.equal(starterKind("http://acme.atlassian.net"), null);
  assert.equal(starterKind("https://acme.sharepoint.com"), "sharepoint");
  assert.equal(starterKind("https://acme-my.sharepoint.com"), "sharepoint");
  assert.equal(starterKind("http://acme.sharepoint.com"), null);
  assert.equal(starterKind("https://github.com"), "github");
  assert.equal(starterKind("http://github.com"), null);
  assert.equal(starterKind("https://github.com.evil.example"), null);
  assert.deepEqual(starterOffers("https://example.com", []), []);
  assert.equal(starterOffers("https://acme.atlassian.net", []).length, 1);
});

test("GitHub organization scopes normalize, deduplicate, and reject unsafe forms", () => {
  assert.deepEqual(parseGitHubOrganizations(" Acme-Corp, partner, ACME-CORP "), ["acme-corp", "partner"]);
  assert.equal(parseGitHubOrganizations(""), null);
  assert.equal(parseGitHubOrganizations("-leading"), null);
  assert.equal(parseGitHubOrganizations("trailing-"), null);
  assert.equal(parseGitHubOrganizations("name with space"), null);
});

test("githubOwnerFromUrl pre-fills a confident owner and nothing else", () => {
  assert.equal(githubOwnerFromUrl("https://github.com/Cisco-SBG/x/blob/main/a.md"), "cisco-sbg");
  assert.equal(githubOwnerFromUrl("https://github.com/orgs/cisco-sbg/repositories"), "cisco-sbg");
  assert.equal(githubOwnerFromUrl("https://github.com/rausified/dotfiles"), "rausified");
  assert.equal(githubOwnerFromUrl("https://github.com/settings/profile"), null);
  assert.equal(githubOwnerFromUrl("https://github.com/"), null);
  assert.equal(githubOwnerFromUrl("https://github.com/orgs"), null);
  assert.equal(githubOwnerFromUrl("https://example.com/cisco-sbg"), null);
  assert.equal(githubOwnerFromUrl("not a URL"), null);
  for (const route of [
    "notifications", "search", "explore", "marketplace", "topics", "trending", "collections",
    "sponsors", "login", "logout", "signup", "new", "pulls", "issues", "codespaces", "dashboard",
    "features", "enterprise", "pricing", "about", "apps", "users", "account", "stars", "watching",
    "organizations", "enterprises", "copilot", "sessions", "join", "security", "settings",
    "readme", "team", "customer-stories", "solutions", "resources", "events", "site", "home",
    "mobile", "nonprofit", "education", "partners",
  ]) assert.equal(githubOwnerFromUrl(`https://github.com/${route}`), null, route);
  assert.equal(githubOwnerFromUrl("https://github.com/-leading"), null);
  assert.equal(githubOwnerFromUrl("https://github.com/trailing-"), null);
});

test("an installed GitHub starter is recognized before scope is typed", () => {
  const installed = starterOffers("https://github.com", ["acme-corp"])[0].adapter;
  const unscopedOffer = starterOffers("https://github.com", [])[0];
  assert.equal(unscopedOffer.adapter, null);
  assert.equal(hasAdapter([installed], unscopedOffer), true);
});

test("Jira draft joins issue pages and board modals under one canonical ID", () => {
  const doc = compiled(starterOffers("https://acme.atlassian.net", []));
  const page = evaluate(doc, "https://acme.atlassian.net/browse/PROJ-42", "[PROJ-42] Repair export - Jira");
  const modal = evaluate(doc,
    "https://acme.atlassian.net/jira/software/projects/PROJ/boards/7?selectedIssue=PROJ-42");
  assert.equal(page.match, true);
  assert.equal(modal.match, true);
  assert.equal(page.artifact_id, "jira:acme.atlassian.net/PROJ-42");
  assert.equal(modal.artifact_id, page.artifact_id);
  assert.equal(evaluate(doc, "https://acme.atlassian.net/issues/?jql=project%3DPROJ").match, false);
});

test("SharePoint and OneDrive drafts key documents by sourcedoc and ignore roots", () => {
  const origin = "https://acme-my.sharepoint.com";
  const doc = compiled(starterOffers(origin, []));
  const guid = "123e4567-e89b-12d3-a456-426614174000";
  const result = evaluate(doc,
    `${origin}/personal/alex/_layouts/15/Doc.aspx?sourcedoc=%7B${guid}%7D`);
  assert.equal(result.match, true);
  assert.equal(result.artifact_id, `sharepoint:acme-my.sharepoint.com/${guid}`);
  assert.equal(evaluate(doc, `${origin}/personal/alex/Documents/Forms/AllItems.aspx`).match, false);
});

test("GitHub drafts are scoped and preserve PR modes", () => {
  const offers = starterOffers("https://github.com", ["acme-corp"]);
  const doc = compiled(offers);
  const review = evaluate(doc, "https://github.com/acme-corp/widget/pull/412/files");
  assert.equal(review.match, true);
  assert.equal(review.artifact_id, "github.pr:github.com/acme-corp/widget/412");
  assert.equal(review.mode, "review");
  const other = evaluate(doc, "https://github.com/acme-corp/widget/pull/412/activity");
  assert.equal(other.match, true);
  assert.equal(other.artifact_id, review.artifact_id);
  assert.equal(other.mode, null);
  assert.equal(evaluate(doc, "https://github.com/personal/widget/pull/412/files").match, false);
  const issue = evaluate(doc, "https://github.com/acme-corp/widget/issues/77");
  assert.equal(issue.match, true);
  assert.equal(issue.artifact_id, "github.issue:github.com/acme-corp/widget/77");
});

test("every starter type is reserved in the host catalog", () => {
  const offers = [
    ...starterOffers("https://acme.atlassian.net", []),
    ...starterOffers("https://github.com", ["acme-corp"]),
    ...starterOffers("https://github.com", []),
    ...starterOffers("https://acme.sharepoint.com", []),
    ...starterOffers("https://acme-my.sharepoint.com", []),
  ];
  const types = new Set(offers.map((offer) => offer.artifact_type));
  assert.deepEqual([...types].sort(), ["github.issue", "github.pr", "github.repo", "jira.issue", "sharepoint.doc"]);
  for (const type of types) assert.ok(catalog.starter_types.includes(type), type);
});

test("withContainerLinks links only rules whose linked copy compiles, and leaves the rest as they are", () => {
  const [pr, issue, repo] = starterOffers("https://github.com", ["acme-corp"]).map((offer) => offer.adapter);
  const { within: _prWithin, ...bareIssue } = issue;
  const handWritten = {
    type: "github.pr", origin: "https://github.com", scope: { owner: ["acme-corp"] },
    match: [{ path: "/{owner:name|lower}/{_}/pull/{number:int}" }],
    id: "github.pr:{host}/{number}",
  };
  const otherOrigin = { ...bareIssue, origin: "https://ghe.example.com" };
  const elsewhere = { ...pr, within: { type: "other.box", id: "other.box:{host}/{owner}" } };
  const artifacts = [handWritten, bareIssue, otherOrigin, elsewhere];
  assert.deepEqual(containerLinks(artifacts, repo), { linked: 1, skipped: 1 });
  const linked = withContainerLinks(artifacts, repo);
  assert.equal(linked[0], handWritten);
  assert.deepEqual(linked[1], { ...bareIssue, within: { type: "github.repo", id: repo.id } });
  assert.equal(linked[2], otherOrigin);
  assert.equal(linked[3], elsewhere);
  assert.deepEqual(containerLinks(artifacts, { ...repo, type: "github.other" }), { linked: 0, skipped: 0 });
});
