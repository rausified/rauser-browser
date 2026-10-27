import assert from "node:assert/strict";
import test from "node:test";

const { storageKey } = await import("../dist/brand.js");
const { PROTOCOL_VERSION } = await import("../dist/native.js");
const { POLICY_STORAGE_KEY } = await import("../dist/model.js");

// Run after build:extension. Each test loads a fresh copy of the settings page
// against a fake DOM, a scripted native host, a stub worker, and a Chrome
// permission store that fires onAdded/onRemoved as Chrome does.
const NOTE_ORIGINS_KEY = storageKey("note_origins_v1");
const SITE = { origin: "https://a.com", path_prefix: "/" };
const PATTERN = "https://a.com:443/*";
const AGENT_STATUS = { state: "not_set_up", harness_version: null, message: "Set up an AI harness." };

class FakeElement {
  constructor(tagName) {
    this.tagName = tagName.toUpperCase();
    this.textContent = "";
    this.value = "";
    this.disabled = false;
    this.children = [];
    this.dataset = {};
    this.listeners = new Map();
    this.classList = { toggle() {}, add() {}, remove() {} };
  }
  addEventListener(type, listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }
  dispatch(type) {
    for (const listener of this.listeners.get(type) ?? []) listener({ type });
  }
  click() {
    if (!this.disabled) this.dispatch("click");
  }
  setAttribute() {}
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = children; }
  querySelectorAll(selector) {
    const tag = selector.toUpperCase();
    const found = [];
    const visit = (node) => {
      for (const child of node.children ?? []) {
        if (child.tagName === tag) found.push(child);
        visit(child);
      }
    };
    visit(this);
    return found;
  }
}

let env;
let loads = 0;

function statusFor(overrides = {}) {
  return {
    queued: 0, next_retry_at: null, overflow_count: 0, rejected_count: 0, last_error: null,
    retry_error: null, policy_expires_at: null, revoked_origins: [], pause_pending: false,
    pause_token: null, navigation_ready: true, locally_removed_sites: [], ...overrides,
  };
}

function configWith(sites) {
  return {
    storage: {
      root: "/vault", profile: "neutral", log_dir: "log", pages_dir: "pages", later_dir: "later", artifacts_dir: "artifacts",
      summaries_dir: null,
    },
    capture_enabled: sites.length > 0,
    sites,
    strip_params: [],
    agent: null,
    adapters: { format: 1, artifacts: [] },
  };
}

function fire(listeners, permissions) {
  setTimeout(() => { for (const listener of listeners) listener(permissions); }, 0);
}

function install({
  sites = [SITE], adapters = [], grants = [], storage = {}, host = {}, worker = {}, workerStatus = {},
} = {}) {
  const elements = new Map();
  const state = {
    grants: new Set(grants),
    storage: new Map(Object.entries(storage)),
    config: { ...configWith(sites), adapters: { format: 1, artifacts: adapters } },
    revision: 1,
    configIssue: null,
    added: [],
    removed: [],
    storageChanged: [],
    requests: [],
    workerRequests: [],
    elements,
  };
  state.firePolicyChange = () => {
    for (const listener of state.storageChanged) {
      listener({ [POLICY_STORAGE_KEY]: { newValue: {
        revision: String(state.revision), expires_at: Date.now() + 60_000,
        capture_enabled: state.config.capture_enabled, sites: state.config.sites,
        adapters: state.config.adapters,
      } } }, "local");
    }
  };
  for (const [id, tag] of [
    ["status", "div"], ["folder-path", "output"], ["choose-folder", "button"], ["site-url", "input"],
    ["enable-site", "button"], ["pause-capture", "button"], ["sites-list", "ul"],
    ["starter-gallery", "div"], ["adapter-overlaps", "div"], ["adapter-list", "div"], ["new-adapter", "button"],
    ["reset-adapters", "button"], ["refresh-artifacts", "button"],
    ["adapter-editor", "div"], ["adapter-editor-heading", "h3"], ["adapter-json", "textarea"],
    ["adapter-validation", "output"], ["adapter-preview-url", "input"],
    ["adapter-preview-title", "input"], ["adapter-preview", "ul"], ["save-adapter", "button"],
    ["cancel-adapter", "button"],
    ["agent-state", "p"], ["detect-harnesses", "button"], ["harness-offers", "ul"], ["harness-env", "ul"],
    ["summaries-dir", "input"],
    ["setup-harness", "button"], ["remove-harness", "button"],
  ]) {
    elements.set(id, new FakeElement(tag));
  }
  elements.get("adapter-editor").hidden = true;
  elements.get("status").textContent = "Connecting to native host…";

  function hostReply(request) {
    state.requests.push(request);
    const custom = host[request.type]?.(request, state);
    if (custom) return custom;
    switch (request.type) {
      case "hello":
        return { type: "hello_result", host_version: "0.0.0", configured: true, config_issue: null };
      case "get_config":
        return {
          type: "config_result", revision: String(state.revision), config: state.config,
          config_issue: state.configIssue, agent_status: AGENT_STATUS,
          adapter_issue: null, adapter_warnings: [],
        };
      case "choose_folder":
        return { type: "folder_chosen", path: "/vault", picker_token: `picker-${state.connects}` };
      case "confirm_config":
        return { type: "config_confirmed", consent_token: "consent", summary: "summary" };
      case "update_config":
        state.config = request.config;
        state.configIssue = null;
        state.revision += 1;
        return { type: "config_updated", revision: String(state.revision), config: state.config };
      case "refresh_artifacts":
        return { type: "artifacts_refreshed", written: 2, unchanged: 3, conflicts: 1, conflict_paths: ["artifacts/work/item.md"] };
      default:
        throw new Error(`Unexpected host request ${request.type}`);
    }
  }

  const has = ({ permissions = [], origins = [] }) =>
    [...permissions, ...origins].every((entry) => state.grants.has(entry));

  env = state;
  globalThis.document = {
    getElementById: (id) => elements.get(id) ?? null,
    createElement: (tag) => new FakeElement(tag),
  };
  globalThis.window = { addEventListener() {} };
  globalThis.chrome = {
    runtime: {
      lastError: undefined,
      reload() {},
      connectNative() {
        const messageListeners = [];
        const disconnectListeners = [];
        // The test can end this host process as Chrome would report it.
        state.exitHost = () => { for (const listener of disconnectListeners) listener(); };
        state.connects = (state.connects ?? 0) + 1;
        return {
          onMessage: { addListener(listener) { messageListeners.push(listener); } },
          onDisconnect: { addListener(listener) { disconnectListeners.push(listener); } },
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
      async sendMessage(request) {
        state.workerRequests.push(request);
        const custom = worker[request.kind]?.(request, state);
        if (custom !== undefined) return { ok: true, value: custom };
        if (request.kind === "remove_site") {
          return { ok: true, value: statusFor({ ...workerStatus, locally_removed_sites: [request.site] }) };
        }
        return { ok: true, value: statusFor(workerStatus) };
      },
    },
    storage: {
      local: {
        async get(key) {
          const result = {};
          for (const name of Array.isArray(key) ? key : [key]) {
            if (state.storage.has(name)) result[name] = structuredClone(state.storage.get(name));
          }
          return result;
        },
        async set(items) {
          for (const [name, value] of Object.entries(items)) state.storage.set(name, structuredClone(value));
        },
        async remove(keys) {
          for (const name of Array.isArray(keys) ? keys : [keys]) state.storage.delete(name);
        },
      },
      onChanged: { addListener(listener) { state.storageChanged.push(listener); } },
    },
    permissions: {
      async contains(permissions) { return has(permissions); },
      async request({ permissions = [], origins = [] }) {
        const entries = [...permissions, ...origins].filter((entry) => !state.grants.has(entry));
        for (const entry of entries) state.grants.add(entry);
        if (entries.length) fire(state.added, { permissions, origins });
        return true;
      },
      async remove({ permissions = [], origins = [] }) {
        for (const entry of [...permissions, ...origins]) state.grants.delete(entry);
        fire(state.removed, { permissions, origins });
        return true;
      },
      onAdded: { addListener(listener) { state.added.push(listener); } },
      onRemoved: { addListener(listener) { state.removed.push(listener); } },
    },
  };
  return state;
}

async function until(condition, what) {
  const deadline = Date.now() + 2_000;
  while (!condition()) {
    if (Date.now() > deadline) {
      throw new Error(`Timed out waiting for ${what}; status: ${env.elements.get("status").textContent}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 1));
  }
}

const statusText = () => env.elements.get("status").textContent;
const idle = () => !env.elements.get("choose-folder").disabled;
const nodeText = (node) => [node.textContent, ...(node.children ?? []).map(nodeText)].join(" ");

async function openSettings({ connected = true } = {}) {
  loads += 1;
  await import(`../dist/options.js?load=${loads}`);
  await until(() => (idle() || !connected) && !statusText().startsWith("Connecting"),
    "the settings page to load");
}

async function typeSite(url) {
  env.elements.get("site-url").value = url;
  env.elements.get("site-url").dispatch("input");
  await until(() => !env.elements.get("enable-site").disabled, "Enable to become available");
}

async function removeListedSite() {
  const [remove] = env.elements.get("sites-list").querySelectorAll("button");
  remove.click();
  await until(() => statusText().startsWith("Removed ") && idle(), "the removal to finish");
}

test("a canceled re-enable after removing a site rolls back Chrome grants", async () => {
  const state = install({
    grants: ["webNavigation", PATTERN],
    host: {
      confirm_config: (_request, current) => current.cancelNext
        ? { type: "error", code: "cancelled", message: "The user canceled" }
        : null,
    },
  });
  await openSettings();
  await typeSite("https://a.com");

  await removeListedSite();
  assert.equal(state.grants.has(PATTERN), false);
  assert.equal(state.grants.has("webNavigation"), false);

  // The preflight taken before removal said both grants were already held.
  await until(() => !env.elements.get("enable-site").disabled, "Enable after removal");
  state.cancelNext = true;
  env.elements.get("enable-site").click();
  await until(() => statusText().startsWith("Setup failed") && idle(), "the canceled setup");
  await until(() => !state.grants.has(PATTERN) && !state.grants.has("webNavigation"),
    "the new Chrome grants to be rolled back");
});

test("removing a logging site keeps an origin grant the side panel made for notes", async () => {
  const state = install({
    grants: ["webNavigation", PATTERN],
    storage: { [NOTE_ORIGINS_KEY]: [SITE.origin] },
  });
  await openSettings();
  await removeListedSite();
  assert.equal(state.grants.has(PATTERN), true);
  assert.equal(state.grants.has("webNavigation"), false);
  assert.deepEqual(state.storage.get(NOTE_ORIGINS_KEY), [SITE.origin]);
  assert.match(statusText(), /kept for its page notes/);
});

test("removing an origin's final site removes its adapters in the same host update", async () => {
  const adapter = {
    type: "work.item", origin: SITE.origin,
    match: [{ path: "/items/{id:int}" }], id: "work.item:{host}/{id}",
  };
  const state = install({ grants: ["webNavigation", PATTERN], adapters: [adapter] });
  await openSettings();
  await removeListedSite();
  assert.deepEqual(state.config.adapters.artifacts, []);
  assert.match(statusText(), /Also removed 1 adapter for that origin/);
});

test("a notes grant Chrome no longer holds does not protect a later logging grant", async () => {
  const state = install({ sites: [], storage: { [NOTE_ORIGINS_KEY]: [SITE.origin] } });
  await openSettings();
  await typeSite("https://a.com");
  env.elements.get("enable-site").click();
  await until(() => statusText().startsWith("Capture enabled") && idle(), "setup to finish");
  await removeListedSite();
  assert.equal(state.grants.has(PATTERN), false);
  assert.deepEqual(state.storage.get(NOTE_ORIGINS_KEY) ?? [], []);
});

test("a settings load failure after hello is not reported as a missing host", async () => {
  install({
    workerStatus: { revoked_origins: ["https://b.com"] },
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: "1", config: current.config, config_issue: "config.json is unreadable",
        agent_status: AGENT_STATUS, adapter_issue: null, adapter_warnings: [],
      }),
    },
  });
  await openSettings();
  assert.doesNotMatch(statusText(), /Native host unavailable/);
  assert.match(statusText(), /config\.json is unreadable/);
});

test("a Chrome revocation narrows host sites while adapter policy needs repair", async () => {
  const futureAdapters = { format: 2, artifacts: [{ future: true }] };
  const state = install({
    workerStatus: { revoked_origins: [SITE.origin] },
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: String(current.revision),
        config: { ...current.config, adapters: futureAdapters }, config_issue: null,
        agent_status: AGENT_STATUS, adapter_issue: "unsupported adapter format 2",
        adapter_warnings: [],
      }),
    },
  });

  await openSettings();

  assert.deepEqual(state.config.sites, []);
  assert.equal(state.config.capture_enabled, false);
  assert.deepEqual(state.config.adapters, futureAdapters);
  assert.ok(state.requests.some((request) => request.type === "update_config"));
});

test("a stale Chrome revocation cannot reinstall an invalid adapter policy", async () => {
  const state = install({
    workerStatus: { revoked_origins: ["https://already-removed.example"] },
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: String(current.revision),
        config: { ...current.config, adapters: { format: 2, artifacts: [{ future: true }] } },
        config_issue: null, agent_status: AGENT_STATUS,
        adapter_issue: "unsupported adapter format 2", adapter_warnings: [],
      }),
    },
  });

  await openSettings();

  assert.equal(state.requests.some((request) => request.type === "update_config"), false);
  assert.equal(state.workerRequests.some((request) => request.kind === "install_policy"), false);
  assert.ok(state.workerRequests.some((request) => request.kind === "suspend_policy"));
  assert.ok(state.workerRequests.some((request) => request.kind === "ack_revocations"));
});

test("revocation reconciliation does not remove an origin re-enabled while it waits", async () => {
  const futureAdapters = { format: 2, artifacts: [{ future: true }] };
  const state = install({
    sites: [],
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: String(current.revision),
        config: { ...current.config, adapters: futureAdapters }, config_issue: null,
        agent_status: AGENT_STATUS, adapter_issue: "unsupported adapter format 2",
        adapter_warnings: [],
      }),
    },
    worker: {
      get_status: (_request, current) => {
        if (current.revocationObserved) return statusFor();
        current.revocationObserved = true;
        // Another settings page acquired the configuration lock first,
        // re-enabled the origin, installed its revision, and acknowledged the
        // old revocation before this page entered the lock.
        current.config = {
          ...configWith([SITE]),
          adapters: futureAdapters,
        };
        current.revision += 1;
        return statusFor({ revoked_origins: [SITE.origin] });
      },
    },
  });

  await openSettings();

  assert.deepEqual(state.config.sites, [SITE]);
  assert.equal(state.config.capture_enabled, true);
  assert.equal(state.requests.some((request) => request.type === "update_config"), false);
});

test("adapter policy problems and overlap warnings are visible in settings", async () => {
  install({
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: "1", config: current.config, config_issue: null,
        agent_status: AGENT_STATUS, adapter_issue: "unsupported adapter format 2",
        adapter_warnings: [],
      }),
    },
  });
  await openSettings();
  assert.match(statusText(), /unsupported adapter format 2/);
  assert.equal(env.elements.get("pause-capture").disabled, false);

  install({
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: "1", config: current.config, config_issue: null,
        agent_status: AGENT_STATUS, adapter_issue: null,
        adapter_warnings: [{ first_adapter: 0, first_rule: 0, second_adapter: 1, second_rule: 0 }],
      }),
    },
  });
  await openSettings();
  assert.match(statusText(), /1 adapter rule overlap needs review/);
});

test("an unsupported adapter document can be removed without blocking unrelated controls", async () => {
  const state = install({
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: String(current.revision),
        config: current.revision === 1
          ? { ...current.config, adapters: { format: 2, artifacts: [{ future: true }] } }
          : current.config,
        config_issue: null, agent_status: AGENT_STATUS,
        adapter_issue: current.revision === 1 || current.config.adapters.format === 2
          ? "unsupported adapter format 2"
          : null,
        adapter_warnings: [],
      }),
    },
  });
  await openSettings();

  assert.equal(env.elements.get("pause-capture").disabled, false);
  assert.equal(env.elements.get("choose-folder").disabled, false);
  assert.equal(env.elements.get("detect-harnesses").disabled, false);
  assert.equal(env.elements.get("reset-adapters").disabled, false);

  env.elements.get("pause-capture").click();
  await until(() => state.config.capture_enabled === false && idle(), "capture to pause");
  assert.equal(state.config.adapters.format, 2);
  assert.equal(env.elements.get("reset-adapters").disabled, false);

  env.elements.get("reset-adapters").click();

  await until(() => state.config.adapters.format === 1 && idle(), "the adapter reset");
  assert.deepEqual(state.config.adapters, { format: 1, artifacts: [] });
  assert.match(statusText(), /adapter rules removed/);
});

test("adapter workspace previews and saves a new adapter through native confirmation", async () => {
  const saves = [];
  const state = install({
    host: { update_config: (request) => { saves.push(request); return null; } },
  });
  await openSettings();
  assert.equal(env.elements.get("new-adapter").disabled, false);

  env.elements.get("new-adapter").click();
  assert.equal(env.elements.get("adapter-editor").hidden, false);
  assert.match(env.elements.get("adapter-json").value, /"origin": "https:\/\/a\.com"/);
  assert.equal(env.elements.get("save-adapter").disabled, false);
  assert.match(nodeText(env.elements.get("adapter-preview")), /All fixed non-artifact probes stay unmatched/);
  assert.match(nodeText(env.elements.get("adapter-preview")), /work\.item:a\.com\/123456/);

  env.elements.get("save-adapter").click();
  await until(() => state.config.adapters.artifacts.length === 1 && idle(), "the adapter save");
  assert.equal(saves.length, 1);
  assert.equal(saves[0].consent_token, "consent");
  assert.equal(state.config.adapters.artifacts[0].type, "work.item");
  assert.match(statusText(), /adapter added/);
});

test("a Jira starter resolves the enabled origin and enters preview before save", async () => {
  const site = { origin: "https://acme.atlassian.net", path_prefix: "/" };
  const state = install({ sites: [site] });
  await openSettings();
  assert.match(nodeText(env.elements.get("starter-gallery")), /Jira issues/);

  const [review] = env.elements.get("starter-gallery").querySelectorAll("button");
  review.click();
  assert.equal(env.elements.get("adapter-editor").hidden, false);
  const draft = JSON.parse(env.elements.get("adapter-json").value);
  assert.equal(draft.origin, site.origin);
  assert.equal(draft.type, "jira.issue");
  assert.match(nodeText(env.elements.get("adapter-preview")), /jira:acme\.atlassian\.net\/ABC-123/);

  env.elements.get("save-adapter").click();
  await until(() => state.config.adapters.artifacts.length === 1 && idle(), "the Jira starter save");
  assert.equal(state.config.adapters.artifacts[0].type, "jira.issue");
  assert.match(nodeText(env.elements.get("starter-gallery")), /Already configured/);
});

test("GitHub starters stay disabled until an explicit valid organization scope", async () => {
  const site = { origin: "https://github.com", path_prefix: "/" };
  install({ sites: [site] });
  await openSettings();
  const gallery = env.elements.get("starter-gallery");
  assert.match(nodeText(gallery), /GitHub pull requests/);
  const [scope] = gallery.querySelectorAll("input");
  const [pullRequest, issue] = gallery.querySelectorAll("button");
  assert.equal(pullRequest.disabled, true);
  assert.equal(issue.disabled, true);

  scope.value = "Acme-Corp, partner-org";
  scope.dispatch("input");
  assert.equal(pullRequest.disabled, false);
  assert.equal(issue.disabled, false);
  pullRequest.click();
  const draft = JSON.parse(env.elements.get("adapter-json").value);
  assert.deepEqual(draft.scope, { owner: ["acme-corp", "partner-org"] });
  assert.equal(draft.unscoped, undefined);
});

test("a missing starter offer cannot become available when scope changes", async () => {
  const site = { origin: "https://github.com", path_prefix: "/" };
  install({ sites: [site] });
  await openSettings();
  const gallery = env.elements.get("starter-gallery");
  const [scope] = gallery.querySelectorAll("input");
  const [pullRequest] = gallery.querySelectorAll("button");
  pullRequest.dataset.starterKey = "missing-starter";

  scope.value = "acme-corp";
  scope.dispatch("input");

  assert.equal(pullRequest.dataset.available, "false");
  assert.equal(pullRequest.disabled, true);
});

test("adapter workspace names overlaps and highlights a matching safety probe", async () => {
  const search = {
    type: "work.search", origin: "https://a.com",
    match: [{ path: "/search", query: { q: "{id:token}" } }],
    id: "work.search:{host}/{id}", fields: { status: ".status" },
  };
  const issue = {
    type: "work.issue", origin: "https://a.com",
    match: [{ path: "/issues/{id:int}" }], id: "work.issue:{host}/{id}",
  };
  install({
    adapters: [search, issue],
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: "1", config: current.config, config_issue: null,
        agent_status: AGENT_STATUS, adapter_issue: null,
        adapter_warnings: [{ first_adapter: 0, first_rule: 0, second_adapter: 1, second_rule: 0 }],
      }),
    },
  });
  await openSettings();
  assert.match(nodeText(env.elements.get("adapter-overlaps")),
    /work\.search rule 1 may overlap work\.issue rule 1; work\.search wins/);
  assert.match(nodeText(env.elements.get("adapter-list")), /Tier 1 fields \(M1\.5b\): status ← \.status/);

  const [edit] = env.elements.get("adapter-list").querySelectorAll("button");
  edit.click();
  assert.match(nodeText(env.elements.get("adapter-preview")), /fixed non-artifact probe matches/);
  assert.match(nodeText(env.elements.get("adapter-preview")), /https:\/\/a\.com\/search\?q=x — Matches work\.search/);
});

test("adapter workspace edits, reorders, and removes through revisioned saves", async () => {
  const issue = {
    type: "work.issue", origin: "https://a.com",
    match: [{ path: "/issues/{id:int}" }], id: "work.issue:{host}/{id}",
  };
  const search = {
    type: "work.search", origin: "https://a.com",
    match: [{ path: "/search", query: { q: "{id:token}" } }], id: "work.search:{host}/{id}",
  };
  const state = install({ adapters: [issue, search] });
  await openSettings();

  // Each card has Edit, Move up, Move down, Remove.
  env.elements.get("adapter-list").querySelectorAll("button")[2].click();
  await until(() => state.config.adapters.artifacts[0].type === "work.search" && idle(), "the reorder");

  env.elements.get("adapter-list").querySelectorAll("button")[0].click();
  const edited = JSON.parse(env.elements.get("adapter-json").value);
  edited.title = "{title} - Search";
  env.elements.get("adapter-json").value = JSON.stringify(edited, null, 2);
  env.elements.get("adapter-json").dispatch("input");
  assert.equal(env.elements.get("save-adapter").disabled, false);
  env.elements.get("save-adapter").click();
  await until(() => state.config.adapters.artifacts[0].title === "{title} - Search" && idle(), "the edit");

  env.elements.get("adapter-list").querySelectorAll("button")[7].click();
  await until(() => state.config.adapters.artifacts.length === 1 && idle(), "the removal");
  assert.equal(state.config.adapters.artifacts[0].type, "work.search");
  assert.ok(state.revision >= 4);
});

test("an open adapter edit follows its target across an external reorder", async () => {
  const issue = {
    type: "work.issue", origin: "https://a.com",
    match: [{ path: "/issues/{id:int}" }], id: "work.issue:{host}/{id}",
  };
  const search = {
    type: "work.search", origin: "https://a.com",
    match: [{ path: "/search", query: { q: "{id:token}" } }], id: "work.search:{host}/{id}",
  };
  const state = install({ adapters: [issue, search] });
  await openSettings();

  env.elements.get("adapter-list").querySelectorAll("button")[0].click();
  const edited = JSON.parse(env.elements.get("adapter-json").value);
  edited.title = "{title} - Issue";
  env.elements.get("adapter-json").value = JSON.stringify(edited, null, 2);

  state.config = {
    ...state.config,
    adapters: { format: 1, artifacts: [structuredClone(search), structuredClone(issue)] },
  };
  state.revision += 1;
  state.firePolicyChange();
  await until(() => state.requests.filter((request) => request.type === "get_config").length >= 2 && idle(),
    "the external adapter reorder to load");

  env.elements.get("adapter-json").dispatch("input");
  env.elements.get("save-adapter").click();
  await until(() => state.config.adapters.artifacts.some((adapter) => adapter.title) && idle(),
    "the reordered adapter edit to save");
  assert.equal(state.config.adapters.artifacts[0].type, "work.search");
  assert.equal(state.config.adapters.artifacts[0].title, undefined);
  assert.equal(state.config.adapters.artifacts[1].type, "work.issue");
  assert.equal(state.config.adapters.artifacts[1].title, "{title} - Issue");
});

test("an open adapter edit refuses to overwrite an externally changed target", async () => {
  const issue = {
    type: "work.issue", origin: "https://a.com",
    match: [{ path: "/issues/{id:int}" }], id: "work.issue:{host}/{id}",
  };
  const state = install({ adapters: [issue] });
  await openSettings();

  env.elements.get("adapter-list").querySelectorAll("button")[0].click();
  const edited = JSON.parse(env.elements.get("adapter-json").value);
  edited.title = "{title} - Local";
  env.elements.get("adapter-json").value = JSON.stringify(edited, null, 2);

  state.config = {
    ...state.config,
    adapters: { format: 1, artifacts: [{ ...structuredClone(issue), title: "{title} - External" }] },
  };
  state.revision += 1;
  state.firePolicyChange();
  await until(() => state.requests.filter((request) => request.type === "get_config").length >= 2 &&
    /changed on another settings page/.test(env.elements.get("adapter-validation").textContent),
    "the external adapter change to be refused");

  assert.equal(env.elements.get("save-adapter").disabled, true);
  env.elements.get("save-adapter").click();
  assert.equal(state.config.adapters.artifacts[0].title, "{title} - External");
  assert.equal(env.elements.get("adapter-editor").hidden, false);
});

test("a failed hello reports the host as unavailable", async () => {
  install({
    host: { hello: () => ({ type: "error", code: "internal", message: "host broke" }) },
  });
  await openSettings({ connected: false });
  assert.match(statusText(), /^Native host unavailable: host broke\./);
});

test("the panel's recorded notes origins survive until Chrome drops the grant", async () => {
  const state = install({ grants: [PATTERN] });
  const { noteOrigins, pruneNoteOrigins, recordNoteOrigin } = await import("../dist/grants.js");
  await recordNoteOrigin(SITE.origin);
  await recordNoteOrigin(SITE.origin);
  await recordNoteOrigin("https://b.com");
  assert.deepEqual(state.storage.get(NOTE_ORIGINS_KEY), [SITE.origin, "https://b.com"]);
  await pruneNoteOrigins();
  assert.deepEqual([...await noteOrigins()], [SITE.origin]);
});

const FOLDER_ISSUE = "The selected notes folder is unavailable or changed. Choose it again.";

test("a kept config with a folder issue needs a new folder pick, which repairs it", async () => {
  const state = install({ grants: ["webNavigation", PATTERN] });
  state.configIssue = FOLDER_ISSUE;
  await openSettings();
  assert.match(statusText(), /needs repair/);
  const [remove] = env.elements.get("sites-list").querySelectorAll("button");
  assert.equal(remove.disabled, true);
  assert.equal(env.elements.get("pause-capture").disabled, true);
  env.elements.get("site-url").value = "https://b.com";
  env.elements.get("site-url").dispatch("input");
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(env.elements.get("enable-site").disabled, true);

  env.elements.get("choose-folder").click();
  await until(() => state.config.storage?.root === "/vault" && idle(), "the repair to be saved");
  assert.match(statusText(), /^Capture is enabled for the listed sites/);
  assert.deepEqual(state.config.sites, [SITE]);
  await until(() => !env.elements.get("enable-site").disabled, "Enable after the repair");
});

test("choosing a folder saves it at once, leaving sites and capture as they were", async () => {
  const saves = [];
  const state = install({
    sites: [],
    host: { update_config: (request) => { saves.push(request); return null; } },
  });
  state.config = { ...state.config, storage: null };
  await openSettings();
  env.elements.get("choose-folder").click();
  await until(() => state.config.storage?.root === "/vault" && idle(), "the folder to be saved");
  assert.equal(saves.length, 1);
  assert.equal(saves[0].picker_token, "picker-1");
  assert.equal(saves[0].consent_token, "consent");
  assert.equal(state.config.capture_enabled, false);
  assert.deepEqual(state.config.sites, []);
  assert.equal(env.elements.get("folder-path").textContent, "/vault");
  assert.match(statusText(), /^No sites are enabled for capture\. Enable a site to begin\./);
});

test("a canceled repair still rolls back the Chrome grants it added", async () => {
  const state = install({
    grants: ["webNavigation", PATTERN],
    host: {
      confirm_config: () => ({ type: "error", code: "cancelled", message: "The user canceled" }),
    },
  });
  state.configIssue = FOLDER_ISSUE;
  await openSettings();
  env.elements.get("choose-folder").click();
  // The declined repair keeps the selection for the site's own confirmation.
  await until(() => statusText().startsWith("Canceled. /vault was not saved") && idle(), "the folder pick");
  await typeSite("https://b.com");
  env.elements.get("enable-site").click();
  await until(() => statusText().startsWith("Setup failed") && idle(), "the canceled setup");
  await until(() => !state.grants.has("https://b.com:443/*"), "the new origin grant to be rolled back");
  assert.equal(state.grants.has("webNavigation"), true);
  assert.equal(state.grants.has(PATTERN), true);
});

test("a host restart drops a folder selection only the old host knew", async () => {
  const state = install({
    sites: [],
    host: {
      // The first folder save is declined, so the selection stays on the page.
      confirm_config: (_request, current) => current.connects === 1
        ? { type: "error", code: "cancelled", message: "The user canceled" }
        : null,
      update_config: (request, current) =>
        request.picker_token !== null && request.picker_token !== `picker-${current.connects}`
        ? { type: "error", code: "unauthorized", message: "folder selection token is unknown" }
        : null,
    },
  });
  state.config = { ...state.config, storage: null };
  await openSettings();
  env.elements.get("choose-folder").click();
  await until(() => statusText().startsWith("Canceled. /vault was not saved") && idle(), "the folder pick");
  await typeSite("https://a.com");

  state.exitHost();
  assert.match(statusText(), /Choose the folder again/);
  assert.equal(env.elements.get("folder-path").textContent, "None selected");
  assert.equal(env.elements.get("enable-site").disabled, true);

  env.elements.get("choose-folder").click();
  await until(() => state.config.storage?.root === "/vault" && idle(), "the second folder pick to be saved");
  await until(() => !env.elements.get("enable-site").disabled, "Enable after the second pick");
  env.elements.get("enable-site").click();
  await until(() => statusText().startsWith("Capture enabled") && idle(), "setup to finish");
});

test("a host restart drops harness offers only the old host knew", async () => {
  const offer = {
    offer_id: "offer-1", adapter: "claude_code", harness_id: "claude-code",
    binary: "/usr/local/bin/claude", real_path: "/opt/claude/bin/claude", version: "2.1.284",
    args: ["-p", "{prompt}"], env_required: ["HOME", "PATH"], env_optional: [], refusal: null,
  };
  const state = install({
    host: {
      discover_harnesses: (_request, current) =>
        ({ type: "harnesses_discovered", offers: [{ ...offer, offer_id: `offer-${current.connects}` }] }),
    },
  });
  await openSettings();
  env.elements.get("detect-harnesses").click();
  await until(() => !env.elements.get("setup-harness").disabled, "Set up to become available");

  state.exitHost();
  assert.match(statusText(), /detected harnesses were lost\. Detect harnesses again/);
  assert.equal(env.elements.get("setup-harness").disabled, true);

  env.elements.get("detect-harnesses").click();
  await until(() => !env.elements.get("setup-harness").disabled, "Set up after detecting again");
});

test("manual artifact refresh reports unowned path conflicts", async () => {
  const state = install();
  await openSettings();
  env.elements.get("refresh-artifacts").click();
  await until(() => statusText().startsWith("Artifact files updated"), "artifact refresh");
  assert.match(statusText(), /2 written, 3 already current/);
  assert.match(statusText(), /1 unowned path conflict remains/);
  assert.match(statusText(), /artifacts\/work\/item\.md/);
  assert.equal(state.requests.at(-1).type, "refresh_artifacts");
});
