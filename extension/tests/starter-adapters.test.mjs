import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const { compileDocument, evaluate } = await import("../dist/adapter-templates.js");
const {
  githubOwnerFromUrl,
  hasAdapter,
  matchingStarter,
  orderContainedFirst,
  parseGitHubOrganizations,
  reviewableOverlaps,
  setStarterFamiliesForTest,
  starterKind,
  starterOffers,
  starterRuleAddition,
  starterRuleUpdate,
  withContainerLinks,
  containerLinks,
} = await import("../dist/starter-adapters.js");

const { EXAMPLE_CONTAINER_FIRST_FAMILY } = await import("./starter-fixtures.mjs");
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
    ...starterOffers("https://miro.com", []),
  ].map((offer) => offer.adapter);
  const expectedTypes = new Set([
    "jira.issue", "confluence.page", "github.pr", "github.issue", "github.repo", "sharepoint.doc", "miro.board",
  ]);
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

// A family's declared container_first pair (protocol/starters/README.md) is
// never reordered on the family's own origins: whichever comes first records
// the pages both match, so a re-sort would move recorded pages. Starters
// offer the container first, and the host refuses a save that puts the
// contained type first or flips a live pair.
function containerFirstSeam(t) {
  setStarterFamiliesForTest([EXAMPLE_CONTAINER_FIRST_FAMILY]);
  t.after(() => setStarterFamiliesForTest(null));
  const nest = "https://nest.example.test";
  const [spaceType, boardType] = EXAMPLE_CONTAINER_FIRST_FAMILY.types;
  return {
    space: { ...spaceType.adapter, origin: nest },
    board: { ...boardType.adapter, origin: nest },
  };
}

test("orderContainedFirst never reorders a declared container_first pair", (t) => {
  const { space, board } = containerFirstSeam(t);
  const other = adapter("custom.page", "https://nest.example.test");
  assert.deepEqual(orderContainedFirst([space, board]), [space, board]);
  // A legacy dashboards-first pair keeps its order, so an unrelated save
  // does not flip it (which the host would refuse).
  assert.deepEqual(orderContainedFirst([board, space]), [board, space]);
  assert.deepEqual(orderContainedFirst([board, other, space]), [board, other, space]);
  const inOrder = [space, other, board];
  assert.deepEqual(orderContainedFirst(inOrder), inOrder);
  const { within: _within, ...unlinked } = board;
  assert.deepEqual(orderContainedFirst([unlinked, space]), [unlinked, space]);
  // The host checks type names on every origin, so the order is kept there
  // too (the overlap warning stays the family's own, below).
  const elsewhereSpace = { ...space, origin: "https://elsewhere.example.test" };
  const elsewhereBoard = { ...board, origin: "https://elsewhere.example.test" };
  assert.deepEqual(orderContainedFirst([elsewhereSpace, elsewhereBoard]), [elsewhereSpace, elsewhereBoard]);
  // Once either adapter differs from what the starter ships in any rule or
  // in path_case, the pair keeps whatever order it already has: whichever
  // comes first records the pages both match, so reordering on an edit (or
  // on reverting one) would move recorded pages from one to the other. The
  // host refuses a save that puts the contained type first.
  const widened = { ...space, match: [space.match[0], { path: "/s/{space:name}/{_}" }] };
  assert.deepEqual(orderContainedFirst([widened, board]), [widened, board]);
  assert.deepEqual(orderContainedFirst([board, widened]), [board, widened]);
  const widenedRoot = { ...space, match: [{ path: "/s/{space:name}/{*}" }, space.match[1]] };
  assert.deepEqual(orderContainedFirst([widenedRoot, board]), [widenedRoot, board]);
  const appended = { ...space, match: [...space.match, { path: "/s/{space:name}/{_}" }] };
  assert.deepEqual(orderContainedFirst([appended, board]), [appended, board]);
  const folded = { ...space, path_case: "insensitive" };
  assert.deepEqual(orderContainedFirst([folded, board]), [folded, board]);
  const editedBoard = { ...board, match: [{ path: "/s/{space:name}/{board:name}/{*}" }] };
  assert.deepEqual(orderContainedFirst([space, editedBoard]), [space, editedBoard]);
  // Reverting an edit (shipped again) never flips a container-first pair.
  // A site saved dashboards first, its app edited, then reverted: the next
  // save keeps dashboards first, so the host has no flip to refuse.
  assert.deepEqual(orderContainedFirst([board, widened]), [board, widened]);
  assert.deepEqual(orderContainedFirst([board, space]), [board, space]);
  assert.deepEqual(orderContainedFirst([space, board]), [space, board]);
  // The evaluated document agrees: the space's built-in page is the space.
  const doc = compileDocument({ format: 1, artifacts: orderContainedFirst([space, board]) });
  assert.equal(doc.ok, true);
  assert.equal(evaluate(doc.doc, "https://nest.example.test/s/acme/settings").artifact_id, "nest.space:nest.example.test/s/acme");
  assert.equal(evaluate(doc.doc, "https://nest.example.test/s/acme/roadmap").artifact_id, "nest.board:nest.example.test/acme/roadmap");
});

test("a multi-origin (kind any) family's container_first declaration holds on each of its origins", (t) => {
  const family = structuredClone(EXAMPLE_CONTAINER_FIRST_FAMILY);
  family.origin = { kind: "any", of: [
    { kind: "exact", origin: "https://nest.example.test" },
    { kind: "subdomain", suffix: ".nest.example.org", labels: 1 },
  ] };
  setStarterFamiliesForTest([family]);
  t.after(() => setStarterFamiliesForTest(null));
  const [spaceType, boardType] = family.types;
  for (const origin of ["https://nest.example.test", "https://acme.nest.example.org"]) {
    const space = { ...spaceType.adapter, origin };
    const board = { ...boardType.adapter, origin };
    assert.deepEqual(orderContainedFirst([board, space]), [board, space], origin);
    assert.deepEqual(orderContainedFirst([space, board]), [space, board], origin);
    const declared = { first_adapter: 0, first_rule: 1, second_adapter: 1, second_rule: 0 };
    assert.deepEqual(reviewableOverlaps([declared], [space, board]), [], origin);
  }
  const off = "https://elsewhere.example.test";
  const [space, board] = [{ ...spaceType.adapter, origin: off }, { ...boardType.adapter, origin: off }];
  assert.deepEqual(orderContainedFirst([space, board]), [space, board]);
});

test("reviewableOverlaps hides only a declared container_first rule pair, unedited and container first", (t) => {
  const { space, board } = containerFirstSeam(t);
  const declared = { first_adapter: 0, first_rule: 1, second_adapter: 1, second_rule: 0 };
  assert.deepEqual(reviewableOverlaps([declared], [space, board]), []);
  // The contained type first (hand-reordered config.toml) is not the declared winner.
  const reversed = { first_adapter: 0, first_rule: 0, second_adapter: 1, second_rule: 1 };
  assert.deepEqual(reviewableOverlaps([reversed], [board, space]), [reversed]);
  // A rule pair the family did not declare is still shown.
  const undeclared = { first_adapter: 0, first_rule: 0, second_adapter: 1, second_rule: 0 };
  assert.deepEqual(reviewableOverlaps([undeclared], [space, board]), [undeclared]);
  // An edited rule on either side is the user's own overlap, shown as before.
  const widened = { ...space, match: [space.match[0], { path: "/s/{space:name}/{*}" }] };
  assert.deepEqual(reviewableOverlaps([declared], [widened, board]), [declared]);
  const edited = { ...board, match: [{ path: "/s/{space:name}/{board:name}/{*}" }] };
  assert.deepEqual(reviewableOverlaps([declared], [space, edited]), [declared]);
  const folded = { ...space, path_case: "insensitive" };
  assert.deepEqual(reviewableOverlaps([declared], [folded, board]), [declared]);
  // So is the declared pair once any other rule of either adapter is edited.
  const widenedRoot = { ...space, match: [{ path: "/s/{space:name}/{*}" }, space.match[1]] };
  assert.deepEqual(reviewableOverlaps([declared], [widenedRoot, board]), [declared]);
  const appended = { ...space, match: [...space.match, { path: "/s/{space:name}/(trash)" }] };
  assert.deepEqual(reviewableOverlaps([declared], [appended, board]), [declared]);
  // Off the family's origin the declaration does not apply.
  const elsewhere = [{ ...space, origin: "https://elsewhere.example.test" }, { ...board, origin: "https://elsewhere.example.test" }];
  assert.deepEqual(reviewableOverlaps([declared], elsewhere), [declared]);
});

test("starter origins are classified narrowly", () => {
  assert.equal(starterKind("https://acme.atlassian.net"), "atlassian");
  assert.equal(starterKind("https://acme.atlassian.net.evil.example"), null);
  assert.equal(starterKind("https://atlassian.net"), null);
  assert.equal(starterKind("http://acme.atlassian.net"), null);
  assert.equal(starterKind("https://acme.sharepoint.com"), "sharepoint");
  assert.equal(starterKind("https://acme-my.sharepoint.com"), "sharepoint");
  assert.equal(starterKind("http://acme.sharepoint.com"), null);
  assert.equal(starterKind("https://github.com"), "github");
  assert.equal(starterKind("http://github.com"), null);
  assert.equal(starterKind("https://github.com.evil.example"), null);
  assert.equal(starterKind("https://miro.com"), "miro");
  assert.equal(starterKind("http://miro.com"), null);
  assert.equal(starterKind("https://www.miro.com"), null);
  assert.equal(starterKind("https://miro.com.evil.example"), null);
  assert.deepEqual(starterOffers("https://example.com", []), []);
  assert.equal(starterOffers("https://acme.atlassian.net", []).length, 2);
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

test("Atlassian sites offer Jira issues and Confluence pages as separate starters", () => {
  const origin = "https://acme.atlassian.net";
  const offers = starterOffers(origin, []);
  assert.deepEqual(offers.map((offer) => [offer.key, offer.kind, offer.artifact_type, offer.label]), [
    [`jira:${origin}`, "atlassian", "jira.issue", "Jira issues"],
    [`confluence.page:${origin}`, "atlassian", "confluence.page", "Confluence pages"],
  ]);
  assert.ok(offers.every((offer) => offer.needs_scope === false && offer.origin === origin));
  // Either compiles alone, so the user can choose one without the other.
  for (const offer of offers) compiled([offer]);
  const [jira, confluence] = offers;
  // An already-configured type is not offered twice, and one Atlassian
  // starter never counts as the other.
  assert.equal(hasAdapter([jira.adapter], jira), true);
  assert.equal(hasAdapter([jira.adapter], confluence), false);
  assert.equal(hasAdapter([confluence.adapter], confluence), true);
  assert.equal(hasAdapter([{ ...confluence.adapter, origin: "https://other.atlassian.net" }], confluence), false);
  assert.equal(matchingStarter(confluence.adapter)?.key, confluence.key);
  assert.equal(matchingStarter(jira.adapter)?.key, jira.key);
  // A Confluence-typed adapter with its own ID is the user's, not the starter.
  assert.equal(matchingStarter({ ...confluence.adapter, id: "confluence:{host}/{id}" }), null);
  // Each origin's starters are its own.
  assert.equal(starterOffers("https://other.atlassian.net", [])[1].adapter.origin, "https://other.atlassian.net");
});

test("Confluence draft keys pages by page ID across view, edit, history and legacy links", () => {
  const origin = "https://acme.atlassian.net";
  const [jira, confluence] = starterOffers(origin, []);
  assert.equal(confluence.adapter.id, "confluence.page:{host}/{id}");
  const doc = compiled([jira, confluence]);
  const id = "confluence.page:acme.atlassian.net/1000001";
  const page = evaluate(doc, `${origin}/wiki/spaces/ENG/pages/1000001/Release+Plan?atl_f=PAGETREE#0a1b2c3d4e5f`,
    "OPS-12 rollout plan - Engineering - Confluence");
  assert.equal(page.artifact_id, id);
  assert.equal(page.mode, null);
  assert.equal(page.title, "OPS-12 rollout plan");
  assert.deepEqual(page.refs, [{ type: "jira.issue", value: "OPS-12" }]);
  // The ID never carries the space key: a moved page keeps its identity.
  assert.equal(evaluate(doc, `${origin}/wiki/spaces/~0123456789abcdef0123456789abcdef012345/pages/1000001`).artifact_id, id);
  const edit = evaluate(doc, `${origin}/wiki/spaces/ENG/pages/edit-v2/1000001`);
  assert.deepEqual([edit.artifact_id, edit.mode], [id, "edit"]);
  const history = evaluate(doc, `${origin}/wiki/spaces/ENG/history/1000001/Release+Plan`,
    "Version history - Release plan - Engineering - Confluence");
  assert.deepEqual([history.artifact_id, history.mode, history.title], [id, "view", "Release plan"]);
  assert.equal(evaluate(doc, `${origin}/wiki/pages/viewpage.action?pageId=1000001`).artifact_id, id);
  for (const path of [
    "/wiki", "/wiki/home", "/wiki/search?text=plan", "/wiki/spaces/ENG/overview", "/wiki/spaces/ENG/database/1000001",
    "/wiki/x/AbCdEf", "/wiki/spaces/ENG/whiteboard/1000001", "/wiki/spaces/ENG/blog/2026/10/30/1000001/Notes",
  ]) assert.equal(evaluate(doc, `${origin}${path}`).match, false, path);
  // Jira keeps its own routes on the shared origin.
  assert.equal(evaluate(doc, `${origin}/browse/OPS-12`).artifact_id, "jira:acme.atlassian.net/OPS-12");
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

test("Miro draft keys boards by their padded board ID and ignores dashboards", () => {
  const [offer, ...rest] = starterOffers("https://miro.com", []);
  assert.equal(rest.length, 0);
  assert.equal(offer.needs_scope, false);
  assert.match(offer.description, /every Miro account/);
  const doc = compiled([offer]);
  const board = evaluate(doc, "https://miro.com/app/board/qK3nR8vDpZx=/", "SSE ZTA SWOT - Miro");
  assert.equal(board.match, true);
  assert.equal(board.artifact_id, "miro.board:miro.com/qK3nR8vDpZx=");
  assert.equal(board.title, "SSE ZTA SWOT");
  const shared = evaluate(doc, "https://miro.com/app/board/qK3nR8vDpZx=/?share_link_id=687498250990");
  assert.equal(shared.artifact_id, board.artifact_id);
  assert.equal(evaluate(doc, "https://miro.com/app/dashboard/").match, false);
  assert.equal(evaluate(doc, "https://miro.com/login/").match, false);
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
    ...starterOffers("https://miro.com", []),
  ];
  const types = new Set(offers.map((offer) => offer.artifact_type));
  assert.deepEqual([...types].sort(),
    ["confluence.page", "github.issue", "github.pr", "github.repo", "jira.issue", "miro.board", "sharepoint.doc"]);
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

const ONEDRIVE = "https://cisco-my.sharepoint.com";
const SHARE_RULE = { path: "/(:w:|:x:|:p:)/(r|g)/(sites|personal)/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } };
// The SharePoint/OneDrive starter as saved before sharing links covered OneDrive.
const staleOneDrive = () => ({
  type: "sharepoint.doc",
  origin: ONEDRIVE,
  path_case: "insensitive",
  match: [
    { path: "/sites/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } },
    { path: "/:w:/r/sites/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } },
    { path: "/personal/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } },
  ],
  id: "sharepoint:{host}/{doc}",
});

const OLD_SHARE_RULE = { path: "/:w:/r/sites/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } };

test("a saved earlier OneDrive starter swaps its retired rule for the current one and keeps a user rule", () => {
  const extra = { path: "/:b:/r/personal/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } };
  const saved = { ...staleOneDrive(), match: [...staleOneDrive().match, extra] };
  const update = starterRuleUpdate(saved);
  assert.ok(update);
  assert.deepEqual(update.added, [SHARE_RULE]);
  assert.deepEqual(update.removed, [OLD_SHARE_RULE]);
  const [sites, , personal] = staleOneDrive().match;
  assert.deepEqual(update.adapter, { ...saved, match: [sites, personal, extra, SHARE_RULE] });
  assert.deepEqual(saved.match.slice(0, 3), staleOneDrive().match, "the saved adapter is not mutated");
  assert.deepEqual(starterRuleUpdate(staleOneDrive()).adapter.match, [sites, personal, SHARE_RULE]);
  // Already at the current rules: nothing to offer.
  assert.equal(starterRuleUpdate(update.adapter), null);
});

test("the real OneDrive sharing link matches the updated adapter", () => {
  const { adapter } = starterRuleUpdate(staleOneDrive());
  const result = compileDocument({ format: 1, artifacts: [adapter] });
  assert.equal(result.ok, true);
  const url = `${ONEDRIVE}/:w:/r/personal/havishwa_cisco_com/_layouts/15/Doc.aspx?sourcedoc=%7B8BAF1543-CDBF-4003-A48F-D4A939774FC0%7D&file=x.docx`;
  const before = compileDocument({ format: 1, artifacts: [staleOneDrive()] });
  assert.equal(evaluate(before.doc, url).match, false);
  const found = evaluate(result.doc, url);
  assert.equal(found.match, true);
  assert.equal(found.artifact_id, "sharepoint:cisco-my.sharepoint.com/8baf1543-cdbf-4003-a48f-d4a939774fc0");
});

test("an up-to-date, narrowed, hand-written, or non-starter adapter has no update", () => {
  for (const offer of [
    ...starterOffers(ONEDRIVE, []),
    ...starterOffers("https://acme.atlassian.net", []),
    ...starterOffers("https://github.com", ["acme-corp"]),
  ]) assert.equal(starterRuleUpdate(offer.adapter), null, offer.key);
  assert.equal(starterRuleUpdate({ ...staleOneDrive(), type: "sharepoint.page" }), null);
  assert.equal(starterRuleUpdate({ ...staleOneDrive(), id: "sharepoint:{doc}" }), null);
  assert.equal(starterRuleUpdate({ ...staleOneDrive(), origin: "https://example.com" }), null);
  assert.equal(starterRuleUpdate({ ...staleOneDrive(), match: "nonsense" }), null);
  // An earlier SharePoint starter the user narrowed matches no known version.
  assert.equal(starterRuleUpdate({ ...staleOneDrive(), match: staleOneDrive().match.slice(0, 2) }), null);
  // A Jira starter whose user removed the board-modal rule stays narrowed.
  const [jira] = starterOffers("https://acme.atlassian.net", []);
  assert.equal(starterRuleUpdate({ ...jira.adapter, match: jira.adapter.match.slice(0, 1) }), null);
  // A hand-written pull request adapter was never the starter, which has
  // always had the mode-tab rule.
  assert.equal(starterRuleUpdate({
    type: "github.pr", origin: "https://github.com", scope: { owner: ["acme-corp"] },
    match: [{ path: "/{owner:name|lower}/{repo:name|lower}/pull/{number:int}" }],
    id: "github.pr:{host}/{owner}/{repo}/{number}",
  }), null);
});

test("an update stays within 16 rules", () => {
  const filler = Array.from({ length: 13 }, (_, at) =>
    ({ path: `/extra${at}/_layouts/15/Doc.aspx`, query: { sourcedoc: "{doc:uuid}" } }));
  const full = { ...staleOneDrive(), match: [...staleOneDrive().match, ...filler] };
  assert.equal(full.match.length, 16);
  const update = starterRuleUpdate(full);
  assert.equal(update.adapter.match.length, 16);
  assert.deepEqual(update.adapter.match.slice(2, 15), filler);
});

// A hand-narrowed SharePoint adapter: the sites rule only.
const narrowed = () => ({ ...staleOneDrive(), origin: "https://acme.sharepoint.com", match: [staleOneDrive().match[0]] });
const PERSONAL_RULE = { path: "/personal/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } };

test("starterRuleAddition appends one current starter rule and changes nothing else", () => {
  const saved = { ...narrowed(), title: "{title}.docx" };
  const addition = starterRuleAddition(saved, { query: { sourcedoc: "{doc:uuid}" }, path: PERSONAL_RULE.path });
  assert.ok(addition);
  assert.deepEqual(addition.added, [PERSONAL_RULE]);
  assert.deepEqual(addition.removed, []);
  assert.deepEqual(addition.adapter, { ...saved, match: [...saved.match, PERSONAL_RULE] });
  assert.equal(saved.match.length, 1, "the saved adapter is not mutated");
  // Not gated by STARTER_HISTORY: starterRuleUpdate refuses the narrowed one.
  assert.equal(starterRuleUpdate(saved), null);
  const doc = compileDocument({ format: 1, artifacts: [addition.adapter] });
  assert.equal(doc.ok, true);
  const url = "https://acme.sharepoint.com/personal/u/_layouts/15/Doc.aspx?sourcedoc=%7B8BAF1543-CDBF-4003-A48F-D4A939774FC0%7D";
  assert.equal(evaluate(doc.doc, url).match, true);
});

test("a Confluence adapter saved without the history rule can gain it, with its fixed view mode", () => {
  const [, confluence] = starterOffers("https://acme.atlassian.net", []);
  const historyRule = confluence.adapter.match[2];
  assert.deepEqual(historyRule, { path: "/wiki/spaces/{_}/history/{id:int}/{*}", mode: "view" });
  const saved = { ...confluence.adapter, match: confluence.adapter.match.filter((rule) => rule !== historyRule) };
  const addition = starterRuleAddition(saved, historyRule);
  assert.ok(addition);
  assert.deepEqual(addition.added, [historyRule]);
  assert.equal(addition.adapter.modes, undefined, "a fixed rule mode needs no modes table");
  const doc = compileDocument({ format: 1, artifacts: [addition.adapter] });
  assert.equal(doc.ok, true);
  const history = evaluate(doc.doc, "https://acme.atlassian.net/wiki/spaces/ENG/history/1000001");
  assert.deepEqual([history.artifact_id, history.mode], ["confluence.page:acme.atlassian.net/1000001", "view"]);
  // The current starter itself needs no update.
  assert.equal(starterRuleUpdate(confluence.adapter), null);
});

test("starterRuleAddition refuses a rule outside the catalog, a duplicate, a 17th rule, and a non-starter", () => {
  assert.equal(starterRuleAddition(narrowed(), { path: "/anything/{*}" }), null, "not in the current starter");
  assert.equal(starterRuleAddition(narrowed(), OLD_SHARE_RULE), null, "a retired rule is not current");
  assert.equal(starterRuleAddition(narrowed(), narrowed().match[0]), null, "already present");
  const filler = Array.from({ length: 15 }, (_, at) =>
    ({ path: `/extra${at}/_layouts/15/Doc.aspx`, query: { sourcedoc: "{doc:uuid}" } }));
  const full = { ...narrowed(), match: [...narrowed().match, ...filler] };
  assert.equal(full.match.length, 16);
  assert.equal(starterRuleAddition(full, PERSONAL_RULE), null, "the 17th rule");
  assert.equal(starterRuleAddition({ ...narrowed(), type: "sharepoint.page" }, PERSONAL_RULE), null);
  assert.equal(starterRuleAddition({ ...narrowed(), match: "nonsense" }, PERSONAL_RULE), null);
});

test("starterRuleAddition adds only the modes the new rule's mode capture needs", () => {
  const [pr] = starterOffers("https://github.com", ["acme-corp"]);
  const handWritten = {
    type: "github.pr", origin: "https://github.com", scope: { owner: ["acme-corp"] },
    match: [{ path: "/{owner:name|lower}/{repo:name|lower}/pull/{number:int}" }],
    modes: { files: "view" },
    id: "github.pr:{host}/{owner}/{repo}/{number}",
  };
  const addition = starterRuleAddition(handWritten, pr.adapter.match[0]);
  assert.ok(addition);
  assert.deepEqual(addition.adapter.modes, { files: "view", commits: "view", checks: "view" },
    "the user's own files mode stays; only the missing modes are added");
  assert.deepEqual(addition.adapter.scope, handWritten.scope);
  assert.equal(Object.hasOwn(addition.adapter, "within"), false);
});
