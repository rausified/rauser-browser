import assert from "node:assert/strict";
import test from "node:test";

// Run after build:extension.
const { REUSE_KEY, SELECT_KEY, openTasksPage, sendReuse, takeReuse } = await import("../dist/task-handoff.js");

function storageArea(entries = new Map()) {
  return {
    entries,
    async get(keys) {
      const result = {};
      for (const name of Array.isArray(keys) ? keys : [keys]) {
        if (entries.has(name)) result[name] = structuredClone(entries.get(name));
      }
      return result;
    },
    async set(items) {
      for (const [name, value] of Object.entries(items)) entries.set(name, structuredClone(value));
    },
    async remove(keys) {
      for (const name of Array.isArray(keys) ? keys : [keys]) entries.delete(name);
    },
  };
}

// A fake Chrome with only runtime, tabs, windows and storage.session, which
// is all openTasksPage uses. `contexts` answers runtime.getContexts.
function fakeChrome(contexts = []) {
  const updatedTabs = [];
  const updatedWindows = [];
  const created = [];
  const session = storageArea();
  return {
    calls: { updatedTabs, updatedWindows, created },
    session,
    runtime: {
      getURL: (path) => `chrome-extension://test-id/${path}`,
      getContexts: async () => contexts,
    },
    tabs: {
      async update(tabId, updateProperties) {
        updatedTabs.push({ tabId, ...updateProperties });
        return { id: tabId, incognito: false, ...updateProperties };
      },
      async create(properties) {
        created.push(properties);
        return { id: 99, incognito: false, ...properties };
      },
    },
    windows: {
      async update(windowId, updateInfo) {
        updatedWindows.push({ windowId, ...updateInfo });
      },
    },
    storage: { session },
  };
}

test("sendReuse stores the instruction, the window and a timestamp", async () => {
  const storage = storageArea();
  const before = Date.now();
  await sendReuse(storage, "Summarize this", 7);
  const stored = storage.entries.get(REUSE_KEY);
  assert.equal(stored.instruction, "Summarize this");
  assert.equal(stored.window_id, 7);
  assert.ok(stored.at >= before);

  await sendReuse(storage, "Any window", null);
  assert.equal(storage.entries.get(REUSE_KEY).window_id, null);
});

test("takeReuse takes a value for its window or for any window, once", async () => {
  const forThisWindow = storageArea(new Map([[REUSE_KEY, { instruction: "For window 7", window_id: 7, at: 1 }]]));
  assert.equal(await takeReuse(forThisWindow, 7), "For window 7");
  assert.equal(forThisWindow.entries.has(REUSE_KEY), false);
  // Taking it again finds nothing: it is gone.
  assert.equal(await takeReuse(forThisWindow, 7), null);

  const forAnyWindow = storageArea(new Map([[REUSE_KEY, { instruction: "For any window", window_id: null, at: 1 }]]));
  assert.equal(await takeReuse(forAnyWindow, 42), "For any window");
  assert.equal(forAnyWindow.entries.has(REUSE_KEY), false);
});

test("takeReuse leaves another window's value", async () => {
  const storage = storageArea(new Map([[REUSE_KEY, { instruction: "For window 7", window_id: 7, at: 1 }]]));
  assert.equal(await takeReuse(storage, 8), null);
  assert.equal(storage.entries.has(REUSE_KEY), true);
  // It is still there for the window it was meant for.
  assert.equal(await takeReuse(storage, 7), "For window 7");
});

test("takeReuse ignores a missing or malformed value", async () => {
  assert.equal(await takeReuse(storageArea(), 1), null);
  const malformed = storageArea(new Map([[REUSE_KEY, { instruction: 42, window_id: 7, at: 1 }]]));
  assert.equal(await takeReuse(malformed, 7), null);
});

const RUN_ID = "00000000-0000-4000-8000-000000000042";

test("openTasksPage focuses an open Tasks tab with the run hash", async () => {
  const chromeApi = fakeChrome([
    { documentUrl: "chrome-extension://test-id/tasks.html#run=00000000-0000-4000-8000-000000000001", tabId: 3, windowId: 7 },
  ]);
  await openTasksPage(chromeApi, RUN_ID);
  assert.deepEqual(chromeApi.calls.created, []);
  assert.deepEqual(chromeApi.calls.updatedTabs, [
    { tabId: 3, active: true, url: `chrome-extension://test-id/tasks.html#run=${RUN_ID}` },
  ]);
  assert.deepEqual(chromeApi.calls.updatedWindows, [{ windowId: 7, focused: true }]);
});

test("openTasksPage opens a new tab when none is open", async () => {
  const noTabs = fakeChrome([]);
  await openTasksPage(noTabs, null);
  assert.deepEqual(noTabs.calls.created, [{ url: "chrome-extension://test-id/tasks.html" }]);
  assert.deepEqual(noTabs.calls.updatedTabs, []);
  assert.deepEqual(noTabs.calls.updatedWindows, []);
  assert.equal(noTabs.session.entries.has(SELECT_KEY), false);

  // A context for another extension page does not count as a Tasks tab.
  const otherPage = fakeChrome([{ documentUrl: "chrome-extension://test-id/panel.html", tabId: 1, windowId: 1 }]);
  await openTasksPage(otherPage, RUN_ID);
  assert.deepEqual(otherPage.calls.created, [{ url: `chrome-extension://test-id/tasks.html#run=${RUN_ID}` }]);
  assert.equal(otherPage.session.entries.has(SELECT_KEY), false);
});

test("openTasksPage signals the open Tasks tab to select the run", async () => {
  const chromeApi = fakeChrome([
    { documentUrl: "chrome-extension://test-id/tasks.html#run=00000000-0000-4000-8000-000000000001", tabId: 3, windowId: 7 },
  ]);
  const before = Date.now();
  await openTasksPage(chromeApi, RUN_ID);
  const signal = chromeApi.session.entries.get(SELECT_KEY);
  assert.equal(signal.run_id, RUN_ID);
  assert.ok(signal.at >= before);

  // The newest-task case (null) does not need a signal: the tab's own
  // hash already names no run, same as a freshly opened one.
  const again = fakeChrome([
    { documentUrl: "chrome-extension://test-id/tasks.html#run=00000000-0000-4000-8000-000000000001", tabId: 3, windowId: 7 },
  ]);
  await openTasksPage(again, null);
  assert.equal(again.session.entries.has(SELECT_KEY), false);
});
