import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { PROTOCOL_VERSION } from "../dist/native.js";

// Drive the compiled panel module and its real native client, with the same
// small DOM convention as options.test.mjs. No browser, network or clipboard.
const html = await readFile(new URL("../dist/panel.html", import.meta.url), "utf8");
const realSetTimeout = globalThis.setTimeout;
globalThis.setTimeout = (...args) => realSetTimeout(...args).unref();
let loadId = 0;

class Element {
  constructor(tag) {
    this.tagName = tag.toUpperCase();
    this.value = "";
    this.textContent = "";
    this.hidden = false;
    this.disabled = false;
    this.children = [];
    this.dataset = {};
    this.listeners = new Map();
    this.classList = { toggle() {} };
  }
  addEventListener(type, fn) { this.listeners.set(type, [...(this.listeners.get(type) ?? []), fn]); }
  dispatch(type, target = this) { for (const fn of this.listeners.get(type) ?? []) fn({ type, target }); }
  click() { if (!this.disabled) this.dispatch("click"); }
  setAttribute(name, value) { this[name] = value; }
  append(...children) { this.children.push(...children); }
  add(child) { this.append(child); }
  replaceChildren(...children) { this.children = children; }
  querySelector() { return this.children[0]; }
}

function memoryStorage() {
  const data = new Map();
  return {
    get length() { return data.size; },
    key: (index) => [...data.keys()][index] ?? null,
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => data.set(key, value),
    removeItem: (key) => data.delete(key),
  };
}

function artifact(overrides = {}) {
  return {
    artifact_id: "smoke:p.test/page", artifact_type: "smoke.page", title: "Page <img src=x>",
    url: "https://p.test/allowed/page", first_seen: "2026-10-27T08:00:00Z",
    last_seen: "2026-10-27T08:02:00Z", focused_seconds: 120, background_seconds: 30,
    opens: 1, days_opened: 1, last_mode: "edit", why: null, linked_count: 1, ...overrides,
  };
}

function fixture(localStorage = memoryStorage()) {
  const elements = new Map();
  for (const [, tag, attrs, id] of html.matchAll(/<([a-z0-9-]+)([^>]*\bid="([^"]+)"[^>]*)>/g)) {
    const item = new Element(tag);
    item.hidden = /(?:^|\s)hidden(?:\s|$)/.test(attrs);
    item.disabled = /(?:^|\s)disabled(?:\s|$)/.test(attrs);
    item.readOnly = /(?:^|\s)readonly(?:\s|$)/.test(attrs);
    elements.set(id, item);
  }
  elements.get("note-conflict").append(new Element("p"));
  elements.get("worklog-period").value = "today";
  const a = artifact();
  const state = {
    elements, localStorage, requests: [], workerRequests: [], intervals: [], windowEvents: new Map(),
    tabEvents: [], body: "saved note", revision: "r1", current: a, older: [],
    rows: [a, artifact({ artifact_id: "doc:d.test/2", artifact_type: "smoke.doc", title: "Design doc", url: "https://d.test/2" })],
    tab: { url: a.url, title: a.title, incognito: false },
    worklogGate: null, saveGate: null, copied: [], copyError: false,
  };
  const workerStatus = {
    live_activity: 0, queued: 0, next_retry_at: null, overflow_count: 0, rejected_count: 0,
    last_error: null, retry_error: null, host_error: null, revoked_origins: [], pause_pending: false,
    pause_token: null, navigation_ready: true, locally_removed_sites: [],
  };
  state.workerStatus = workerStatus;
  const config = {
    storage: { root: "/vault", profile: "neutral", log_dir: "log", pages_dir: "pages", later_dir: "later", artifacts_dir: "artifacts", summaries_dir: null },
    capture_enabled: true, sites: [{ origin: "https://p.test", path_prefix: "/" }], strip_params: [],
    agent_denylist: [], agent_denylist_confirmed: false, log_incognito: false, agent: null,
    adapters: { format: 1, artifacts: [] },
  };
  const key = { kind: "artifact", value: a.artifact_id };
  const note = () => ({ type: "note_loaded", key, exists: true, title: a.title, body: state.body, revision: state.revision, older_notes: state.older });
  async function reply(request) {
    state.requests.push(request);
    switch (request.type) {
      case "hello": return { type: "hello_result", host_version: "test", configured: true, config_issue: null };
      case "get_config": return { type: "config_result", revision: "config1", config, config_issue: null, adapter_issue: null, adapter_warnings: [], agent_status: { state: "not_set_up", harness_version: null, message: null } };
      case "load_note": return note();
      case "save_note": {
        state.body = request.body;
        state.revision += "s";
        const result = { type: "note_saved", key, revision: state.revision, outcome: "replaced", relative_path: "pages/note.md", message: null };
        if (state.saveGate) await state.saveGate;
        return result;
      }
      case "get_artifact": return { type: "artifact_result", recognized: true, artifact: state.current };
      case "record_why":
        state.current = { ...state.current, why: request.event.text };
        return { type: "why_recorded", event_id: request.event.event_id, outcome: "persisted", relative_path: "log/day.md", reason: null };
      case "list_artifacts": return {
        type: "artifact_list_result", artifact_types: ["smoke.page", "smoke.doc"], truncated: false,
        items: state.rows.filter((item) => (!request.artifact_type || item.artifact_type === request.artifact_type) &&
          (!request.title_query || item.title.toLowerCase().includes(request.title_query.toLowerCase())))
          .map((item) => ({ ...item, local_date: "2026-10-27" })),
      };
      case "worklog":
        if (state.worklogGate) await state.worklogGate;
        return { type: "worklog_result", start_date: request.start_date, end_date: request.end_date, text: `Worklog ${request.start_date} to ${request.end_date}: ${state.current.focused_seconds}`, artifact_count: 1 };
      case "view_older_note": return { ...note(), key: state.older[0].key, body: "legacy text" };
      case "use_artifact_note":
        state.body = "legacy text";
        state.older = [];
        return note();
      default: throw new Error(`Unexpected panel request ${request.type}`);
    }
  }
  globalThis.document = { getElementById: (id) => elements.get(id), createElement: (tag) => new Element(tag) };
  globalThis.Option = class extends Element { constructor(text, value) { super("option"); this.textContent = text; this.value = value; } };
  globalThis.window = { localStorage, addEventListener: (name, fn) => state.windowEvents.set(name, fn) };
  globalThis.setInterval = (fn) => { state.intervals.push(fn); return state.intervals.length; };
  globalThis.clearInterval = () => {};
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: {
    locks: { request: (_name, _options, fn) => fn() },
    clipboard: { async writeText(text) { if (state.copyError) throw new Error("clipboard denied"); state.copied.push(text); } },
  } });
  globalThis.chrome = {
    runtime: {
      connectNative() {
        const listeners = [];
        let disconnected = false;
        return {
          onMessage: { addListener: (fn) => listeners.push(fn) }, onDisconnect: { addListener() {} },
          postMessage(request) {
            void reply(request).then((result) => {
              if (!disconnected) for (const fn of listeners) fn({ ...result, protocol_version: PROTOCOL_VERSION, request_id: request.request_id });
            });
          },
          disconnect() { disconnected = true; },
        };
      },
      async sendMessage(request) {
        state.workerRequests.push(request);
        if (request.kind === "sync_now") state.onSync?.();
        return { ok: true, value: workerStatus };
      },
    },
    storage: { onChanged: { addListener() {} } },
    permissions: { contains: async () => true },
    tabs: { query: async () => [state.tab], onActivated: { addListener: (fn) => state.tabEvents.push(fn) }, onUpdated: { addListener() {} } },
  };
  state.el = (id) => elements.get(id);
  state.input = (id, text, event = "input") => { state.el(id).value = text; state.el(id).dispatch(event); };
  state.close = () => state.windowEvents.get("pagehide")();
  return state;
}

async function until(check, message) {
  const end = Date.now() + 3_000;
  while (!check()) {
    assert.ok(Date.now() < end, message);
    await delay(5);
  }
}

async function openPanel(state, t) {
  await import(`../dist/panel.js?fixture=${++loadId}`);
  t.after(() => state.close());
  await until(() => !state.el("note-body").disabled && !state.el("artifact-card").hidden &&
    state.workerRequests.some((request) => request.kind === "sync_now"), "panel initialized");
  await delay(0); // Finish initialization's background history refresh.
}

test("the built artifact card renders literal content and saves one-line context", async (t) => {
  const h = fixture();
  await openPanel(h, t);
  assert.equal(h.el("artifact-heading").textContent, "Page <img src=x>");
  assert.match(h.el("artifact-measures").textContent, /Focused 2 min/);
  h.input("artifact-why", "Review release blockers");
  h.el("save-why").click();
  await until(() => h.el("status").textContent === "Artifact context saved.", "context saved");
  assert.equal(h.el("artifact-why").value, "Review release blockers");
  assert.equal(h.requests.find((request) => request.type === "record_why").event.url, h.tab.url);
});

test("Recent filters and worklog periods drive host requests and Copy waits for the selected result", async (t) => {
  const h = fixture();
  await openPanel(h, t);
  h.el("tab-recent").click();
  await until(() => !h.el("copy-worklog").disabled, "worklog generated");
  assert.equal(h.el("recent-list").children.filter((item) => item.tagName === "A").length, 2);
  h.input("recent-type", "smoke.doc", "change");
  await until(() => h.el("recent-list").children.filter((item) => item.tagName === "A").length === 1, "type filter applied");
  h.input("recent-title", "missing");
  await until(() => h.el("recent-list").children[0]?.textContent === "No matching artifact activity yet.", "empty title filter");
  const filtered = h.requests.filter((request) => request.type === "list_artifacts").at(-1);
  assert.equal(filtered.artifact_type, "smoke.doc");
  assert.equal(filtered.title_query, "missing");
  for (const period of ["yesterday", "week", "today"]) {
    let release;
    h.worklogGate = new Promise((resolve) => { release = resolve; });
    h.input("worklog-period", period, "change");
    assert.equal(h.el("copy-worklog").disabled, true);
    h.el("copy-worklog").click();
    assert.equal(h.copied.length, 0);
    release();
    await until(() => !h.el("copy-worklog").disabled, `${period} worklog loaded`);
    const request = h.requests.filter((item) => item.type === "worklog").at(-1);
    const date = new Date();
    date.setHours(12, 0, 0, 0);
    if (period === "yesterday") date.setDate(date.getDate() - 1);
    const expectedDay = new Intl.DateTimeFormat("en-CA", { year: "numeric", month: "2-digit", day: "2-digit" }).format(date);
    assert.equal(request.end_date, expectedDay);
    if (period !== "week") assert.equal(request.start_date, expectedDay);
    else {
      assert.ok(request.start_date <= request.end_date);
      assert.equal(new Date(`${request.start_date}T12:00:00`).getDay(), 1);
    }
  }
  h.el("copy-worklog").click();
  await until(() => h.el("status").textContent === "Worklog copied.", "copy succeeded");
  assert.deepEqual(h.copied, [h.el("worklog-text").textContent]);
  h.copyError = true;
  h.el("copy-worklog").click();
  await until(() => h.el("status").textContent.includes("clipboard denied"), "copy failure visible");
});

test("older notes can be viewed read-only then explicitly migrated through the built controls", async (t) => {
  const h = fixture();
  h.older = [{ key: { kind: "url", value: "legacy" }, canonical_url: h.tab.url, title: "Legacy", revision: "old-r1" }];
  await openPanel(h, t);
  const row = h.el("older-note-list").children[0];
  h.el("older-note-list").dispatch("click", row.children[1]);
  await until(() => !h.el("older-note-view").hidden, "older note opened");
  assert.equal(h.el("older-note-view-body").value, "legacy text");
  assert.equal(h.el("older-note-view-body").readOnly, true);
  assert.equal(h.requests.some((request) => request.type === "use_artifact_note"), false);
  h.el("older-note-list").dispatch("click", row.children[2]);
  await until(() => h.el("note-body").value === "legacy text", "migration reload");
  assert.equal(h.requests.find((request) => request.type === "use_artifact_note").expected_revision, "old-r1");
});

test("closing a built panel during debounce exposes recovery after reopening without writing it back", async (t) => {
  const first = fixture();
  await openPanel(first, t);
  first.input("note-body", "pending debounce text");
  first.close();
  const reopened = fixture(first.localStorage);
  await openPanel(reopened, t);
  assert.equal(reopened.el("note-conflict").hidden, false);
  assert.equal(reopened.el("note-unsaved").value, "pending debounce text");
  assert.equal(reopened.requests.some((request) => request.type === "save_note"), false);
  reopened.el("copy-unsaved").click();
  assert.deepEqual(reopened.copied, ["pending debounce text"]);
  reopened.el("discard-unsaved").click();
  assert.equal(reopened.el("note-conflict").hidden, true);
  assert.equal(reopened.localStorage.length, 0);
});

test("closing during an in-flight save keeps newer typing even when the older save eventually succeeds", async (t) => {
  const first = fixture();
  let release;
  first.saveGate = new Promise((resolve) => { release = resolve; });
  await openPanel(first, t);
  first.input("note-body", "first save");
  await until(() => first.el("note-status").textContent === "Saving…", "save in flight");
  first.input("note-body", "first save plus newer typing");
  first.close();
  release();
  const reopened = fixture(first.localStorage);
  reopened.body = "first save";
  await openPanel(reopened, t);
  assert.equal(reopened.el("note-body").value, "first save");
  assert.equal(reopened.el("note-unsaved").value, "first save plus newer typing");
  assert.equal(reopened.requests.some((request) => request.type === "save_note"), false);
});

test("an open panel refreshes artifact measures and visible filtered history after background delivery", async (t) => {
  const h = fixture();
  await openPanel(h, t);
  h.el("tab-recent").click();
  await until(() => !h.el("copy-worklog").disabled, "initial history");
  h.input("recent-type", "smoke.page", "change");
  h.input("artifact-why", "unfinished context draft");
  h.current = { ...h.current, focused_seconds: 600 };
  h.rows = [h.current];
  h.intervals[0]();
  await until(() => h.el("artifact-measures").textContent.includes("10 min") &&
    h.el("worklog-text").textContent.endsWith(": 600"), "background history refreshed");
  assert.equal(h.el("recent-type").value, "smoke.page");
  assert.equal(h.el("artifact-why").value, "unfinished context draft");
});

test("Send pending records refreshes the displayed history after the worker sync finishes", async (t) => {
  const h = fixture();
  await openPanel(h, t);
  h.el("tab-recent").click();
  await until(() => !h.el("copy-worklog").disabled, "initial worklog");
  h.workerStatus.queued = 1;
  h.el("capture-strip").open = true;
  h.intervals[0]();
  await until(() => !h.el("replay").disabled, "Send pending enabled");
  h.onSync = () => { h.current = { ...h.current, focused_seconds: 900 }; };
  h.el("replay").click();
  await until(() => h.el("artifact-measures").textContent.includes("15 min") &&
    h.el("worklog-text").textContent.endsWith(": 900"), "sync refreshed history");
});
