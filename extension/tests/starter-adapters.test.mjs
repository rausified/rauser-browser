import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const { compileDocument, evaluate } = await import("../dist/adapter-templates.js");
const {
  parseGitHubOrganizations,
  starterKind,
  starterOffers,
} = await import("../dist/starter-adapters.js");

const site = (origin) => ({ origin, path_prefix: "/" });
const vectors = JSON.parse(await readFile(new URL("../../protocol/adapter-templates/vectors.json", import.meta.url)));
const recorded = vectors.fixtures.starters.artifacts;

function compiled(offers) {
  const artifacts = offers.map((offer) => offer.adapter);
  const result = compileDocument({ format: 1, artifacts });
  assert.equal(result.ok, true, result.ok ? "" : JSON.stringify(result.error));
  return result.doc;
}

test("generated starters stay identical to the shared cross-engine fixtures", () => {
  const generated = [
    ...starterOffers(site("https://acme.atlassian.net"), null),
    ...starterOffers(site("https://github.com"), ["acme-corp"]),
    ...starterOffers(site("https://acme.sharepoint.com"), null),
    ...starterOffers(site("https://acme-my.sharepoint.com"), null),
  ].map((offer) => offer.adapter);
  const expectedTypes = new Set(["jira.issue", "github.pr", "github.issue", "sharepoint.doc"]);
  const expected = recorded.filter((adapter) => expectedTypes.has(adapter.type));
  assert.deepEqual(generated, expected);
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
  assert.deepEqual(starterOffers({ origin: "https://acme.atlassian.net", path_prefix: "/browse" }, null), []);
});

test("GitHub organization scopes normalize, deduplicate, and reject unsafe forms", () => {
  assert.deepEqual(parseGitHubOrganizations(" Acme-Corp, partner, ACME-CORP "), ["acme-corp", "partner"]);
  assert.equal(parseGitHubOrganizations(""), null);
  assert.equal(parseGitHubOrganizations("-leading"), null);
  assert.equal(parseGitHubOrganizations("trailing-"), null);
  assert.equal(parseGitHubOrganizations("name with space"), null);
});

test("Jira draft joins issue pages and board modals under one canonical ID", () => {
  const doc = compiled(starterOffers(site("https://acme.atlassian.net"), null));
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
  const doc = compiled(starterOffers(site(origin), null));
  const guid = "123e4567-e89b-12d3-a456-426614174000";
  const result = evaluate(doc,
    `${origin}/personal/alex/_layouts/15/Doc.aspx?sourcedoc=%7B${guid}%7D`);
  assert.equal(result.match, true);
  assert.equal(result.artifact_id, `sharepoint:acme-my.sharepoint.com/${guid}`);
  assert.equal(evaluate(doc, `${origin}/personal/alex/Documents/Forms/AllItems.aspx`).match, false);
});

test("GitHub drafts are scoped and preserve PR modes", () => {
  const offers = starterOffers(site("https://github.com"), ["acme-corp"]);
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
