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
  ["outside scope", "https://github.com/personal/repo/pull/12", {}, /outside the workspaces or accounts/],
  ["unmatched page", "https://github.com/search?q=work", {}, /does not match this site's capture rules/],
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

test("an unknown site's explanation opens the existing Capture Site review", async () => {
  const page = await install({
    tab: { id: 1, url: JIRA_URL, title: "Issue" }, config: CAPTURING, workerStatus: LIVE_POLICY,
  });
  await noteLoaded(page);
  await until(() => !page.el("capture-explanation-action").disabled, "capture review action");
  assert.match(page.el("capture-explanation-text").textContent, /no capture rules/);
  assert.equal(page.el("capture-explanation-action").textContent, "Review Capture Site");
  page.el("capture-explanation-action").click();
  await until(() => !page.el("capture-site-review").hidden, "the existing site review");
  assert.equal(updates(page).length, 0, "review still requires explicit save");
});

test("Review Capture Site from Recent reveals the page view containing the review", async () => {
  const page = await install({
    tab: { id: 1, url: JIRA_URL, title: "Issue" }, config: CAPTURING, workerStatus: LIVE_POLICY,
  });
  await noteLoaded(page);
  await until(() => !page.el("capture-explanation-action").disabled, "capture review action");
  page.el("tab-recent").click();
  assert.equal(page.el("page-view").hidden, true);
  page.el("capture-explanation-action").click();
  await until(() => !page.el("capture-site-review").hidden, "the existing site review");
  assert.equal(page.el("page-view").hidden, false, "the review's parent must be visible");
  assert.equal(page.el("recent-view").hidden, true);
  assert.equal(page.el("tab-page").getAttribute("aria-selected"), "true");
  assert.equal(updates(page).length, 0, "review still requires explicit save");
});

test("capture explanation follows navigation and the latest policy revision and scope", async () => {
  const page = await install({
    tab: { id: 1, url: "https://github.com/personal/repo/pull/12", title: "Pull request" },
    config: GITHUB_SCOPED, workerStatus: LIVE_POLICY,
  });
  await noteLoaded(page);
  assert.match(page.el("capture-explanation-text").textContent, /outside the workspaces/);
  page.state.config.adapters = {
    format: 1, artifacts: starterOffersFor("https://github.com", ["personal"]).map((offer) => offer.adapter),
  };
  page.state.revision = "scope-personal";
  page.policyChanged({ revision: "scope-personal" });
  await until(() => page.el("capture-explanation").hidden, "new scope recognizes the current page");
  page.navigate("https://github.com/acme/repo/pull/12");
  await until(() => !page.el("capture-explanation").hidden, "the formerly included workspace is excluded");
  assert.match(page.el("capture-explanation-text").textContent, /outside the workspaces/);
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
  assert.match(explanation, /does not match/);
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
