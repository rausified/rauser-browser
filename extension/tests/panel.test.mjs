import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";

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
    artifacts_dir: "artifacts", summaries_dir: null,
  },
  capture_enabled: false,
  sites: [],
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

async function install({ tab, contains }) {
  const elements = pageElements();
  const state = {
    tab: tab === undefined ? undefined : { incognito: false, ...tab },
    contains,
    created: [],
    counts: new Map(),
    added: [],
    removed: [],
  };
  const count = (name) => state.counts.set(name, (state.counts.get(name) ?? 0) + 1);

  globalThis.document = {
    getElementById: (id) => elements.get(id) ?? null,
    createElement: (tag) => new FakeElement(tag),
  };
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
            setTimeout(() => {
              const reply = hostReply(request);
              for (const listener of messageListeners) {
                listener({ ...reply, protocol_version: PROTOCOL_VERSION, request_id: request.request_id });
              }
            }, 0);
          },
          disconnect() {},
        };
      },
      async sendMessage() {
        return { ok: true, value: statusFor() };
      },
    },
    storage: {
      local: {
        async get() { return {}; },
        async set() {},
        async remove() {},
      },
      onChanged: { addListener() {} },
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
