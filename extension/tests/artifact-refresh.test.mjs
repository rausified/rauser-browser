import assert from "node:assert/strict";
import test from "node:test";

// Run after build:extension.
const { ArtifactRefresher, deliveredSince, followsTabReplacement, followsTabUpdate } = await import("../dist/artifact-refresh.js");

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

async function settle() {
  for (let index = 0; index < 20; index += 1) await Promise.resolve();
}

function harness() {
  const state = { page: null, calls: [], rendered: [], failed: [] };
  const refresher = new ArtifactRefresher(
    (page) => {
      const pending = deferred();
      state.calls.push({ page, ...pending });
      return pending.promise;
    },
    () => state.page,
    (page, result) => state.rendered.push({ page, result }),
    (page, error) => state.failed.push({ page, error }),
  );
  return { state, refresher };
}

test("only the followed tab's title, address, or load completion refreshes", () => {
  assert.equal(followsTabUpdate(7, { title: "Inbox (3)" }, { active: false }, 3), false);
  assert.equal(followsTabUpdate(7, { status: "complete" }, { active: true }, 3), false);
  assert.equal(followsTabUpdate(3, { title: "New title" }, { active: true }, 3), true);
  assert.equal(followsTabUpdate(3, { url: "https://example.com/b" }, { active: true }, 3), true);
  assert.equal(followsTabUpdate(3, { status: "complete" }, { active: true }, 3), true);
  assert.equal(followsTabUpdate(3, { status: "loading" }, { active: true }, 3), false);
  assert.equal(followsTabUpdate(3, { favIconUrl: "x" }, { active: true }, 3), false);
  // Before the panel knows its tab, only an active tab can be the one it shows.
  assert.equal(followsTabUpdate(9, { title: "t" }, { active: true }, null), true);
  assert.equal(followsTabUpdate(9, { title: "t" }, { active: false }, null), false);
});

test("replacing the followed tab (prerender, Instant) is followed", () => {
  // Chrome can swap the active tab without an onActivated event.
  assert.equal(followsTabReplacement(11, 3, 3), true);
  assert.equal(followsTabReplacement(11, 4, 3), false);
  assert.equal(followsTabReplacement(11, 4, null), true);
});

test("a new delivery generation means the worker delivered activity", () => {
  assert.equal(deliveredSince(null, 4), false);
  assert.equal(deliveredSince(4, 4), false);
  assert.equal(deliveredSince(4, 5), true);
  // The counter lives in extension storage, which a purge can reset.
  assert.equal(deliveredSince(4, 0), true);
});

test("repeated refreshes of the page already shown ask the host once", async () => {
  const { state, refresher } = harness();
  state.page = { url: "https://example.com/a", title: "A" };
  for (let index = 0; index < 10; index += 1) {
    // Each tab event builds a fresh but equal page object.
    state.page = { url: "https://example.com/a", title: "A" };
    void refresher.refresh();
  }
  assert.equal(state.calls.length, 1);
  state.calls[0].resolve("card");
  await settle();
  void refresher.refresh();
  assert.equal(state.calls.length, 1, "an answered page is not asked again");
  assert.deepEqual(state.rendered.map((entry) => entry.result), ["card"]);
});

test("an answer for an equal page object is rendered, not discarded", async () => {
  const { state, refresher } = harness();
  state.page = { url: "https://example.com/a", title: "A" };
  void refresher.refresh();
  state.page = { url: "https://example.com/a", title: "A" };
  state.calls[0].resolve("card");
  await settle();
  assert.equal(state.rendered.length, 1);
});

test("a burst of page changes keeps one request in flight and renders the latest", async () => {
  const { state, refresher } = harness();
  for (let index = 0; index < 30; index += 1) {
    state.page = { url: "https://example.com/a", title: `(${index}) A` };
    void refresher.refresh();
  }
  assert.equal(state.calls.length, 1);
  state.calls[0].resolve("old");
  await settle();
  assert.equal(state.calls.length, 2, "one trailing request follows the burst");
  assert.equal(state.calls[1].page.title, "(29) A");
  assert.equal(state.rendered.length, 0, "a superseded page's answer is not shown");
  state.calls[1].resolve("latest");
  await settle();
  assert.deepEqual(state.rendered.map((entry) => entry.result), ["latest"]);
  assert.equal(state.calls.length, 2);
});

test("a forced refresh asks again and resolves after the newer answer renders", async () => {
  const { state, refresher } = harness();
  state.page = { url: "https://example.com/a", title: "A" };
  void refresher.refresh();
  state.calls[0].resolve("before");
  await settle();
  let done = false;
  const forced = refresher.refresh(true).then(() => { done = true; });
  assert.equal(state.calls.length, 2);
  await settle();
  assert.equal(done, false);
  state.calls[1].resolve("after");
  await forced;
  assert.deepEqual(state.rendered.map((entry) => entry.result), ["before", "after"]);
});

test("a forced refresh during a request waits for a fresh answer", async () => {
  const { state, refresher } = harness();
  state.page = { url: "https://example.com/a", title: "A" };
  void refresher.refresh();
  let done = false;
  const forced = refresher.refresh(true).then(() => { done = true; });
  state.calls[0].resolve("stale");
  await settle();
  assert.equal(state.calls.length, 2, "the in-flight answer predates the forced refresh");
  assert.equal(done, false);
  state.calls[1].resolve("fresh");
  await forced;
  assert.equal(state.rendered.at(-1).result, "fresh");
});

test("clearing forgets the shown page and drops answers for it", async () => {
  const { state, refresher } = harness();
  state.page = { url: "https://example.com/a", title: "A" };
  void refresher.refresh();
  state.page = null;
  refresher.clear();
  state.calls[0].resolve("late");
  await settle();
  assert.equal(state.rendered.length, 0);
  state.page = { url: "https://example.com/a", title: "A" };
  void refresher.refresh();
  assert.equal(state.calls.length, 2, "returning to the page asks again");
});

test("failures are reported for the current page and do not block a retry", async () => {
  const { state, refresher } = harness();
  state.page = { url: "https://example.com/a", title: "A" };
  void refresher.refresh();
  state.calls[0].reject(new Error("busy"));
  await settle();
  assert.equal(state.failed.length, 1);
  void refresher.refresh();
  assert.equal(state.calls.length, 2, "a failed page is asked again on the next refresh");
});
