// Shared harness for the built side-panel tests in extension/tests/panel/.
// Put a helper here only when more than one feature file uses it.

import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";

const { POLICY_STORAGE_KEY } = await import("../../dist/model.js");
export const { PROTOCOL_VERSION } = await import("../../dist/native.js");
export const { isResponseShape } = await import("../../dist/protocol-shape.js");
export const { PANEL_NOTICE_KEY } = await import("../../dist/model.js");
export const { SETUP_WARNING } = await import("../../dist/agent.js");
export const { NoteRecovery } = await import("../../dist/note-recovery.js");
export const { storageKey } = await import("../../dist/brand.js");
export const { EXTRACT_LIMITS } = await import("../../dist/extract.js");
export const { starterOffers: starterOffersFor } = await import("../../dist/starter-adapters.js");

// Run after build:extension. Each test loads a fresh copy of the side panel
// against a fake DOM built from the shipped panel.html, a scripted native
// host, a stub worker, and a fake Chrome whose site access can be withdrawn
// and restored as the user would in chrome://extensions.
export const PANEL_HTML = readFileSync(new URL("../../dist/panel.html", import.meta.url), "utf8");
const SITE_ACCESS = ["http://*/*", "https://*/*"];

export class FakeElement {
  constructor(tagName, hidden = false) {
    this.tagName = tagName.toUpperCase();
    this.ownText = "";
    this.value = "";
    this.disabled = false;
    this.hidden = hidden;
    this.open = false;
    this.checked = false;
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
    element.checked = /(^|\s)checked(\s|\/|$)/.test(`${before} ${after}`);
    const start = match.index + match[0].length;
    const end = PANEL_HTML.indexOf(`</${tag}>`, start);
    if (end !== -1) {
      element.ownText = PANEL_HTML.slice(start, end).replace(/<[^>]*>/g, "").replace(/\s+/g, " ").trim();
    }
    elements.set(id, element);
  }
  return elements;
}

export function statusFor() {
  return {
    queued: 0, next_retry_at: null, overflow_count: 0, rejected_count: 0, last_error: null,
    retry_error: null, policy_expires_at: null, pause_pending: false,
    pause_token: null, pending_removals: [], live_activity: 0,
    host_error: null,
  };
}

export const CONFIG = {
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
export const READY = { state: "ready", harness_version: "1.0.0", message: null };

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

export function memoryStorage() {
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
export let teardown = [];

test.afterEach(() => {
  for (const listener of teardown) listener();
  teardown = [];
});

export function hostReply(request, config = CONFIG) {
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
    case "get_site_restore":
      return { type: "site_restore_loaded", origin: request.origin, revision: request.expected_revision, adapters: { format: 1, artifacts: [] } };
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
export async function install({
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
      openOptionsPage: async () => { count("openOptionsPage"); },
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
  await import(`../../dist/panel.js?load=${loads}`);
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

export async function until(condition, what) {
  const deadline = Date.now() + 2_000;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error(`Timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 1));
  }
}

// The panel has settled once its first tab refresh rendered the page area.
export const shownTitle = (page) => page.el("page-title").textContent;

// --- Shared helpers from the page patterns tests ---

export const JIRA_URL = "https://acme.atlassian.net/browse/PROJ-42";
export const GITHUB_URL = "https://github.com/acme-corp/widget/pull/42/files";

// --- Shared helpers from the capture site restore tests ---
export const updates = (page) => page.state.requests.filter((request) => request.type === "update_config");

export async function captureReview(url = JIRA_URL, options = {}) {
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

export async function saveCapture(page) {
  await until(() => !page.el("capture-site-save").disabled, "Save after the starter preflight");
  page.el("capture-site-save").click();
  await until(() => page.el("capture-site-review").hidden, "site capture to save");
  await until(() => !page.el("note-body").disabled, "the note to reload after enabling capture");
}

// --- Shared helpers from the artifact card tests ---

export const CAPTURING = { ...CONFIG, capture_enabled: true };

export function artifact(id, title, why = null) {
  return {
    artifact_id: id, artifact_type: "doc", title, url: `https://docs.example/${id}`,
    first_seen: "2026-10-01", last_seen: "2026-10-27", focused_seconds: 60,
    background_seconds: 0, opens: 1, days_opened: 1, last_mode: null, why, linked_count: 0,
    within: null, contained: [], incoming_trails: [],
  };
}

export const recognized = (value) => ({ type: "artifact_result", recognized: true, artifact: value, summary: null });
export const settle = (ms = 50) => new Promise((resolve) => setTimeout(resolve, ms));

export function typeWhy(page, text) {
  page.el("artifact-why").value = text;
  page.el("artifact-why").dispatch("input");
}

export async function noteLoaded(page) {
  await until(() => page.sent("load_note").length > 0 && page.el("note-body").disabled === false,
    "the note to load");
}

export async function outageWithDraft(page) {
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

export const withStoredWhy = (request) => request.type === "get_artifact"
  ? recognized(artifact("doc:a", "Doc A", "old reason"))
  : undefined;

// --- Shared helpers from the summarize tests ---

export const ARTICLE_URL = "https://read.example/article";
export const RUN_VIEW = {
  run_id: "00000000-0000-4000-8000-000000000039", state: "queued", harness: "codex",
  record_key: { kind: "url", value: ARTICLE_URL }, written_record_key: null,
  url: ARTICLE_URL, title: "Readable title", cwd: null,
  created_at: "2026-10-27T09:00:00-07:00", started_at: null, finished_at: null,
  error: null, error_kind: null, thread_id: null, session_id: null, summary_path: null,
  stream_bytes: 0, can_retry_save: false, answer: null,
};
export const WARNING_SEEN_KEY = storageKey("summarize_warning_seen_v1");

// A page Readability can read: the bundled library loads, then the extractor
// returns the page.
export async function readablePage(injection) {
  if (injection.files) return [{ frameId: 0 }];
  return [{
    frameId: 0,
    result: {
      title: "Readable title", content: "Body text", selection: "picked", truncated: false, readable: true,
      href: ARTICLE_URL,
    },
  }];
}

export function startsRun(request) {
  if (request.type === "start_run") return { type: "run_started", run: RUN_VIEW, existing: false };
  if (request.type === "get_run") return { type: "run_fetched", run: RUN_VIEW, stream_text: "", next_offset: 0, more: false };
  return null;
}

export function buttonIn(root, text) {
  for (const child of root.children) {
    if (child.tagName === "BUTTON" && child.textContent === text) return child;
    const found = buttonIn(child, text);
    if (found) return found;
  }
  return null;
}

// A harness set up in settings, as config_result reports it.
export const SET_UP = {
  ...CONFIG,
  agent: { harness: "codex", binary: "/usr/local/bin/codex", version: "1.0.0", timeout_secs: 600, probed_at: "2026-10-27T09:00:00-07:00" },
  storage: { ...CONFIG.storage, summaries_dir: "summaries" },
};

export async function summarizePanel(options = {}) {
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Tab title" }, agentStatus: READY, config: SET_UP,
    executeScript: readablePage, intercept: startsRun, ...options,
  });
  await until(() => page.el("note-editor").hidden === false, "the page to show");
  return page;
}

// --- Shared helpers from the page summary tests ---

export function descendants(root) {
  return root.children.flatMap((child) => [child, ...descendants(child)]);
}

// --- Shared helpers from the note editor tests ---
export function editNote(page, text) {
  page.el("note-body").value = text;
  page.el("note-body").dispatch("input");
}

export function recoveryRows(page) {
  return page.el("note-recovery-list").children;
}
export function recoveryBody(row) {
  return row.children.find((child) => child.tagName === "TEXTAREA");
}

// --- Shared helpers from the capture site draft tests ---

export const WIKI = "https://wiki.example.com";

export const WIKI_URL = `${WIKI}/pages/42`;

export const LINEAR_URL = "https://linear.app/acme/issue/ABC-12";

export const draftRule = (index, example, extra = {}) => ({
  rule: index, checked: true, unchecked_reason: null, visited: 1, links: 0, example, ...extra,
});

export function wikiDraft(revision = "1") {
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

export function linearDraft(revision = "1") {
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

export const ruleBox = (page, adapter, rule) => descendants(page.el("capture-site-draft"))
  .find((node) => node.tagName === "INPUT" && node.dataset.rule === `${adapter}/${rule}`);
export const draftText = (page) => ["capture-site-draft", "capture-site-preview", "capture-site-dropped", "capture-site-message"]
  .map((id) => page.el(id).textContent).join("\n");
// The host requests sent from `from` on, less the panel's own background
// reads of its tab and settings (artifact lookups follow startup delivery).
const BACKGROUND = new Set(["get_config", "get_artifact", "load_note", "list_runs"]);
export const sentAfter = (page, from) => [...new Set(page.state.requests.slice(from).map((request) => request.type))]
  .filter((type) => !BACKGROUND.has(type));

// Opens Capture Site on an unknown site. `draft` answers draft_site (a
// function of the request and state, or a reply); the host is otherwise the
// default fake.
export async function draftReview(url = WIKI_URL, options = {}) {
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

export const drafted = (page) => until(() => page.el("capture-site-draft").children.length > 0 ||
  page.el("capture-site-advanced").hidden === false, "the drafted review");

// --- Shared helpers from the capture site draft save tests ---

// A host-like check_site_draft: a stale revision is a conflict, a table that
// carries an overlay with drafted rules is refused as draft_forbidden_key,
// and a pending scope needs a choice.
export function siteChecked(request, state, views = []) {
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

export const checks = (page) => page.sent("check_site_draft");

export const indexOf = (page, type) => page.state.requests.findIndex((request) => request.type === type);

// A drafted review whose host also answers check_site_draft (`check`).
export async function draftedSave(url = WIKI_URL, options = {}) {
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

// --- Shared helpers from the capture site recurring tests ---

const WIKI_PAGES = { type: "wiki.pages", origin: WIKI, match: [{ path: "/pages/{num:int}" }], id: "wiki.pages:{host}/{num}" };

export const NOTES_URL = `${WIKI}/notes/42`;

export const sampleRow = (shape, count) => ({ shape, count, last_seen: Date.now() - 1_000 });

export const notesDraft = (revision = "1") => ({
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
export async function configuredPage(url = NOTES_URL, options = {}) {
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

export async function recurringReview(url = NOTES_URL, options = {}) {
  const page = await configuredPage(url, options);
  await until(() => !page.el("capture-site-recurring").hidden && !page.el("capture-site-recurring").disabled, "the recurring hint");
  page.from = page.state.requests.length;
  page.el("capture-site-recurring").click();
  assert.equal(page.el("capture-site-review").hidden, false);
  await drafted(page);
  return page;
}

export function previewRow(source = "visited") {
  return {
    source, shape: "/issue/{num}", url: "https://work.example/issue/1",
    adapter: 0, rule: 0, artifact_id: "work.issue:work.example/1",
  };
}

export function siteDrafted() {
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
