// Built side-panel tests: page patterns. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  install, until,
} from "./harness.mjs";

test("page-pattern review on an unknown site reads links only on click without changing capture", async () => {
  const url = "https://unknown.example/items/123456";
  const page = await install({ tab: { id: 1, url, title: "Private Customer Title" },
    script: async () => ({ current: "/items/{num}", links: [{ shape: "/items/{num}/{slug}", count: 2 }] }),
    patterns: [{ shape: "/documents/{uuid}", count: 3, last_seen: Date.now() }],
  });
  await until(() => !page.el("site-pattern-area").hidden && !page.el("site-pattern-review").disabled, "pattern review to be ready");
  assert.equal(page.state.scripts.length, 0);
  assert.equal(page.el("capture-site-review").hidden, true, "Capture Site reads nothing until clicked");
  page.el("site-pattern-review").click();
  await until(() => page.el("site-pattern-rows").textContent.includes("Link pattern"), "pattern results");
  const [injection] = page.state.scripts;
  assert.deepEqual(injection.target, { tabId: 1, frameIds: [0] });
  assert.equal(injection.world, "ISOLATED");
  assert.deepEqual(injection.args, [url, true]);
  assert.equal(typeof injection.func, "function");
  assert.match(page.el("site-pattern-message").textContent, /Review only/);
  assert.match(page.el("site-pattern-rows").textContent, /Visited pattern \(3 navigations\)/);
  assert.doesNotMatch(page.el("site-pattern-rows").textContent, /123456|Private Customer/);
  assert.equal(page.state.requests.filter((request) => request.type === "update_config").length, 0);
  assert.equal(page.calls("permissions.request"), 0);
  page.el("site-pattern-close").click();
  assert.equal(page.el("site-pattern-result").hidden, true);
  assert.equal(page.el("site-pattern-rows").textContent, "");
});

test("reloading the reviewed page discards its link patterns, including a held read", async () => {
  const url = "https://unknown.example/items/1";
  const page = await install({ tab: { id: 1, url, title: "A" },
    script: async () => ({ current: "/items/{num}", links: [{ shape: "/items/{num}/{slug}", count: 2 }] }) });
  await until(() => !page.el("site-pattern-review").disabled, "pattern review ready");
  page.el("site-pattern-review").click();
  await until(() => page.el("site-pattern-rows").textContent.includes("Link pattern"), "pattern results");
  page.tabUpdated({ status: "loading" });
  assert.equal(page.el("site-pattern-result").hidden, true);
  assert.equal(page.el("site-pattern-rows").textContent, "");

  let finish;
  const held = new Promise((resolve) => { finish = resolve; });
  const second = await install({ tab: { id: 1, url, title: "A" }, script: () => held });
  await until(() => !second.el("site-pattern-review").disabled, "pattern review ready");
  second.el("site-pattern-review").click();
  await until(() => second.state.scripts.length === 1, "the held script");
  second.tabUpdated({ url: "https://unknown.example/items/2" });
  second.tabUpdated({ url });
  finish({ current: "/items/{num}", links: [{ shape: "/stale/{num}", count: 1 }] });
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(second.el("site-pattern-result").hidden, true);
  assert.equal(second.el("site-pattern-rows").textContent, "");
});

test("a failed visited-pattern read still shows the page's own patterns", async () => {
  const page = await install({ tab: { id: 1, url: "https://unknown.example/items/1", title: "A" },
    script: async () => ({ current: "/items/{num}", links: [{ shape: "/items/{num}/{slug}", count: 2 }] }),
    patternsError: "Pending site removals are unreadable",
  });
  await until(() => !page.el("site-pattern-review").disabled, "pattern review ready");
  page.el("site-pattern-review").click();
  await until(() => page.el("site-pattern-rows").textContent.includes("Link pattern"), "pattern results");
  assert.match(page.el("site-pattern-message").textContent, /Review only/);
  assert.match(page.el("site-pattern-rows").textContent, /Current page: \/items\/\{num\}/);
  assert.match(page.el("site-pattern-rows").textContent, /Visited patterns unavailable: .*unreadable/);
});

test("navigating or canceling a held pattern read drops its result and allows a fresh review", async () => {
  for (const cancel of [false, true]) {
    let finish;
    const held = new Promise((resolve) => { finish = resolve; });
    const page = await install({ tab: { id: 1, url: "https://unknown.example/items/1", title: "A" }, script: () => held });
    await until(() => !page.el("site-pattern-review").disabled, "pattern review ready");
    page.el("site-pattern-review").click();
    await until(() => page.state.scripts.length === 1, "the held script");
    if (cancel) page.el("site-pattern-close").click();
    else {
      page.navigate("https://other.example/pages/2", 2);
      await until(() => page.el("page-url").textContent.includes("other.example"), "page switch");
    }
    finish({ current: "/old/{num}", links: [{ shape: "/stale/{num}", count: 1 }] });
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.equal(page.el("site-pattern-result").hidden, true);
    assert.equal(page.el("site-pattern-rows").textContent, "");
    assert.equal(page.state.workerRequests.filter((request) => request.kind === "get_site_patterns").length, 0);
    assert.equal(page.state.requests.filter((request) => request.type === "update_config").length, 0);
  }
});

test("stale script output and scripting failures show an honest error without enabling capture", async () => {
  for (const script of [async () => ({ current: null, links: [] }), async () => { throw new Error("Site access withdrawn"); }]) {
    const page = await install({ tab: { id: 1, url: "https://unknown.example/items/1", title: "A" }, script });
    await until(() => !page.el("site-pattern-review").disabled, "pattern review ready");
    page.el("site-pattern-review").click();
    await until(() => page.el("site-pattern-message").textContent.startsWith("Could not review"), "inspection error");
    assert.equal(page.el("site-pattern-rows").textContent, "");
    assert.equal(page.state.config.capture_enabled, false);
  }
});

test("collapsing page patterns clears the review's link shapes", async () => {
  const page = await install({ tab: { id: 1, url: "https://unknown.example/items/1" } });
  await until(() => !page.el("site-pattern-review").disabled, "pattern review ready");
  page.el("site-pattern-area").open = true;
  page.el("site-pattern-area").dispatch("toggle");
  page.el("site-pattern-review").click();
  await until(() => page.el("site-pattern-rows").textContent.includes("Current page"), "the pattern review");
  page.el("site-pattern-area").open = false;
  page.el("site-pattern-area").dispatch("toggle");
  assert.equal(page.el("site-pattern-result").hidden, true);
  assert.equal(page.el("site-pattern-rows").textContent, "");
});

test("private and nonweb tabs never offer page-pattern inspection", async () => {
  for (const tab of [{ id: 1, url: "https://unknown.example/items/1", incognito: true }, { id: 1, url: "chrome://settings/" }, { id: 1, url: undefined }]) {
    const page = await install({ tab });
    await until(() => page.calls("tabs.query") >= 1 && page.el("status").textContent !== "Connecting", "page state");
    assert.equal(page.el("site-pattern-area").hidden, true);
    assert.equal(page.state.scripts.length, 0);
  }
});
