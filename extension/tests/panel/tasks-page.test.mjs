// Built Tasks page tests (tasks.html / tasks.js). Run after build:extension;
// the fake DOM elements and shared helpers are in ./harness.mjs. Each test
// loads a fresh copy of the page against a scripted native host.
import assert from "node:assert/strict";
import test, { mock } from "node:test";
import {
  ARTICLE_URL, FakeElement, NOT_SET_UP, PROTOCOL_VERSION, READY, RUN_VIEW, SET_UP, TASKS_HTML, buttonIn,
  descendants, hostReply, install, pageElements, storageArea, storageKey, teardown,
} from "./harness.mjs";

const { serviceSetupText } = await import("../../dist/agent.js");
const { TASK_NEEDS_HARNESS_MESSAGE } = await import("../../dist/task.js");

const RUNS_MAY_EXIST_KEY = storageKey("runs_may_exist_v1");
const DISMISSED_KEY = storageKey("dismissed_runs_v1");
const SELECT_KEY = storageKey("tasks_select_v1");
const MINUTE = 60_000;
const BASE = Date.now();
const LIST_REFRESH_MS = 2000;

// Advance only when a test asks for a poll. Draining zero-delay transport
// replies keeps the real page controller in use without wall-clock waits.
async function settle(ms = 0) {
  mock.timers.tick(ms);
  for (let turn = 0; turn < 20; turn += 1) {
    await new Promise((resolve) => setImmediate(resolve));
    mock.timers.tick(0);
  }
}

async function until(condition, what) {
  for (let turn = 0; turn < 100; turn += 1) {
    if (condition()) return;
    await settle();
  }
  assert.fail(`Timed out waiting for ${what}`);
}

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}

test.beforeEach(() => mock.timers.enable({ apis: ["setTimeout"] }));
test.afterEach(() => mock.timers.reset());

function runId(n) {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

// Task n was created n minutes ago, so a lower n is newer.
function task(n, overrides = {}) {
  const created = new Date(BASE - n * MINUTE).toISOString();
  const state = overrides.state ?? "succeeded";
  return {
    ...RUN_VIEW,
    run_id: runId(n), kind: "task", instruction: `Task ${n}`, task_report: null,
    url: "https://work.example/page", title: "Work page",
    created_at: created,
    started_at: state === "queued" ? null : created,
    finished_at: ["succeeded", "failed", "canceled"].includes(state) ? new Date(BASE - n * MINUTE + 30_000).toISOString() : null,
    ...overrides,
    state,
  };
}

let loads = 0;

// Loads tasks.js. `runs` is what the service holds; list_runs pages it
// `pageSize` at a time (by `before`), and get_run answers each run with its
// full instruction. `intercept(request, state)` may answer first.
async function openTasks({
  runs = [task(1), task(2), task(3)], pageSize = 20, agentStatus = READY, local = new Map(),
  hash = "", platform = "MacIntel", intercept = () => null, localGet = null,
  // This page's own window (chrome.windows.getCurrent), shared session
  // storage for Reuse (task-handoff.ts), and whether chrome.sidePanel.open
  // rejects (as it does with no window to open it in).
  windowId = 1, session = new Map(), sidePanelFails = false,
  // Whether chrome.tabs.create rejects (Open page's error path).
  createFails = false,
} = {}) {
  const elements = pageElements(TASKS_HTML);
  // A select's value is its first option's until one is chosen.
  elements.get("tasks-filter").value = "all";
  const state = {
    runs: structuredClone(runs), requests: [], created: [], counts: new Map(), local,
    session, hidden: false, sidePanelOpens: [], pageSize,
    // What get_config reports; a test may change it, as setting up a harness does.
    agentStatus,
  };
  const count = (name) => state.counts.set(name, (state.counts.get(name) ?? 0) + 1);
  const docListeners = new Map();
  const windowListeners = new Map();
  const on = (map) => (type, listener) => map.set(type, [...(map.get(type) ?? []), listener]);
  const storageListeners = [];

  globalThis.document = {
    getElementById: (id) => elements.get(id) ?? null,
    // Focus is tracked, so a test can see which element holds it.
    createElement: (tag) => {
      const created = new FakeElement(tag);
      created.focus = () => { created.focused = true; state.active = created; };
      return created;
    },
    get activeElement() { return state.active ?? null; },
    get visibilityState() { return state.hidden ? "hidden" : "visible"; },
    get hidden() { return state.hidden; },
    addEventListener: on(docListeners),
  };
  const location = { hash };
  globalThis.location = location;
  globalThis.history = {
    replaceState(_state, _title, url) {
      const at = String(url).indexOf("#");
      location.hash = at === -1 ? "" : String(url).slice(at);
    },
  };
  globalThis.window = {
    addEventListener: on(windowListeners),
    location,
  };
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: {
    platform,
    locks: { request: async (_name, _options, action) => action() },
    clipboard: { async writeText() {} },
  } });

  function reply(request) {
    if (request.type === "get_config") return { ...hostReply(request), agent_status: state.agentStatus };
    if (request.type === "list_runs") {
      const sorted = [...state.runs].sort((a, b) => (a.created_at < b.created_at ? 1 : -1));
      const start = request.before
        ? sorted.findIndex((run) => run.run_id === request.before.run_id) + 1
        : 0;
      const page = sorted.slice(start, start + state.pageSize).map((run) => ({ ...run, task_report: null }));
      return { type: "runs_listed", runs: page, more: start + state.pageSize < sorted.length };
    }
    if (request.type === "get_run") {
      const run = state.runs.find((candidate) => candidate.run_id === request.run_id);
      if (!run) return { type: "error", code: "invalid_request", message: "unknown run" };
      return { type: "run_fetched", run, stream_text: "", next_offset: 0, more: false };
    }
    return hostReply(request);
  }

  globalThis.chrome = {
    runtime: {
      id: "test-id",
      lastError: undefined,
      getManifest: () => ({ options_ui: { page: "options.html" } }),
      getURL: (path) => `chrome-extension://test-id/${path}`,
      openOptionsPage: async () => { count("openOptionsPage"); },
      sendMessage: async () => ({ ok: true, value: null }),
      onMessage: { addListener() {} },
      connectNative() {
        const messageListeners = [];
        let disconnected = false;
        const port = {
          onMessage: { addListener(listener) { messageListeners.push(listener); } },
          onDisconnect: { addListener() {} },
          postMessage(request) {
            state.requests.push(structuredClone(request));
            setTimeout(async () => {
              const answer = (await intercept(request, state)) ?? reply(request);
              for (const listener of disconnected ? [] : messageListeners) {
                listener({ ...answer, protocol_version: PROTOCOL_VERSION, request_id: request.request_id });
              }
            }, 0);
          },
          disconnect() { disconnected = true; },
        };
        return port;
      },
    },
    storage: {
      local: localGet ? { ...storageArea(local), get: localGet } : storageArea(local),
      session: storageArea(state.session),
      onChanged: { addListener(listener) { storageListeners.push(listener); } },
    },
    tabs: {
      async create(properties) {
        state.created.push(properties);
        if (createFails) throw new Error("Tabs cannot be edited right now.");
        return { id: 99, incognito: false, ...properties };
      },
    },
    windows: {
      async getCurrent() {
        return { id: windowId };
      },
    },
    sidePanel: {
      async open(options) {
        state.sidePanelOpens.push(options);
        if (sidePanelFails) throw new Error("No window to open the side panel in.");
      },
    },
  };

  loads += 1;
  await import(`../../dist/tasks.js?load=${loads}`);
  const page = {
    state,
    location,
    el(id) {
      const found = elements.get(id);
      if (!found) throw new Error(`No element ${id} in tasks.html`);
      return found;
    },
    sent: (type) => state.requests.filter((request) => request.type === type),
    calls: (name) => state.counts.get(name) ?? 0,
    rows: () => descendants(elements.get("tasks-list")).filter((node) => (node.className ?? "").split(" ").includes("task-row")),
    selected: () => page.rows().filter((row) => row.getAttribute("aria-selected") === "true").map((row) => row.dataset.runId),
    detailId: () => elements.get("tasks-detail").children[0]?.dataset.runId,
    pill: (id) => descendants(page.rows().find((row) => row.dataset.runId === id))
      .find((node) => (node.className ?? "").split(" ").includes("task-pill")),
    setHidden(hidden) {
      state.hidden = hidden;
      for (const listener of docListeners.get("visibilitychange") ?? []) listener({ type: "visibilitychange" });
    },
    hashChanged(next) {
      location.hash = next;
      for (const listener of windowListeners.get("hashchange") ?? []) listener({ type: "hashchange" });
    },
    close() {
      state.hidden = true;
      for (const listener of windowListeners.get("pagehide") ?? []) listener({ type: "pagehide" });
    },
    // A select signal (task-handoff.ts) another Tasks tab wrote for this
    // one, the way `openTasksPage` sends Tasks ↗ to an already-open tab.
    selectSignal(runId) {
      const value = { run_id: runId, at: Date.now() };
      state.session.set(SELECT_KEY, value);
      for (const listener of storageListeners) listener({ [SELECT_KEY]: { newValue: value } }, "session");
    },
  };
  // A loaded page keeps polling; hiding it at the end of each test stops it.
  teardown.push(() => page.close());
  return page;
}

const listed = (page) => until(() => page.rows().length > 0, "the task list");

// Polling is driven independently of zero-delay host replies.
async function afterPoll(condition, what) {
  for (let poll = 0; poll < 5; poll += 1) {
    await settle(LIST_REFRESH_MS);
    if (condition()) return;
  }
  assert.fail(`Timed out waiting for ${what}`);
}

test("the page lists every page of tasks and selects the newest", async () => {
  const page = await openTasks({ runs: [task(1), task(2), task(3), task(4), task(5)], pageSize: 2 });
  await until(() => page.rows().length === 5, "all five tasks");
  const lists = page.sent("list_runs");
  assert.equal(lists.length, 3);
  for (const request of lists) assert.equal(request.kind, "task");
  assert.equal(lists[0].before, undefined);
  assert.deepEqual(lists[1].before, { created_at: task(2).created_at, run_id: runId(2) });
  assert.deepEqual(page.rows().map((row) => row.dataset.runId), [1, 2, 3, 4, 5].map(runId));
  assert.deepEqual(page.selected(), [runId(1)]);
  await until(() => page.detailId() === runId(1), "the newest task's detail");
  assert.equal(page.location.hash, `#run=${runId(1)}`);
  assert.equal(page.el("tasks-empty").hidden, true);
  assert.equal(page.el("tasks-layout").hidden, false);
  const row = page.rows()[0];
  assert.equal(row.getAttribute("role"), "option");
  for (const group of descendants(page.el("tasks-list")).filter((node) => node.getAttribute("role") === "group")) {
    const label = group.children[0];
    assert.equal(group.getAttribute("aria-labelledby"), label.id);
    assert.equal(label.getAttribute("role"), "presentation", "listbox group names must not introduce heading children");
  }
  assert.match(row.textContent, /Task 1/);
  assert.match(page.pill(runId(1)).className, /\bdone\b/);
  assert.match(page.pill(runId(1)).textContent, /^Done · 30s$/);
});

test("the page says when history is incomplete", async () => {
  // A host that always answers more: true with a distinct, advancing
  // cursor, so the walk never finds a last page and stops only at
  // listAllTasks' page cap (100), leaving real history unread.
  let served = 0;
  const page = await openTasks({
    intercept: (request) => {
      if (request.type !== "list_runs") return null;
      served += 1;
      return { type: "runs_listed", runs: [task(served)], more: true };
    },
  });
  await until(() => page.rows().length === 100, "every page up to the cap");
  assert.equal(page.el("tasks-status").textContent, "Showing the newest 100 tasks. Older history could not be loaded.");
  assert.equal(page.el("tasks-status-box").hidden, false);
  assert.equal(page.el("tasks-status-settings").hidden, true);

  // A later refresh (shown again after being hidden) must not clear the
  // note: it is not a list error.
  page.setHidden(true);
  page.setHidden(false);
  await settle(50);
  assert.equal(page.el("tasks-status").textContent, "Showing the newest 100 tasks. Older history could not be loaded.");
  assert.equal(page.el("tasks-status-box").hidden, false);
});

test("#run=<id> selects that task", async () => {
  const page = await openTasks({ hash: `#run=${runId(3)}` });
  await listed(page);
  assert.deepEqual(page.selected(), [runId(3)]);
  await until(() => page.detailId() === runId(3), "task 3's detail");
  assert.equal(page.location.hash, `#run=${runId(3)}`);

  page.hashChanged(`#run=${runId(2)}`);
  await until(() => page.detailId() === runId(2), "task 2's detail after the hash changes");
  assert.deepEqual(page.selected(), [runId(2)]);
});

test("a select signal moves the selection with no task active and no hashchange", async () => {
  const page = await openTasks();
  await listed(page);
  assert.deepEqual(page.selected(), [runId(1)]);
  // Every task is terminal (nothing polling); the hash changes the way
  // `tabs.update`'s own hash-only navigation does here, with no
  // `hashchange` event, so only the storage signal moves the selection.
  page.location.hash = `#run=${runId(2)}`;
  page.selectSignal(runId(2));
  await until(() => page.detailId() === runId(2), "task 2's detail after the select signal");
  assert.deepEqual(page.selected(), [runId(2)]);
});

test("a select signal for a run not yet listed refreshes, then selects it", async () => {
  const page = await openTasks({ runs: [task(1), task(2)] });
  await listed(page);
  assert.deepEqual(page.selected(), [runId(1)]);
  assert.equal(page.sent("list_runs").length, 1);
  // A run started after this page's list, which it does not know about yet.
  page.state.runs.push(task(0));
  page.selectSignal(runId(0));
  await until(() => page.detailId() === runId(0), "the newly started task's detail");
  assert.deepEqual(page.selected(), [runId(0)]);
  assert.equal(page.location.hash, `#run=${runId(0)}`);
  // One refresh found it; the signal does not poll repeatedly.
  assert.equal(page.sent("list_runs").length, 2);
});

test("an unknown #run= selects the newest task", async () => {
  for (const hash of [`#run=${runId(99)}`, "#run=<script>", "#other"]) {
    const page = await openTasks({ hash });
    await listed(page);
    assert.deepEqual(page.selected(), [runId(1)], hash);
    await until(() => page.detailId() === runId(1), `the newest task for ${hash}`);
    assert.equal(page.location.hash, `#run=${runId(1)}`);
    assert.equal(page.el("tasks-status-box").hidden, true);
  }
});

for (const firstFails of [false, true]) {
  test(`a hashchange during a held ${firstFails ? "failed" : "successful"} refresh finds a just-started run`, async () => {
    const held = deferred();
    let lists = 0;
    const page = await openTasks({
      runs: [task(1), task(2)],
      intercept: (request) => request.type === "list_runs" && ++lists === 2 ? held.promise : null,
    });
    await listed(page);
    page.setHidden(true);
    page.setHidden(false);
    await until(() => lists === 2, "the held refresh");
    page.state.runs.unshift(task(0));
    page.hashChanged(`#run=${runId(0)}`);
    // Multiple refresh requests during a read need only one follow-up.
    page.setHidden(true);
    page.setHidden(false);
    await settle();
    assert.equal(lists, 2, "no overlapping list request");
    held.resolve(firstFails
      ? { type: "error", code: "internal", message: "disk full" }
      : { type: "runs_listed", runs: [task(1), task(2)], more: false });
    await until(() => page.detailId() === runId(0), "the newly started task");
    assert.deepEqual(page.selected(), [runId(0)]);
    assert.equal(page.location.hash, `#run=${runId(0)}`);
    assert.equal(lists, 3, "exactly one follow-up");
    assert.equal(page.el("tasks-status-box").hidden, true);
  });
}

test("a held refresh follows the latest hash instead of an earlier handler's target", async () => {
  const held = deferred();
  let lists = 0;
  const page = await openTasks({
    runs: [task(1), task(2)],
    intercept: (request) => request.type === "list_runs" && ++lists === 2 ? held.promise : null,
  });
  await listed(page);
  page.setHidden(true);
  page.setHidden(false);
  await until(() => lists === 2, "the held refresh");
  page.state.runs.unshift(task(0));
  page.hashChanged(`#run=${runId(0)}`);
  page.hashChanged(`#run=${runId(2)}`);
  held.resolve({ type: "runs_listed", runs: [task(0), task(1), task(2)], more: false });
  await until(() => lists === 3, "the follow-up");
  await settle();
  assert.deepEqual(page.selected(), [runId(2)]);
  assert.equal(page.location.hash, `#run=${runId(2)}`);
});

test("an initially empty history preserves the requested hash through an intermediate list", async () => {
  const held = deferred();
  let lists = 0;
  const page = await openTasks({
    runs: [],
    intercept: (request) => request.type === "list_runs" && ++lists === 2 ? held.promise : null,
  });
  await until(() => page.el("tasks-empty").hidden === false, "the empty history");
  page.setHidden(true);
  page.setHidden(false);
  await until(() => lists === 2, "the held refresh");
  page.state.runs = [task(0), task(1)];
  page.hashChanged(`#run=${runId(0)}`);
  held.resolve({ type: "runs_listed", runs: [task(1)], more: false });
  await until(() => lists === 3, "the follow-up");
  await settle();
  assert.deepEqual(page.selected(), [runId(0)]);
  assert.equal(page.location.hash, `#run=${runId(0)}`);
});

test("pagehide stops a pending refresh and its queued follow-up", async () => {
  const held = deferred();
  let lists = 0;
  const page = await openTasks({
    runs: [task(1)],
    intercept: (request) => request.type === "list_runs" && ++lists === 2 ? held.promise : null,
  });
  await listed(page);
  page.setHidden(true);
  page.setHidden(false);
  await until(() => lists === 2, "the held refresh");
  page.hashChanged(`#run=${runId(0)}`);
  page.close();
  held.resolve({ type: "runs_listed", runs: [task(0), task(1)], more: false });
  await settle(LIST_REFRESH_MS * 2);
  assert.equal(lists, 2);
  assert.deepEqual(page.rows().map((row) => row.dataset.runId), [runId(1)]);
});

test("the page does not list runs without a harness or a run record", async () => {
  const page = await openTasks({ agentStatus: NOT_SET_UP });
  await until(() => page.el("tasks-status").textContent !== "" && page.sent("get_config").length > 0, "the status");
  await settle();
  assert.equal(page.sent("list_runs").length, 0);
  assert.equal(page.el("tasks-status").textContent, TASK_NEEDS_HARNESS_MESSAGE);
  assert.equal(page.el("tasks-status-box").hidden, false);
  assert.equal(page.el("tasks-status-settings").hidden, false);
  assert.equal(page.el("tasks-layout").hidden, true);
  page.el("tasks-status-settings").click();
  assert.equal(page.calls("openOptionsPage"), 1);

  // With a record that runs may exist, the page lists them anyway.
  const recorded = await openTasks({ agentStatus: NOT_SET_UP, local: new Map([[RUNS_MAY_EXIST_KEY, 1]]) });
  await listed(recorded);
  assert.equal(recorded.sent("list_runs").length, 1);
});

test("the page shows the service setup text with Open settings when the service is unavailable", async () => {
  const page = await openTasks({
    intercept: (request) => request.type === "list_runs"
      ? { type: "error", code: "service_unavailable", message: "The service is not running." }
      : null,
  });
  const text = serviceSetupText("The service is not running.");
  await until(() => page.el("tasks-status").textContent === text, "the service setup text");
  assert.equal(page.el("tasks-status-settings").hidden, false);
  assert.equal(page.el("tasks-layout").hidden, true);
  page.el("tasks-status-settings").click();
  assert.equal(page.calls("openOptionsPage"), 1);
});

test("the page lists tasks once a harness is set up and the page is shown again", async () => {
  const page = await openTasks({ agentStatus: NOT_SET_UP });
  await until(() => page.el("tasks-status").textContent === TASK_NEEDS_HARNESS_MESSAGE, "the setup text");
  await settle();
  assert.equal(page.sent("list_runs").length, 0);
  // The user sets up a harness in settings, then comes back to this tab.
  page.state.agentStatus = READY;
  page.setHidden(true);
  page.setHidden(false);
  await until(() => page.rows().length === 3, "the task list after setup");
  assert.equal(page.el("tasks-status-box").hidden, true);
  assert.deepEqual(page.selected(), [runId(1)]);
  // A second show while the list is up does not open the page again.
  const configs = page.sent("get_config").length;
  page.setHidden(true);
  page.setHidden(false);
  await settle(20);
  assert.equal(page.sent("get_config").length, configs);
});

test("a first list that finds the service unavailable recovers when the page is shown again", async () => {
  let failures = 1;
  const page = await openTasks({
    intercept: (request) => request.type === "list_runs" && failures-- > 0
      ? { type: "error", code: "service_unavailable", message: "The service is not running." }
      : null,
  });
  const text = serviceSetupText("The service is not running.");
  await until(() => page.el("tasks-status").textContent === text, "the service setup text");
  page.setHidden(true);
  page.setHidden(false);
  await until(() => page.rows().length === 3, "the task list once the service answers");
  assert.equal(page.el("tasks-status-box").hidden, true);
  assert.equal(page.el("tasks-status-settings").hidden, true);
  assert.equal(page.el("tasks-layout").hidden, false);
});

test("a select signal before the first list worked opens the page again on that run", async () => {
  let failures = 1;
  const page = await openTasks({
    intercept: (request) => request.type === "list_runs" && failures-- > 0
      ? { type: "error", code: "service_unavailable", message: "The service is not running." }
      : null,
  });
  await until(() => page.el("tasks-status").textContent === serviceSetupText("The service is not running."), "the setup text");
  page.location.hash = `#run=${runId(2)}`;
  page.selectSignal(runId(2));
  await until(() => page.detailId() === runId(2), "task 2's detail after the signal");
  assert.deepEqual(page.selected(), [runId(2)]);
});

test("an active row missing from the first page is fetched once and shows its finish", async () => {
  // Task 3 is still running but the first page (two runs) leaves it out.
  const page = await openTasks({ runs: [task(1), task(2), task(3, { state: "running", stream_bytes: 17 })], pageSize: 2 });
  await until(() => page.rows().length === 3, "all three tasks");
  assert.match(page.pill(runId(3)).className, /\brunning\b/);
  // It finishes before the next refresh.
  page.state.runs[2] = task(3, { state: "succeeded", stream_bytes: 17 });
  await afterPoll(() => /\bdone\b/.test(page.pill(runId(3)).className), "task 3's finish");
  const fetches = page.sent("get_run").filter((request) => request.run_id === runId(3));
  assert.equal(fetches.length, 1);
  assert.equal(fetches[0].since_offset, 17);
  // Nothing is active now, so the page stops polling.
  const lists = page.sent("list_runs").length;
  await settle(LIST_REFRESH_MS * 2);
  assert.equal(page.sent("list_runs").length, lists, "no list refresh once nothing is active");
});

test("an active row the service no longer holds is dropped from the list", async () => {
  const page = await openTasks({ runs: [task(1), task(2), task(3, { state: "running" })], pageSize: 2 });
  await until(() => page.rows().length === 3, "all three tasks");
  // Pruned before the next refresh: get_run answers invalid_request.
  page.state.runs.splice(2, 1);
  await afterPoll(() => page.rows().length === 2, "the pruned task to go");
  assert.equal(page.sent("get_run").filter((request) => request.run_id === runId(3)).length, 1);
});

test("off-page fetches start together and preserve partial successes", async () => {
  const held = deferred();
  let fetching = false;
  const page = await openTasks({
    runs: [task(1), ...[2, 3, 4].map((n) => task(n, { state: "running" }))],
    intercept: (request) => {
      if (!fetching || request.type !== "get_run") return null;
      if (request.run_id === runId(2)) return held.promise;
      if (request.run_id === runId(3)) return { type: "error", code: "internal", message: "disk full" };
      return null;
    },
  });
  await until(() => page.rows().length === 4, "all four tasks");
  await until(() => page.detailId() === runId(1), "the terminal detail");
  fetching = true;
  page.state.pageSize = 1;
  page.state.runs = [task(1), task(2), task(3, { state: "running" })];
  page.setHidden(true);
  page.setHidden(false);
  await until(() => page.sent("get_run").some((request) => request.run_id === runId(4)), "all fetches to start before releasing task 2");
  held.resolve({ type: "run_fetched", run: task(2), stream_text: "", next_offset: 0, more: false });
  await until(() => page.rows().length === 3 && page.pill(runId(2)).textContent.startsWith("Done"), "the successful finish and pruned row");
  assert.match(page.pill(runId(3)).textContent, /^Running/);
  fetching = false;
  page.state.runs[2] = task(3);
  await afterPoll(() => page.pill(runId(3)).textContent.startsWith("Done"), "the failed fetch retried");
});

test("off-page replies after pagehide do not update the list", async () => {
  const held = deferred();
  let fetching = false;
  const page = await openTasks({
    runs: [task(1), task(2, { state: "running" })], pageSize: 1,
    intercept: (request) => fetching && request.type === "get_run" && request.run_id === runId(2) ? held.promise : null,
  });
  await until(() => page.rows().length === 2, "both tasks");
  fetching = true;
  page.setHidden(true);
  page.setHidden(false);
  await until(() => page.sent("get_run").some((request) => request.run_id === runId(2)), "the off-page fetch");
  page.close();
  held.resolve({ type: "run_fetched", run: task(2), stream_text: "", next_offset: 0, more: false });
  await settle(LIST_REFRESH_MS * 2);
  assert.match(page.pill(runId(2)).textContent, /^Running/);
});

test("an older host gets the rebuild text", async () => {
  const page = await openTasks({
    intercept: (request) => request.type === "list_runs"
      ? { type: "error", code: "invalid_request", message: "invalid or unsupported request JSON" }
      : null,
  });
  await until(() => page.el("tasks-status").textContent === "Rebuild the Brauser host to see task history.", "the rebuild text");
  assert.equal(page.el("tasks-status-settings").hidden, true);

  const other = await openTasks({
    intercept: (request) => request.type === "list_runs"
      ? { type: "error", code: "internal", message: "disk full" }
      : null,
  });
  await until(() => other.el("tasks-status").textContent === "Could not list tasks: disk full", "the generic text");
  assert.equal(other.el("tasks-status-settings").hidden, true);
});

test("on Windows the page shows only the Windows text", async () => {
  const page = await openTasks({ platform: "Win32" });
  await until(() => page.el("tasks-status").textContent !== "", "the status");
  await settle();
  assert.equal(page.el("tasks-status").textContent, "Tasks are not supported on Windows yet.");
  assert.equal(page.el("tasks-status-settings").hidden, true);
  assert.equal(page.state.requests.length, 0);
});

test("on Windows the page hides its search and filter controls", async () => {
  const page = await openTasks({ platform: "Win32" });
  await until(() => page.el("tasks-status").textContent !== "", "the status");
  await settle();
  assert.equal(page.el("tasks-controls").hidden, true);
});

test("panel.css hides a hidden .tasks-controls despite its display rule", async () => {
  // `.tasks-page .tasks-controls { display: flex }` outranks Chrome's
  // [hidden] style, so the controls need their own rule or the Windows
  // hide above hides nothing on screen.
  const { readFile } = await import("node:fs/promises");
  const css = await readFile(new URL("../../panel.css", import.meta.url), "utf8");
  assert.match(css, /\.tasks-page \.tasks-controls\[hidden\][^{]*\{\s*display:\s*none;/);
});

test("the page says so when there are no tasks", async () => {
  const page = await openTasks({ runs: [] });
  await until(() => page.el("tasks-empty").hidden === false, "the empty text");
  assert.equal(page.el("tasks-empty").textContent, "No tasks yet. Start one from the side panel.");
  assert.equal(page.el("tasks-layout").hidden, true);
});

test("search and status filter narrow the list; Running includes queued", async () => {
  const page = await openTasks({
    runs: [
      task(1, { state: "queued", instruction: "Alpha queued" }),
      task(2, { state: "running", instruction: "Beta running" }),
      task(3, { instruction: "Alpha done" }),
      task(4, { state: "failed", error: "boom", instruction: "Gamma failed", title: "Alpha page" }),
      task(5, { state: "canceled", instruction: "Delta canceled" }),
    ],
  });
  await until(() => page.rows().length === 5, "all tasks");
  const ids = () => page.rows().map((row) => row.dataset.runId);
  const filter = (value) => {
    page.el("tasks-filter").value = value;
    page.el("tasks-filter").dispatch("change");
  };
  filter("running");
  assert.deepEqual(ids(), [runId(1), runId(2)]);
  filter("done");
  assert.deepEqual(ids(), [runId(3)]);
  filter("failed");
  assert.deepEqual(ids(), [runId(4)]);
  filter("canceled");
  assert.deepEqual(ids(), [runId(5)]);
  filter("all");
  page.el("tasks-search").value = "alpha";
  page.el("tasks-search").dispatch("input");
  // The title matches too.
  assert.deepEqual(ids(), [runId(1), runId(3), runId(4)]);
  filter("running");
  assert.deepEqual(ids(), [runId(1)]);
  assert.match(page.pill(runId(1)).className, /\brunning\b/);
  assert.equal(page.pill(runId(1)).textContent, "Queued");
  page.el("tasks-search").value = "no matching task";
  page.el("tasks-search").dispatch("input");
  assert.deepEqual(ids(), []);
  const none = page.el("tasks-none");
  assert.equal(none.textContent, "No tasks match.");
  assert.equal(none.hidden, false);
  assert.equal(page.el("tasks-list").contains(none), false, "the empty message belongs outside the listbox");
  assert.equal(page.el("tasks-list").hidden, true, "a listbox without options must not be exposed");
  assert.equal(page.el("tasks-layout").hidden, false, "the selected detail stays available");
  page.el("tasks-search").value = "";
  page.el("tasks-search").dispatch("input");
  assert.equal(none.hidden, true);
  assert.equal(page.el("tasks-list").hidden, false);
  assert.equal(ids().length, 2);
});

test("the page stops polling while hidden and refreshes on show", async () => {
  const page = await openTasks({ runs: [task(1, { state: "running" }), task(2)] });
  await listed(page);
  await afterPoll(() => page.sent("list_runs").length >= 2, "a list refresh while a task runs");
  page.setHidden(true);
  await settle(20);
  const before = page.sent("list_runs").length;
  await settle(LIST_REFRESH_MS * 2);
  assert.equal(page.sent("list_runs").length, before, "no list refresh while hidden");
  page.state.runs.unshift(task(0));
  page.setHidden(false);
  await until(() => page.sent("list_runs").length === before + 1, "one refresh on show");
  await until(() => page.rows().some((row) => row.dataset.runId === runId(0)), "the new task in the list");
  assert.equal(page.sent("list_runs").at(-1).before, undefined);
});

test("a task finishing updates its row pill", async () => {
  const page = await openTasks({ runs: [task(1, { state: "running" }), task(2)] });
  await listed(page);
  await until(() => page.detailId() === runId(1), "the running task's detail");
  assert.match(page.pill(runId(1)).className, /\brunning\b/);
  assert.match(page.pill(runId(1)).textContent, /^Running/);
  page.state.runs[0] = task(1, { state: "failed", error: "boom" });
  await afterPoll(() => /\bfailed\b/.test(page.pill(runId(1)).className), "the failed pill");
  assert.equal(page.pill(runId(1)).textContent, "Failed");
  // A finished task moves from Running to its day.
  const groups = descendants(page.el("tasks-list")).filter((node) => node.tagName === "H3");
  assert.deepEqual(groups.map((node) => node.textContent), ["Today"]);
});

test("Dismiss removes the row unless Show dismissed is on", async () => {
  const page = await openTasks();
  await listed(page);
  await until(() => page.detailId() === runId(1), "the newest task's detail");
  const dismiss = () => buttonIn(page.el("tasks-detail"), "Dismiss");
  await until(() => dismiss(), "the Dismiss button");
  dismiss().click();
  await until(() => !page.rows().some((row) => row.dataset.runId === runId(1)), "the row to go");
  assert.deepEqual(page.state.local.get(DISMISSED_KEY), [runId(1)]);
  // The next task is selected.
  assert.deepEqual(page.selected(), [runId(2)]);

  page.el("tasks-show-dismissed").checked = true;
  page.el("tasks-show-dismissed").dispatch("change");
  assert.deepEqual(page.rows().map((row) => row.dataset.runId), [1, 2, 3].map(runId));

  // A dismissed task stays hidden on the next load.
  const again = await openTasks({ local: page.state.local });
  await listed(again);
  assert.deepEqual(again.rows().map((row) => row.dataset.runId), [2, 3].map(runId));
});

test("arrow keys move the selection", async () => {
  const page = await openTasks();
  await listed(page);
  const key = (name) => {
    let prevented = false;
    page.el("tasks-list").dispatch("keydown", { key: name, preventDefault: () => { prevented = true; } });
    return prevented;
  };
  assert.equal(key("ArrowDown"), true);
  assert.deepEqual(page.selected(), [runId(2)]);
  await until(() => page.detailId() === runId(2), "task 2's detail");
  assert.equal(page.location.hash, `#run=${runId(2)}`);
  key("ArrowDown");
  key("ArrowDown");
  assert.deepEqual(page.selected(), [runId(3)], "the last row stays selected");
  key("ArrowUp");
  assert.deepEqual(page.selected(), [runId(2)]);
  page.rows()[0].click();
  assert.deepEqual(page.selected(), [runId(1)]);
  const third = page.rows()[2];
  page.el("tasks-list").dispatch("keydown", { key: "Enter", target: third, preventDefault() {} });
  assert.deepEqual(page.selected(), [runId(3)]);
});

test("Open page opens the task's own URL and Reuse opens the side panel and hands off the instruction", async () => {
  const page = await openTasks({ windowId: 9 });
  await listed(page);
  await until(() => buttonIn(page.el("tasks-detail"), "Reuse"), "the Reuse button");
  buttonIn(page.el("tasks-detail"), "Open page").click();
  assert.deepEqual(page.state.created, [{ url: "https://work.example/page" }]);
  buttonIn(page.el("tasks-detail"), "Reuse").click();
  await until(() => page.el("tasks-status").textContent === "Instruction sent to the task bar.", "the Reuse text");
  // The side panel was opened for this page's own window, read at load.
  assert.deepEqual(page.state.sidePanelOpens, [{ windowId: 9 }]);
  const handed = page.state.session.get(storageKey("task_reuse_v1"));
  assert.equal(handed.instruction, "Task 1");
  assert.equal(handed.window_id, 9);
  assert.equal(typeof handed.at, "number");
});

test("an Open page error stays after a refresh", async () => {
  const page = await openTasks({ runs: [task(1), task(2, { state: "running" })], createFails: true });
  await listed(page);
  await until(() => buttonIn(page.el("tasks-detail"), "Open page"), "the Open page button");
  buttonIn(page.el("tasks-detail"), "Open page").click();
  const text = "Could not open the page: Tabs cannot be edited right now.";
  await until(() => page.el("tasks-status").textContent === text, "the Open page error");
  // Task 2 keeps the page polling; a refresh that works leaves the line.
  const lists = page.sent("list_runs").length;
  await afterPoll(() => page.sent("list_runs").length > lists, "a list refresh");
  await settle(20);
  assert.equal(page.el("tasks-status").textContent, text);
  assert.equal(page.el("tasks-status-box").hidden, false);
});

test("a list error clears once a refresh works", async () => {
  let failures = 0;
  const page = await openTasks({
    runs: [task(1, { state: "running" }), task(2)],
    intercept: (request) => request.type === "list_runs" && failures-- > 0
      ? { type: "error", code: "internal", message: "disk full" }
      : null,
  });
  await listed(page);
  failures = 1;
  await afterPoll(() => page.el("tasks-status").textContent === "Could not list tasks: disk full", "the list error");
  // The next poll works and clears it.
  await afterPoll(() => page.el("tasks-status-box").hidden, "the list error to clear");
});

test("Reuse still hands off the instruction, to any window, when opening the side panel fails", async () => {
  const session = new Map();
  const page = await openTasks({ sidePanelFails: true, windowId: 9, session });
  await listed(page);
  await until(() => buttonIn(page.el("tasks-detail"), "Reuse"), "the Reuse button");
  buttonIn(page.el("tasks-detail"), "Reuse").click();
  await until(() => page.el("tasks-status").textContent === "Instruction sent to the task bar.", "the Reuse text");
  const handed = page.state.session.get(storageKey("task_reuse_v1"));
  assert.deepEqual(page.state.sidePanelOpens, [{ windowId: 9 }], "Reuse first attempts to open this page's own window");
  assert.equal(handed.instruction, "Task 1");
  assert.equal(handed.window_id, null);
  teardown.splice(0).forEach((listener) => listener());
  const panel = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Tab title" }, agentStatus: READY, config: SET_UP, session, windowId: 4,
  });
  await until(() => panel.el("task-input").value === "Task 1", "a different window's task bar filled from Reuse");
  assert.equal(session.has(storageKey("task_reuse_v1")), false, "the other panel consumed the handoff");
});

test("Reuse opens the side panel and fills its task bar, end to end", async () => {
  const session = new Map();
  const tasksPage = await openTasks({ windowId: 4, session });
  await listed(tasksPage);
  await until(() => buttonIn(tasksPage.el("tasks-detail"), "Reuse"), "the Reuse button");
  buttonIn(tasksPage.el("tasks-detail"), "Reuse").click();
  await until(
    () => tasksPage.el("tasks-status").textContent === "Instruction sent to the task bar.",
    "the Reuse text",
  );
  teardown.splice(0).forEach((listener) => listener());

  // A side panel in the same window (the hand-off's whole point) reads the
  // instruction it left and fills its task bar with it, unprompted.
  const panel = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Tab title" }, agentStatus: READY, config: SET_UP, session, windowId: 4,
  });
  await until(() => panel.el("task-input").value === "Task 1", "the task bar filled from Reuse");
  assert.equal(panel.state.session.has(storageKey("task_reuse_v1")), false);
});

test("the list keeps keyboard focus across a refresh", async () => {
  const page = await openTasks({ runs: [task(1, { state: "running" }), task(2), task(3)] });
  await listed(page);
  page.el("tasks-list").dispatch("keydown", { key: "ArrowDown", preventDefault() {} });
  const focused = document.activeElement;
  assert.equal(focused.dataset.runId, runId(2));
  // The visibility handler runs the same refresh as the poll, without
  // racing its two-second interval. A changed pill proves it has rendered;
  // the unchanged row order keeps the existing elements in place.
  page.state.runs[2].state = "failed";
  page.setHidden(true);
  page.setHidden(false);
  await until(() => page.pill(runId(3)).textContent.startsWith("Failed"), "the refreshed task status");
  assert.equal(document.activeElement, focused);
  assert.ok(page.rows().includes(focused), "the focused row is still in the list");
  // A refresh that adds a row rebuilds the list and puts focus back on the selected row.
  page.state.runs.unshift(task(0, { state: "running" }));
  page.setHidden(true);
  page.setHidden(false);
  await until(() => page.rows().some((row) => row.dataset.runId === runId(0)), "the new task");
  assert.equal(document.activeElement.dataset.runId, runId(2));
  assert.ok(page.rows().includes(document.activeElement));
  assert.deepEqual(page.selected(), [runId(2)]);
});

test("a dismissed-list read failure still shows the tasks", async () => {
  const page = await openTasks({
    localGet: async () => { throw new Error("storage is unavailable"); },
  });
  await until(() => page.rows().length === 3, "the task list");
  assert.equal(page.el("tasks-status-box").hidden, true);
  assert.deepEqual(page.selected(), [runId(1)]);
});
