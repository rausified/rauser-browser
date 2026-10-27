import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";

const { POLICY_STORAGE_KEY } = await import("../dist/model.js");
const { PROTOCOL_VERSION } = await import("../dist/native.js");

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
  adapters: { format: 1, artifacts: [] },
};

let loads = 0;
let teardown = [];

test.afterEach(() => {
  for (const listener of teardown) listener();
  teardown = [];
});

function hostReply(request) {
  switch (request.type) {
    case "hello":
      return { type: "hello_result", host_version: "0.0.0", configured: true, config_issue: null };
    case "get_config":
      return {
        type: "config_result", revision: "1", config: CONFIG, config_issue: null,
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
    default:
      return { type: "error", code: "internal", message: `Unexpected host request ${request.type}` };
  }
}

async function install({ tab, contains = true, config = CONFIG, workerStatus = {}, configIssue = null, adapterIssue = null, intercept = () => null,
  script = async () => ({ current: "/items/{num}", links: [] }), patterns = [], patternsError = null,
}) {
  const elements = pageElements();
  const state = {
    tab: tab === undefined ? undefined : { incognito: false, ...tab },
    contains,
    config: structuredClone(config), revision: "1", configIssue, adapterIssue,
    workerStatus: { ...statusFor(), ...workerStatus }, requests: [], workerRequests: [],
    created: [], scripts: [],
    counts: new Map(),
    added: [],
    removed: [],
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
  const listeners = () => {
    const registered = [];
    return { registered, addListener(listener) { registered.push(listener); } };
  };
  const tabActivated = listeners();
  const tabUpdated = listeners();
  globalThis.chrome = {
    scripting: {
      async executeScript(injection) {
        state.scripts.push(injection);
        return [{ frameId: 0, result: await script(injection, state) }];
      },
    },
    runtime: {
      id: "test-id",
      lastError: undefined,
      getManifest: () => ({ options_ui: { page: "options.html" } }),
      openOptionsPage: async () => undefined,
      connectNative() {
        const messageListeners = [];
        return {
          onMessage: { addListener(listener) { messageListeners.push(listener); } },
          onDisconnect: { addListener() {} },
          postMessage(request) {
            state.requests.push(structuredClone(request));
            setTimeout(async () => {
              let reply = await intercept(request, state);
              if (!reply && request.type === "get_config") {
                reply = { ...hostReply(request), config: state.config, revision: state.revision,
                  config_issue: state.configIssue, adapter_issue: state.adapterIssue };
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
      },
      async sendMessage(request) {
        state.workerRequests.push(structuredClone(request));
        if (request.kind === "get_site_patterns") {
          return patternsError ? { ok: false, error: patternsError } : { ok: true, value: structuredClone(patterns) };
        }
        return { ok: true, value: structuredClone(state.workerStatus) };
      },
    },
    storage: {
      local: {
        async get() { return {}; },
        async set() {},
        async remove() {},
      },
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
    created: state.created,
    navigate(url, id = state.tab?.id ?? 1) {
      state.tab = { ...state.tab, url, id, incognito: false };
      for (const listener of tabActivated.registered) listener({ tabId: id, windowId: 1 });
    },
    tabUpdated(changeInfo, id = state.tab?.id ?? 1) {
      for (const listener of tabUpdated.registered) listener(id, changeInfo, { ...state.tab, id });
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

test("page-pattern review on an unknown site reads links only on click without changing capture", async () => {
  const url = "https://unknown.example/items/123456";
  const page = await install({ tab: { id: 1, url, title: "Private Customer Title" },
    script: async () => ({ current: "/items/{num}", links: [{ shape: "/items/{num}/{slug}", count: 2 }] }),
    patterns: [{ shape: "/documents/{uuid}", count: 3, last_seen: Date.now() }],
  });
  await until(() => !page.el("site-pattern-area").hidden && !page.el("site-pattern-review").disabled, "pattern review to be ready");
  assert.equal(page.state.scripts.length, 0);
  assert.equal(page.el("capture-site-area").hidden, true, "unknown-site capture is still unavailable");
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
  assert.deepEqual([...new Set(page.state.requests.map((r) => r.type))].sort(),
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
