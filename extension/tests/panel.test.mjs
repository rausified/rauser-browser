import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";

const { POLICY_STORAGE_KEY } = await import("../dist/model.js");
const { PROTOCOL_VERSION } = await import("../dist/native.js");
const { PANEL_NOTICE_KEY } = await import("../dist/model.js");
const { SETUP_WARNING } = await import("../dist/agent.js");
const { storageKey } = await import("../dist/brand.js");
const { EXTRACT_LIMITS } = await import("../dist/extract.js");

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
  setAttribute() {}
  focus() { this.focused = true; }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = children; }
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
    root: "/vault", profile: "neutral", log_dir: "log", pages_dir: "pages", later_dir: "later",
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
      };
    case "load_note":
      return {
        type: "note_loaded", exists: false, revision: "", title: "", body: "",
        key: { kind: "url", value: request.url }, older_notes: [],
      };
    case "get_artifact":
      return { type: "artifact_result", recognized: false, artifact: null };
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
// `executeScript(injection)` answers chrome.scripting.executeScript; `local`
// and `session` are Maps backing chrome.storage, so a test can share them
// between panel loads.
async function install({
  tab, contains = true, config = CONFIG, workerStatus = {}, configIssue = null, adapterIssue = null,
  intercept = () => null, workerReply = () => undefined, agentStatus = NOT_SET_UP,
  executeScript = async () => { throw new Error("Cannot access contents of the page"); },
  local = new Map(), session = new Map(),
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
    created: [],
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
  Object.defineProperty(globalThis, "navigator", { configurable: true, value: {
    locks: { request: async (_name, _options, action) => action() },
  } });
  globalThis.window = {
    addEventListener(type, listener) { if (type === "pagehide") teardown.push(listener); },
    confirm: () => false,
  };
  const tabActivated = listeners();
  const tabUpdated = listeners();
  globalThis.chrome = {
    runtime: {
      id: "test-id",
      lastError: undefined,
      getManifest: () => ({ options_ui: { page: "options.html" } }),
      openOptionsPage: async () => undefined,
      connectNative() {
        const messageListeners = [];
        const disconnectListeners = [];
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
              for (const listener of messageListeners) {
                listener({ ...reply, protocol_version: PROTOCOL_VERSION, request_id: request.request_id });
              }
            }, 0);
          },
          disconnect() {},
        };
        state.ports.push(port);
        return port;
      },
      async sendMessage(request) {
        state.workerRequests.push(structuredClone(request));
        return (await workerReply(request)) ?? { ok: true, value: structuredClone(state.workerStatus) };
      },
      onMessage: state.runtimeMessages,
    },
    storage: {
      local: storageArea(local),
      session: storageArea(session),
      onChanged: listeners(),
    },
    scripting: {
      async executeScript(injection) {
        state.injections.push(injection);
        return executeScript(injection);
      },
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
    created: state.created,
    navigate(url, id = state.tab?.id ?? 1) {
      state.tab = { ...state.tab, url, id, incognito: false };
      for (const listener of tabActivated.registered) listener({ tabId: id, windowId: 1 });
    },
    policyChanged() {
      for (const listener of chrome.storage.onChanged.registered) {
        listener({ [POLICY_STORAGE_KEY]: { newValue: null } }, "local");
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
const updates = (page) => page.state.requests.filter((request) => request.type === "update_config");

async function captureReview(url = JIRA_URL, options = {}) {
  const page = await install({ tab: { id: 1, url, title: "Work page" }, ...options });
  await until(() => !page.el("capture-site").disabled && !page.el("capture-site-area").hidden, "Capture Site to be ready");
  await until(() => !page.el("note-body").disabled, "the note to load");
  // The panel sends pending visits once on startup.
  await until(() => !page.el("capture-site").disabled, "startup delivery to finish");
  page.el("capture-site").click();
  assert.equal(page.el("capture-site-review").hidden, false);
  return page;
}

async function saveCapture(page) {
  assert.equal(page.el("capture-site-save").disabled, false);
  page.el("capture-site-save").click();
  await until(() => page.el("capture-site-review").hidden, "site capture to save");
  await until(() => !page.el("note-body").disabled, "the note to reload after enabling capture");
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
    ["get_artifact", "get_config", "hello", "load_note", "update_config"]);
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

test("GitHub requires separate inline scopes and saves exactly the selected kinds", async () => {
  const page = await captureReview(GITHUB_URL);
  assert.equal(page.el("capture-site-github").hidden, false);
  assert.equal(page.el("capture-site-save").disabled, true);
  page.el("capture-site-pr-orgs").value = "Acme-Corp";
  page.el("capture-site-pr-orgs").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true, "issue scope is a separate choice");
  page.el("capture-site-issue-orgs").value = "other-org";
  page.el("capture-site-issue-orgs").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, false);
  assert.match(page.el("capture-site-preview").textContent, /pull requests: acme-corp/);
  assert.match(page.el("capture-site-preview").textContent, /issues: other-org/);
  page.el("capture-site-issues").checked = false;
  page.el("capture-site-issues").dispatch("input");
  assert.equal(page.el("capture-site-issue-orgs").disabled, true);
  await saveCapture(page);
  assert.equal(updates(page)[0].config.adapters.artifacts.length, 1);
  assert.deepEqual(updates(page)[0].config.adapters.artifacts[0].scope, { owner: ["acme-corp"] });
});

test("canceling and switching tabs clear a review without changing capture", async () => {
  const page = await captureReview(GITHUB_URL);
  page.el("capture-site-pr-orgs").value = "sensitive-org";
  page.el("capture-site-cancel").click();
  assert.equal(page.el("capture-site-review").hidden, true);
  assert.equal(page.el("capture-site-pr-orgs").value, "");
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

test("unknown origins and private tabs do not offer the starter flow", async () => {
  for (const tab of [
    { id: 1, url: "https://unknown.example/items/1" },
    { id: 1, url: JIRA_URL, incognito: true },
  ]) {
    const page = await install({ tab });
    await until(() => page.calls("tabs.query") > 0, "tab query");
    assert.equal(page.el("capture-site-area").hidden, true);
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
  };
}

const recognized = (value) => ({ type: "artifact_result", recognized: true, artifact: value });
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

async function summarizePanel(options = {}) {
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Tab title" }, agentStatus: READY,
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
