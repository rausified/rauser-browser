import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";

const { POLICY_STORAGE_KEY } = await import("../dist/model.js");
const { PROTOCOL_VERSION } = await import("../dist/native.js");
const { isResponseShape } = await import("../dist/protocol-shape.js");
const { PANEL_NOTICE_KEY } = await import("../dist/model.js");
const { SETUP_WARNING } = await import("../dist/agent.js");
const { storageKey } = await import("../dist/brand.js");
const { EXTRACT_LIMITS } = await import("../dist/extract.js");
const { starterOffers: starterOffersFor } = await import("../dist/starter-adapters.js");

// Run after build:extension. Each test loads a fresh copy of the side panel
// against a fake DOM built from the shipped panel.html, a scripted native
// host, a stub worker, and a fake Chrome whose site access can be withdrawn
// and restored as the user would in chrome://extensions.
const PANEL_HTML = readFileSync(new URL("../dist/panel.html", import.meta.url), "utf8");
const SITE_ACCESS = ["http://*/*", "https://*/*"];

class FakeElement {
  constructor(tagName, hidden = false) {
    this.tagName = tagName.toUpperCase();
    this.ownText = "";
    this.value = "";
    this.disabled = false;
    this.hidden = hidden;
    this.open = false;
    this.children = [];
    this.dataset = {};
    this.listeners = new Map();
    this.classList = { toggle() {}, add() {}, remove() {} };
  }
  // Like the DOM: a node's text includes its children's, and setting it
  // replaces them.
  get textContent() {
    return [this.ownText, ...this.children.map((child) => child.textContent ?? "")].join("");
  }
  set textContent(value) {
    this.ownText = String(value);
    this.children = [];
  }
  addEventListener(type, listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }
  dispatch(type) {
    for (const listener of this.listeners.get(type) ?? []) listener({ type, target: this });
  }
  click() {
    if (!this.disabled) this.dispatch("click");
  }
  setAttribute(name, value) {
    this.attributes ??= {};
    this.attributes[name] = String(value);
  }
  getAttribute(name) { return this.attributes?.[name] ?? null; }
  focus() { this.focused = true; }
  append(...children) { this.children.push(...children); }
  add(child) { this.children.push(child); }
  replaceChildren(...children) { this.children = children; }
  // Agent text reaches the panel; markup must never be parsed from it.
  set innerHTML(_value) { throw new Error("The panel must set text through textContent, not innerHTML"); }
  insertAdjacentHTML() { throw new Error("The panel must set text through textContent, not insertAdjacentHTML"); }
  querySelector(selector) {
    return this.children.find((child) => child.tagName === selector.toUpperCase()) ?? new FakeElement(selector);
  }
}

// Every element with an id in panel.html, with its initial `hidden` state and
// static text, so the panel fails with "Missing page element" for markup that
// is not there. The text is the markup up to the first matching end tag,
// which is exact for elements that do not nest their own tag.
function pageElements() {
  const elements = new Map();
  for (const match of PANEL_HTML.matchAll(/<([a-z0-9]+)\b([^>]*?)\bid="([^"]+)"([^>]*)>/g)) {
    const [, tag, before, id, after] = match;
    const element = new FakeElement(tag, /(^|\s)hidden(\s|\/|$)/.test(`${before} ${after}`));
    const start = match.index + match[0].length;
    const end = PANEL_HTML.indexOf(`</${tag}>`, start);
    if (end !== -1) {
      element.ownText = PANEL_HTML.slice(start, end).replace(/<[^>]*>/g, "").replace(/\s+/g, " ").trim();
    }
    elements.set(id, element);
  }
  return elements;
}

function statusFor() {
  return {
    queued: 0, next_retry_at: null, overflow_count: 0, rejected_count: 0, last_error: null,
    retry_error: null, policy_expires_at: null, pause_pending: false,
    pause_token: null, pending_removals: [], live_activity: 0,
    host_error: null,
  };
}

const CONFIG = {
  storage: {
    root: "/vault", profile: "neutral", log_dir: "logs", pages_dir: "notes", later_dir: "later",
    artifacts_dir: "artifacts", summaries_dir: null, knowledge_base_dir: null,
  },
  capture_enabled: false,
  strip_params: [],
  agent: null,
  summary_vocabulary: { document_types: ["report"], categories: ["news"] },
  adapters: { format: 1, artifacts: [] },
};

const NOT_SET_UP = { state: "not_set_up", harness_version: null, message: "Set up an AI harness." };
const READY = { state: "ready", harness_version: "1.0.0", message: null };

function storageArea(entries) {
  return {
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

let loads = 0;
let teardown = [];

test.afterEach(() => {
  for (const listener of teardown) listener();
  teardown = [];
});

function hostReply(request, config = CONFIG) {
  switch (request.type) {
    case "hello":
      return { type: "hello_result", host_version: "0.0.0", configured: true, config_issue: null };
    case "get_config":
      return {
        type: "config_result", revision: "1", config, config_issue: null,
        agent_status: { state: "not_set_up", harness_version: null, message: "Set up an AI harness." },
        adapter_issue: null, adapter_warnings: [],
        adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
      };
    case "check_capture_site":
      return { type: "capture_site_checked", revision: request.expected_revision };
    case "load_note":
      return {
        type: "note_loaded", exists: false, revision: "", title: "", body: "",
        key: { kind: "url", value: request.url }, older_notes: [],
      };
    case "get_artifact":
      return { type: "artifact_result", recognized: false, artifact: null, summary: null };
    case "list_runs":
      return { type: "runs_listed", runs: [] };
    case "record_why":
      return {
        type: "why_recorded", event_id: request.event.event_id, outcome: "persisted", reason: null,
        relative_path: "log/2026-10-27.md",
      };
    default:
      return { type: "error", code: "internal", message: `Unexpected host request ${request.type}` };
  }
}

// `intercept(request, state)` may answer a host request (or return a Promise
// to hold it); a falsy result falls back to the default reply. `workerReply`
// does the same for messages to the worker. With `state.hostDead`, a new host
// process closes its port without answering, as one that cannot start does.
// `executeScript(injection)` answers chrome.scripting.executeScript for
// summary extraction and `script(injection, state)` for site-pattern review;
// `local`
// and `session` are Maps backing chrome.storage, so a test can share them
// between panel loads.
async function install({
  tab, contains = true, config = CONFIG, workerStatus = {}, configIssue = null, adapterIssue = null,
  intercept = () => null, workerReply = () => undefined, agentStatus = NOT_SET_UP,
  executeScript = async () => { throw new Error("Cannot access contents of the page"); },
  local = new Map(), session = new Map(),
  localStorage = memoryStorage(),
  script = async () => ({ current: "/items/{num}", links: [] }), patterns = [], patternsError = null,
}) {
  const elements = pageElements();
  const listeners = () => {
    const registered = [];
    return { registered, addListener(listener) { registered.push(listener); } };
  };
  const state = {
    tab: tab === undefined ? undefined : { incognito: false, ...tab },
    contains,
    config: structuredClone(config), revision: "1", configIssue, adapterIssue,
    workerStatus: { ...statusFor(), ...workerStatus }, requests: [], workerRequests: [],
    created: [], scripts: [], copied: [], localStorage,
    counts: new Map(),
    added: [],
    removed: [],
    ports: [],
    hostDead: false,
    agentStatus,
    injections: [],
    local,
    session,
    runtimeMessages: listeners(),
  };
  const count = (name) => state.counts.set(name, (state.counts.get(name) ?? 0) + 1);

  globalThis.document = {
    getElementById: (id) => elements.get(id) ?? null,
    createElement: (tag) => new FakeElement(tag),
  };
  globalThis.Option = class extends FakeElement {
    constructor(text = "", value = text) {
      super("option");
      this.ownText = text;
      this.value = value;
    }
  };
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: {
    locks: { request: async (_name, _options, action) => action() },
    clipboard: { async writeText(text) { state.copied.push(text); } },
  } });
  let close;
  globalThis.window = {
    localStorage,
    addEventListener(type, listener) {
      if (type === "pagehide") {
        let closed = false;
        close = () => {
          if (closed) return;
          closed = true;
          listener();
        };
        teardown.push(close);
      }
    },
    confirm: () => false,
  };
  const tabActivated = listeners();
  const tabUpdated = listeners();
  globalThis.chrome = {
    scripting: {
      // Site-pattern review injections go to `script` and `state.scripts`;
      // summary extraction goes to `executeScript` and `state.injections`.
      async executeScript(injection) {
        if (injection.args?.length === 2 && injection.args[1] === true) {
          state.scripts.push(injection);
          return [{ frameId: 0, result: await script(injection, state) }];
        }
        state.injections.push(injection);
        return executeScript(injection);
      },
    },
    runtime: {
      id: "test-id",
      lastError: undefined,
      getManifest: () => ({ options_ui: { page: "options.html" } }),
      getURL: (path) => `chrome-extension://test-id/${path}`,
      openOptionsPage: async () => undefined,
      connectNative() {
        const messageListeners = [];
        const disconnectListeners = [];
        let disconnected = false;
        const port = {
          onMessage: { addListener(listener) { messageListeners.push(listener); } },
          onDisconnect: { addListener(listener) { disconnectListeners.push(listener); } },
          fireDisconnect() {
            for (const listener of disconnectListeners) listener();
          },
          postMessage(request) {
            state.requests.push(structuredClone(request));
            if (state.hostDead) {
              setTimeout(() => port.fireDisconnect(), 0);
              return;
            }
            setTimeout(async () => {
              let reply = await intercept(request, state);
              if (!reply && request.type === "get_config") {
                reply = { ...hostReply(request), config: state.config, revision: state.revision,
                  config_issue: state.configIssue, adapter_issue: state.adapterIssue,
                  agent_status: state.agentStatus };
              }
              if (!reply && request.type === "update_config") {
                if (request.expected_revision !== state.revision) {
                  reply = { type: "error", code: "conflict", message: "Settings changed" };
                } else {
                  state.config = structuredClone(request.config);
                  state.revision = String(Number(state.revision) + 1);
                  reply = { type: "config_updated", config: state.config, revision: state.revision };
                }
              }
              if (!reply && request.type === "save_note") {
                reply = { type: "note_saved", revision: "saved", key: request.expected_key, outcome: "created", relative_path: "pages/note.md" };
              }
              reply ??= hostReply(request);
              for (const listener of disconnected ? [] : messageListeners) {
                listener({ ...reply, protocol_version: PROTOCOL_VERSION, request_id: request.request_id });
              }
            }, 0);
          },
          disconnect() { disconnected = true; },
        };
        state.ports.push(port);
        return port;
      },
      async sendMessage(request) {
        state.workerRequests.push(structuredClone(request));
        if (request.kind === "get_site_patterns") {
          return patternsError ? { ok: false, error: patternsError } : { ok: true, value: structuredClone(patterns) };
        }
        return (await workerReply(request)) ?? { ok: true, value: structuredClone(state.workerStatus) };
      },
      onMessage: state.runtimeMessages,
    },
    storage: {
      local: storageArea(local),
      session: storageArea(session),
      onChanged: listeners(),
    },
    tabs: {
      async query() {
        count("tabs.query");
        return state.tab ? [{ ...state.tab }] : [];
      },
      async create(properties) {
        state.created.push(properties);
        return { id: 99, incognito: false, ...properties };
      },
      onActivated: tabActivated,
      onUpdated: tabUpdated,
    },
    permissions: {
      async contains(permissions) {
        count("permissions.contains");
        const origins = permissions.origins ?? [];
        assert.deepEqual([...origins].sort(), [...SITE_ACCESS].sort());
        return state.contains;
      },
      async request() {
        count("permissions.request");
        throw new Error("The panel must never request a Chrome permission");
      },
      async remove() {
        count("permissions.remove");
        throw new Error("The panel must never remove a Chrome permission");
      },
      onAdded: { addListener(listener) { state.added.push(listener); } },
      onRemoved: { addListener(listener) { state.removed.push(listener); } },
    },
  };

  loads += 1;
  await import(`../dist/panel.js?load=${loads}`);
  return {
    state,
    close: () => close(),
    created: state.created,
    navigate(url, id = state.tab?.id ?? 1) {
      state.tab = { ...state.tab, url, id, incognito: false };
      for (const listener of tabActivated.registered) listener({ tabId: id, windowId: 1 });
    },
    tabUpdated(changeInfo, id = state.tab?.id ?? 1) {
      for (const listener of tabUpdated.registered) listener(id, changeInfo, { ...state.tab, id });
    },
    policyChanged(lease = null) {
      for (const listener of chrome.storage.onChanged.registered) {
        listener({ [POLICY_STORAGE_KEY]: { newValue: lease } }, "local");
      }
    },
    el(id) {
      const found = elements.get(id);
      if (!found) throw new Error(`No element ${id} in panel.html`);
      return found;
    },
    calls: (name) => state.counts.get(name) ?? 0,
    // The worker broadcasts a notice to open extension pages.
    notify(notice) {
      for (const listener of state.runtimeMessages.registered) {
        listener(notice, { id: "test-id", url: "chrome-extension://test-id/worker.js" }, () => undefined);
      }
    },
    sent: (type) => state.requests.filter((request) => request.type === type),
    // The followed tab moves to another address in place.
    followLink(url, title) {
      state.tab.url = url;
      state.tab.title = title;
      for (const listener of tabUpdated.registered) listener(state.tab.id, { url, title }, state.tab);
    },
    titleChanged(title) {
      state.tab.title = title;
      for (const listener of tabUpdated.registered) listener(state.tab.id, { title }, state.tab);
    },
    // The running host process exits after having answered.
    hostExits() {
      chrome.runtime.lastError = { message: "Native host has exited." };
      state.ports.at(-1).fireDisconnect();
      chrome.runtime.lastError = undefined;
    },
    // Chrome fires these after the user changes site access.
    grant() {
      state.contains = true;
      for (const listener of state.added) listener({ origins: SITE_ACCESS });
    },
    withdraw() {
      state.contains = false;
      for (const listener of state.removed) listener({ origins: SITE_ACCESS });
    },
  };
}

async function until(condition, what) {
  const deadline = Date.now() + 2_000;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error(`Timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 1));
  }
}

// The panel has settled once its first tab refresh rendered the page area.
const shownTitle = (page) => page.el("page-title").textContent;

for (const exists of [true, false]) {
  test(`older notes offer View${exists ? " only" : " and Use"} when the artifact note ${exists ? "exists" : "is absent"}`, async () => {
    const sourceUrl = "https://example.com/board?selected=7";
    const page = await install({
      tab: { id: 1, url: "https://example.com/items/7", title: "Issue 7" },
      intercept(request) {
        if (request.type === "load_note") return {
          type: "note_loaded", exists, revision: exists ? "shared" : "missing",
          title: "Issue 7", body: exists ? "shared text" : "",
          key: { kind: "artifact", value: "work:example.com/7" },
          older_notes: [{ canonical_url: sourceUrl, revision: "old", title: "Older note",
            key: { kind: "url", value: "older-url-key" } }],
        };
        if (request.type === "view_older_note") return {
          type: "note_loaded", exists: true, revision: "old", title: "Older note", body: "older text",
          key: { kind: "url", value: "older-url-key" }, older_notes: [],
        };
        return null;
      },
    });
    await until(() => page.el("older-note-list").children.length > 0 && !page.el("note-body").disabled, "artifact note to load");
    const buttons = page.el("older-note-list").children[0].children.filter((child) => child.tagName === "BUTTON");
    assert.deepEqual(buttons.map((button) => button.dataset.olderAction), exists ? ["view"] : ["view", "use"]);
    for (const listener of page.el("older-note-list").listeners.get("click")) listener({ target: buttons[0] });
    await until(() => !page.el("older-note-view").hidden, "older note to open read-only");
    assert.equal(page.el("older-note-view-body").value, "older text");
    assert.equal(page.el("note-body").value, exists ? "shared text" : "");
    assert.equal(page.sent("view_older_note")[0].source_url, sourceUrl);
    assert.equal(page.sent("use_artifact_note").length, 0);
    assert.equal(page.sent("save_note").length, 0);
  });
}

for (const phase of ["before hello", "during policy install", "after startup", "before policy failure"]) {
  test(`Recent loads once when opened ${phase}`, async () => {
    const waitForHello = phase.startsWith("before");
    let releaseHello;
    const hello = new Promise((resolve) => { releaseHello = resolve; });
    let releasePolicy;
    const policy = new Promise((resolve) => { releasePolicy = resolve; });
    globalThis.Option = class extends FakeElement {
      constructor(text, value) {
        super("option");
        this.textContent = text;
        this.value = value;
      }
    };
    const page = await install({
      tab: { id: 1, url: "https://new.example/a", title: "A" },
      intercept(request) {
        if (request.type === "hello" && waitForHello) return hello;
        if (request.type === "list_artifacts") return {
          type: "artifact_list_result", artifact_types: ["work.item"], truncated: false,
          items: [{ artifact_id: "work:example/a", artifact_type: "work.item", title: "Brief review",
            url: "https://new.example/a", local_date: "2026-10-27", focused_seconds: 30,
            background_seconds: 20, opens: 1, why: null, summary: null }],
        };
        if (request.type === "worklog") return {
          type: "worklog_result", text: "Reviewed the release", artifact_count: 1,
          start_date: request.start_date, end_date: request.end_date,
        };
        return null;
      },
      workerReply(request) {
        if (request.kind === "install_policy" && phase === "during policy install") return policy;
        if (request.kind === "install_policy" && phase === "before policy failure") {
          return { ok: false, error: "Worker policy install failed" };
        }
      },
    });
    if (phase === "during policy install") {
      await until(() => page.state.workerRequests.some((request) => request.kind === "install_policy"), "policy installation");
    } else if (phase === "after startup") {
      await until(() => page.sent("load_note").length > 0 && page.el("note-body").disabled === false, "connected note");
    }
    page.el("tab-recent").click();
    try {
      if (waitForHello) {
        assert.equal(page.el("recent-status").textContent, "Waiting for the host…");
        assert.equal(page.el("worklog-text").textContent, "Waiting for the host…");
        assert.equal(page.el("copy-worklog").disabled, true);
        assert.equal(page.sent("list_artifacts").length, 0);
        assert.equal(page.sent("worklog").length, 0);
      }
    } finally {
      releaseHello(hostReply({ type: "hello" }));
      releasePolicy({ ok: true, value: statusFor() });
      // Drain startup even if the waiting-state assertions fail.
      if (phase === "before policy failure") {
        await until(() => page.el("status").textContent.includes("Worker policy install failed"), "policy error");
      } else {
        await until(() => page.state.workerRequests.some((request) => request.kind === "sync_now"), "startup replay");
      }
    }
    await until(() => page.el("worklog-text").textContent === "Reviewed the release", "worklog result");
    await until(() => page.el("recent-list").textContent.includes("Brief review"), "Recent result");
    assert.equal(page.sent("list_artifacts").length, 1);
    assert.equal(page.sent("worklog").length, 1);
    assert.equal(page.el("copy-worklog").disabled, false);
  });
}

test("Recent opened before hello loads once configuration arrives, while policy installation is still pending", async () => {
  let releaseHello;
  const hello = new Promise((resolve) => { releaseHello = resolve; });
  let releasePolicy;
  const policy = new Promise((resolve) => { releasePolicy = resolve; });
  globalThis.Option = class extends FakeElement {
    constructor(text, value) {
      super("option");
      this.textContent = text;
      this.value = value;
    }
  };
  const page = await install({
    tab: { id: 1, url: "https://new.example/a", title: "A" },
    intercept(request) {
      if (request.type === "hello") return hello;
      if (request.type === "list_artifacts") return {
        type: "artifact_list_result", artifact_types: ["work.item"], truncated: false,
        items: [{ artifact_id: "work:example/a", artifact_type: "work.item", title: "Brief review",
          url: "https://new.example/a", local_date: "2026-10-27", focused_seconds: 30,
          background_seconds: 20, opens: 1, why: null, summary: null }],
      };
      if (request.type === "worklog") return {
        type: "worklog_result", text: "Reviewed the release", artifact_count: 1,
        start_date: request.start_date, end_date: request.end_date,
      };
      return null;
    },
    workerReply(request) {
      if (request.kind === "install_policy") return policy;
    },
  });
  page.el("tab-recent").click();
  assert.equal(page.el("recent-status").textContent, "Waiting for the host…");
  releaseHello(hostReply({ type: "hello" }));
  try {
    await until(() => page.state.workerRequests.some((request) => request.kind === "install_policy"), "policy installation");
    await until(() => page.el("worklog-text").textContent === "Reviewed the release", "worklog before policy install finishes");
    await until(() => page.el("recent-list").textContent.includes("Brief review"), "Recent before policy install finishes");
  } finally {
    releasePolicy({ ok: true, value: statusFor() });
  }
  await until(() => page.state.workerRequests.some((request) => request.kind === "sync_now"), "startup replay");
  assert.equal(page.sent("list_artifacts").length, 1);
  assert.equal(page.sent("worklog").length, 1);
});

test("Recent replaces its startup wait with setup guidance when no home folder is chosen", async () => {
  let releaseHello;
  const hello = new Promise((resolve) => { releaseHello = resolve; });
  const page = await install({
    tab: { id: 1, url: "https://new.example/a", title: "A" },
    config: { ...CONFIG, storage: null },
    intercept: (request) => request.type === "hello" ? hello : null,
  });
  page.el("tab-recent").click();
  assert.equal(page.el("recent-status").textContent, "Waiting for the host…");
  releaseHello(hostReply({ type: "hello" }));
  await until(() => page.state.workerRequests.some((request) => request.kind === "sync_now"), "startup replay");
  for (const clickAgain of [false, true]) {
    if (clickAgain) page.el("tab-recent").click();
    assert.match(page.el("recent-status").textContent, /home folder.*settings/i);
    assert.match(page.el("worklog-text").textContent, /home folder.*settings/i);
    assert.equal(page.el("copy-worklog").disabled, true);
  }
  assert.equal(page.sent("list_artifacts").length, 0);
  assert.equal(page.sent("worklog").length, 0);
});

test("an HTTP(S) tab shows the note editor and no grant or address warning", async () => {
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, contains: true });
  await until(() => page.el("note-editor").hidden === false, "the note editor to show");
  assert.equal(page.el("page-url").textContent, "https://new.example/a");
  assert.equal(shownTitle(page), "A");
  assert.equal(page.el("note-editor").hidden, false);
  assert.equal(page.el("site-access-warning").hidden, true);
  assert.equal(page.el("page-unusable").hidden, true);
  assert.equal(page.calls("permissions.request"), 0);
  await until(() => page.el("note-body").disabled === false, "the note to load");
});

test("a note refusal from damaged adopted-contract state shows the host's named repair", async () => {
  // The host names the state file and its repair; the panel must show it
  // whole and keep the editor read-only, so nothing autosaves under a guess.
  const refusal = "Notes on this page are paused to protect note identity: Brauser's record of the " +
    "adapters it has used (/config/brauser/adopted-contracts.json) cannot be read. Move " +
    "adopted-contracts.json aside (rename it, keeping a backup) or fix it to restore notes on pages " +
    "the current adapters do not recognize.";
  const page = await install({
    tab: { id: 1, url: "https://work.example/boards/1", title: "Board" },
    intercept: (request) => request.type === "load_note"
      ? { type: "error", code: "internal", message: refusal }
      : undefined,
  });
  await until(() => page.el("note-status").textContent.includes("adopted-contracts.json"),
    "the named refusal");
  assert.equal(page.el("note-status").textContent, `Could not load this page's note: ${refusal}`);
  assert.equal(page.el("note-body").disabled, true);
  await settle();
  assert.equal(page.sent("save_note").length, 0);
});

test("an HTTP(S) tab shows the note editor even when Chrome reports access withdrawn", async () => {
  // A URL means Chrome granted this site, whatever the all-sites check says.
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, contains: false });
  await until(() => page.el("note-editor").hidden === false, "the note editor to show");
  assert.equal(page.el("site-access-warning").hidden, true);
});

test("a tab whose address Chrome withholds while access is withdrawn shows the single banner", async () => {
  const page = await install({ tab: { id: 1, url: undefined }, contains: false });
  await until(() => page.el("site-access-warning").hidden === false, "the site-access banner");
  assert.match(page.el("site-access-warning").textContent, /Brauser needs site access — re-enable in Chrome/);
  assert.equal(page.el("note-editor").hidden, true);
  assert.equal(page.el("page-unusable").hidden, true);
  assert.equal(page.calls("permissions.request"), 0);
});

test("a chrome:// tab with access granted shows no site-access banner", async () => {
  const page = await install({ tab: { id: 1, url: undefined, title: "Extensions" }, contains: true });
  // The "not a web page" state is the settled answer for this tab.
  await until(() => page.el("page-unusable").hidden === false, "the not-a-web-page state");
  assert.equal(page.el("site-access-warning").hidden, true);
  assert.equal(page.el("note-editor").hidden, true);
  assert.equal(shownTitle(page), "Extensions");
});

test("withdrawing then restoring access hides the banner without a reload", async () => {
  const page = await install({ tab: { id: 1, url: undefined }, contains: false });
  await until(() => page.el("site-access-warning").hidden === false, "the site-access banner");

  // Re-enabling in Chrome makes the tab's address visible again.
  page.state.tab.url = "https://new.example/a";
  page.state.tab.title = "A";
  page.grant();
  await until(() => page.el("site-access-warning").hidden === true, "the banner to hide");
  await until(() => page.el("note-editor").hidden === false, "the note editor to show");
  assert.equal(page.el("page-url").textContent, "https://new.example/a");
  await until(() => page.el("note-body").disabled === false, "the note to load");

  // Withdrawing again brings the one banner back.
  page.state.tab.url = undefined;
  page.withdraw();
  await until(() => page.el("site-access-warning").hidden === false, "the banner to return");
  assert.equal(page.el("note-editor").hidden, true);
});

test("re-enable opens this extension's Chrome page", async () => {
  const page = await install({ tab: { id: 1, url: undefined }, contains: false });
  await until(() => page.el("site-access-warning").hidden === false, "the site-access banner");
  page.el("site-access-reenable").click();
  assert.deepEqual(page.created, [{ url: "chrome://extensions/?id=test-id" }]);
});

test("the panel markup has no grant button or address-unavailable note", () => {
  assert.ok(!PANEL_HTML.includes('id="note-grant-origin"'));
  assert.ok(!PANEL_HTML.includes('id="note-unavailable"'));
  assert.ok(!PANEL_HTML.includes("can't see this tab's address"));
});

const JIRA_URL = "https://acme.atlassian.net/browse/PROJ-42";
const GITHUB_URL = "https://github.com/acme-corp/widget/pull/42/files";

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
const updates = (page) => page.state.requests.filter((request) => request.type === "update_config");

async function captureReview(url = JIRA_URL, options = {}) {
  const { waitForCheck = true, ...installOptions } = options;
  const page = await install({ tab: { id: 1, url, title: "Work page" }, ...installOptions });
  await until(() => !page.el("capture-site").disabled && !page.el("capture-site-area").hidden, "Capture Site to be ready");
  await until(() => !page.el("note-body").disabled, "the note to load");
  // The panel sends pending visits once on startup.
  await until(() => !page.el("capture-site").disabled, "startup delivery to finish");
  page.el("capture-site").click();
  assert.equal(page.el("capture-site-review").hidden, false);
  if (waitForCheck) await until(() => page.el("capture-site-message").textContent !== "Checking saved site identities…", "the starter preflight");
  return page;
}

async function saveCapture(page) {
  await until(() => !page.el("capture-site-save").disabled, "Save after the starter preflight");
  page.el("capture-site-save").click();
  await until(() => page.el("capture-site-review").hidden, "site capture to save");
  await until(() => !page.el("note-body").disabled, "the note to reload after enabling capture");
}

for (const history of ["retired", "hand-removed"]) {
  test(`Capture Site refuses a starter conflicting with a ${history} contract before save`, async () => {
    const page = await captureReview(JIRA_URL, { intercept(request) {
      if (request.type === "check_capture_site") return {
        type: "error", code: "invalid_config",
        message: 'Recorded adapter "custom.issue" on https://acme.atlassian.net uses "jira:{host}/issues/{key}". Review its rules in settings.',
      };
    } });
    await settle();
    assert.equal(page.el("capture-site-save").disabled, true);
    assert.match(page.el("capture-site-message").textContent, /custom.issue.*jira:\{host\}\/issues\/\{key\}/);
    assert.doesNotMatch(page.el("capture-site-message").textContent, /Saving enables/);
    page.el("capture-site-save").click();
    assert.equal(updates(page).length, 0);
  });
}

test("Capture Site waits for the host's history check before promising a save", async () => {
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const page = await captureReview(JIRA_URL, { waitForCheck: false, intercept: (request) =>
    request.type === "check_capture_site" ? held : undefined });
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.doesNotMatch(page.el("capture-site-message").textContent, /Saving enables/);
  release({ type: "capture_site_checked", revision: "1" });
  await until(() => !page.el("capture-site-save").disabled, "the history check to pass");
  assert.match(page.el("capture-site-message").textContent, /Saving enables/);
  await saveCapture(page);
});

test("Capture Site ignores history replies for superseded selections and canceled reviews", async () => {
  const pending = [];
  const page = await captureReview(GITHUB_URL, { waitForCheck: false, intercept(request) {
    if (request.type === "check_capture_site") return new Promise((resolve) => pending.push({ request, resolve }));
  } });
  await until(() => pending.length === 1, "the first preflight");
  page.el("capture-site-orgs").value = "other-org";
  page.el("capture-site-orgs").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
  await until(() => pending.length === 2, "the edited preflight");
  assert.deepEqual(pending[1].request.adapters.artifacts[0].scope, { owner: ["other-org"] });
  assert.deepEqual(Object.keys(pending[1].request).sort(),
    ["adapters", "expected_revision", "protocol_version", "request_id", "type"]);
  pending[1].resolve({ type: "error", code: "invalid_config", message: "Recorded adapter conflicts; use settings." });
  await until(() => /Recorded adapter/.test(page.el("capture-site-message").textContent), "the current refusal");
  pending[0].resolve({ type: "capture_site_checked", revision: "1" });
  await settle();
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.match(page.el("capture-site-message").textContent, /Recorded adapter/);

  page.el("capture-site-orgs").value = "third-org";
  page.el("capture-site-orgs").dispatch("input");
  await until(() => pending.length === 3, "another preflight");
  page.el("capture-site-orgs").value = "";
  page.el("capture-site-orgs").dispatch("input");
  pending[2].resolve({ type: "capture_site_checked", revision: "1" });
  await settle();
  assert.equal(page.el("capture-site-save").disabled, true, "a reply cannot enable an invalid selection");

  page.el("capture-site-orgs").value = "fourth-org";
  page.el("capture-site-orgs").dispatch("input");
  await until(() => pending.length === 4, "the canceled preflight");
  page.el("capture-site-cancel").click();
  page.el("capture-site").click();
  await until(() => pending.length === 5, "the reopened review's preflight");
  pending[3].resolve({ type: "error", code: "invalid_config", message: "Obsolete refusal" });
  await settle();
  assert.doesNotMatch(page.el("capture-site-message").textContent, /Obsolete/);
  assert.equal(page.el("capture-site-save").disabled, true);
  pending[4].resolve({ type: "capture_site_checked", revision: "1" });
  await until(() => !page.el("capture-site-save").disabled, "the new review's check");
  await saveCapture(page);
});

for (const reply of [
  { type: "error", code: "invalid_request", message: "Upgrade the native host." },
  { type: "capture_site_checked", revision: "other-revision" },
  { type: "capture_site_checked" },
]) {
  test(`Capture Site stays disabled after a refused or mismatched check: ${JSON.stringify(reply)}`, async () => {
    const page = await captureReview(JIRA_URL, { intercept: (request) =>
      request.type === "check_capture_site" ? reply : undefined });
    assert.equal(page.el("capture-site-save").disabled, true);
    assert.doesNotMatch(page.el("capture-site-message").textContent, /Saving enables/);
    page.el("capture-site-save").click();
    assert.equal(updates(page).length, 0);
  });
}

test("Capture Site previews Jira and installs the host-returned policy in one save", async () => {
  const page = await captureReview();
  assert.match(page.el("capture-site-preview").textContent, /This page: will be captured/);
  assert.match(page.el("capture-site-preview").textContent, /Illustrative example/);
  assert.match(page.el("capture-site-preview").textContent, /27.*examples skipped/);
  assert.match(page.el("capture-site-preview").textContent, /Skipped: https:\/\/acme.atlassian.net\/share/);
  assert.doesNotMatch(page.el("capture-site-preview").textContent, /"match"|"id"/);
  assert.equal(page.el("capture-site-heading").focused, true);
  await saveCapture(page);
  assert.equal(updates(page).length, 1);
  const update = updates(page)[0];
  assert.equal(update.expected_revision, "1");
  assert.equal(update.config.capture_enabled, true);
  assert.equal(update.config.adapters.artifacts[0].type, "jira.issue");
  assert.equal(update.picker_token, null);
  assert.equal(update.knowledge_base_token, null);
  const installed = page.state.workerRequests.filter((request) => request.kind === "install_policy").at(-1);
  assert.equal(installed.lease.revision, "2");
  assert.deepEqual(installed.lease.adapters, update.config.adapters);
  assert.equal(page.calls("permissions.request"), 0);
  assert.equal(page.calls("permissions.remove"), 0);
  // The panel's opening list_runs restores run cards; it is not part of the save.
  assert.deepEqual([...new Set(page.state.requests.map((r) => r.type).filter((type) => type !== "list_runs"))].sort(),
    ["check_capture_site", "get_artifact", "get_config", "hello", "load_note", "update_config"]);
});

test("Capture Site recognizes OneDrive document routes without scope or JSON", async () => {
  const url = "https://acme-my.sharepoint.com/personal/alex/_layouts/15/Doc.aspx?sourcedoc=%7B123e4567-e89b-12d3-a456-426614174000%7D";
  const page = await captureReview(url);
  assert.equal(page.el("capture-site-github").hidden, true);
  assert.match(page.el("capture-site-preview").textContent, /OneDrive for work documents/);
  assert.match(page.el("capture-site-preview").textContent, /will be captured/);
  await saveCapture(page);
  assert.equal(updates(page)[0].config.adapters.artifacts[0].id, "sharepoint:{host}/{doc}");
});

test("Capture Site pre-fills the organization from the current GitHub page", async () => {
  const page = await captureReview(GITHUB_URL);
  assert.equal(page.el("capture-site-github").hidden, false);
  assert.equal(page.el("capture-site-orgs").value, "acme-corp");
  assert.equal(page.el("capture-site-repos").checked, true);
  assert.equal(page.el("capture-site-prs").checked, true);
  assert.equal(page.el("capture-site-issues").checked, true);
  assert.equal(page.el("capture-site-save").disabled, false);
  assert.match(page.el("capture-site-preview").textContent, /Records repositories, pull requests and issues owned by: acme-corp\./);
  await saveCapture(page);
  assert.equal(updates(page)[0].config.adapters.artifacts.length, 3);
  for (const artifact of updates(page)[0].config.adapters.artifacts) {
    assert.deepEqual(artifact.scope, { owner: ["acme-corp"] });
  }
});

test("editing the pre-filled organization keeps the user's edit, not the page's owner", async () => {
  const page = await captureReview(GITHUB_URL);
  page.el("capture-site-orgs").value = "other-org";
  page.el("capture-site-orgs").dispatch("input");
  assert.match(page.el("capture-site-preview").textContent, /owned by: other-org\./);
  await saveCapture(page);
  assert.deepEqual(updates(page)[0].config.adapters.artifacts[0].scope, { owner: ["other-org"] });
});

test("unchecking repositories omits the repo adapter", async () => {
  const page = await captureReview(GITHUB_URL);
  page.el("capture-site-repos").checked = false;
  page.el("capture-site-repos").dispatch("input");
  await until(() => !page.el("capture-site-save").disabled, "the edited selection preflight");
  await saveCapture(page);
  assert.deepEqual(updates(page)[0].config.adapters.artifacts.map((artifact) => artifact.type),
    ["github.pr", "github.issue"]);
});

test("the confirmation line names only the checked types, truthfully and in a fixed order", async () => {
  const page = await captureReview(GITHUB_URL);
  page.el("capture-site-repos").checked = false;
  page.el("capture-site-repos").dispatch("input");
  page.el("capture-site-prs").checked = false;
  page.el("capture-site-prs").dispatch("input");
  assert.match(page.el("capture-site-preview").textContent, /Records issues owned by: acme-corp\./);

  page.el("capture-site-repos").checked = true;
  page.el("capture-site-repos").dispatch("input");
  page.el("capture-site-prs").checked = true;
  page.el("capture-site-prs").dispatch("input");
  page.el("capture-site-issues").checked = false;
  page.el("capture-site-issues").dispatch("input");
  assert.match(page.el("capture-site-preview").textContent, /Records repositories and pull requests owned by: acme-corp\./);

  page.el("capture-site-issues").checked = true;
  page.el("capture-site-issues").dispatch("input");
  assert.match(page.el("capture-site-preview").textContent, /Records repositories, pull requests and issues owned by: acme-corp\./);
});

test("unchecking every GitHub kind disables save, and clearing the organization blocks it", async () => {
  const page = await captureReview(GITHUB_URL);
  for (const id of ["capture-site-repos", "capture-site-prs", "capture-site-issues"]) {
    page.el(id).checked = false;
    page.el(id).dispatch("input");
  }
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(page.el("capture-site-orgs").disabled, true);
  page.el("capture-site-repos").checked = true;
  page.el("capture-site-repos").dispatch("input");
  page.el("capture-site-orgs").value = "";
  page.el("capture-site-orgs").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
});

test("canceling and switching tabs clear a review without changing capture", async () => {
  const page = await captureReview(GITHUB_URL);
  page.el("capture-site-orgs").value = "sensitive-org";
  page.el("capture-site-cancel").click();
  assert.equal(page.el("capture-site-review").hidden, true);
  assert.equal(page.el("capture-site-orgs").value, "");
  assert.equal(updates(page).length, 0);
  page.el("capture-site").click();
  page.navigate(JIRA_URL, 2);
  await until(() => page.el("capture-site-review").hidden, "tab change to dismiss the review");
  page.el("capture-site-save").click();
  assert.equal(updates(page).length, 0);
  page.navigate("chrome://extensions", 3);
  await until(() => page.el("capture-site-area").hidden, "non-web tab to hide capture setup");
});

test("a same-URL tab switch and an SPA route change dismiss the review", async () => {
  const page = await captureReview();
  page.navigate(JIRA_URL, 2);
  await until(() => page.el("capture-site-review").hidden, "same-URL tab switch to dismiss review");
  page.el("capture-site").click();
  assert.equal(page.el("capture-site-review").hidden, false);
  page.navigate("https://acme.atlassian.net/browse/PROJ-43", 2);
  await until(() => page.el("capture-site-review").hidden, "SPA navigation to dismiss review");
  assert.equal(updates(page).length, 0);
});

test("a concurrent configuration save refuses the old review rather than rebasing it", async () => {
  const page = await captureReview();
  page.state.config.strip_params = ["tracking"];
  page.state.revision = "2";
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("Settings changed"), "stale config refusal");
  await until(() => page.el("capture-site-message").textContent.includes("Settings changed"), "stale review marker");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(updates(page).length, 0);
  assert.deepEqual(page.state.config.strip_params, ["tracking"]);
});

test("host revision conflicts keep the reviewed site unsaved", async () => {
  const page = await captureReview(JIRA_URL, { intercept(request) {
    if (request.type === "update_config") return { type: "error", code: "conflict", message: "Settings changed at commit" };
  } });
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("Settings changed at commit"), "revision conflict to surface");
  await until(() => !page.el("note-body").disabled, "old note to return after failed commit");
  assert.equal(page.state.config.adapters.artifacts.length, 0);
  assert.equal(updates(page).length, 1);
  assert.equal(page.el("capture-site-review").hidden, false);
});

test("a pending local pause or removal blocks capture both at review and at save", async () => {
  for (const workerStatus of [
    { pause_pending: true, pause_token: "pause" },
    { pending_removals: ["https://acme.atlassian.net"] },
  ]) {
    const page = await captureReview(JIRA_URL, { workerStatus });
    assert.equal(page.el("capture-site-save").disabled, true);
    assert.match(page.el("capture-site-message").textContent, /pause|removal/i);
    assert.equal(updates(page).length, 0);
  }
  const page = await captureReview();
  page.state.workerStatus.pause_pending = true;
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("pending"), "new local pause to refuse save");
  assert.equal(updates(page).length, 0);
});

test("adding another site leaves global capture paused", async () => {
  const config = structuredClone(CONFIG);
  const { starterOffers } = await import("../dist/starter-adapters.js");
  config.adapters.artifacts = starterOffers("https://first.atlassian.net", []).map((offer) => offer.adapter);
  const page = await captureReview(JIRA_URL, { config });
  assert.match(page.el("capture-site-save").textContent, /keep capture paused/);
  await saveCapture(page);
  assert.equal(updates(page)[0].config.capture_enabled, false);
  assert.equal(updates(page)[0].config.adapters.artifacts.length, 2);
  assert.deepEqual(updates(page)[0].config.adapters.artifacts[0], config.adapters.artifacts[0]);
});

test("configured sites, missing folders, and invalid policies cannot use Capture Site", async () => {
  const { starterOffers } = await import("../dist/starter-adapters.js");
  for (const options of [
    { config: { ...CONFIG, storage: null } },
    { adapterIssue: "Future policy format" },
    { configIssue: "Folder moved" },
    { config: { ...CONFIG, adapters: { format: 1, artifacts: starterOffers("https://acme.atlassian.net", []).map((offer) => offer.adapter) } } },
  ]) {
    const page = await install({ tab: { id: 1, url: JIRA_URL }, ...options });
    await until(() => !page.el("capture-site-area").hidden, "capture area to settle");
    assert.ok(page.el("capture-site").disabled || page.el("capture-site").hidden);
    page.el("capture-site").click();
    assert.equal(updates(page).length, 0);
    assert.equal(page.el("capture-site-review").hidden, true);
  }
});


test("pending note edits save before capture changes identity, then load the new identity", async () => {
  const page = await captureReview();
  page.el("note-body").value = "Keep this text";
  page.el("note-body").dispatch("input");
  await saveCapture(page);
  const requests = page.state.requests;
  const saved = requests.findIndex((request) => request.type === "save_note");
  const updated = requests.findIndex((request) => request.type === "update_config");
  assert.ok(saved >= 0 && saved < updated);
  assert.equal(requests[saved].body, "Keep this text");
  assert.ok(requests.slice(updated + 1).some((request) => request.type === "load_note"));
});

test("the save rechecks the active tab even before Chrome delivers a tab event", async () => {
  const page = await captureReview();
  page.state.tab.url = "https://other.atlassian.net/browse/OTHER-1";
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("active page changed"), "fresh-tab refusal");
  assert.equal(updates(page).length, 0);
});

test("note loads stay suspended through title updates during a capture save", async () => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const page = await captureReview(JIRA_URL, { async intercept(request) {
    if (request.type === "update_config") await gate;
  } });
  page.el("capture-site-save").click();
  page.el("capture-site-save").click();
  await until(() => updates(page).length === 1, "in-flight config save");
  const initialLoads = page.state.requests.filter((request) => request.type === "load_note").length;
  const initialQueries = page.calls("tabs.query");
  page.navigate(JIRA_URL);
  await until(() => page.calls("tabs.query") > initialQueries, "title/tab refresh during save");
  await new Promise((resolve) => setTimeout(resolve, 5));
  assert.equal(page.el("note-body").disabled, true);
  assert.equal(page.state.requests.filter((request) => request.type === "load_note").length, initialLoads);
  release();
  await until(() => page.el("capture-site-review").hidden, "capture save to complete");
  await until(() => !page.el("note-body").disabled, "new note identity to load");
  assert.equal(updates(page).length, 1);
});

test("a failed old-note save keeps a copy-out through capture setup", async () => {
  const page = await captureReview(JIRA_URL, { intercept(request) {
    if (request.type === "save_note") return { type: "error", code: "internal", message: "Notes volume unavailable" };
  } });
  page.el("note-body").value = "Never lose my edit";
  page.el("note-body").dispatch("input");
  await saveCapture(page);
  assert.equal(page.el("note-conflict").hidden, false);
  assert.equal(page.el("note-unsaved").value, "Never lose my edit");
  assert.equal(updates(page).length, 1);
});

test("switching pages during the post-capture note reload follows the new note", async () => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  let reloading = false;
  const nextUrl = "https://acme.atlassian.net/browse/PROJ-43";
  const page = await captureReview(JIRA_URL, { async intercept(request, state) {
    if (request.type === "load_note" && state.revision === "2") {
      if (request.url === JIRA_URL) {
        reloading = true;
        await gate;
      }
      return { ...hostReply(request), body: request.url };
    }
  } });
  page.el("capture-site-save").click();
  await until(() => reloading, "post-save note reload");
  try {
    page.navigate(nextUrl, 2);
    await until(() => page.el("page-url").textContent === nextUrl, "new page header");
  } finally {
    release();
  }
  await until(() => !page.el("note-body").disabled && page.el("note-body").value === nextUrl,
    "the new page's note after capture setup");
  page.el("note-body").value = "Edit for the new page";
  page.el("note-body").dispatch("input");
  // A tab change flushes the edit without waiting for the autosave timer.
  page.navigate("https://other.example/", 3);
  await until(() => page.state.requests.some((request) => request.type === "save_note"), "note save");
  const saved = page.state.requests.find((request) => request.type === "save_note");
  assert.equal(saved.url, nextUrl);
  assert.equal(saved.body, "Edit for the new page");
});

test("a verification failure after commit says the site was saved and never resubmits", async () => {
  const page = await captureReview(JIRA_URL, { intercept(request, state) {
    if (request.type === "get_config" && state.revision === "2") {
      return { type: "error", code: "internal", message: "Status temporarily unavailable" };
    }
  } });
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("Site saved, but"), "post-commit failure message");
  assert.equal(updates(page).length, 1);
  assert.equal(page.state.config.adapters.artifacts[0].type, "jira.issue");
  assert.ok(page.state.workerRequests.some((request) => request.kind === "suspend_policy"));
  assert.equal(page.el("capture-site-review").hidden, true);
});

test("a policy update makes an open review stale before it can be saved", async () => {
  const page = await captureReview();
  page.state.revision = "2";
  page.policyChanged();
  await until(() => page.el("capture-site-message").textContent.includes("Settings changed"), "policy revision refresh");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(updates(page).length, 0);
});

// --- Artifact card: "Why I'm here" -------------------------------------------

const CAPTURING = { ...CONFIG, capture_enabled: true };

function artifact(id, title, why = null) {
  return {
    artifact_id: id, artifact_type: "doc", title, url: `https://docs.example/${id}`,
    first_seen: "2026-10-01", last_seen: "2026-10-27", focused_seconds: 60,
    background_seconds: 0, opens: 1, days_opened: 1, last_mode: null, why, linked_count: 0,
    within: null, contained: [], incoming_trails: [],
  };
}

test("artifact shape validation requires the container fields", () => {
  const result = (value) => isResponseShape(recognized(value));
  const repo = { artifact_id: "github.repo:github.com/acme/widgets", artifact_type: "github.repo",
    title: "acme/widgets", url: "https://github.com/acme/widgets" };
  const child = { artifact_id: "github.pr:github.com/acme/widgets/7", artifact_type: "github.pr",
    title: "Fix", url: "https://github.com/acme/widgets/pull/7", focused_seconds: 60 };
  assert.equal(result(artifact("doc:a", "A")), true);
  assert.equal(result({ ...artifact("doc:a", "A"), within: repo }), true);
  assert.equal(result({ ...artifact("doc:a", "A"), contained: [child] }), true);
  const { contained: _contained, ...missingContained } = artifact("doc:a", "A");
  assert.equal(result(missingContained), false);
  const { within: _within, ...missingWithin } = artifact("doc:a", "A");
  assert.equal(result(missingWithin), false);
  assert.equal(result({ ...artifact("doc:a", "A"), within: { ...repo, url: 7 } }), false);
  assert.equal(result({ ...artifact("doc:a", "A"), contained: [{ ...child, focused_seconds: -1 }] }), false);
  assert.equal(result({ ...artifact("doc:a", "A"), contained: Array(21).fill(child) }), false);
});

const recognized = (value) => ({ type: "artifact_result", recognized: true, artifact: value, summary: null });
const settle = (ms = 50) => new Promise((resolve) => setTimeout(resolve, ms));

function typeWhy(page, text) {
  page.el("artifact-why").value = text;
  page.el("artifact-why").dispatch("input");
}

async function noteLoaded(page) {
  await until(() => page.sent("load_note").length > 0 && page.el("note-body").disabled === false,
    "the note to load");
}

test("Save why waits for the opening capture replay, then records", async () => {
  let finishReplay;
  const replay = new Promise((resolve) => { finishReplay = resolve; });
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(artifact("doc:a", "Doc A")) : undefined,
    workerReply: async (request) => {
      if (request.kind === "sync_now") await replay;
      return undefined;
    },
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  await noteLoaded(page);
  await settle();
  typeWhy(page, "reviewing for launch");
  // The replay is still running, so the click would be ignored: Save says so
  // by being disabled rather than doing nothing.
  assert.equal(page.el("save-why").disabled, true);

  finishReplay();
  await until(() => page.el("save-why").disabled === false, "Save to enable after the replay");
  page.el("save-why").click();
  await until(() => page.sent("record_why").length === 1, "record_why");
  const [request] = page.sent("record_why");
  assert.equal(request.event.artifact_id, "doc:a");
  assert.equal(request.event.url, "https://docs.example/a");
  assert.equal(request.event.text, "reviewing for launch");
  await settle(200);
  assert.equal(page.el("status").textContent, "Artifact context saved.");
});

test("Save why is disabled until there is text to save", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(artifact("doc:a", "Doc A")) : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  await noteLoaded(page);
  await settle();
  assert.equal(page.el("save-why").disabled, true);
  typeWhy(page, "  ");
  assert.equal(page.el("save-why").disabled, true);
  typeWhy(page, "context");
  assert.equal(page.el("save-why").disabled, false);
  typeWhy(page, "");
  assert.equal(page.el("save-why").disabled, true);
});

test("a failed artifact lookup for a new page clears the previous page's card", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => {
      if (request.type !== "get_artifact") return undefined;
      return request.url.endsWith("/a")
        ? recognized(artifact("doc:a", "Doc A"))
        : { type: "error", code: "internal", message: "index busy" };
    },
  });
  await until(() => page.el("artifact-heading").textContent === "Doc A", "page A's card");
  await noteLoaded(page);
  await settle();
  typeWhy(page, "context for A");

  page.followLink("https://other.example/b", "B");
  await until(() => page.sent("get_artifact").some((request) => request.url.endsWith("/b")), "B's lookup");
  await until(() => /unavailable: index busy/.test(page.el("artifact-status").textContent), "the lookup error");
  assert.equal(page.el("artifact-card").hidden, true, "page B still shows page A's card");
  assert.equal(page.el("save-why").disabled, true);
  // Nothing can attach page B's URL to page A's artifact.
  page.el("save-why").dispatch("click");
  await settle();
  assert.equal(page.sent("record_why").length, 0);
  assert.notEqual(page.el("status").textContent, "Artifact context saved.");
});

test("a failed artifact lookup for the same page keeps its card and draft", async () => {
  let fail = false;
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => {
      if (request.type !== "get_artifact") return undefined;
      return fail
        ? { type: "error", code: "internal", message: "index busy" }
        : recognized(artifact("doc:a", "Doc A", "old reason"));
    },
  });
  await until(() => page.el("artifact-why").value === "old reason", "the stored why");
  await noteLoaded(page);
  await settle();
  typeWhy(page, "my draft");
  fail = true;
  page.titleChanged("A (1)");
  await until(() => /unavailable: index busy/.test(page.el("artifact-status").textContent), "the lookup error");
  assert.equal(page.el("artifact-card").hidden, false);
  assert.equal(page.el("artifact-why").value, "my draft");
  assert.equal(page.el("save-why").disabled, false);
});

async function outageWithDraft(page) {
  await until(() => page.el("artifact-why").value === "old reason", "the stored why");
  await noteLoaded(page);
  await settle();
  typeWhy(page, "my unsaved draft");
  // The host process exits, and the next start fails.
  page.hostExits();
  page.state.hostDead = true;
  page.titleChanged("A (1)"); // get_artifact cannot start the host
  await settle();
  page.titleChanged("A (2)"); // the panel now knows the host is unreachable
  await until(() => /unavailable/i.test(page.el("artifact-status").textContent), "the host-unavailable status");
}

const withStoredWhy = (request) => request.type === "get_artifact"
  ? recognized(artifact("doc:a", "Doc A", "old reason"))
  : undefined;

test("an unsaved Why draft survives a host outage", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" }, config: CAPTURING, intercept: withStoredWhy,
  });
  await outageWithDraft(page);
  assert.equal(page.el("artifact-card").hidden, false);
  assert.equal(page.el("artifact-why").value, "my unsaved draft");

  // The host is back; the user follows a link to another URL of the same artifact.
  page.state.hostDead = false;
  page.followLink("https://docs.example/a?view=1", "A");
  await until(() => page.el("artifact-status").textContent === "● Artifact recognized", "the lookup to succeed");
  await settle();
  assert.equal(page.el("artifact-card").hidden, false);
  assert.equal(page.el("artifact-why").value, "my unsaved draft",
    "the unsaved Why draft was replaced after the host outage");
});

test("Save why reports a host outage and records once the host is back", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" }, config: CAPTURING, intercept: withStoredWhy,
  });
  await outageWithDraft(page);
  // The click itself restarts the host, so Save stays available.
  assert.equal(page.el("save-why").disabled, false);
  page.el("save-why").click();
  await until(() => /Could not save artifact context/.test(page.el("status").textContent), "the save failure");
  assert.equal(page.el("artifact-why").value, "my unsaved draft");

  page.state.hostDead = false;
  await until(() => page.el("save-why").disabled === false, "Save to enable again");
  page.el("save-why").click();
  await until(() => page.el("status").textContent === "Artifact context saved.", "the saved status");
  const recorded = page.sent("record_why");
  assert.equal(recorded.at(-1).event.text, "my unsaved draft");
  assert.equal(recorded.at(-1).event.url, "https://docs.example/a");
});

test("Save why is disabled while capture is paused, keeps the draft, and points to Resume capture", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: { ...CONFIG, capture_enabled: false },
    intercept: (request) => request.type === "get_artifact" ? recognized(artifact("doc:a", "Doc A")) : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  await noteLoaded(page);
  await settle();
  typeWhy(page, "a draft while paused");
  assert.equal(page.el("save-why").disabled, true);
  assert.equal(page.el("artifact-why").value, "a draft while paused");
  const hint = page.el("why-paused-hint");
  assert.equal(hint.hidden, false);
  assert.match(hint.textContent, /Use Resume capture in settings to turn it back on\./);
  assert.equal(page.sent("record_why").length, 0);
});

test("Save why stays disabled while a local pause waits for the host", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(artifact("doc:a", "Doc A")) : undefined,
    // The worker paused, but the host still has capture on.
    workerReply: () => ({ ok: true, value: { ...statusFor(), pause_pending: true } }),
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  await noteLoaded(page);
  await settle();
  typeWhy(page, "a draft while paused");
  assert.equal(page.el("save-why").disabled, true);
  assert.equal(page.el("why-paused-hint").hidden, false);
  // Resume capture is not offered until the host confirms the pause.
  assert.match(page.el("why-paused-hint").textContent, /Use Pause capture to finish saving this setting in the host\./);
  page.el("save-why").dispatch("click");
  await settle();
  assert.equal(page.sent("record_why").length, 0);
  assert.match(page.el("status").textContent, /Use Pause capture to finish/);
});

// --- Artifact card: container and contents -----------------------------------

const INCOMING_SOURCE = {
  artifact_id: "doc:source", artifact_type: "doc", title: "<img src=x onerror=alert(1)> Source",
  url: "http://localhost:8123/docs/source",
};
const INCOMING_TRAIL = { source: INCOMING_SOURCE, transition: "link", last_seen: "2026-10-27" };

test("incoming trail response shapes bound the list and reject unsafe links and unknown transitions", () => {
  const valid = artifact("doc:target", "Target");
  const result = (trails) => isResponseShape(recognized({ ...valid, incoming_trails: trails }));
  assert.equal(result([INCOMING_TRAIL]), true);
  assert.equal(result(Array(5).fill(INCOMING_TRAIL)), true);
  assert.equal(result(Array(6).fill(INCOMING_TRAIL)), false);
  const { incoming_trails: _trails, ...missing } = valid;
  assert.equal(isResponseShape(recognized(missing)), false);
  for (const transition of ["mentions", "co-focused", "within", "forward_back", "invented"]) {
    assert.equal(result([{ ...INCOMING_TRAIL, transition }]), false, transition);
  }
  for (const url of ["javascript:alert(1)", "data:text/html,hi", "file:///tmp/note", "/relative", "not a URL"]) {
    assert.equal(result([{ ...INCOMING_TRAIL, source: { ...INCOMING_SOURCE, url } }]), false, url);
  }
  assert.equal(result([{ ...INCOMING_TRAIL, source: null }]), false);
  assert.equal(result([{ ...INCOMING_TRAIL, last_seen: "<script>" }]), false);
});

test("incoming source links open the local source fixture and clear when the page changes", async () => {
  const target = { ...artifact("doc:target", "Target"), incoming_trails: [INCOMING_TRAIL,
    { ...INCOMING_TRAIL, transition: "form_submit" }] };
  const page = await install({
    tab: { id: 1, url: "http://localhost:8123/docs/target", title: "Target" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact"
      ? recognized(request.url === INCOMING_SOURCE.url ? artifact("doc:source", "Source") : target)
      : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the target card");
  const rows = page.el("artifact-incoming-list").children;
  assert.equal(rows.length, 2);
  assert.equal(rows[0].textContent, `Link navigation from ${INCOMING_SOURCE.title} · last recorded 2026-10-27`);
  assert.equal(rows[1].textContent, `Form submission navigation from ${INCOMING_SOURCE.title} · last recorded 2026-10-27`);
  assert.equal(page.el("artifact-incoming-empty").hidden, true);
  const link = rows[0].querySelector("a");
  assert.equal(link.href, INCOMING_SOURCE.url);
  assert.equal(link.target, "_blank");
  assert.equal(link.rel, "noopener noreferrer");
  // The fake DOM has no default anchor action. Follow its explicit URL as
  // Chrome does when the newly opened source tab becomes active.
  page.navigate(link.href, 2);
  await until(() => page.el("artifact-heading").textContent === "Source", "the source fixture card");
  assert.equal(page.el("artifact-incoming-list").children.length, 0);
  assert.equal(page.el("artifact-incoming-list").hidden, true);
  assert.equal(page.el("artifact-incoming-empty").hidden, false);
  assert.equal(page.calls("permissions.request"), 0);
  page.navigate("chrome://extensions", 3);
  await until(() => page.el("artifact-card").hidden, "no card on a Chrome page");
  assert.equal(page.el("artifact-incoming-list").children.length, 0);
});

const REPO = {
  artifact_id: "github.repo:github.com/acme-corp/widgets",
  artifact_type: "github.repo",
  title: "acme-corp/widgets",
  url: "https://github.com/acme-corp/widgets",
};

test("artifact card links a PR to its repository", async () => {
  const pr = { ...artifact("github.pr:github.com/acme-corp/widgets/7", "Fix the thing"), within: REPO };
  const page = await install({
    tab: { id: 1, url: "https://github.com/acme-corp/widgets/pull/7", title: "Fix the thing" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(pr) : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  assert.equal(page.el("artifact-within").hidden, false);
  assert.equal(page.el("artifact-within").textContent, "In acme-corp/widgets");
  const link = page.el("artifact-within").querySelector("a");
  assert.equal(link.href, REPO.url);
});

test("repository card lists contained work", async () => {
  const pr = { artifact_id: "github.pr:github.com/acme-corp/widgets/7", artifact_type: "github.pr",
    title: "Fix the thing", url: "https://github.com/acme-corp/widgets/pull/7", focused_seconds: 125 };
  const issue = { artifact_id: "github.issue:github.com/acme-corp/widgets/3", artifact_type: "github.issue",
    title: "Flaky test", url: "https://github.com/acme-corp/widgets/issues/3", focused_seconds: 40 };
  const repo = { ...artifact("github.repo:github.com/acme-corp/widgets", "acme-corp/widgets"), contained: [pr, issue] };
  const page = await install({
    tab: { id: 1, url: REPO.url, title: "acme-corp/widgets" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(repo) : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  assert.equal(page.el("artifact-contained").hidden, false);
  assert.equal(page.el("artifact-contained-heading").textContent, "Worked on inside");
  const rows = page.el("artifact-contained-list").children;
  assert.equal(rows.length, 2, "one row per contained artifact");
  const firstLink = rows[0].querySelector("a");
  assert.equal(firstLink.textContent, "Fix the thing · 2 min");
  assert.equal(firstLink.href, pr.url);
  const secondLink = rows[1].querySelector("a");
  assert.equal(secondLink.textContent, "Flaky test · 40 sec");
  assert.equal(secondLink.href, issue.url);
});

test("card hides container rows when absent", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(artifact("doc:a", "Doc A")) : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  assert.equal(page.el("artifact-within").hidden, true);
  assert.equal(page.el("artifact-within").textContent, "");
  assert.equal(page.el("artifact-contained").hidden, true);
  assert.equal(page.el("artifact-contained-list").children.length, 0);
});

// --- Summarize & log (plan Task 39) -----------------------------------------

const ARTICLE_URL = "https://read.example/article";
const RUN_VIEW = {
  run_id: "00000000-0000-4000-8000-000000000039", state: "queued", harness: "codex",
  record_key: { kind: "url", value: ARTICLE_URL }, written_record_key: null,
  url: ARTICLE_URL, title: "Readable title", cwd: null,
  created_at: "2026-10-27T09:00:00-07:00", started_at: null, finished_at: null,
  error: null, error_kind: null, thread_id: null, session_id: null, summary_path: null,
  stream_bytes: 0, can_retry_save: false, answer: null,
};
const WARNING_SEEN_KEY = storageKey("summarize_warning_seen_v1");
const RUNS_MAY_EXIST_KEY = storageKey("runs_may_exist_v1");

// A page Readability can read: the bundled library loads, then the extractor
// returns the page.
async function readablePage(injection) {
  if (injection.files) return [{ frameId: 0 }];
  return [{
    frameId: 0,
    result: {
      title: "Readable title", content: "Body text", selection: "picked", truncated: false, readable: true,
      href: ARTICLE_URL,
    },
  }];
}

// The extractor read a document at `href`, not the page the run is for.
function pageAt(href) {
  return async (injection) => {
    const [answer] = await readablePage(injection);
    return [injection.files ? answer : { ...answer, result: { ...answer.result, href } }];
  };
}

function startsRun(request) {
  if (request.type === "start_run") return { type: "run_started", run: RUN_VIEW, existing: false };
  if (request.type === "get_run") return { type: "run_fetched", run: RUN_VIEW, stream_text: "", next_offset: 0, more: false };
  return null;
}

function buttonIn(root, text) {
  for (const child of root.children) {
    if (child.tagName === "BUTTON" && child.textContent === text) return child;
    const found = buttonIn(child, text);
    if (found) return found;
  }
  return null;
}

// A harness set up in settings, as config_result reports it.
const SET_UP = {
  ...CONFIG,
  agent: { harness: "codex", binary: "/usr/local/bin/codex", version: "1.0.0", timeout_secs: 600, probed_at: "2026-10-27T09:00:00-07:00" },
  storage: { ...CONFIG.storage, summaries_dir: "summaries" },
};

async function summarizePanel(options = {}) {
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Tab title" }, agentStatus: READY, config: SET_UP,
    executeScript: readablePage, intercept: startsRun, ...options,
  });
  await until(() => page.el("note-editor").hidden === false, "the page to show");
  return page;
}

test("Summarize & log shows on any HTTP(S) tab and needs a ready harness", async () => {
  const unset = await install({ tab: { id: 1, url: ARTICLE_URL, title: "A" } });
  await until(() => unset.el("summarize").hidden === false, "the Summarize button");
  // The note loads only after the panel has read the host config.
  await until(() => unset.el("note-body").disabled === false, "the note to load");
  assert.equal(unset.el("summarize").disabled, true);
  assert.equal(unset.el("summarize").title, "Set up a summary harness in settings first.");
  teardown.splice(0).forEach((listener) => listener());

  const ready = await summarizePanel();
  await until(() => ready.el("summarize").disabled === false, "the button to enable");
  assert.equal(ready.el("summarize").hidden, false);
  assert.equal(ready.el("summarize").title, "");
  teardown.splice(0).forEach((listener) => listener());

  const other = await install({ tab: { id: 1, url: "chrome://extensions/", title: "Extensions" }, agentStatus: READY });
  await until(() => other.el("page-unusable").hidden === false, "the not-a-web-page state");
  assert.equal(other.el("summarize").hidden, true);
});

test("clicking Summarize extracts in the top frame then sends start_run with no record key and a local requested_at", async () => {
  const page = await summarizePanel({ local: new Map([[WARNING_SEEN_KEY, true]]) });
  await until(() => page.el("summarize").disabled === false, "the button to enable");
  page.el("summarize").click();
  await until(() => page.sent("start_run").length === 1, "start_run");

  const [library, extract] = page.state.injections;
  assert.deepEqual(library, { target: { tabId: 1, frameIds: [0] }, files: ["vendor/readability.js"] });
  assert.deepEqual(extract.target, { tabId: 1, frameIds: [0] });
  assert.equal(extract.func.name, "extractReadablePage");
  assert.deepEqual(extract.args, [EXTRACT_LIMITS]);

  const [request] = page.sent("start_run");
  assert.equal(request.url, ARTICLE_URL);
  assert.equal(request.title, "Readable title");
  assert.equal(request.content, "Body text");
  assert.equal(request.selection, "picked");
  assert.equal(request.harness, null);
  assert.equal(request.record_key, null);
  assert.match(request.requested_at, /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d[+-]\d\d:\d\d$/);
  await until(() => /Readable title.*Queued/.test(page.el("run-area").textContent), "the run card");
});

test("an executeScript failure shows \"can't read this page\"", async () => {
  const page = await summarizePanel({
    local: new Map([[WARNING_SEEN_KEY, true]]),
    executeScript: async () => { throw new Error("Cannot access contents of the page"); },
  });
  await until(() => page.el("summarize").disabled === false, "the button to enable");
  page.el("summarize").click();
  await until(() => /can't read this page/.test(page.el("run-area").textContent), "the unreadable message");
  assert.equal(page.sent("start_run").length, 0);
  teardown.splice(0).forEach((listener) => listener());

  // Readability and the body text both came back empty.
  const empty = await summarizePanel({
    local: new Map([[WARNING_SEEN_KEY, true]]),
    executeScript: async (injection) => injection.files ? [{ frameId: 0 }] : [{
      frameId: 0, result: { title: "", content: "", selection: null, truncated: false, readable: false, href: ARTICLE_URL },
    }],
  });
  await until(() => empty.el("summarize").disabled === false, "the button to enable");
  empty.el("summarize").click();
  await until(() => /can't read this page/.test(empty.el("run-area").textContent), "the unreadable message");
  assert.equal(empty.sent("start_run").length, 0);
});

test("Summarize refuses when the tab holds another document by extraction time", async () => {
  // A navigation, or an SPA's pushState, between the click and the extractor.
  for (const href of ["https://read.example/next", "https://read.example/article?page=2", undefined]) {
    const page = await summarizePanel({ local: new Map([[WARNING_SEEN_KEY, true]]), executeScript: pageAt(href) });
    await until(() => page.el("summarize").disabled === false, "the button to enable");
    page.el("summarize").click();
    await until(() => /page changed/.test(page.el("run-area").textContent), "the page-changed message");
    assert.equal(page.sent("start_run").length, 0);
    teardown.splice(0).forEach((listener) => listener());
  }

  // The same address, spelled as Chrome canonicalizes it, is the same page.
  const same = await summarizePanel({
    local: new Map([[WARNING_SEEN_KEY, true]]), executeScript: pageAt("HTTPS://READ.example:443/article"),
  });
  await until(() => same.el("summarize").disabled === false, "the button to enable");
  same.el("summarize").click();
  await until(() => same.sent("start_run").length === 1, "start_run");
});

test("start_run never carries a lone surrogate, whichever field it came from", async () => {
  // Chrome's tab title stands in when Readability finds none.
  const page = await summarizePanel({
    tab: { id: 1, url: ARTICLE_URL, title: "Tab \uD800 title" },
    local: new Map([[WARNING_SEEN_KEY, true]]),
    executeScript: async (injection) => injection.files ? [{ frameId: 0 }] : [{
      frameId: 0,
      result: {
        title: "", content: "Body \uDC00 text 😀", selection: "\uD83D", truncated: false, readable: true, href: ARTICLE_URL,
      },
    }],
  });
  await until(() => page.el("summarize").disabled === false, "the button to enable");
  page.el("summarize").click();
  await until(() => page.sent("start_run").length === 1, "start_run");
  const [request] = page.sent("start_run");
  assert.equal(request.title, "Tab \uFFFD title");
  assert.equal(request.content, "Body \uFFFD text 😀");
  assert.equal(request.selection, "\uFFFD");
  assert.doesNotMatch(JSON.stringify(request), /\\ud[89a-f][0-9a-f]{2}/i);
});

test("a slow summary's line stays with the tab it was for", async () => {
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const page = await summarizePanel({
    local: new Map([[WARNING_SEEN_KEY, true]]),
    intercept: (request) => request.type === "start_run" ? held : startsRun(request),
  });
  await until(() => page.el("summarize").disabled === false, "the button to enable");
  page.el("summarize").click();
  await until(() => page.sent("start_run").length === 1, "start_run");
  assert.match(page.el("run-area").textContent, /Reading this page/);

  // The user moves to tab 2 while the service cold-starts.
  page.navigate("https://other.example/b", 2);
  await until(() => page.el("page-url").textContent === "https://other.example/b", "tab 2 to show");
  assert.doesNotMatch(page.el("run-area").textContent, /Reading this page/);
  release({ type: "error", code: "service_unavailable", message: "The service did not start." });
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.doesNotMatch(page.el("run-area").textContent, /did not start/);
});

test("a summary that finishes after a tab switch leaves the new tab's line alone", async () => {
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const page = await summarizePanel({
    local: new Map([[WARNING_SEEN_KEY, true]]),
    intercept: (request) => request.type === "start_run" ? held : startsRun(request),
  });
  await until(() => page.el("summarize").disabled === false, "the button to enable");
  page.el("summarize").click();
  await until(() => page.sent("start_run").length === 1, "start_run");
  page.navigate("https://other.example/b", 2);
  await until(() => page.el("page-url").textContent === "https://other.example/b", "tab 2 to show");
  page.notify({ kind: "summarize_failed", message: "Tab two's own notice.", tab_id: 2, at: Date.now() });
  await until(() => page.el("run-area").textContent.includes("Tab two's own notice."), "tab 2's notice");
  release({ type: "run_started", run: RUN_VIEW, existing: false });
  await until(() => /Queued/.test(page.el("run-area").textContent), "the run card");
  assert.match(page.el("run-area").textContent, /Tab two's own notice\./);
});

test("a worker notice for a tab the panel has not followed yet shows once that tab is followed", async () => {
  const session = new Map();
  const page = await summarizePanel({ session, local: new Map([[WARNING_SEEN_KEY, true]]) });
  // Startup reads (and clears) whatever was stored before the panel loaded.
  await until(() => page.sent("list_runs").length === 1, "startup's list_runs");
  await new Promise((resolve) => setTimeout(resolve, 20));
  // The shortcut fired on tab 2 just after it was activated, while the panel
  // still followed tab 1: the live notice is not for the followed tab.
  const notice = { kind: "summarize_failed", message: "Tab two failed early.", tab_id: 2, at: Date.now() };
  session.set(PANEL_NOTICE_KEY, notice);
  page.notify(notice);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.doesNotMatch(page.el("run-area").textContent, /Tab two failed early/);
  assert.ok(session.has(PANEL_NOTICE_KEY), "a notice for another tab stays stored");

  page.navigate("https://other.example/b", 2);
  await until(() => page.el("run-area").textContent.includes("Tab two failed early."), "tab 2's stored notice");
  await until(() => !session.has(PANEL_NOTICE_KEY), "the notice to be consumed");
});

test("the setup warning shows once", async () => {
  const local = new Map();
  const first = await summarizePanel({ local });
  await until(() => first.el("summarize").disabled === false, "the button to enable");
  first.el("summarize").click();
  await until(() => first.el("run-area").textContent.includes(SETUP_WARNING), "the setup warning");
  // The warning does not hold the run back.
  await until(() => first.sent("start_run").length === 1, "start_run");
  assert.equal(local.get(WARNING_SEEN_KEY), true);
  buttonIn(first.el("run-area"), "Got it").click();
  assert.ok(!first.el("run-area").textContent.includes(SETUP_WARNING));
  teardown.splice(0).forEach((listener) => listener());

  const second = await summarizePanel({ local });
  await until(() => second.el("summarize").disabled === false, "the button to enable");
  second.el("summarize").click();
  await until(() => second.sent("start_run").length === 1, "start_run");
  await until(() => /Queued/.test(second.el("run-area").textContent), "the run card");
  assert.ok(!second.el("run-area").textContent.includes(SETUP_WARNING));
});

test("the setup warning shows once for a summary the shortcut or menu started", async () => {
  const listRuns = (request) => request.type === "list_runs" ? { type: "runs_listed", runs: [RUN_VIEW] } : startsRun(request);
  // The worker opened the panel and stored its notice before the panel loaded.
  const local = new Map();
  const session = new Map([[PANEL_NOTICE_KEY, { kind: "run_started", run_id: RUN_VIEW.run_id, tab_id: 1, at: Date.now() }]]);
  const opened = await summarizePanel({ local, session, intercept: listRuns });
  await until(() => opened.el("run-area").textContent.includes(SETUP_WARNING), "the setup warning");
  await until(() => /Queued/.test(opened.el("run-area").textContent), "the run card");
  assert.equal(local.get(WARNING_SEEN_KEY), true);
  teardown.splice(0).forEach((listener) => listener());

  // A panel already open hears the worker's notice; the warning is not shown twice.
  const fresh = new Map();
  const open = await summarizePanel({ local: fresh, intercept: listRuns });
  open.notify({ kind: "run_started", run_id: RUN_VIEW.run_id, tab_id: 1, at: Date.now() });
  await until(() => open.el("run-area").textContent.includes(SETUP_WARNING), "the live setup warning");
  assert.equal(fresh.get(WARNING_SEEN_KEY), true);
  teardown.splice(0).forEach((listener) => listener());

  const again = await summarizePanel({ local: fresh, intercept: listRuns });
  again.notify({ kind: "run_started", run_id: RUN_VIEW.run_id, tab_id: 1, at: Date.now() + 1 });
  await until(() => /Queued/.test(again.el("run-area").textContent), "the run card");
  assert.ok(!again.el("run-area").textContent.includes(SETUP_WARNING));
});

test("a summarize failure sent before the panel loads is shown when it opens", async () => {
  const message = "The Brauser service is not running. Register it again.";
  const session = new Map([[PANEL_NOTICE_KEY, { kind: "summarize_failed", message, tab_id: 1, at: Date.now() }]]);
  const page = await summarizePanel({ session });
  await until(() => page.el("run-area").textContent.includes(message), "the stored notice");
  await until(() => !session.has(PANEL_NOTICE_KEY), "the notice to clear");
  teardown.splice(0).forEach((listener) => listener());

  // A notice older than two minutes, or for another tab, is cleared unseen.
  for (const stale of [
    { kind: "summarize_failed", message, tab_id: 1, at: Date.now() - 3 * 60_000 },
    { kind: "summarize_failed", message, tab_id: 2, at: Date.now() },
  ]) {
    const later = new Map([[PANEL_NOTICE_KEY, stale]]);
    const other = await summarizePanel({ session: later });
    await until(() => !later.has(PANEL_NOTICE_KEY), "the stale notice to clear");
    assert.ok(!other.el("run-area").textContent.includes(message));
    teardown.splice(0).forEach((listener) => listener());
  }

  // A notice the worker sends while the panel is open shows at once.
  const open = await summarizePanel();
  open.notify({ kind: "summarize_failed", message: "Summaries work on http(s) pages.", tab_id: 1, at: Date.now() });
  await until(() => open.el("run-area").textContent.includes("Summaries work on http(s) pages."), "the live notice");
});

test("with no summary harness set up, opening the panel does not wake the service", async () => {
  // list_runs makes the host start the registered service (launchctl), which
  // is on demand: only a set-up harness can have runs to restore.
  const page = await install({ tab: { id: 1, url: ARTICLE_URL, title: "A" } });
  await until(() => page.el("note-body").disabled === false, "the note to load");
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(page.sent("list_runs").length, 0);
  teardown.splice(0).forEach((listener) => listener());

  const ready = await summarizePanel();
  await until(() => ready.sent("list_runs").length === 1, "list_runs");
});

test("a run still in the service restores with Cancel after the harness is removed", async () => {
  // The harness was set up, a run started, then the harness was removed:
  // the service still runs it, so the panel must show it and let it stop.
  const running = { ...RUN_VIEW, state: "running", started_at: "2026-10-27T09:00:01-07:00" };
  const local = new Map([[RUNS_MAY_EXIST_KEY, Date.now() - 60_000]]);
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "A" }, local,
    intercept: (request) => {
      if (request.type === "list_runs") return { type: "runs_listed", runs: [running] };
      if (request.type === "get_run") return { type: "run_fetched", run: running, stream_text: "", next_offset: 0, more: false };
      return null;
    },
  });
  await until(() => /Running/.test(page.el("run-area").textContent), "the restored card");
  assert.ok(buttonIn(page.el("run-area"), "Cancel"), "the restored card offers Cancel");
  assert.equal(page.sent("list_runs").length, 1);
  teardown.splice(0).forEach((listener) => listener());

  // Once list_runs finds nothing left to show, later opens stop asking.
  const empty = await install({ tab: { id: 1, url: ARTICLE_URL, title: "A" }, local });
  await until(() => empty.sent("list_runs").length === 1, "list_runs");
  await until(() => !local.has(RUNS_MAY_EXIST_KEY), "the record to clear");
  teardown.splice(0).forEach((listener) => listener());
  const later = await install({ tab: { id: 1, url: ARTICLE_URL, title: "A" }, local });
  await until(() => later.el("note-body").disabled === false, "the note to load");
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(later.sent("list_runs").length, 0);
});

test("a started summary records that runs may exist", async () => {
  const local = new Map([[WARNING_SEEN_KEY, true]]);
  const page = await summarizePanel({ local });
  await until(() => page.el("summarize").disabled === false, "the button to enable");
  page.el("summarize").click();
  await until(() => page.sent("start_run").length === 1, "start_run");
  await until(() => typeof local.get(RUNS_MAY_EXIST_KEY) === "number", "the record");
});

test("the panel restores run cards on load and shows a run the shortcut started", async () => {
  const restored = { ...RUN_VIEW, run_id: "00000000-0000-4000-8000-000000000040", title: "Restored run", state: "running" };
  const started = { ...RUN_VIEW, run_id: "00000000-0000-4000-8000-000000000041", title: "Shortcut run" };
  let runs = [restored];
  const page = await summarizePanel({
    intercept: (request) => {
      if (request.type === "list_runs") return { type: "runs_listed", runs };
      if (request.type === "get_run") {
        return { type: "run_fetched", run: runs.find((run) => run.run_id === request.run_id), stream_text: "", next_offset: 0, more: false };
      }
      return null;
    },
  });
  await until(() => page.el("run-area").textContent.includes("Restored run"), "the restored card");
  runs = [started, restored];
  page.notify({ kind: "run_started", run_id: started.run_id, tab_id: 1, at: Date.now() });
  await until(() => page.el("run-area").textContent.includes("Shortcut run"), "the shortcut's card");
  assert.equal(page.el("run-area").textContent.split("Restored run").length, 2, "each run shows once");
});

// --- The page's existing summary (Task 51) ---------------------------------

const SUMMARY = {
  summarized_at: "2026-10-27T10:00:00-07:00", document_type: "specification",
  category: "technical specification", importance: "high",
  summary_path: "summaries/2026-10-27 Spec.md", harness: "codex",
};

const opensSummary = (request) => request.type === "open_summary"
  ? { type: "summary_opened", relative_path: SUMMARY.summary_path }
  : null;

test("an unrecognized page with a summary shows its line and opens it", async () => {
  let fail = false;
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Article" },
    intercept: (request) => {
      if (request.type === "get_artifact") {
        return { type: "artifact_result", recognized: false, artifact: null, summary: SUMMARY };
      }
      if (request.type === "open_summary" && fail) return { type: "error", code: "conflict", message: "not a summary" };
      return opensSummary(request);
    },
  });
  await until(() => page.el("page-summary").hidden === false, "the page's summary line");
  assert.equal(page.el("page-summary-text").textContent, "specification · technical specification · high importance");
  assert.equal(page.el("artifact-card").hidden, true, "an unrecognized page has no artifact card");
  assert.equal(page.el("artifact-status").textContent, "○ Not an artifact");

  page.el("page-summary-open").click();
  await until(() => page.sent("open_summary").length === 1, "open_summary");
  assert.equal(page.sent("open_summary")[0].url, ARTICLE_URL);
  assert.equal(page.sent("open_summary")[0].summary_path, SUMMARY.summary_path);

  fail = true;
  page.el("page-summary-open").click();
  await until(() => /Could not open the summary: not a summary/.test(page.el("status").textContent), "the open failure");
});

test("a recognized page shows its adapter artifact's summary", async () => {
  // An adapter artifact with a summary and no measured activity yet.
  const summaryOnly = {
    ...artifact("doc:a", "Doc A"), first_seen: "2026-10-27", last_seen: "2026-10-27",
    focused_seconds: 0, background_seconds: 0, opens: 0, days_opened: 0,
  };
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact"
      ? { ...recognized(summaryOnly), summary: { ...SUMMARY, document_type: "report", importance: "low" } }
      : opensSummary(request),
  });
  await until(() => page.el("artifact-heading").textContent === "Doc A", "the artifact card");
  assert.equal(page.el("artifact-card").hidden, false);
  assert.equal(page.el("artifact-measures").textContent, "No activity measured yet");
  assert.equal(page.el("page-summary").hidden, false);
  assert.equal(page.el("page-summary-text").textContent, "report · technical specification · low importance");
  page.el("page-summary-open").click();
  await until(() => page.sent("open_summary").length === 1, "open_summary");
  assert.equal(page.sent("open_summary")[0].url, "https://docs.example/a");
});

test("a page without a summary hides the line", async () => {
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Article" },
    intercept: (request) => {
      if (request.type !== "get_artifact") return null;
      if (request.url.endsWith("/broken")) return { type: "error", code: "internal", message: "index busy" };
      return {
        type: "artifact_result", recognized: false, artifact: null,
        summary: request.url === ARTICLE_URL ? SUMMARY : null,
      };
    },
  });
  assert.equal(page.el("page-summary").hidden, true, "hidden before any lookup");
  await until(() => page.el("page-summary").hidden === false, "the summarized page's line");

  page.followLink("https://read.example/plain", "Plain");
  await until(() => page.sent("get_artifact").some((request) => request.url.endsWith("/plain")), "the next lookup");
  await until(() => page.el("page-summary").hidden === true, "the line to hide");
  assert.equal(page.el("page-summary-text").textContent, "");

  // A failed lookup for another page never shows the previous page's line.
  page.followLink(ARTICLE_URL, "Article");
  await until(() => page.el("page-summary").hidden === false, "the line again");
  page.followLink("https://read.example/broken", "Broken");
  await until(() => /unavailable: index busy/.test(page.el("artifact-status").textContent), "the lookup error");
  assert.equal(page.el("page-summary").hidden, true);
  page.el("page-summary-open").click();
  await settle();
  assert.equal(page.sent("open_summary").length, 0);
});

function descendants(root) {
  return root.children.flatMap((child) => [child, ...descendants(child)]);
}

test("a summarized Recent row says so and renders agent text as text", async () => {
  const hostile = "<img src=https://evil.example/x.png>";
  const row = (title, url, summary) => ({
    local_date: "2026-10-27", artifact_id: `web:${title}`, artifact_type: "web.page", title, url,
    focused_seconds: 0, background_seconds: 0, opens: 0, why: null, summary,
  });
  const items = [
    row("<b>Spec</b>", "https://read.example/spec", { ...SUMMARY, document_type: hostile, importance: "medium" }),
    row("Plain page", "https://read.example/plain", null),
  ];
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Article" },
    intercept: (request) => request.type === "list_artifacts"
      ? { type: "artifact_list_result", items, artifact_types: ["web.page"], truncated: false }
      : opensSummary(request),
  });
  await noteLoaded(page);
  page.el("tab-recent").click();
  await until(() => page.el("recent-list").textContent.includes("Plain page"), "the Recent rows");

  const [summarized, plain] = page.el("recent-list").children.filter((child) => child.tagName !== "H3");
  assert.ok(summarized.textContent.includes("<b>Spec</b>"));
  assert.ok(summarized.textContent.includes(`Summarized · ${hostile}, medium importance`));
  assert.ok(!plain.textContent.includes("Summarized"));
  assert.equal(buttonIn(plain, "Open summary"), null);
  // Every piece of agent text is a text node, never markup.
  assert.ok(descendants(summarized).every((node) => !/^(IMG|B)$/.test(node.tagName)));

  // The button's accessible name says which row it opens.
  assert.equal(buttonIn(summarized, "Open summary").getAttribute("aria-label"), "Open summary: <b>Spec</b>");

  buttonIn(summarized, "Open summary").click();
  await until(() => page.sent("open_summary").length === 1, "open_summary");
  assert.equal(page.sent("open_summary")[0].url, "https://read.example/spec");
  // The row's own summary file, which the URL alone may no longer resolve to.
  assert.equal(page.sent("open_summary")[0].summary_path, SUMMARY.summary_path);
});

test("following a new tab clears the previous page's summary before its lookup returns", async () => {
  let release;
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Article" },
    intercept: (request) => {
      if (request.type !== "get_artifact") return opensSummary(request);
      if (request.url === ARTICLE_URL) return { type: "artifact_result", recognized: false, artifact: null, summary: SUMMARY };
      return new Promise((resolve) => {
        release = () => resolve({ type: "artifact_result", recognized: false, artifact: null, summary: null });
      });
    },
  });
  await until(() => page.el("page-summary").hidden === false, "the page's summary line");
  page.followLink("https://read.example/slow", "Slow");
  await until(() => page.sent("get_artifact").some((request) => request.url.endsWith("/slow")), "the next lookup");
  await settle();
  // The lookup is still out: the previous page's line is gone already.
  assert.equal(page.el("page-summary").hidden, true);
  assert.equal(page.el("page-summary-text").textContent, "");
  page.el("page-summary-open").click();
  await settle();
  assert.equal(page.sent("open_summary").length, 0);
  release();
  await settle();
  assert.equal(page.el("page-summary").hidden, true);
});

test("Open summary buttons are disabled while the host is disconnected", async () => {
  const items = [{
    local_date: "2026-10-27", artifact_id: "web:spec", artifact_type: "web.page", title: "Spec",
    url: "https://read.example/spec", focused_seconds: 0, background_seconds: 0, opens: 0, why: null,
    summary: SUMMARY,
  }];
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Article" },
    intercept: (request) => {
      if (request.type === "get_artifact") return { type: "artifact_result", recognized: false, artifact: null, summary: SUMMARY };
      if (request.type === "list_artifacts") return { type: "artifact_list_result", items, artifact_types: ["web.page"], truncated: false };
      return opensSummary(request);
    },
  });
  await until(() => page.el("page-summary").hidden === false, "the page's summary line");
  await noteLoaded(page);
  page.el("tab-recent").click();
  await until(() => page.el("recent-list").textContent.includes("Spec"), "the Recent row");
  const recentOpen = () => buttonIn(page.el("recent-list"), "Open summary");
  assert.equal(page.el("page-summary-open").disabled, false);
  assert.equal(recentOpen().disabled, false);

  page.hostExits();
  page.state.hostDead = true;
  page.titleChanged("Article (1)");
  await settle();
  page.titleChanged("Article (2)");
  await until(() => /unavailable/i.test(page.el("artifact-status").textContent), "the host-unavailable status");
  assert.equal(page.el("page-summary-open").disabled, true);
  assert.equal(recentOpen().disabled, true);

  // The host is back; the next page's calls reach it.
  page.state.hostDead = false;
  page.followLink("https://read.example/next", "Next");
  await until(() => page.el("page-summary-open").disabled === false, "Open summary to enable again");
  assert.equal(recentOpen().disabled, false);
});

const RECOVERY_TAB = { id: 1, url: "https://docs.example/a", title: "A" };
async function notePanel(options = {}) {
  const page = await install({ tab: RECOVERY_TAB, ...options });
  await until(() => page.sent("load_note").length > 0 && !page.el("note-body").disabled,
    "the note editor to load");
  return page;
}
function editNote(page, text) {
  page.el("note-body").value = text;
  page.el("note-body").dispatch("input");
}

// A policy revision written by settings or another window's panel. The
// host owns note identity; the fake host models its URL-to-artifact change.
function installExternalAdapter(page) {
  page.state.config.adapters = { format: 1, artifacts: [starterOffersFor(new URL(JIRA_URL).origin, [])[0].adapter] };
  page.state.revision = "2";
  page.policyChanged({
    revision: "2", expires_at: Date.now() + 60_000, capture_enabled: false,
    adapters: page.state.config.adapters,
  });
}

const EXTERNAL_NOTE_KEY = { kind: "artifact", value: "jira:acme.atlassian.net/PROJ-42" };
function externalNoteReply(request, state) {
  if (request.type === "load_note" && state.revision === "2") return {
    type: "note_loaded", exists: true, revision: "artifact-r1", title: "Issue", body: "Shared note",
    key: EXTERNAL_NOTE_KEY, older_notes: [],
  };
  if (request.type === "save_note" && state.revision === "2" && request.expected_key.kind === "url") return {
    type: "note_conflict", exists: true, revision: "artifact-r1", title: "Issue", body: "Shared note",
    key: EXTERNAL_NOTE_KEY, reason: "key_changed",
  };
  return null;
}

test("an external adapter revision reloads the note before the next edit uses its new key", async () => {
  const page = await notePanel({ tab: { id: 1, url: JIRA_URL, title: "Issue" }, intercept: externalNoteReply });
  assert.equal(page.sent("load_note").length, 1);
  installExternalAdapter(page);
  await until(() => page.sent("load_note").length === 2 && !page.el("note-body").disabled,
    "the external revision to reload note identity");
  assert.equal(page.sent("load_note")[1].url, JIRA_URL);
  assert.equal(page.el("note-body").value, "Shared note");
  editNote(page, "Shared note with my next edit");
  await until(() => page.el("note-status").textContent === "Saved", "the artifact autosave");
  const [save] = page.sent("save_note");
  assert.deepEqual(save.expected_key, EXTERNAL_NOTE_KEY);
  assert.equal(save.expected_revision, "artifact-r1");
  assert.equal(save.body, "Shared note with my next edit");
  assert.equal(page.el("note-conflict").hidden, true);
});

for (const inFlight of [false, true]) {
  test(`an external revision preserves a ${inFlight ? "saving" : "pending"} note draft in copy-out`, async () => {
    let release;
    const held = new Promise((resolve) => { release = resolve; });
    const page = await notePanel({ tab: { id: 1, url: JIRA_URL, title: "Issue" }, intercept(request, state) {
      if (request.type === "save_note" && request.expected_key.kind === "url") return held;
      return externalNoteReply(request, state);
    } });
    const draft = inFlight ? "Draft already saving plus newer typing" : "Draft still pending";
    if (inFlight) {
      editNote(page, "Draft already saving");
      await until(() => page.sent("save_note").length === 1, "the old-key autosave");
    }
    editNote(page, draft);
    installExternalAdapter(page);
    const reads = page.sent("get_config").length;
    await until(() => page.sent("get_config").length > reads, "the config refresh during save");
    await settle();
    assert.equal(page.sent("save_note").length, 1);
    assert.equal(page.sent("save_note")[0].expected_key.kind, "url");
    assert.equal(page.sent("load_note").length, 1, "note reload must wait for the old-key save");
    assert.equal(page.el("note-body").disabled, true, "stop edits while rebinding the note");
    release(externalNoteReply({ type: "save_note", expected_key: { kind: "url" } }, page.state));
    await until(() => page.el("note-body").value === "Shared note" && !page.el("note-body").disabled,
      "the current artifact note");
    assert.equal(page.el("note-conflict").hidden, false);
    assert.equal(page.el("note-unsaved").value, draft);
    assert.equal(page.sent("save_note").length, 1, "no automatic replay of the refused draft");
    assert.ok(page.state.localStorage.length > 0, "keep the unacknowledged draft in durable recovery");
    editNote(page, "A fresh artifact edit");
    await until(() => page.el("note-status").textContent === "Saved", "the next artifact edit to save");
    assert.deepEqual(page.sent("save_note").at(-1).expected_key, EXTERNAL_NOTE_KEY);
    assert.equal(page.el("note-unsaved").value, draft);
    assert.ok(page.state.localStorage.length > 0, "acknowledging a new edit must keep the conflict copy");
  });
}

test("a policy refresh with the same host revision keeps the editor and pending draft", async () => {
  const page = await notePanel();
  editNote(page, "Still typing");
  const lookups = page.sent("get_artifact").length;
  page.policyChanged(); // The worker invalidated its lease; the host revision is unchanged.
  await until(() => page.sent("get_artifact").length > lookups, "the refresh to finish");
  await settle();
  assert.equal(page.sent("load_note").length, 1);
  assert.equal(page.el("note-body").value, "Still typing");
  assert.equal(page.sent("save_note").length, 0, "an unchanged refresh must not flush the debounce early");
});

test("closing the built panel before debounce restores copy-only text and supports explicit Copy and Discard", async () => {
  const first = await notePanel();
  editNote(first, "pending <img src=x> text");
  assert.equal(first.state.localStorage.length, 1, "recovery is synchronous before pagehide");
  first.close();
  const reopened = await notePanel({ localStorage: first.state.localStorage });
  assert.equal(reopened.el("note-conflict").hidden, false);
  assert.equal(reopened.el("note-unsaved").value, "pending <img src=x> text");
  assert.equal(reopened.el("note-body").value, "");
  assert.equal(reopened.sent("save_note").length, 0, "no automatic replay");
  reopened.el("copy-unsaved").click();
  assert.deepEqual(reopened.state.copied, ["pending <img src=x> text"]);
  reopened.el("discard-unsaved").click();
  assert.equal(reopened.el("note-conflict").hidden, true);
  assert.equal(reopened.state.localStorage.length, 0);
});

test("closing during a built-panel save preserves newer typing after the old host save completes", async () => {
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const first = await notePanel({ intercept: (request) => request.type === "save_note" ? held : undefined });
  editNote(first, "first save");
  await until(() => first.sent("save_note").length === 1, "the first autosave to start");
  editNote(first, "first save plus newer typing");
  first.close();
  release({
    type: "note_saved", revision: "r2", key: { kind: "url", value: RECOVERY_TAB.url },
    outcome: "created", relative_path: "notes/a.md",
  });
  const reopened = await notePanel({
    localStorage: first.state.localStorage,
    intercept: (request) => request.type === "load_note" ? {
      type: "note_loaded", exists: true, revision: "r2", title: "A", body: "first save",
      key: { kind: "url", value: RECOVERY_TAB.url }, older_notes: [],
    } : undefined,
  });
  assert.equal(reopened.el("note-body").value, "first save");
  assert.equal(reopened.el("note-unsaved").value, "first save plus newer typing");
  assert.equal(reopened.sent("save_note").length, 0);
});

test("a built-panel conflict including an empty deletion survives reopen and later confirmed edits", async () => {
  const first = await notePanel({ intercept: (request) => request.type === "save_note" ? {
    type: "note_conflict", revision: "other-r2", body: "another window's note", exists: true, title: "A",
    key: { kind: "url", value: RECOVERY_TAB.url }, reason: "stale",
  } : undefined });
  editNote(first, "");
  await until(() => !first.el("note-conflict").hidden, "the conflict copy to show");
  assert.equal(first.el("note-unsaved").value, "");
  first.close();
  const reopened = await notePanel({ localStorage: first.state.localStorage });
  assert.equal(reopened.el("note-conflict").hidden, false);
  assert.equal(reopened.el("note-unsaved").value, "");
  editNote(reopened, "a fresh edit");
  await until(() => reopened.el("note-status").textContent === "Saved", "a confirmed new save");
  assert.equal(reopened.state.localStorage.length, 1, "the conflict stays until explicit discard");
  reopened.el("discard-unsaved").click();
  assert.equal(reopened.state.localStorage.length, 0);
});

test("the built panel warns about recovery quota without preventing native autosave", async () => {
  const localStorage = memoryStorage();
  localStorage.setItem = () => { throw new Error("quota full"); };
  const page = await notePanel({ localStorage });
  editNote(page, "save through host");
  assert.equal(page.el("note-recovery-warning").hidden, false);
  assert.match(page.el("note-recovery-warning").textContent, /quota full/);
  await until(() => page.el("note-status").textContent === "Saved", "the native save");
  assert.equal(page.sent("save_note")[0].body, "save through host");
  assert.equal(page.el("note-recovery-warning").hidden, false);
});

test("private tabs do not offer Capture Site and unknown origins never get the starter flow", async () => {
  const page = await install({ tab: { id: 1, url: JIRA_URL, incognito: true } });
  await until(() => page.calls("tabs.query") > 0, "tab query");
  assert.equal(page.el("capture-site-area").hidden, true);
  const unknown = await install({ tab: { id: 1, url: "https://unknown.example/items/1" } });
  await until(() => !unknown.el("capture-site-area").hidden, "the drafted flow to be offered");
  assert.equal(unknown.el("capture-site-github").hidden, true);
  assert.equal(unknown.el("capture-site-review").hidden, true);
});

const WIKI = "https://wiki.example.com";

const WIKI_URL = `${WIKI}/pages/42`;

const LINEAR_URL = "https://linear.app/acme/issue/ABC-12";

const draftRule = (index, example, extra = {}) => ({
  rule: index, checked: true, unchecked_reason: null, visited: 1, links: 0, example, ...extra,
});

function wikiDraft(revision = "1") {
  return {
    type: "site_drafted", revision, drafter: "structural",
    adapters: [
      {
        adapter: {
          type: "wiki.pages", origin: WIKI,
          match: [{ path: "/pages/{num:int}" }, { path: "/archive/{num:int}" }],
          id: "wiki.pages:{host}/{num}",
        },
        scope: "not_shared", tenant: null,
        rules: [
          draftRule(0, `${WIKI}/pages/123`, { visited: 3, links: 2 }),
          draftRule(1, `${WIKI}/archive/123`, { checked: false, unchecked_reason: "link_only", visited: 0, links: 4 }),
        ],
      },
      {
        adapter: { type: "wiki.space", origin: WIKI, match: [{ path: "/space/{id:token}" }], id: "wiki.space:{host}/{id}" },
        scope: "not_shared", tenant: null,
        rules: [draftRule(0, `${WIKI}/space/a1b2`, { checked: false, unchecked_reason: "token_only" })],
      },
    ],
    dropped: [
      { shape: "/search?q={slug}", adapter: null, rule: null, reason: "search_or_paging" },
      { shape: "/share/{id}", adapter: null, rule: null, reason: "blocked_route" },
      { shape: "/dashboard", adapter: null, rule: null, reason: "probe" },
      { shape: "/{slug}/{tailhex32}", adapter: null, rule: null, reason: "no_route_word" },
      { shape: null, adapter: 0, rule: 2, reason: "draft_overlap" },
    ],
    preview: [
      { source: "visited", shape: "/pages/{num}", url: `${WIKI}/pages/123`, adapter: 0, rule: 0, artifact_id: "wiki.pages:wiki.example.com/123" },
      { source: "visited", shape: "/settings", url: `${WIKI}/settings`, adapter: null, rule: null, artifact_id: null },
      { source: "link", shape: "/archive/{num}", url: `${WIKI}/archive/123`, adapter: 0, rule: 1, artifact_id: "wiki.pages:wiki.example.com/123" },
      { source: "link", shape: "/space/{id}", url: `${WIKI}/space/a1b2`, adapter: 1, rule: 0, artifact_id: "wiki.space:wiki.example.com/a1b2" },
      { source: "link", shape: "/search?q={slug}", url: `${WIKI}/search?q=example`, adapter: null, rule: null, artifact_id: null },
      { source: "probe", shape: "/login", url: `${WIKI}/login`, adapter: null, rule: null, artifact_id: null },
      { source: "probe", shape: "/share", url: `${WIKI}/share`, adapter: null, rule: null, artifact_id: null },
    ],
    taken_types: [], remapped: 0,
  };
}

function notionDraft(revision = "1") {
  return {
    type: "site_drafted", revision, drafter: "structural",
    adapters: [{
      adapter: {
        type: "notion.page", origin: "https://www.notion.so",
        match: [{ path: "/{workspace:name|lower}/{hex:hex(32)@tail}" }],
        id: "notion.page:{host}/{workspace}/{hex}",
      },
      scope: "pending", tenant: { segment: 0, capture: "workspace" },
      rules: [draftRule(0, "https://www.notion.so/example/0123456789abcdef0123456789abcdef")],
    }],
    dropped: [], preview: [], taken_types: [], remapped: 0,
  };
}

function linearDraft(revision = "1") {
  return {
    type: "site_drafted", revision, drafter: "structural",
    adapters: [{
      adapter: {
        type: "linear.issue", origin: "https://linear.app",
        match: [{ path: "/{workspace:name|lower}/issue/{key:jira_key}" }],
        id: "linear.issue:{host}/{workspace}/{key}",
      },
      scope: "pending", tenant: { segment: 0, capture: "workspace" },
      rules: [draftRule(0, "https://linear.app/example/issue/ABC-123")],
    }],
    dropped: [],
    preview: [{ source: "visited", shape: "/{slug}/issue/{key}", url: "https://linear.app/example/issue/ABC-123", adapter: 0, rule: 0, artifact_id: "linear.issue:linear.app/example/ABC-123" }],
    taken_types: [], remapped: 0,
  };
}

const ruleBox = (page, adapter, rule) => descendants(page.el("capture-site-draft"))
  .find((node) => node.tagName === "INPUT" && node.dataset.rule === `${adapter}/${rule}`);
const typeField = (page, adapter) => descendants(page.el("capture-site-draft"))
  .find((node) => node.tagName === "INPUT" && node.dataset.type === String(adapter));
const draftText = (page) => ["capture-site-draft", "capture-site-preview", "capture-site-dropped", "capture-site-message"]
  .map((id) => page.el(id).textContent).join("\n");
// The host requests sent from `from` on, less the panel's own background
// reads of its tab and settings (artifact lookups follow startup delivery).
const BACKGROUND = new Set(["get_config", "get_artifact", "load_note", "list_runs"]);
const sentAfter = (page, from) => [...new Set(page.state.requests.slice(from).map((request) => request.type))]
  .filter((type) => !BACKGROUND.has(type));

// Opens Capture Site on an unknown site. `draft` answers draft_site (a
// function of the request and state, or a reply); the host is otherwise the
// default fake.
async function draftReview(url = WIKI_URL, options = {}) {
  const { draft = () => wikiDraft(), title = "Work page", intercept, script, ...rest } = options;
  const page = await install({
    tab: { id: 1, url, title },
    script: script ?? (async () => ({ current: "/pages/{num}", links: [{ shape: "/archive/{num}", count: 4 }, { shape: "/space/{id}", count: 1 }] })),
    intercept: (request, state) => request.type === "draft_site"
      ? (typeof draft === "function" ? draft(request, state) : draft)
      : intercept?.(request, state),
    ...rest,
  });
  await until(() => !page.el("capture-site").disabled && !page.el("capture-site-area").hidden, "Capture Site to be ready");
  await until(() => !page.el("note-body").disabled, "the note to load");
  await until(() => !page.el("capture-site").disabled, "startup delivery to finish");
  page.from = page.state.requests.length;
  page.el("capture-site").click();
  assert.equal(page.el("capture-site-review").hidden, false);
  return page;
}

const drafted = (page) => until(() => page.el("capture-site-draft").children.length > 0 ||
  page.el("capture-site-advanced").hidden === false, "the drafted review");

test("Capture Site on an unknown site drafts with no permission request or native dialog", async () => {
  let confirms = 0;
  const page = await draftReview();
  globalThis.window.confirm = () => { confirms += 1; return false; };
  await drafted(page);
  assert.equal(page.calls("permissions.request"), 0);
  assert.equal(page.calls("permissions.remove"), 0);
  assert.equal(confirms, 0);
  assert.deepEqual(sentAfter(page, page.from), ["draft_site"]);
  const [request] = page.sent("draft_site");
  assert.equal(request.origin, WIKI);
  assert.deepEqual(request.visited, [{ shape: "/pages/{num}", count: 1 }]);
  assert.deepEqual(request.links, [{ shape: "/archive/{num}", count: 4 }, { shape: "/space/{id}", count: 1 }]);
  // The origin has no adapter, so the worker has no sample to read.
  assert.equal(page.state.workerRequests.filter((message) => message.kind === "get_site_patterns").length, 0);
  assert.equal(page.el("capture-site-heading").textContent, "Capture work on wiki.example.com");
  assert.equal(page.el("capture-site-github").hidden, true);
  assert.equal(updates(page).length, 0);
});

test("link-only and token-only rules start unchecked with their labels", async () => {
  const page = await draftReview();
  await drafted(page);
  assert.equal(ruleBox(page, 0, 0).checked, true);
  assert.equal(ruleBox(page, 0, 1).checked, false);
  assert.equal(ruleBox(page, 1, 0).checked, false);
  const rows = descendants(page.el("capture-site-draft")).filter((node) => node.tagName === "LABEL");
  const labelOf = (adapter, rule) => rows.find((row) => row.children.includes(ruleBox(page, adapter, rule))).textContent;
  assert.match(labelOf(0, 1), /seen only in links on this page/);
  assert.match(labelOf(1, 0), /loose: its ID has no fixed shape/);
  assert.doesNotMatch(labelOf(0, 0), /seen only in links|loose/);
  assert.equal(typeField(page, 0).value, "wiki.pages");
});

test("the review lists what will and will not be captured with the real current page", async () => {
  const page = await draftReview();
  await drafted(page);
  const text = () => draftText(page);
  assert.match(text(), /Brauser will remember: wiki\.pages — pages like \/pages\/<number>/);
  assert.match(text(), /Illustrative example: https:\/\/wiki\.example\.com\/pages\/123/);
  assert.match(page.el("capture-site-preview").textContent, /This page: will be captured — https:\/\/wiki\.example\.com\/pages\/42/);
  assert.match(page.el("capture-site-preview").textContent, /Matches 1 of 2 visited patterns and 0 of 3 link patterns/);
  assert.match(page.el("capture-site-preview").textContent, /Illustrative example \(captured\): https:\/\/wiki\.example\.com\/pages\/123/);
  assert.doesNotMatch(text(), /"match"|"id"|\{num:int\}/);
  assert.equal(page.el("capture-site-save").disabled, false);
  // Opting into the link-only rule counts its link pattern.
  ruleBox(page, 0, 1).checked = true;
  ruleBox(page, 0, 1).dispatch("input");
  assert.match(page.el("capture-site-preview").textContent, /Matches 1 of 2 visited patterns and 1 of 3 link patterns/);
  // With this page's rule dropped, the review says so.
  ruleBox(page, 0, 0).checked = false;
  ruleBox(page, 0, 0).dispatch("input");
  assert.match(page.el("capture-site-preview").textContent, /This page: will not be captured/);
  // Nothing checked leaves nothing to save.
  ruleBox(page, 0, 1).checked = false;
  ruleBox(page, 0, 1).dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.match(page.el("capture-site-message").textContent, /Choose at least one/);
  // A rename that collides with the other kind is refused in words.
  ruleBox(page, 0, 0).checked = true;
  ruleBox(page, 0, 0).dispatch("input");
  ruleBox(page, 1, 0).checked = true;
  ruleBox(page, 1, 0).dispatch("input");
  typeField(page, 0).value = "wiki.space";
  typeField(page, 0).dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.match(page.el("capture-site-message").textContent, /both named "wiki\.space"/);
  typeField(page, 0).value = "wiki.doc";
  typeField(page, 0).dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, false);
  assert.match(text(), /Brauser will remember: wiki\.doc/);
  assert.equal(updates(page).length, 0);
});

test("dropped rules and safety probes are listed in plain words", async () => {
  const page = await draftReview();
  await drafted(page);
  const dropped = page.el("capture-site-dropped").textContent;
  assert.match(dropped, /Brauser won't capture/);
  assert.match(dropped, /\/search\?q=\{slug\}: search or paging pages/);
  assert.match(dropped, /\/share\/\{id\}: sign-in, sharing or account pages/);
  assert.match(dropped, /\/dashboard: .*dashboard/);
  assert.match(dropped, /no fixed route word/);
  assert.match(dropped, /already captured/);
  assert.match(dropped, /Safety checks: 2 sign-in, search, dashboard, sharing and other pages skipped/);
  assert.match(dropped, /Skipped: https:\/\/wiki\.example\.com\/login/);
  assert.doesNotMatch(dropped, /draft_|search_or_paging|blocked_route|no_route_word/);
});

test("canceling, closing or switching tabs discards the draft and its link shapes and saves nothing", async () => {
  const cleared = (page) => {
    assert.equal(page.el("capture-site-review").hidden, true);
    assert.equal(page.el("capture-site-draft").children.length, 0);
    assert.equal(page.el("capture-site-dropped").children.length, 0);
    assert.equal(page.el("capture-site-preview").children.length, 0);
    assert.doesNotMatch(draftText(page), /archive|space/);
  };
  const page = await draftReview();
  await drafted(page);
  page.el("capture-site-cancel").click();
  cleared(page);
  page.el("capture-site-save").click();
  // Opening again drafts afresh.
  page.el("capture-site").click();
  await drafted(page);
  assert.equal(page.sent("draft_site").length, 2);
  // A reload of the reviewed tab (same URL) discards the review.
  page.tabUpdated({ status: "loading" });
  cleared(page);
  page.el("capture-site").click();
  await drafted(page);
  page.navigate("https://other.example/pages/1", 2);
  await until(() => page.el("capture-site-review").hidden, "a tab switch to discard the draft");
  cleared(page);
  assert.equal(updates(page).length, 0);

  // A draft that answers after Cancel is dropped.
  let finish;
  const held = new Promise((resolve) => { finish = resolve; });
  const second = await draftReview(WIKI_URL, { draft: () => held.then(() => wikiDraft()) });
  await until(() => second.sent("draft_site").length === 1, "the held draft");
  second.el("capture-site-cancel").click();
  finish();
  await new Promise((resolve) => setTimeout(resolve, 10));
  cleared(second);
  assert.equal(updates(second).length, 0);
});

test("a page that navigated before links were read cancels the draft without calling draft_site", async () => {
  // The tab moved on while the link shapes were being read.
  const moved = await draftReview(WIKI_URL, { script: async (_injection, state) => {
    state.tab.url = `${WIKI}/pages/43`;
    return { current: "/pages/{num}", links: [{ shape: "/stale/{num}", count: 1 }] };
  } });
  await until(() => moved.el("capture-site-review").hidden, "the draft to cancel");
  assert.equal(moved.sent("draft_site").length, 0);
  // A reload while the read is held.
  let finish;
  const held = new Promise((resolve) => { finish = resolve; });
  const reloaded = await draftReview(WIKI_URL, { script: () => held });
  await until(() => reloaded.state.scripts.length === 1, "the held link read");
  reloaded.tabUpdated({ status: "loading" });
  finish({ current: "/pages/{num}", links: [{ shape: "/stale/{num}", count: 1 }] });
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(reloaded.el("capture-site-review").hidden, true);
  assert.equal(reloaded.sent("draft_site").length, 0);
  // The inspector itself found another document there.
  const stale = await draftReview(WIKI_URL, { script: async () => ({ current: null, links: [] }) });
  await until(() => stale.el("capture-site-message").textContent.startsWith("Could not draft"), "the honest error");
  assert.equal(stale.sent("draft_site").length, 0);
  assert.equal(stale.el("capture-site-save").disabled, true);
});

// Step 7.6's shared-origin flow, as John decided on 2026-10-27: the tenant
// is pre-filled from this tab's URL and shown as "Capture for <tenant>", so
// one click on Save confirms it; it stays editable, and the every-account
// statement stays the alternative. This replaced "scope pending, Enable
// disabled until the user chooses" (DESIGN §5.1, §12.2 step 7.6).
test("a shared origin pre-fills this tab's tenant as Capture for <tenant> and saves with one click", async () => {
  const page = await draftReview(LINEAR_URL, { draft: () => linearDraft(),
    script: async () => ({ current: "/{slug}/issue/{key}", links: [] }) });
  await drafted(page);
  assert.equal(page.el("capture-site-scope").hidden, false);
  assert.equal(page.el("capture-site-scope-values").value, "acme", "prefilled from this tab's URL");
  assert.equal(page.el("capture-site-scoped").checked, true, "the pre-filled tenant is already chosen");
  assert.equal(page.el("capture-site-scoped-text").textContent, "Capture for acme");
  assert.match(page.el("capture-site-scope-intro").textContent, /filled in acme from this page/);
  assert.equal(page.el("capture-site-unscoped").checked, false);
  assert.equal(page.el("capture-site-scope-values").disabled, false, "the tenant stays editable");
  assert.equal(page.el("capture-site-save").disabled, false, "Save is the one-click confirmation");
  assert.match(page.el("capture-site-preview").textContent, /This page: will be captured/);
  assert.match(page.el("capture-site-preview").textContent, /Records pages only for: acme\./);
  assert.match(page.el("capture-site-scope").textContent, /every account in this Chrome profile/);
  assert.match(page.el("capture-site-scope").textContent, /separate Chrome profile for work/);
  // The prefill never came from the host: the draft's example names a stand-in.
  assert.doesNotMatch(JSON.stringify(page.sent("draft_site")), /acme/);
  // Editing the list changes what is confirmed.
  page.el("capture-site-scope-values").value = "acme, partner";
  page.el("capture-site-scope-values").dispatch("input");
  assert.equal(page.el("capture-site-scoped-text").textContent, "Capture for acme and partner");
  assert.equal(page.el("capture-site-save").disabled, false);
  page.el("capture-site-scope-values").value = "";
  page.el("capture-site-scope-values").dispatch("input");
  assert.equal(page.el("capture-site-scoped-text").textContent, "Only these workspaces or accounts");
  assert.equal(page.el("capture-site-save").disabled, true);
  page.el("capture-site-unscoped").checked = true;
  page.el("capture-site-unscoped").dispatch("input");
  assert.equal(page.el("capture-site-scoped").checked, false, "the two choices exclude each other");
  assert.equal(page.el("capture-site-scope-values").disabled, true);
  assert.equal(page.el("capture-site-save").disabled, false);
  page.el("capture-site-unscoped").checked = false;
  page.el("capture-site-unscoped").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(updates(page).length, 0, "nothing is saved before Save");

  // A tab whose URL names no tenant at that position pre-fills nothing, so
  // the user still chooses before Save.
  const invite = await draftReview("https://linear.app/invite/abc123", { draft: () => linearDraft(),
    script: async () => ({ current: "/invite/{id}", links: [] }) });
  await drafted(invite);
  assert.equal(invite.el("capture-site-scope-values").value, "");
  assert.equal(invite.el("capture-site-scoped").checked, false);
  assert.equal(invite.el("capture-site-scoped-text").textContent, "Only these workspaces or accounts");
  assert.equal(invite.el("capture-site-scope-intro").textContent,
    "This site holds many workspaces and accounts. Choose which ones Brauser remembers.");
  assert.equal(invite.el("capture-site-save").disabled, true);
  assert.match(invite.el("capture-site-message").textContent, /workspaces or accounts/);

  // An app route at the tenant position is no tenant either: no drafted
  // rule records linear.app/settings/account, so nothing is pre-filled.
  const settings = await draftReview("https://linear.app/settings/account", { draft: () => linearDraft(),
    script: async () => ({ current: "/{slug}/{slug}", links: [] }) });
  await drafted(settings);
  assert.equal(settings.el("capture-site-scope-values").value, "");
  assert.equal(settings.el("capture-site-scoped").checked, false);
  assert.doesNotMatch(settings.el("capture-site-scope-intro").textContent, /filled in/);
  assert.equal(settings.el("capture-site-save").disabled, true);

  // A workspace-less Notion page: its title slug is no workspace.
  const hex = "0123456789abcdef0123456789abcdef";
  const notion = await draftReview(`https://www.notion.so/Meeting-Notes-${hex}`, { draft: () => notionDraft(),
    script: async () => ({ current: "/{tailhex32}", links: [] }) });
  await drafted(notion);
  assert.equal(notion.el("capture-site-scope-values").value, "");
  assert.equal(notion.el("capture-site-scoped").checked, false);
  assert.doesNotMatch(notion.el("capture-site-scope-intro").textContent, /filled in/);
  assert.equal(notion.el("capture-site-save").disabled, true);

  // A not-shared origin shows no scope controls.
  const wiki = await draftReview();
  await drafted(wiki);
  assert.equal(wiki.el("capture-site-scope").hidden, true);
  // An origin with no tenant position offers only the unscoped statement.
  const docsUrl = "https://docs.google.com/document/d/a1b2c3d4e5f6a7b8c9d0e1/edit";
  const docs = await draftReview(docsUrl, { draft: () => ({
    ...linearDraft(),
    adapters: [{
      adapter: { type: "docs.document", origin: "https://docs.google.com", match: [{ path: "/document/d/{doc:token}/edit" }], id: "docs.document:{host}/{doc}" },
      scope: "unscoped_only", tenant: null,
      rules: [draftRule(0, "https://docs.google.com/document/d/a1b2/edit")],
    }],
  }), script: async () => ({ current: "/document/d/{id}/edit", links: [] }) });
  await drafted(docs);
  assert.equal(docs.el("capture-site-scope").hidden, false);
  assert.equal(docs.el("capture-site-scope-values").hidden, true);
  assert.equal(docs.el("capture-site-scoped").hidden, true);
  assert.equal(docs.el("capture-site-save").disabled, true);
  docs.el("capture-site-unscoped").checked = true;
  docs.el("capture-site-unscoped").dispatch("input");
  assert.equal(docs.el("capture-site-save").disabled, false);
});

test("an empty draft explains why and offers advanced settings", async () => {
  const page = await draftReview(WIKI_URL, { draft: () => ({
    ...wikiDraft(), adapters: [], preview: [],
    dropped: [{ shape: "/{slug}/{tailhex32}", adapter: null, rule: null, reason: "draft_no_leading_literal" }],
  }) });
  await drafted(page);
  assert.equal(page.el("capture-site-advanced").hidden, false);
  assert.match(page.el("capture-site-message").textContent, /could not draft/i);
  assert.match(page.el("capture-site-dropped").textContent, /no fixed route word/);
  assert.equal(page.el("capture-site-save").disabled, true);
  page.el("capture-site-advanced").click();
  await until(() => page.state.created.length === 1, "advanced settings to open");
  const [key] = [...page.state.session.keys()].filter((name) => name.startsWith("captureSiteDraft:"));
  assert.equal(page.state.created[0].url, `chrome-extension://test-id/options.html#draft=${key.slice("captureSiteDraft:".length)}`);
  assert.deepEqual(page.state.session.get(key).adapters, []);
  assert.equal(page.el("capture-site-review").hidden, true, "the handed-off draft leaves the panel");
  assert.equal(page.el("capture-site-advanced").hidden, true);
  assert.equal(updates(page).length, 0);
});

test("the advanced handoff stores only the drafted adapters and shapes for 30 minutes", async () => {
  const { DRAFT_HANDOFF_TTL_MS, canonicalJson } = await import("../dist/capture-site-handoff.js");
  const title = "Secret Launch Plan · Acme Wiki";
  const page = await draftReview(WIKI_URL, { title });
  await drafted(page);
  assert.equal(page.el("capture-site-advanced").hidden, false, "a drafted review also offers the advanced editor");
  page.el("capture-site-title-suffix").value = " · Acme Wiki";
  page.el("capture-site-title-suffix").dispatch("input");
  const before = Date.now();
  page.el("capture-site-advanced").click();
  await until(() => page.state.created.length === 1, "advanced settings to open");
  const keys = [...page.state.session.keys()].filter((name) => name.startsWith("captureSiteDraft:"));
  assert.equal(keys.length, 1);
  const entry = page.state.session.get(keys[0]);
  assert.equal(`captureSiteDraft:${entry.id}`, keys[0]);
  assert.equal(page.state.created[0].url, `chrome-extension://test-id/options.html#draft=${entry.id}`);
  assert.equal(entry.origin, WIKI);
  assert.ok(entry.created >= before && entry.created <= Date.now());
  assert.ok(DRAFT_HANDOFF_TTL_MS === 30 * 60_000);
  // The reviewed selection (rule 1 starts unchecked) without the title overlay.
  const table = wikiDraft().adapters[0].adapter;
  const kept = { ...table, match: [table.match[0]] };
  assert.deepEqual(entry.adapters, [kept]);
  assert.deepEqual(entry.baseline_rules, [kept.match.map(canonicalJson)]);
  assert.deepEqual(entry.visited, [{ shape: "/pages/{num}", count: 1 }]);
  assert.deepEqual(entry.links, [{ shape: "/archive/{num}", count: 4 }, { shape: "/space/{id}", count: 1 }]);
  const text = JSON.stringify(entry);
  assert.equal(text.includes(WIKI_URL), false, "no raw URL");
  assert.equal(text.includes("Secret Launch Plan"), false, "no tab title");
  assert.equal(text.includes("Acme Wiki"), false, "no title ending");
  assert.equal(page.el("capture-site-review").hidden, true);
  assert.equal(updates(page).length, 0);
  assert.deepEqual(sentAfter(page, page.from), ["draft_site"]);
});

test("the advanced handoff with no valid plan keeps only checked rules", async () => {
  const { canonicalJson } = await import("../dist/capture-site-handoff.js");
  // The pre-filled workspace cleared, so the review has no plan; one rule
  // is link-only and one adapter is token-only, both unchecked by the host.
  const linear = linearDraft();
  linear.adapters[0].adapter.match.push({ path: "/{workspace:name|lower}/project/{key:jira_key}" });
  linear.adapters[0].rules.push(draftRule(1, "https://linear.app/example/project/ABC-123",
    { checked: false, unchecked_reason: "link_only", visited: 0, links: 3 }));
  linear.adapters.push({
    adapter: { type: "linear.doc", origin: "https://linear.app", match: [{ path: "/{workspace:name|lower}/document/{id:token}" }], id: "linear.doc:{host}/{workspace}/{id}" },
    scope: "pending", tenant: { segment: 0, capture: "workspace" },
    rules: [draftRule(0, "https://linear.app/example/document/a1b2", { checked: false, unchecked_reason: "token_only" })],
  });
  const page = await draftReview(LINEAR_URL, { draft: () => linear,
    script: async () => ({ current: "/{slug}/issue/{key}", links: [] }) });
  await drafted(page);
  page.el("capture-site-scope-values").value = "";
  page.el("capture-site-scope-values").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true, "no workspace is chosen");
  page.el("capture-site-advanced").click();
  await until(() => page.state.created.length === 1, "advanced settings to open");
  const [key] = [...page.state.session.keys()].filter((name) => name.startsWith("captureSiteDraft:"));
  const entry = page.state.session.get(key);
  const issue = { ...linear.adapters[0].adapter, match: [linear.adapters[0].adapter.match[0]] };
  assert.deepEqual(entry.adapters, [issue]);
  assert.deepEqual(entry.baseline_rules, [issue.match.map(canonicalJson)]);
  assert.doesNotMatch(JSON.stringify(entry), /project|document/);
});

test("with every rule unchecked the advanced handoff holds no drafted rule", async () => {
  const page = await draftReview();
  await drafted(page);
  ruleBox(page, 0, 0).checked = false;
  ruleBox(page, 0, 0).dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
  page.el("capture-site-advanced").click();
  await until(() => page.state.created.length === 1, "advanced settings to open");
  const [key] = [...page.state.session.keys()].filter((name) => name.startsWith("captureSiteDraft:"));
  const entry = page.state.session.get(key);
  assert.deepEqual(entry.adapters, []);
  assert.deepEqual(entry.baseline_rules, []);
});

test("the tab title is shown only in the review and never sent to the drafter", async () => {
  const title = "Secret Launch Plan · Acme Wiki";
  const page = await draftReview(WIKI_URL, { title });
  await drafted(page);
  assert.equal(page.el("capture-site-title").hidden, false);
  assert.match(page.el("capture-site-title-current").textContent, /Secret Launch Plan · Acme Wiki/);
  page.el("capture-site-title-suffix").value = " · Acme Wiki";
  page.el("capture-site-title-suffix").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, false);
  page.el("capture-site-title-suffix").value = " · Other";
  page.el("capture-site-title-suffix").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.match(page.el("capture-site-message").textContent, /does not end with/);
  // Only the origin and shapes cross: no raw URL, link href or tab title.
  const [request] = page.sent("draft_site");
  assert.deepEqual(Object.keys(request).sort(), ["links", "origin", "protocol_version", "request_id", "type", "visited"]);
  assert.equal(request.origin, WIKI);
  const shapes = JSON.stringify([request.visited, request.links]);
  for (const secret of ["Secret", "Acme Wiki", "http", "42", "wiki.example"]) assert.doesNotMatch(shapes, new RegExp(secret));
  assert.equal(page.sent("draft_site").length, 1);
  assert.deepEqual(sentAfter(page, page.from), ["draft_site"]);
});

test("an older host without draft_site asks for a host rebuild", async () => {
  const page = await draftReview(WIKI_URL, { draft: () => ({
    type: "error", code: "invalid_request", message: "invalid or unsupported request JSON",
  }) });
  await until(() => /rebuild/i.test(page.el("capture-site-message").textContent), "the rebuild message");
  assert.match(page.el("capture-site-message").textContent, /Rebuild and reinstall the Brauser host/);
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(page.el("capture-site-draft").children.length, 0);
  // Any other refusal is shown as it is.
  const other = await draftReview(WIKI_URL, { draft: () => ({
    type: "error", code: "invalid_config", message: "adapter policy is unavailable",
  }) });
  await until(() => other.el("capture-site-message").textContent.includes("adapter policy is unavailable"), "the refusal");
  assert.doesNotMatch(other.el("capture-site-message").textContent, /rebuild/i);
});

test("a draft from another revision is stale before it can be saved", async () => {
  const page = await draftReview(WIKI_URL, { draft: () => wikiDraft("2") });
  await until(() => page.el("capture-site-message").textContent.includes("Settings changed"), "the stale draft");
  assert.equal(page.el("capture-site-save").disabled, true);
});

test("Page patterns review still works through the shared collector", async () => {
  const page = await draftReview();
  await drafted(page);
  page.el("site-pattern-review").click();
  await until(() => page.el("site-pattern-rows").textContent.includes("Link pattern"), "pattern results");
  assert.equal(page.state.scripts.length, 2);
  for (const injection of page.state.scripts) {
    assert.deepEqual(injection.target, { tabId: 1, frameIds: [0] });
    assert.equal(injection.world, "ISOLATED");
    assert.deepEqual(injection.args, [WIKI_URL, true]);
  }
  assert.equal(page.state.scripts[0].func, page.state.scripts[1].func);
  assert.match(page.el("site-pattern-rows").textContent, /Current page: \/pages\/\{num\}/);
  // Each review keeps its own state: closing one leaves the other.
  page.el("site-pattern-close").click();
  assert.equal(page.el("capture-site-review").hidden, false);
  assert.ok(page.el("capture-site-draft").children.length > 0);
});

// A host-like check_site_draft: a stale revision is a conflict, a table that
// carries an overlay with drafted rules is refused as draft_forbidden_key,
// and a pending scope needs a choice.
function siteChecked(request, state, views = []) {
  if (request.expected_revision !== state.revision) {
    return { type: "error", code: "conflict", message: "configuration changed; draft the site again" };
  }
  const OVERLAYS = ["title", "scope", "unscoped", "alias", "fields", "path_case"];
  return {
    type: "site_draft_checked", revision: state.revision,
    rules: request.adapters.map((table, a) => request.provenance[a].map(() => {
      const forbidden = OVERLAYS.some((key) => Object.hasOwn(table, key));
      return forbidden ? { verdict: "dropped", reason: "draft_forbidden_key" } : { verdict: "kept", reason: null };
    })),
    scope: request.scope.map((choice, a) => {
      const scopeState = views[a] ?? "not_shared";
      const ok = scopeState === "pending" ? choice !== null
        : scopeState === "unscoped_only" ? choice === "unscoped" : choice === null;
      return {
        state: scopeState,
        resolved: !ok ? null : choice === "unscoped" ? "unscoped" : choice ? "scoped" : "not_shared",
        reason: ok ? null : scopeState === "pending" ? "draft_scope_pending" : "draft_scope_choice",
      };
    }),
    preview: [],
  };
}

const checks = (page) => page.sent("check_site_draft");

const indexOf = (page, type) => page.state.requests.findIndex((request) => request.type === type);

// A drafted review whose host also answers check_site_draft (`check`).
async function draftedSave(url = WIKI_URL, options = {}) {
  const { check, scopes, intercept, ...rest } = options;
  const page = await draftReview(url, {
    ...rest,
    intercept: (request, state) => request.type === "check_site_draft"
      ? (check ? check(request, state) : siteChecked(request, state, scopes))
      : intercept?.(request, state),
  });
  await drafted(page);
  return page;
}

test("saving a draft unchecked down to one rule saves exactly that rule", async () => {
  const page = await draftedSave();
  assert.equal(ruleBox(page, 0, 1).checked, false);
  assert.equal(ruleBox(page, 1, 0).checked, false);
  await saveCapture(page);
  assert.equal(checks(page).length, 1);
  const [check] = checks(page);
  const [draftRequest] = page.sent("draft_site");
  assert.equal(check.origin, WIKI);
  assert.equal(check.expected_revision, "1");
  assert.deepEqual(check.adapters, [{
    type: "wiki.pages", origin: WIKI, match: [{ path: "/pages/{num:int}" }], id: "wiki.pages:{host}/{num}",
  }]);
  assert.deepEqual(check.provenance, [["drafted"]]);
  assert.deepEqual(check.scope, [null]);
  assert.deepEqual(check.visited, draftRequest.visited);
  assert.deepEqual(check.links, draftRequest.links);
  assert.ok(indexOf(page, "check_site_draft") < indexOf(page, "update_config"));
  assert.equal(updates(page).length, 1);
  const [update] = updates(page);
  assert.equal(update.expected_revision, "1");
  assert.deepEqual(update.config.adapters.artifacts, check.adapters);
  assert.equal(update.config.capture_enabled, true);
  assert.equal(page.calls("permissions.request"), 0);
  assert.match(page.el("status").textContent, /Saved capture rules for wiki\.example\.com\. Capture is on\./);
});

test("a revision change between draft and save refuses without rebasing", async () => {
  // Another settings page saved after the draft was reviewed.
  const page = await draftedSave();
  page.state.config.strip_params = ["tracking"];
  page.state.revision = "2";
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("Settings changed"), "stale config refusal");
  await until(() => page.el("capture-site-message").textContent.includes("Settings changed"), "stale review marker");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(checks(page).length, 0);
  assert.equal(updates(page).length, 0);
  assert.deepEqual(page.state.config.strip_params, ["tracking"]);

  // Another save lands while the selection is being re-checked: the check
  // passed for revision 1, and update_config is refused, never rebased.
  const raced = await draftedSave(WIKI_URL, { check(request, state) {
    const reply = siteChecked(request, state);
    state.config = { ...state.config, strip_params: ["raced"] };
    state.revision = "2";
    return reply;
  } });
  raced.el("capture-site-save").click();
  await until(() => raced.el("status").textContent.includes("Could not capture site"), "conflict at commit");
  assert.equal(updates(raced).length, 1);
  assert.deepEqual(raced.state.config.strip_params, ["raced"]);
  assert.equal(raced.state.config.adapters.artifacts.length, 0);

  // A check answered for another revision is refused before any save.
  const other = await draftedSave(WIKI_URL, { check: (request, state) => ({ ...siteChecked(request, state), revision: "9" }) });
  other.el("capture-site-save").click();
  await until(() => other.el("status").textContent.includes("Settings changed"), "stale check refusal");
  assert.equal(updates(other).length, 0);
});

test("a check_site_draft refusal at save time saves nothing", async () => {
  // A rule that now overlaps a rule saved meanwhile is dropped by the host.
  const page = await draftedSave(WIKI_URL, { check: (request, state) => ({
    ...siteChecked(request, state),
    rules: [[{ verdict: "dropped", reason: "draft_overlap" }]],
  }) });
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("Could not capture site"), "check refusal");
  assert.match(page.el("status").textContent, /already captured by this site's rules/);
  assert.doesNotMatch(page.el("status").textContent, /draft_overlap/);
  assert.equal(checks(page).length, 1);
  assert.equal(updates(page).length, 0);
  assert.equal(page.state.requests.filter((request) => request.type === "save_note").length, 0);
  assert.equal(page.el("capture-site-review").hidden, false);

  // A refused scope choice.
  const scoped = await draftedSave(LINEAR_URL, { draft: () => linearDraft(),
    script: async () => ({ current: "/{slug}/issue/{key}", links: [] }),
    check: (request, state) => ({
      ...siteChecked(request, state, ["pending"]),
      scope: [{ state: "pending", resolved: null, reason: "draft_scope_choice" }],
    }) });
  scoped.el("capture-site-scoped").checked = true;
  scoped.el("capture-site-scoped").dispatch("input");
  scoped.el("capture-site-save").click();
  await until(() => scoped.el("status").textContent.includes("Could not capture site"), "scope refusal");
  assert.equal(updates(scoped).length, 0);

  // A host error, or a reply that does not answer every checked rule.
  for (const check of [
    () => ({ type: "error", code: "invalid_request", message: "checked adapters are invalid or over their bounds" }),
    (request, state) => ({ ...siteChecked(request, state), rules: [[]] }),
  ]) {
    const refused = await draftedSave(WIKI_URL, { check });
    refused.el("capture-site-save").click();
    await until(() => refused.el("status").textContent.includes("Could not capture site"), "refusal");
    assert.equal(updates(refused).length, 0);
  }
});

test("saving a pending-scope shared-origin draft with chosen scope succeeds", async () => {
  const page = await draftedSave(LINEAR_URL, { draft: () => linearDraft(), scopes: ["pending"],
    script: async () => ({ current: "/{slug}/issue/{key}", links: [] }) });
  // One click: the tab's tenant is already chosen.
  assert.equal(page.el("capture-site-scoped").checked, true);
  assert.equal(page.el("capture-site-scope-values").value, "acme");
  await saveCapture(page);
  const [check] = checks(page);
  assert.deepEqual(check.adapters, [linearDraft().adapters[0].adapter]);
  assert.equal("scope" in check.adapters[0], false);
  assert.deepEqual(check.scope, [{ values: ["acme"] }]);
  const saved = updates(page)[0].config.adapters.artifacts;
  assert.equal(saved.length, 1);
  assert.deepEqual(saved[0].scope, { workspace: ["acme"] });
  assert.equal(saved[0].unscoped, undefined);
});

test("saving an unscoped-only origin draft with the unscoped statement succeeds", async () => {
  const docsUrl = "https://docs.google.com/document/d/a1b2c3d4e5f6a7b8c9d0e1/edit";
  const table = { type: "docs.document", origin: "https://docs.google.com", match: [{ path: "/document/d/{doc:token}/edit" }], id: "docs.document:{host}/{doc}" };
  const page = await draftedSave(docsUrl, { scopes: ["unscoped_only"], draft: () => ({
    ...linearDraft(),
    adapters: [{ adapter: table, scope: "unscoped_only", tenant: null, rules: [draftRule(0, "https://docs.google.com/document/d/a1b2/edit")] }],
    preview: [],
  }), script: async () => ({ current: "/document/d/{id}/edit", links: [] }) });
  page.el("capture-site-unscoped").checked = true;
  page.el("capture-site-unscoped").dispatch("input");
  await saveCapture(page);
  const [check] = checks(page);
  assert.deepEqual(check.adapters, [table]);
  assert.deepEqual(check.scope, ["unscoped"]);
  const saved = updates(page)[0].config.adapters.artifacts;
  assert.deepEqual(saved, [{ ...table, unscoped: "acknowledged" }]);
});

test("a copied scope is not echoed into check_site_draft and still saves", async () => {
  // The panel offers drafting on configured origins only from Task 13, so
  // this drives the same check-then-save sequence the commit runs.
  const { starterOffers } = await import("../dist/starter-adapters.js");
  const { planDraftedSite } = await import("../dist/capture-site.js");
  const { checkDraftedSave } = await import("../dist/capture-site-panel.js");
  const { ConfigSession } = await import("../dist/settings.js");
  const GITHUB = "https://github.com";
  // Pull request and issue rules only: a repository rule's catch-all would
  // also cover the drafted route, which the host drops as an overlap.
  const existing = starterOffers(GITHUB, ["acme-corp"])
    .filter((offer) => offer.artifact_type !== "github.repo")
    .map(({ adapter: { within: _within, ...adapter } }) => adapter);
  const config = { ...CONFIG, adapters: { format: 1, artifacts: existing } };
  const table = { type: "github.issues", origin: GITHUB, match: [{ path: "/{owner:name|lower}/issues/{num:int}" }], id: "github.issues:{host}/{owner}/{num}" };
  const draft = {
    ...linearDraft(),
    adapters: [{ adapter: table, scope: "copied", tenant: { segment: 0, capture: "owner" }, rules: [draftRule(0, `${GITHUB}/example/issues/123`)] }],
    preview: [],
  };
  const url = `${GITHUB}/acme-corp/issues/7`;
  const page = await install({ tab: { id: 1, url }, config,
    intercept: (request, state) => request.type === "check_site_draft" ? siteChecked(request, state, ["copied"]) : null });
  await until(() => !page.el("note-body").disabled, "the note to load");
  const result = planDraftedSite(config, draft, { checked: [[true]], types: [null], scope: [null], titleSuffix: "" }, url, "");
  assert.equal(result.ok, true, result.message);
  const plan = result.plan;
  const session = new ConfigSession(() => undefined);
  session.config = structuredClone(config);
  session.revision = "1";
  await checkDraftedSave(session, plan, "1", { origin: GITHUB, visited: [{ shape: "/{slug}/issues/{num}", count: 1 }], links: [] });
  const [check] = page.sent("check_site_draft");
  assert.deepEqual(check.scope, [null]);
  assert.deepEqual(check.adapters, [table]);
  assert.equal(JSON.stringify(check).includes("acme-corp"), false);
  assert.equal(await session.saveConfig({ ...config, adapters: plan.document, capture_enabled: plan.startsCapture }, null), true);
  const saved = updates(page)[0].config.adapters.artifacts;
  assert.deepEqual(saved.slice(0, 2), existing);
  assert.deepEqual(saved[2], { ...table, scope: { owner: ["acme-corp"] } });
});

test("a title entry does not trip draft_forbidden_key at save", async () => {
  const title = "Secret Launch Plan · Acme Wiki";
  const page = await draftedSave(WIKI_URL, { title });
  page.el("capture-site-title-suffix").value = " · Acme Wiki";
  page.el("capture-site-title-suffix").dispatch("input");
  await saveCapture(page);
  const [check] = checks(page);
  assert.equal("title" in check.adapters[0], false);
  for (const secret of ["Secret", "Acme Wiki", "Launch"]) assert.doesNotMatch(JSON.stringify(check), new RegExp(secret));
  assert.equal(updates(page)[0].config.adapters.artifacts[0].title, "{title} · Acme Wiki");
});

test("a saved drafted site starts capture and reloads the note under its host-derived key", async () => {
  const key = { kind: "artifact", value: "wiki.pages:wiki.example.com/42" };
  const page = await draftedSave(WIKI_URL, { intercept(request, state) {
    if (request.type === "load_note" && state.revision === "2") return { ...hostReply(request), key, body: "artifact note" };
  } });
  page.el("note-body").value = "Keep this text";
  page.el("note-body").dispatch("input");
  await saveCapture(page);
  const requests = page.state.requests;
  const checked = indexOf(page, "check_site_draft");
  const saved = indexOf(page, "save_note");
  const updated = indexOf(page, "update_config");
  // The selection is re-checked, then old note edits finish, then the save.
  assert.ok(checked >= 0 && checked < saved && saved < updated);
  assert.equal(requests[saved].body, "Keep this text");
  assert.equal(updates(page)[0].config.capture_enabled, true);
  const installed = page.state.workerRequests.filter((request) => request.kind === "install_policy").at(-1);
  assert.equal(installed.lease.revision, "2");
  assert.deepEqual(installed.lease.adapters, updates(page)[0].config.adapters);
  assert.ok(requests.slice(updated + 1).some((request) => request.type === "load_note"));
  await until(() => page.el("note-body").value === "artifact note", "the artifact note");
});

test("a pending pause or removal blocks the drafted save", async () => {
  for (const workerStatus of [{ pause_pending: true, pause_token: "pause" }, { pending_removals: [WIKI] }]) {
    const page = await draftedSave(WIKI_URL, { workerStatus });
    assert.equal(page.el("capture-site-save").disabled, true);
    assert.match(page.el("capture-site-message").textContent, /pause|removal/i);
  }
  for (const change of [
    (status) => { status.pause_pending = true; },
    (status) => { status.pending_removals = [WIKI]; },
  ]) {
    const page = await draftedSave();
    change(page.state.workerStatus);
    page.el("capture-site-save").click();
    await until(() => page.el("status").textContent.includes("pending"), "the pending refusal");
    assert.equal(checks(page).length, 0);
    assert.equal(updates(page).length, 0);
  }
});

const WIKI_PAGES = { type: "wiki.pages", origin: WIKI, match: [{ path: "/pages/{num:int}" }], id: "wiki.pages:{host}/{num}" };

const NOTES_URL = `${WIKI}/notes/42`;

const sampleRow = (shape, count) => ({ shape, count, last_seen: Date.now() - 1_000 });

const notesDraft = (revision = "1") => ({
  type: "site_drafted", revision, drafter: "structural",
  adapters: [{
    adapter: { type: "wiki.notes", origin: WIKI, match: [{ path: "/notes/{num:int}" }], id: "wiki.notes:{host}/{num}" },
    scope: "not_shared", tenant: null,
    rules: [draftRule(0, `${WIKI}/notes/123`, { visited: 3 })],
  }],
  dropped: [{ shape: "/pages/{num}", adapter: null, rule: null, reason: "already_recognized" }],
  preview: [
    { source: "visited", shape: "/notes/{num}", url: `${WIKI}/notes/123456`, adapter: 0, rule: 0, artifact_id: "wiki.notes:wiki.example.com/123456" },
    { source: "visited", shape: "/pages/{num}", url: `${WIKI}/pages/123456`, adapter: null, rule: null, artifact_id: null },
  ],
  taken_types: ["wiki.pages"], remapped: 0,
});

// A configured origin whose worker sample holds `patterns`. Resolves once
// the startup reads settle; the hint may or may not be shown.
async function configuredPage(url = NOTES_URL, options = {}) {
  const { artifacts = [WIKI_PAGES], draft = () => notesDraft(), intercept, current = "/notes/{num}", ...rest } = options;
  const page = await install({
    tab: { id: 1, url, title: "Work page" },
    config: { ...CONFIG, capture_enabled: true, adapters: { format: 1, artifacts } },
    script: async () => ({ current, links: [{ shape: "/notes/{num}", count: 2 }] }),
    intercept: (request, state) => request.type === "draft_site"
      ? (typeof draft === "function" ? draft(request, state) : draft)
      : intercept?.(request, state),
    ...rest,
  });
  await until(() => !page.el("note-body").disabled, "the note to load");
  await until(() => page.state.workerRequests.some((message) => message.kind === "get_site_patterns"), "the sample read");
  await settle();
  return page;
}

async function recurringReview(url = NOTES_URL, options = {}) {
  const page = await configuredPage(url, options);
  await until(() => !page.el("capture-site-recurring").hidden && !page.el("capture-site-recurring").disabled, "the recurring hint");
  page.from = page.state.requests.length;
  page.el("capture-site-recurring").click();
  assert.equal(page.el("capture-site-review").hidden, false);
  await drafted(page);
  return page;
}

test("the recurring-pages hint shows only after three unrecognized visits and sends nothing to the host", async () => {
  const two = await configuredPage(NOTES_URL, { patterns: [sampleRow("/notes/{num}", 2), sampleRow("/pages/{num}", 9)] });
  assert.equal(two.el("capture-site-area").hidden, true);
  assert.equal(two.el("capture-site-recurring").hidden, true);
  const recognized = await configuredPage(`${WIKI}/pages/42`, { current: "/pages/{num}", patterns: [sampleRow("/notes/{num}", 9)] });
  assert.equal(recognized.el("capture-site-area").hidden, true);
  const page = await configuredPage(NOTES_URL, { patterns: [sampleRow("/notes/{num}", 3)] });
  await until(() => !page.el("capture-site-recurring").hidden, "the recurring hint");
  assert.equal(page.el("capture-site-area").hidden, false);
  assert.equal(page.el("capture-site").hidden, true);
  assert.equal(page.el("capture-site-hint").textContent, "Looks like you have recurring pages on wiki.example.com.");
  assert.equal(page.el("capture-site-recurring").textContent, "Draft rules for these pages");
  for (const shown of [two, recognized, page]) {
    assert.deepEqual(shown.sent("draft_site"), []);
    assert.deepEqual(shown.sent("check_site_draft"), []);
    assert.equal(shown.state.scripts.length, 0, "no page links are read before the click");
  }
});

test("the sample reaches the host only after the user asks", async () => {
  const patterns = [sampleRow("/notes/{num}", 3), sampleRow("/pages/{num}", 5)];
  const page = await configuredPage(NOTES_URL, { patterns });
  await until(() => !page.el("capture-site-recurring").hidden, "the recurring hint");
  assert.deepEqual(page.sent("draft_site"), []);
  const from = page.state.requests.length;
  page.el("capture-site-recurring").click();
  await drafted(page);
  assert.deepEqual(sentAfter(page, from), ["draft_site"]);
  const [request] = page.sent("draft_site");
  assert.equal(request.origin, WIKI);
  // The current page's shape carries its sampled count; the sample follows.
  assert.deepEqual(request.visited, [{ shape: "/notes/{num}", count: 3 }, { shape: "/pages/{num}", count: 5 }]);
  assert.deepEqual(request.links, [{ shape: "/notes/{num}", count: 2 }]);
  assert.doesNotMatch(JSON.stringify({ ...request, origin: null, request_id: null }), /http|Work page|last_seen|42/);
  assert.equal(page.el("capture-site-heading").textContent, "Capture work on wiki.example.com");
  assert.match(draftText(page), /Brauser will remember: wiki\.notes — pages like \/notes\/<number>/);
  assert.match(page.el("capture-site-preview").textContent, /This page: will be captured/);
  // Canceling returns to the hint and saves nothing.
  page.el("capture-site-cancel").click();
  assert.equal(page.el("capture-site-review").hidden, true);
  assert.equal(page.el("capture-site-recurring").hidden, false);
  assert.equal(page.el("capture-site").hidden, true);
  assert.equal(updates(page).length, 0);
});

test("drafting on a configured origin adds adapters and never changes existing ones", async () => {
  const custom = { id: "wiki.pages:{host}/{num}", match: [{ path: "/pages/{num:int}" }], origin: WIKI, type: "wiki.pages", title: "{title} · Wiki" };
  const artifacts = [custom, { type: "tools.item", origin: "https://tools.example.com", match: [{ path: "/items/{id:int}" }], id: "tools.item:{host}/{id}" }];
  const page = await recurringReview(NOTES_URL, {
    artifacts, patterns: [sampleRow("/notes/{num}", 4)],
    intercept: (request, state) => request.type === "check_site_draft" ? siteChecked(request, state) : null,
  });
  assert.equal(page.el("capture-site-save").textContent, "Start capturing this site");
  await saveCapture(page);
  const [check] = checks(page);
  assert.deepEqual(check.adapters, [notesDraft().adapters[0].adapter]);
  const [update] = updates(page);
  assert.equal(update.config.adapters.artifacts.length, 3);
  assert.equal(JSON.stringify(update.config.adapters.artifacts.slice(0, 2)), JSON.stringify(artifacts));
  assert.deepEqual(update.config.adapters.artifacts[2], notesDraft().adapters[0].adapter);
  assert.equal(update.config.capture_enabled, true);
});

test("rules overlapping the origin's existing adapters are listed as already captured", async () => {
  // GitHub issues for one organization only. The sampled issues shape is
  // recognized by that adapter only without its owner scope, so the hint
  // offers it and the host lists it as already captured.
  const issues = starterOffersFor("https://github.com", ["acme-corp"]).find((offer) => offer.artifact_type === "github.issue").adapter;
  const github = "https://github.com";
  const page = await recurringReview(`${github}/acme-corp/widget/discussions/5`, {
    artifacts: [issues], current: "/{slug}/{slug}/discussions/{num}",
    patterns: [sampleRow("/{slug}/{slug}/discussions/{num}", 3), sampleRow("/{slug}/{slug}/issues/{num}", 4)],
    draft: (request) => ({
      type: "site_drafted", revision: "1", drafter: "structural",
      adapters: [{
        adapter: {
          type: "github.discussions", origin: github,
          match: [{ path: "/{owner:name|lower}/{repo:name|lower}/discussions/{num:int}" }],
          id: "github.discussions:{host}/{owner}/{repo}/{num}",
        },
        scope: "copied", tenant: { segment: 0, capture: "owner" },
        rules: [draftRule(0, `${github}/example/repo/discussions/123`, { visited: 3 })],
      }],
      dropped: [
        { shape: request.visited[1].shape, adapter: null, rule: null, reason: "already_recognized" },
        { shape: null, adapter: 0, rule: 1, reason: "draft_overlap" },
      ],
      preview: [], taken_types: ["github.issue"], remapped: 0,
    }),
  });
  const [request] = page.sent("draft_site");
  assert.deepEqual(request.visited.map((row) => row.shape), ["/{slug}/{slug}/discussions/{num}", "/{slug}/{slug}/issues/{num}"]);
  const dropped = page.el("capture-site-dropped").textContent;
  assert.match(dropped, /\/\{slug\}\/\{slug\}\/issues\/\{num\}: already captured by this site's rules/);
  assert.match(dropped, /A drafted github\.discussions rule: already captured by this site's rules/);
  // The copied scope keeps the new kind to the same organization.
  assert.match(page.el("capture-site-preview").textContent, /This page: will be captured/);
  assert.equal(page.el("capture-site-scope").hidden, true);
});

function previewRow(source = "visited") {
  return {
    source, shape: "/issue/{num}", url: "https://work.example/issue/1",
    adapter: 0, rule: 0, artifact_id: "work.issue:work.example/1",
  };
}

function siteDrafted() {
  return {
    type: "site_drafted", protocol_version: PROTOCOL_VERSION, request_id: "r1",
    revision: "sha256:00", drafter: "structural",
    adapters: [{
      adapter: { type: "work.issue", origin: "https://work.example", match: [{ path: "/issue/{num:int}" }], id: "work.issue:{host}/{num}" },
      scope: "pending", tenant: { segment: 0, capture: "org" },
      rules: [{ rule: 0, checked: false, unchecked_reason: "link_only", visited: 0, links: 3, example: "https://work.example/issue/1" }],
    }],
    dropped: [
      { shape: "/search", adapter: null, rule: null, reason: "probe" },
      { shape: null, adapter: 0, rule: 1, reason: "draft_overlap" },
    ],
    preview: [previewRow("visited"), previewRow("link"), { ...previewRow("probe"), adapter: null, rule: null, artifact_id: null }],
    taken_types: ["jira.issue"], remapped: 0,
  };
}

function siteDraftChecked() {
  return {
    type: "site_draft_checked", protocol_version: PROTOCOL_VERSION, request_id: "r2", revision: "sha256:00",
    rules: [[{ verdict: "kept", reason: null }], [{ verdict: "warned", reason: "draft_overlap" }]],
    scope: [
      { state: "pending", resolved: "scoped", reason: null },
      { state: "unscoped_only", resolved: null, reason: "draft_scope_choice" },
    ],
    preview: [previewRow()],
  };
}

test("site_drafted with an extra key or an unbounded list fails the response shape check", () => {
  assert.equal(isResponseShape(siteDrafted()), true);
  assert.equal(isResponseShape(siteDraftChecked()), true);
  const mutations = [
    (draft) => { draft.extra = 1; },
    (draft) => { draft.adapters[0].extra = 1; },
    (draft) => { draft.adapters[0].tenant.extra = 1; },
    (draft) => { draft.adapters[0].rules[0].extra = 1; },
    (draft) => { draft.dropped[0].extra = 1; },
    (draft) => { draft.preview[0].extra = 1; },
    (draft) => { delete draft.remapped; },
    (draft) => { draft.drafter = "agent"; },
    (draft) => { draft.adapters[0].scope = "scoped"; },
    (draft) => { draft.adapters[0].rules[0].unchecked_reason = "planted"; },
    (draft) => { draft.dropped[0].reason = "because"; },
    (draft) => { draft.preview[0].source = "tab"; },
    (draft) => { draft.adapters[0].adapter = []; },
    (draft) => { draft.adapters = Array.from({ length: 9 }, () => siteDrafted().adapters[0]); },
    (draft) => { draft.adapters[0].rules = Array.from({ length: 17 }, () => siteDrafted().adapters[0].rules[0]); },
    (draft) => { draft.dropped = Array.from({ length: 129 }, () => siteDrafted().dropped[0]); },
    (draft) => { draft.preview = Array.from({ length: 129 }, () => previewRow()); },
    (draft) => { draft.taken_types = Array.from({ length: 161 }, (_, n) => `t${n}`); },
    (draft) => { draft.remapped = -1; },
  ];
  for (const mutate of mutations) {
    const draft = siteDrafted();
    mutate(draft);
    assert.equal(isResponseShape(draft), false, mutate.toString());
  }
  const checkedMutations = [
    (checked) => { checked.extra = 1; },
    (checked) => { checked.rules[0][0].extra = 1; },
    (checked) => { checked.rules[0][0].verdict = "maybe"; },
    (checked) => { checked.scope[0].extra = 1; },
    (checked) => { checked.scope[0].state = "copied_maybe"; },
    (checked) => { checked.scope[0].resolved = "pending"; },
    (checked) => { checked.rules = Array.from({ length: 9 }, () => [{ verdict: "kept", reason: null }]); },
    (checked) => { checked.rules[0] = Array.from({ length: 17 }, () => ({ verdict: "kept", reason: null })); },
    (checked) => { checked.scope = Array.from({ length: 9 }, () => siteDraftChecked().scope[0]); },
    (checked) => { checked.preview = Array.from({ length: 129 }, () => previewRow()); },
    (checked) => { delete checked.preview; },
  ];
  for (const mutate of checkedMutations) {
    const checked = siteDraftChecked();
    mutate(checked);
    assert.equal(isResponseShape(checked), false, mutate.toString());
  }
});

// Assigned panel refresh regressions (brwr-09c, brwr-b50, brwr-mm0).
for (const hasAdapters of [false, true]) {
  test(`enabled-capture wording describes adapter sites (${hasAdapters ? "with" : "without"} adapters)`, async () => {
    const adapters = hasAdapters
      ? [starterOffersFor("https://acme.atlassian.net")[0].adapter]
      : [];
    const page = await install({
      tab: { id: 1, url: JIRA_URL, title: "Issue" },
      config: { ...CAPTURING, adapters: { format: 1, artifacts: adapters } },
    });
    await noteLoaded(page);
    await settle();
    assert.doesNotMatch(page.el("status").textContent, /listed sites/i);
    assert.match(page.el("status").textContent, /sites with adapters/i);
    assert.equal(page.el("artifact-status").textContent, "○ Not an artifact",
      "a readable unrecognized page still has an artifact verdict");
  });
}

for (const [name, tab, contains, warning] of [
  ["site-access", { id: 1, title: "Hidden page" }, false, "site-access-warning"],
  ["chrome", { id: 1, url: "chrome://settings/", title: "Settings" }, true, "page-unusable"],
]) {
  test(`an unreadable ${name} tab has no artifact verdict`, async () => {
    const page = await install({
      tab: { id: 1, url: "https://docs.example/a", title: "A" },
      intercept: withStoredWhy,
    });
    await noteLoaded(page);
    await until(() => !page.el("artifact-card").hidden, "the initial artifact card");
    await settle();
    const before = page.sent("get_artifact").length;
    page.state.tab = tab;
    page.state.contains = contains;
    page.tabUpdated({ status: "complete" });
    await until(() => !page.el(warning).hidden, "the unreadable page warning");
    assert.equal(page.el("artifact-status").textContent, "");
    assert.equal(page.el("artifact-card").hidden, true);
    assert.equal(page.sent("get_artifact").length, before);
  });
}

test("note autosave after a host reconnect refreshes the artifact once and preserves context", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" }, config: CAPTURING,
    intercept: (request, state) => request.type === "get_artifact"
      ? recognized({ ...artifact("doc:a", "Doc A", "old reason"), focused_seconds: state.focused ?? 60 })
      : undefined,
  });
  await outageWithDraft(page);
  await until(() => /not connected/.test(page.el("artifact-status").textContent), "the disconnected status");
  const before = page.sent("get_artifact").length;
  page.state.focused = 120;
  page.state.hostDead = false;
  editNote(page, "reconnect by saving my note");
  await until(() => page.el("note-status").textContent === "Saved", "the autosave");
  await until(() => page.el("artifact-status").textContent === "● Artifact recognized", "the recovered artifact status");
  assert.match(page.el("artifact-measures").textContent, /Focused 2 min/);
  assert.equal(page.el("artifact-why").value, "my unsaved draft");
  await settle();
  assert.equal(page.sent("get_artifact").length, before + 1);
  // Successful host calls on an already-connected session are not reconnects.
  editNote(page, "another ordinary autosave");
  await until(() => page.sent("save_note").length === 2 && page.el("note-status").textContent === "Saved", "the second autosave");
  await settle();
  assert.equal(page.sent("get_artifact").length, before + 1);
});

for (const action of [null, "dismiss-notices", "pause-capture", "discard-pending"]) {
  test(`delivery refresh survives ${action ?? "the normal poll"} and unchanged polls send no lookup`, async (t) => {
    const originalInterval = globalThis.setInterval;
    let poll;
    t.mock.method(globalThis, "setInterval", (callback, delay, ...args) => {
      if (delay === 30_000) poll = callback;
      return originalInterval(callback, delay, ...args);
    });
    const page = await install({
      tab: { id: 1, url: "https://docs.example/a", title: "A" }, config: CAPTURING,
      workerStatus: { delivered_batches: 1, rejected_count: 1, queued: 1 },
      intercept: (request, state) => request.type === "get_artifact"
        ? recognized({ ...artifact("doc:a", "Doc A"), focused_seconds: state.focused ?? 60 })
        : undefined,
      workerReply: (request) => request.kind === "get_pending_ids"
        ? { ok: true, value: ["pending-visit"] }
        : undefined,
    });
    await noteLoaded(page);
    await until(() => !page.el("pause-capture").disabled, "startup replay to finish");
    await settle();
    const before = page.sent("get_artifact").length;
    page.state.focused = 180;
    page.state.workerStatus.delivered_batches = 2;
    if (action) {
      window.confirm = () => true;
      assert.equal(page.el(action).disabled, false);
      page.el(action).click();
      await until(() => !page.el("pause-capture").disabled || !page.state.config.capture_enabled,
        "the action to finish");
      await settle();
    }
    assert.equal(typeof poll, "function");
    poll();
    await until(() => /Focused 3 min/.test(page.el("artifact-measures").textContent), "the delivered totals");
    await settle();
    assert.equal(page.sent("get_artifact").length, before + 1);
    assert.equal(page.sent("get_artifact").at(-1).url, "https://docs.example/a");
    poll();
    await settle();
    assert.equal(page.sent("get_artifact").length, before + 1, "unchanged delivery generation does not refresh");
  });
}

test("pending-pause hints never point at Pause capture while the host is unavailable", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" }, config: CAPTURING,
    workerStatus: { pause_pending: true }, intercept: withStoredWhy,
  });
  await noteLoaded(page);
  await settle();
  assert.equal(page.el("pause-capture").disabled, false);
  assert.match(page.el("why-paused-hint").textContent, /Use Pause capture/);
  page.hostExits();
  page.state.hostDead = true;
  page.titleChanged("A (1)");
  await settle();
  page.titleChanged("A (2)");
  await until(() => page.el("pause-capture").disabled, "the host outage");
  for (const id of ["why-paused-hint", "status"]) {
    assert.doesNotMatch(page.el(id).textContent, /Use Pause capture/);
  }
  assert.match(page.el("why-paused-hint").textContent, /host.*unavailable/i);
  typeWhy(page, "keep this draft");
  page.el("save-why").dispatch("click");
  await settle();
  assert.equal(page.sent("record_why").length, 0);
  assert.doesNotMatch(page.el("status").textContent, /Use Pause capture/);
  page.state.hostDead = false;
  page.followLink("https://docs.example/a?view=1", "A");
  await until(() => page.el("pause-capture").disabled === false, "the host to answer again");
  assert.match(page.el("why-paused-hint").textContent, /Use Pause capture/);
});

for (const pendingOrigin of ["https://docs.example", "https://other.example"]) {
  test(`Why saves respect this page's pending removal: ${pendingOrigin}`, async () => {
    const page = await install({
      tab: { id: 1, url: "https://docs.example/a", title: "A" }, config: CAPTURING,
      workerStatus: { pending_removals: [pendingOrigin] }, intercept: withStoredWhy,
    });
    await noteLoaded(page);
    await settle();
    typeWhy(page, "my context draft");
    const blocked = pendingOrigin === "https://docs.example";
    assert.equal(page.el("save-why").disabled, blocked);
    if (blocked) {
      assert.equal(page.el("why-paused-hint").hidden, false);
      assert.match(page.el("why-paused-hint").textContent, /settings/i);
      assert.match(page.el("why-paused-hint").textContent, /remov/i);
    }
    // Exercise the handler guard as well as disabled-button presentation.
    page.el("save-why").dispatch("click");
    await settle();
    assert.equal(page.sent("record_why").length, blocked ? 0 : 1);
    if (blocked) assert.equal(page.el("artifact-why").value, "my context draft");
  });
}
