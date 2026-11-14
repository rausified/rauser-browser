// Built side-panel tests: capture explanation. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  CAPTURING, JIRA_URL, install, noteLoaded, starterOffersFor, until, updates,
} from "./harness.mjs";

// Capture explanations use the same ordered matcher as the worker, with
// local pauses/removals taking precedence. No page inspection or save is
// authorized by displaying an explanation or opening settings.
const GITHUB_SCOPED = {
  ...CAPTURING,
  adapters: { format: 1, artifacts: starterOffersFor("https://github.com", ["acme"]).map((offer) => offer.adapter) },
};
const LIVE_POLICY = { policy_expires_at: Date.now() + 86_400_000 };

for (const [name, url, options, expected] of [
  ["outside scope", "https://github.com/personal/repo/pull/12", {},
    /^github\.com captures repositories, pull requests and issues for acme; this page isn't one of them\. If it should be, review this site's scope in settings\.$/],
  ["unmatched page", "https://github.com/search?q=work", {},
    /^github\.com captures repositories, pull requests and issues for acme; this page isn't one of them\. Only matching pages are remembered\.$/],
  ["host pause", "https://github.com/acme/repo/pull/12", { config: { ...GITHUB_SCOPED, capture_enabled: false } }, /Capture is paused\. Resume/],
  ["local pause", "https://github.com/acme/repo/pull/12", { workerStatus: { ...LIVE_POLICY, pause_pending: true } }, /paused in this browser/],
  ["pending removal", "https://github.com/acme/repo/pull/12", { workerStatus: { ...LIVE_POLICY, pending_removals: ["https://github.com"] } }, /removal is pending/],
  ["expired policy", "https://github.com/acme/repo/pull/12", { workerStatus: { policy_expires_at: 1 } }, /unavailable or out of date/],
  ["missing home", "https://github.com/acme/repo/pull/12", { config: { ...GITHUB_SCOPED, storage: null } }, /needs a home folder/],
  ["configuration repair", "https://github.com/acme/repo/pull/12", { configIssue: "Bad config" }, /configuration is repaired/],
  ["adapter repair", "https://github.com/acme/repo/pull/12", { adapterIssue: "Unsupported format" }, /configuration is repaired/],
  ["credential-bearing address", "https://user:password@github.com/acme/repo/pull/12", {}, /sign-in credentials are excluded/],
]) {
  test(`capture explanation describes ${name} and opens settings without changing capture`, async () => {
    const page = await install({
      tab: { id: 1, url, title: "Page" }, config: GITHUB_SCOPED, workerStatus: LIVE_POLICY, ...options,
    });
    await noteLoaded(page);
    await until(() => !page.el("capture-explanation-action").disabled, "capture explanation action");
    assert.equal(page.el("capture-explanation").hidden, false);
    assert.match(page.el("capture-explanation-text").textContent, expected);
    const writes = updates(page).length;
    page.el("capture-explanation-action").click();
    assert.equal(page.calls("openOptionsPage"), 1);
    assert.equal(updates(page).length, writes);
    assert.equal(page.state.scripts.length, 0, "explaining capture never reads page anchors");
    assert.equal(page.sent("draft_site").length, 0);
  });
}

// GitHub puts /orgs/..., /settings/... in the owner slot, so these pages are
// out_of_scope; the line must not claim they belong to another account.
for (const url of ["https://github.com/orgs/acme/repositories", "https://github.com/settings/profile"]) {
  test(`an out-of-scope verdict on a non-account route (${new URL(url).pathname}) names no other account`, async () => {
    const page = await explained(url, GITHUB_SCOPED);
    assert.equal(page.el("capture-explanation-text").textContent,
      "github.com captures repositories, pull requests and issues for acme; this page isn't one of them. " +
      "If it should be, review this site's scope in settings.");
    assert.equal(page.el("capture-explanation-action").textContent, "Open settings");
  });
}

test("adapters scoped to different accounts name none of them", async () => {
  const [pr, issue, repo] = starterOffersFor("https://github.com", ["acme"]).map((offer) => offer.adapter);
  const page = await explained("https://github.com/beta/web/pull/12",
    withAdapters([pr, { ...issue, scope: { owner: ["beta"] } }, repo]));
  assert.equal(page.el("capture-explanation-text").textContent,
    "This page is outside the workspaces or accounts chosen for capture. Review this site's scope in settings.");
});

async function explained(url, config, title = "Page") {
  const page = await install({ tab: { id: 1, url, title }, config, workerStatus: LIVE_POLICY });
  await noteLoaded(page);
  await until(() => !page.el("capture-explanation-action").disabled, "capture explanation action");
  assert.equal(page.el("capture-explanation").hidden, false);
  return page;
}
const withAdapters = (artifacts) => ({ ...CAPTURING, adapters: { format: 1, artifacts } });

test("an unrecognized page says what the site captures, and the chip stays the host's verdict", async () => {
  const page = await explained("https://github.com/search?q=work", GITHUB_SCOPED);
  assert.equal(page.el("capture-explanation-text").textContent,
    "github.com captures repositories, pull requests and issues for acme; this page isn't one of them. Only matching pages are remembered.");
  await until(() => page.el("artifact-status").textContent === "○ Not an artifact", "the host verdict chip");
  assert.equal(page.el("capture-explanation-action").textContent, "Open settings");
});

test("a Jira page that is not an issue names Jira issues with no accounts", async () => {
  const jira = withAdapters(starterOffersFor("https://acme.atlassian.net", [])
    .filter((offer) => offer.artifact_type === "jira.issue").map((offer) => offer.adapter));
  const page = await explained("https://acme.atlassian.net/jira/your-work", jira);
  assert.equal(page.el("capture-explanation-text").textContent,
    "acme.atlassian.net captures Jira issues; this page isn't one of them. Only matching pages are remembered.");
});

test("an Atlassian site with both starters names Jira issues and Confluence pages", async () => {
  const both = withAdapters(starterOffersFor("https://acme.atlassian.net", []).map((offer) => offer.adapter));
  const page = await explained("https://acme.atlassian.net/wiki/spaces/ENG/overview", both);
  assert.equal(page.el("capture-explanation-text").textContent,
    "acme.atlassian.net captures Jira issues and Confluence pages; this page isn't one of them. Only matching pages are remembered.");
});

test("custom types, long account lists and mixed scope read as configured", async () => {
  const custom = { type: "example.page", origin: "https://example.com", match: [{ path: "/p/{id:int}" }], id: "example.page:{host}/{id}" };
  let page = await explained("https://example.com/about", withAdapters([custom]));
  assert.equal(page.el("capture-explanation-text").textContent,
    "example.com captures example.page; this page isn't one of them. Only matching pages are remembered.");
  const many = starterOffersFor("https://github.com", ["a", "b", "c", "d"]).map((offer) => offer.adapter);
  page = await explained("https://github.com/search?q=work", withAdapters(many));
  assert.match(page.el("capture-explanation-text").textContent, /issues for a, b, c and 1 more; this page isn't one of them\./);
  const [pr, issue, repo] = many;
  const { scope: _scope, ...unscopedIssue } = issue;
  page = await explained("https://github.com/search?q=work", withAdapters([pr, { ...unscopedIssue, unscoped: "acknowledged" }, repo]));
  assert.match(page.el("capture-explanation-text").textContent,
    /^github\.com captures repositories, pull requests and issues; this page isn't one of them\./);
  assert.equal(updates(page).length, 0);
});

// Near-miss offers (brwr-0t1): settings opens the review; the panel saves nothing.
const GUID = "5E2A9C47-1B3D-4F60-9A8E-7C21D4B06F13";
const SOURCEDOC = { sourcedoc: "{doc:uuid}" };
const SHARE_RULE = { path: "/(:w:|:x:|:p:)/(r|g)/(sites|personal)/{_}/_layouts/15/(Doc.aspx|doc2.aspx)", query: SOURCEDOC };
const PERSONAL_RULE = { path: "/personal/{_}/_layouts/15/(Doc.aspx|doc2.aspx)", query: SOURCEDOC };
const staleOneDrive = () => ({
  type: "sharepoint.doc", origin: "https://acme-my.sharepoint.com", path_case: "insensitive",
  match: [
    { path: "/sites/{_}/_layouts/15/Doc.aspx", query: SOURCEDOC },
    { path: "/:w:/r/sites/{_}/_layouts/15/Doc.aspx", query: SOURCEDOC },
    { path: "/personal/{_}/_layouts/15/Doc.aspx", query: SOURCEDOC },
  ],
  id: "sharepoint:{host}/{doc}",
});

function reviewOpened(page) {
  assert.equal(page.created.length, 1, "one new tab");
  const { url } = page.created[0];
  const prefix = "chrome-extension://test-id/options.html#adapter-review=";
  assert.ok(url.startsWith(prefix), url);
  // The payload is URI-encoded, so check the decoded hand-off.
  const decoded = decodeURIComponent(url);
  assert.equal(decoded.toLowerCase().includes(GUID.toLowerCase()), false, "no document ID in the hand-off");
  assert.equal(decoded.includes("/u/"), false, "no page path in the hand-off");
  assert.equal(page.calls("openOptionsPage"), 0);
  assert.equal(updates(page).length, 0, "the panel never saves rules");
  assert.equal(page.sent("save_config").length, 0);
  return JSON.parse(decodeURIComponent(url.slice(prefix.length)));
}

test("a stale OneDrive starter offers the current rule for review in settings", async () => {
  const adapter = staleOneDrive();
  const page = await explained(
    `https://acme-my.sharepoint.com/:w:/r/personal/u/_layouts/15/Doc.aspx?sourcedoc={${GUID}}`, withAdapters([adapter]));
  assert.equal(page.el("capture-explanation-text").textContent,
    "acme-my.sharepoint.com captures OneDrive for work documents, but none of this site's saved rules covers this address. " +
    "The current OneDrive for work documents starter has a rule for pages like this one.");
  assert.equal(page.el("capture-explanation-action").textContent, "Review rule");
  page.el("capture-explanation-action").click();
  await until(() => page.created.length > 0, "the settings tab");
  assert.deepEqual(reviewOpened(page), { adapter: [adapter.origin, adapter.type, adapter.id], rule: SHARE_RULE });
});

test("a hand-narrowed SharePoint adapter is offered only the starter rule for the page", async () => {
  const narrowed = { ...staleOneDrive(), origin: "https://acme.sharepoint.com", match: [staleOneDrive().match[0]] };
  const page = await explained(
    `https://acme.sharepoint.com/personal/u/_layouts/15/Doc.aspx?sourcedoc={${GUID}}`, withAdapters([narrowed]));
  assert.equal(page.el("capture-explanation-text").textContent,
    "acme.sharepoint.com captures SharePoint documents, but none of this site's saved rules covers this address. " +
    "The current SharePoint documents starter has a rule for pages like this one.");
  page.el("capture-explanation-action").click();
  await until(() => page.created.length > 0, "the settings tab");
  assert.deepEqual(reviewOpened(page).rule, PERSONAL_RULE);
});

test("a query identifier on an uncovered route is explained and opens Edit rules", async () => {
  const custom = { type: "example.doc", origin: "https://docs.example", match: [{ path: "/a/Doc.aspx", query: SOURCEDOC }], id: "example.doc:{host}/{doc}" };
  const page = await explained(`https://docs.example/b/Doc.aspx?sourcedoc={${GUID}}`, withAdapters([custom]));
  assert.equal(page.el("capture-explanation-text").textContent,
    "docs.example captures example.doc, but none of this site's saved rules covers this address. " +
    "The address carries an ID Brauser recognizes, so a rule could cover it.");
  assert.equal(page.el("capture-explanation-action").textContent, "Edit rules");
  page.el("capture-explanation-action").click();
  await until(() => page.created.length > 0, "the settings tab");
  assert.deepEqual(reviewOpened(page), { adapter: [custom.origin, custom.type, custom.id], rule: null });
});

test("an identifier on a covered route blocked by another condition does not blame the route", async () => {
  for (const rule of [
    { path: "/doc", query: { ...SOURCEDOC, view: "full" } },
    { path: "/doc", query: SOURCEDOC, absent: ["preview"] },
  ]) {
    const custom = { type: "example.doc", origin: "https://docs.example", match: [rule], id: "example.doc:{host}/{doc}" };
    const page = await explained(`https://docs.example/doc?sourcedoc={${GUID}}&view=compact&preview=1`, withAdapters([custom]));
    assert.equal(page.el("capture-explanation-text").textContent,
      "docs.example captures example.doc, but none of this site's saved rules covers this address. " +
      "The address carries an ID Brauser recognizes, so a rule could cover it.");
    assert.equal(page.el("capture-explanation-action").textContent, "Edit rules");
  }
});

const ONEDRIVE_PLAIN = "acme-my.sharepoint.com captures OneDrive for work documents; this page isn't one of them. Only matching pages are remembered.";
const GITHUB_PLAIN = "github.com captures repositories, pull requests and issues for acme; this page isn't one of them. Only matching pages are remembered.";
for (const [name, url, config, expected] of [
  ["Doc.aspx with no sourcedoc", "https://acme-my.sharepoint.com/:w:/r/personal/u/_layouts/15/Doc.aspx", null, ONEDRIVE_PLAIN],
  ["a non-GUID sourcedoc", "https://acme-my.sharepoint.com/:w:/r/personal/u/_layouts/15/Doc.aspx?sourcedoc=nope", null, ONEDRIVE_PLAIN],
  ["a file=-only link", "https://acme-my.sharepoint.com/:w:/r/personal/u/_layouts/15/Doc.aspx?file=x.docx", null, ONEDRIVE_PLAIN],
  ["two sourcedoc values", `https://acme-my.sharepoint.com/:w:/r/personal/u/_layouts/15/Doc.aspx?sourcedoc={${GUID}}&sourcedoc={${GUID}}`, null, ONEDRIVE_PLAIN],
  ["a single-segment GitHub page", "https://github.com/search?q=work", GITHUB_SCOPED, GITHUB_PLAIN],
  ["a GitHub repository outside scope", "https://github.com/personal/repo", GITHUB_SCOPED,
    "github.com captures repositories, pull requests and issues for acme; this page isn't one of them. If it should be, review this site's scope in settings."],
]) {
  test(`${name} is not a near-miss and keeps Open settings`, async () => {
    const page = await explained(url, config ?? withAdapters([staleOneDrive()]));
    assert.equal(page.el("capture-explanation-text").textContent, expected);
    assert.equal(page.el("capture-explanation-action").textContent, "Open settings");
    page.el("capture-explanation-action").click();
    assert.equal(page.calls("openOptionsPage"), 1);
    assert.equal(page.created.length, 0);
  });
}

for (const [name, url] of [["starter", JIRA_URL], ["unknown", "https://wiki.example.com/pages/123"]]) {
  test(`an unconfigured ${name} site offers Capture Site without a duplicate explanation`, async () => {
    const page = await install({
      tab: { id: 1, url, title: "Page" }, config: CAPTURING, workerStatus: LIVE_POLICY,
    });
    await noteLoaded(page);
    await until(() => !page.el("capture-site").disabled, "capture action");
    assert.equal(page.el("capture-explanation").hidden, true);
    assert.equal(page.el("capture-explanation-text").textContent, "");
    assert.equal(page.el("capture-site-hint").textContent, "");
    assert.equal(page.el("capture-site").hidden, false);
    assert.equal(updates(page).length, 0);
    page.el("capture-site").click();
    await until(() => !page.el("capture-site-review").hidden, "the site review");
    assert.equal(updates(page).length, 0, "review still requires explicit save");
  });
}

test("capture explanation follows navigation and the latest policy revision and scope", async () => {
  const page = await install({
    tab: { id: 1, url: "https://github.com/personal/repo/pull/12", title: "Pull request" },
    config: GITHUB_SCOPED, workerStatus: LIVE_POLICY,
  });
  await noteLoaded(page);
  assert.match(page.el("capture-explanation-text").textContent, /for acme; this page isn't one of them\. If it should be, review/);
  page.state.config.adapters = {
    format: 1, artifacts: starterOffersFor("https://github.com", ["personal"]).map((offer) => offer.adapter),
  };
  page.state.revision = "scope-personal";
  page.policyChanged({ revision: "scope-personal" });
  await until(() => page.el("capture-explanation").hidden, "new scope recognizes the current page");
  page.navigate("https://github.com/acme/repo/pull/12");
  await until(() => !page.el("capture-explanation").hidden, "the formerly included workspace is excluded");
  assert.match(page.el("capture-explanation-text").textContent, /for personal; this page isn't one of them\. If it should be, review/);
  page.navigate("chrome://settings/");
  await until(() => !page.el("page-unusable").hidden, "a non-web tab");
  assert.equal(page.el("capture-explanation").hidden, true);
  assert.equal(page.el("capture-explanation-text").textContent, "");
  assert.equal(updates(page).length, 0);
});

test("link patterns cannot change a configured page's capture explanation", async () => {
  const page = await install({
    tab: { id: 1, url: "https://github.com/search?q=work", title: "Search" },
    config: GITHUB_SCOPED, workerStatus: LIVE_POLICY,
    patterns: [{ shape: "/{slug}/{slug}/pull/{num}", count: 20, last_seen: Date.now() }],
    script: async () => ({ current: "/search?q={slug}", links: [{ shape: "/{slug}/{slug}/pull/{num}", count: 50 }] }),
  });
  await noteLoaded(page);
  await until(() => !page.el("site-pattern-review").disabled, "pattern review ready");
  const explanation = page.el("capture-explanation-text").textContent;
  page.el("site-pattern-area").open = true;
  page.el("site-pattern-review").click();
  await until(() => page.el("site-pattern-rows").children.length > 0, "link-pattern review");
  assert.equal(page.el("capture-explanation-text").textContent, explanation);
  assert.match(explanation, /this page isn't one of them/);
  assert.equal(updates(page).length, 0);
});

test("capture explanations update when local pauses and removals change without a new policy", async (t) => {
  const originalInterval = globalThis.setInterval;
  let poll;
  t.mock.method(globalThis, "setInterval", (callback, delay, ...args) => {
    if (delay === 30_000) poll = callback;
    return originalInterval(callback, delay, ...args);
  });
  const page = await install({
    tab: { id: 1, url: "https://github.com/acme/repo/pull/12", title: "Pull request" },
    config: GITHUB_SCOPED, workerStatus: LIVE_POLICY,
  });
  await noteLoaded(page);
  await until(() => !page.el("pause-capture").disabled, "startup tasks to finish");
  assert.equal(page.el("capture-explanation").hidden, true);
  page.state.workerStatus.pause_pending = true;
  poll();
  await until(() => /paused in this browser/.test(page.el("capture-explanation-text").textContent), "local pause");
  page.state.workerStatus.pending_removals = ["https://github.com"];
  poll();
  await until(() => /removal is pending/.test(page.el("capture-explanation-text").textContent), "pending removal takes precedence");
  page.state.workerStatus.pending_removals = ["https://other.example"];
  page.state.workerStatus.pause_pending = false;
  poll();
  await until(() => page.el("capture-explanation").hidden, "unrelated removal leaves this site capturing");
  assert.equal(updates(page).length, 0);
});
