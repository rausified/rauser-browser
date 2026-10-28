import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";

const { PROTOCOL_VERSION } = await import("../dist/native.js");
const { POLICY_STORAGE_KEY } = await import("../dist/model.js");

// Run after build:extension. Each test loads a fresh copy of the settings page
// against a fake DOM, a scripted native host, a stub worker, and a
// Chrome permissions API that fails any use.
const SITE = { origin: "https://a.com", path_prefix: "/" };
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
    retry_error: null, policy_expires_at: null, pause_pending: false,
    pause_token: null, pending_removals: [], ...overrides,
  };
}

// A site is on while it has an adapter, so capture defaults to on with one.
function configWith(adapters, captureEnabled, stripParams = []) {
  return {
    storage: {
      root: "/vault", profile: "neutral", log_dir: "logs", pages_dir: "notes", later_dir: "later", artifacts_dir: "artifacts",
      summaries_dir: "summaries", knowledge_base_dir: null,
    },
    capture_enabled: captureEnabled ?? adapters.length > 0,
    strip_params: stripParams,
    agent: null,
    summary_vocabulary: { document_types: ["report"], categories: ["news"] },
    adapters: { format: 1, artifacts: [] },
  };
}

function install({
  adapters = [], captureEnabled, stripParams = [], storage = {}, host = {}, worker = {}, workerStatus = {},
  session = new Map(), hash = "",
} = {}) {
  const elements = new Map();
  const state = {
    storage: new Map(Object.entries(storage)),
    session,
    config: { ...configWith(adapters, captureEnabled, stripParams), adapters: { format: 1, artifacts: adapters } },
    revision: 1,
    configIssue: null,
    adoptedState: { issue: null, revision: "missing", warnings: [], full: false },
    storageChanged: [],
    requests: [],
    workerRequests: [],
    permissionCalls: [],
    elements,
  };
  state.firePolicyChange = () => {
    for (const listener of state.storageChanged) {
      listener({ [POLICY_STORAGE_KEY]: { newValue: {
        revision: String(state.revision), expires_at: Date.now() + 60_000,
        capture_enabled: state.config.capture_enabled,
        adapters: state.config.adapters,
      } } }, "local");
    }
  };
  for (const [id, tag] of [
    ["status", "div"], ["folder-path", "output"], ["choose-folder", "button"], ["site-origin", "input"],
    ["pause-capture", "button"], ["resume-capture", "button"], ["sites-list", "ul"],
    ["starter-gallery", "div"], ["adapter-overlaps", "div"], ["strip-conflicts", "div"], ["adapter-list", "div"], ["new-adapter", "button"],
    ["reset-adapters", "button"], ["refresh-artifacts", "button"],
    ["adapter-editor", "div"], ["adapter-editor-heading", "h3"], ["adapter-editor-review", "div"], ["adapter-json", "textarea"],
    ["adapter-validation", "output"], ["adapter-preview-url", "input"],
    ["adapter-preview-title", "input"], ["adapter-preview", "ul"], ["save-adapter", "button"],
    ["cancel-adapter", "button"],
    ["harness-warning", "p"], ["harness-state", "p"], ["detect-harnesses", "button"], ["harness-list", "ul"],
    ["harness-timeout", "input"], ["test-harness", "button"], ["remove-harness", "button"],
    ["kb-path", "output"], ["choose-kb", "button"], ["kb-use-notes", "button"],
    ["agent-env-names", "p"], ["reveal-agent-env", "button"],
    ["vocab-types", "textarea"], ["vocab-categories", "textarea"], ["save-vocabulary", "button"],
    ["build-id", "span"], ["host-build", "span"],
    ["adopted-state", "div"], ["adopted-issue", "p"], ["adopted-warnings", "ul"],
    ["forget-adopted", "button"], ["forget-adopted-confirm", "div"],
    ["forget-adopted-yes", "button"], ["forget-adopted-no", "button"],
  ]) {
    elements.set(id, new FakeElement(tag));
  }
  elements.get("adapter-editor").hidden = true;
  for (const id of ["adopted-state", "forget-adopted", "forget-adopted-confirm"]) elements.get(id).hidden = true;
  elements.get("status").textContent = "Connecting to native host…";
  elements.get("build-id").textContent = "abc1234def56";

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
          adapter_issue: null, adapter_warnings: [], adopted_state: state.adoptedState,
        };
      case "forget_adopted_contracts":
        state.adoptedState = { issue: null, revision: "sha256:fresh", warnings: [], full: false };
        return hostReply({ ...request, type: "get_config" });
      case "choose_folder":
        return { type: "folder_chosen", path: "/vault", picker_token: `picker-${state.connects}` };
      case "update_config":
        state.config = request.config;
        state.configIssue = null;
        state.revision += 1;
        return { type: "config_updated", revision: String(state.revision), config: state.config };
      case "get_agent_env":
        return { type: "agent_env_listed", names: [], problem: null };
      case "refresh_artifacts":
        return { type: "artifacts_refreshed", written: 2, unchanged: 3, conflicts: 1, conflict_paths: ["artifacts/work/item.md"] };
      default:
        throw new Error(`Unexpected host request ${request.type}`);
    }
  }

  env = state;
  globalThis.document = {
    getElementById: (id) => elements.get(id) ?? null,
    createElement: (tag) => new FakeElement(tag),
  };
  globalThis.window = { addEventListener() {} };
  globalThis.location = { hash };
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
      session: {
        async get(key) {
          const result = {};
          for (const name of Array.isArray(key) ? key : [key]) {
            if (session.has(name)) result[name] = structuredClone(session.get(name));
          }
          return result;
        },
        async set(items) {
          for (const [name, value] of Object.entries(items)) session.set(name, structuredClone(value));
        },
        async remove(keys) {
          for (const name of Array.isArray(keys) ? keys : [keys]) session.delete(name);
        },
      },
      onChanged: { addListener(listener) { state.storageChanged.push(listener); } },
    },
    permissions: {
      // Brauser's site access is granted at install, so the settings page
      // never asks Chrome about, for, or to give back a permission.
      async contains() {
        state.permissionCalls.push("contains");
        throw new Error("chrome.permissions.contains must not be called");
      },
      async request() {
        state.permissionCalls.push("request");
        throw new Error("chrome.permissions.request must not be called");
      },
      async remove() {
        state.permissionCalls.push("remove");
        throw new Error("chrome.permissions.remove must not be called");
      },
      onAdded: { addListener() { state.permissionCalls.push("onAdded"); } },
      onRemoved: { addListener() { state.permissionCalls.push("onRemoved"); } },
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

function artifactFor(origin, type = "work.item", path = "/items/{id:int}") {
  return {
    type, origin,
    match: [{ path }], id: `${type}:{host}/{id}`,
  };
}

const lastUpdate = () => env.requests.findLast((request) => request.type === "update_config");
// Each site row is its origin followed by a Remove site button.
const siteRows = () => env.elements.get("sites-list").children.map((item) => item.children[0]?.textContent ?? item.textContent);

test("settings inspect and clear local page patterns without saving configuration", async () => {
  const origin = "https://patterns.example";
  let rows = [{ shape: "/items/{num}", count: 3, last_seen: Date.now() }];
  install({ adapters: [artifactFor(origin)], worker: {
    get_site_patterns: (request) => { assert.equal(request.origin, origin); return rows; },
    clear_site_patterns: (request) => { assert.equal(request.origin, origin); rows = []; return rows; },
  } });
  await openSettings();
  const site = env.elements.get("sites-list").children[0];
  const details = site.children.find((child) => child.tagName === "DETAILS");
  assert.ok(details);
  const [show, clear] = details.querySelectorAll("button");
  assert.equal(show.textContent, "Show patterns");
  assert.equal(clear.textContent, "Clear patterns");
  assert.equal(env.workerRequests.some((request) => request.kind === "get_site_patterns"), false, "samples load only on request");
  const requestsBefore = env.requests.length;
  show.click();
  await until(() => !show.disabled, "patterns to load");
  assert.match(details.querySelectorAll("li")[0].textContent, /\/items\/\{num\} · 3 navigations/);
  clear.click();
  await until(() => !clear.disabled, "patterns to clear");
  assert.equal(details.querySelectorAll("li").length, 0);
  assert.ok(details.children.some((child) => child.textContent.startsWith("Patterns cleared.")));
  assert.equal(env.requests.length, requestsBefore, "no native request for reading or clearing samples");
  assert.equal(env.config.capture_enabled, true);
  assert.deepEqual(env.permissionCalls, []);
});

test("pattern controls report worker failures and remain usable during host repair", async () => {
  const origin = "https://pattern-repair.example";
  install({ adapters: [artifactFor(origin)], worker: {
    get_site_patterns: () => { throw new Error("Pattern storage unavailable"); },
    clear_site_patterns: () => [],
  } });
  env.configIssue = "Repair the host configuration";
  await openSettings();
  const details = env.elements.get("sites-list").children[0].children.find((child) => child.tagName === "DETAILS");
  const [show, clear] = details.querySelectorAll("button");
  assert.equal(show.disabled, false);
  assert.equal(clear.disabled, false);
  show.click();
  await until(() => !show.disabled, "the failed pattern read");
  assert.ok(details.children.some((child) => child.textContent.includes("Pattern storage unavailable")));
  clear.click();
  await until(() => !clear.disabled, "local clearing during repair");
  assert.ok(details.children.some((child) => child.textContent.startsWith("Patterns cleared.")));
  assert.equal(env.requests.some((request) => request.type === "update_config"), false);
});

async function openSettings({ connected = true } = {}) {
  loads += 1;
  await import(`../dist/options.js?load=${loads}`);
  await until(() => (idle() || !connected) && !statusText().startsWith("Connecting"),
    "the settings page to load");
  // A connected page then reads the agent.env names without holding the page busy.
  if (connected) await until(() => env.elements.get("agent-env-names").textContent !== "", "the agent.env names");
}

function typeSite(url) {
  env.elements.get("site-origin").value = url;
  env.elements.get("site-origin").dispatch("input");
}

async function removeListedSite(index = 0) {
  const remove = env.elements.get("sites-list").querySelectorAll("button").filter((button) => button.textContent === "Remove site")[index];
  remove.click();
  await until(() => statusText().startsWith("Removed ") && idle(), "the removal to finish");
}

async function saveOpenAdapter(what) {
  const before = env.revision;
  env.elements.get("save-adapter").click();
  await until(() => env.revision > before && idle(), what);
}

test("the header names the host build and flags one from another commit", async () => {
  install({ host: { hello: () => ({
    type: "hello_result", host_version: "0.1.0+0123456789ab", configured: true, config_issue: null,
  }) } });
  await openSettings();
  assert.equal(env.elements.get("host-build").textContent,
    " · host 0123456789ab (not the extension's commit; rebuild both)");
});

test("a restarted host's build replaces the exited one's in the header", async () => {
  const state = install({ host: { hello: (_request, current) => ({
    type: "hello_result", host_version: `0.1.0+${current.connects === 1 ? "abc1234def56" : "0123456789ab"}`,
    configured: true, config_issue: null,
  }) } });
  await openSettings();
  assert.equal(env.elements.get("host-build").textContent, " · host abc1234def56");

  state.exitHost();
  // Whatever answers next may be a rebuilt binary.
  assert.equal(env.elements.get("host-build").textContent, "");
  env.elements.get("refresh-artifacts").click();
  await until(() => env.elements.get("host-build").textContent !== "", "the new host's build");
  assert.equal(env.elements.get("host-build").textContent,
    " · host 0123456789ab (not the extension's commit; rebuild both)");
});

test("the site list is the distinct adapter origins", async () => {
  install({ adapters: [
    artifactFor("https://b.example", "work.doc"),
    artifactFor("https://a.example"),
    artifactFor("https://a.example", "work.issue", "/issues/{id:int}"),
  ] });
  await openSettings();
  assert.deepEqual(siteRows(), ["https://a.example", "https://b.example"]);
  const { siteOrigins } = await import("../dist/settings.js");
  assert.deepEqual(siteOrigins({ format: 1, artifacts: env.config.adapters.artifacts }),
    ["https://a.example", "https://b.example"]);
});

test("saving the first adapter turns its site and capture on, with no permission request", async () => {
  const state = install({ adapters: [] });
  await openSettings();
  assert.equal(state.config.capture_enabled, false);
  typeSite("https://acme.atlassian.net");
  const [review] = env.elements.get("starter-gallery").querySelectorAll("button");
  review.click();
  await saveOpenAdapter("the first adapter save");
  assert.equal(lastUpdate().config.capture_enabled, true);
  assert.equal("sites" in lastUpdate().config, false);
  assert.deepEqual(lastUpdate().config.adapters.artifacts.map((adapter) => adapter.origin),
    ["https://acme.atlassian.net"]);
  assert.deepEqual(siteRows(), ["https://acme.atlassian.net"]);
  assert.deepEqual(state.permissionCalls, []);
  assert.match(statusText(), /jira\.issue adapter added/);
});

test("saving an adapter while capture is paused keeps capture paused", async () => {
  install({ adapters: [artifactFor(SITE.origin)], captureEnabled: false });
  await openSettings();
  typeSite("https://b.com");
  env.elements.get("new-adapter").click();
  await saveOpenAdapter("the second adapter save");
  assert.equal(lastUpdate().config.capture_enabled, false);
  assert.deepEqual(lastUpdate().config.adapters.artifacts.map((adapter) => adapter.origin),
    [SITE.origin, "https://b.com"]);
});

test("removing a site removes all its adapters", async () => {
  const other = artifactFor("https://b.com");
  const state = install({ adapters: [
    artifactFor(SITE.origin), other, artifactFor(SITE.origin, "work.issue", "/issues/{id:int}"),
  ] });
  await openSettings();
  assert.deepEqual(siteRows(), [SITE.origin, "https://b.com"]);
  await removeListedSite(0);
  const update = lastUpdate();
  assert.equal(update.config.adapters.artifacts.some((adapter) => adapter.origin === SITE.origin), false);
  assert.deepEqual(update.config.adapters.artifacts, [other]);
  assert.equal(update.config.capture_enabled, true);
  assert.equal(state.requests.filter((request) => request.type === "update_config").length, 1);
  assert.match(statusText(), /Removed https:\/\/a\.com and its 2 adapters/);
  assert.deepEqual(siteRows(), ["https://b.com"]);
});

test("removing the last site turns capture off and never removes a Chrome permission", async () => {
  const state = install({ adapters: [artifactFor(SITE.origin)] });
  await openSettings();
  await removeListedSite();
  assert.deepEqual(state.config.adapters.artifacts, []);
  assert.equal(state.config.capture_enabled, false);
  assert.deepEqual(state.permissionCalls, []);
  assert.deepEqual(siteRows(), ["None"]);
});

test("resume capture saves capture_enabled true", async () => {
  const state = install({ adapters: [artifactFor(SITE.origin)], captureEnabled: false });
  await openSettings();
  assert.match(statusText(), /^Capture is paused\./);
  assert.equal(env.elements.get("pause-capture").disabled, true);
  assert.equal(env.elements.get("resume-capture").disabled, false);
  env.elements.get("resume-capture").click();
  await until(() => state.config.capture_enabled === true && idle(), "capture to resume");
  assert.equal(lastUpdate().config.capture_enabled, true);
  assert.deepEqual(lastUpdate().config.adapters, state.config.adapters);
  assert.match(statusText(), /Capturing on sites with adapters\./);
  assert.equal(env.elements.get("resume-capture").disabled, true);
});

test("Resume capture is shown only while a finished setup is paused", async () => {
  install({ adapters: [artifactFor(SITE.origin)], captureEnabled: false });
  await openSettings();
  assert.equal(env.elements.get("resume-capture").hidden, false);
  assert.match(statusText(), /Click Resume capture to turn it back on\./);

  install({ adapters: [artifactFor(SITE.origin)], captureEnabled: true });
  await openSettings();
  assert.equal(env.elements.get("resume-capture").hidden, true);
});

test("Resume capture waits for an invalid adapter policy to be repaired", async () => {
  install({
    captureEnabled: false,
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: String(current.revision), config: current.config, config_issue: null,
        agent_status: AGENT_STATUS, adapter_issue: "unsupported adapter format 2",
        adapter_warnings: [],
        adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
      }),
    },
  });
  await openSettings();
  assert.equal(env.elements.get("resume-capture").hidden, true);
  assert.match(statusText(), /unsupported adapter format 2/);
});

test("capturePaused tells a paused setup apart from an unfinished one", async () => {
  const { capturePaused } = await import("../dist/settings.js");
  const paused = configWith([artifactFor(SITE.origin)], false);
  assert.equal(capturePaused(paused, null), true);
  assert.equal(capturePaused({ ...paused, capture_enabled: true }, null), false);
  assert.equal(capturePaused({ ...paused, storage: null }, null), false);
  assert.equal(capturePaused(paused, "config.toml is unreadable"), false);
  assert.equal(capturePaused(null, null), false);
});

test("setupProblem ignores sites and pause", async () => {
  const { setupProblem, captureStateText } = await import("../dist/settings.js");
  const configured = configWith([artifactFor(SITE.origin)]);
  assert.equal(setupProblem({ ...configured, capture_enabled: false, adapters: { format: 1, artifacts: [] } }, null), null);
  assert.equal(setupProblem({ ...configured, storage: null }, null), "No home folder is chosen.");
  assert.equal(setupProblem(configured, "bad file"), "The host configuration needs repair: bad file");
  assert.equal(captureStateText({ ...configured, capture_enabled: false }), "Capture is paused.");
  assert.equal(captureStateText(configured), "Capturing on sites with adapters.");
});

test("Add adapter needs no site and starts from the typed site address", async () => {
  install({ adapters: [] });
  await openSettings();
  assert.equal(env.elements.get("new-adapter").disabled, false);
  typeSite("https://c.example:8443");
  env.elements.get("new-adapter").click();
  assert.equal(JSON.parse(env.elements.get("adapter-json").value).origin, "https://c.example:8443");
});

test("a settings load failure after hello is not reported as a missing host", async () => {
  install({
    workerStatus: { pending_removals: ["https://b.com"] },
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: "1", config: current.config, config_issue: "config.json is unreadable",
        agent_status: AGENT_STATUS, adapter_issue: null, adapter_warnings: [],
        adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
      }),
    },
  });
  await openSettings();
  assert.doesNotMatch(statusText(), /Native host unavailable/);
  assert.match(statusText(), /config\.json is unreadable/);
});

// The worker reports pending removals until clear_pending_removals arrives.
function pendingWorker(origins) {
  return {
    get_status: (_request, current) =>
      statusFor({ pending_removals: current.pendingCleared ? [] : origins }),
    clear_pending_removals: (_request, current) => {
      current.pendingCleared = true;
      return statusFor();
    },
  };
}

test("settings applies pending removals with one update_config", async () => {
  const other = { origin: "https://b.com" };
  const state = install({
    adapters: [artifactFor(SITE.origin), artifactFor(other.origin)],
    worker: pendingWorker([SITE.origin]),
  });

  await openSettings();
  await until(() => state.pendingCleared, "pending removals to clear");

  const updates = state.requests.filter((request) => request.type === "update_config");
  assert.equal(updates.length, 1);
  assert.deepEqual(updates[0].config.adapters.artifacts.map((adapter) => adapter.origin), [other.origin]);
  assert.equal(updates[0].config.capture_enabled, true);
  const kinds = state.workerRequests.map((request) => request.kind);
  assert.ok(kinds.lastIndexOf("install_policy") < kinds.lastIndexOf("clear_pending_removals"),
    "the worker sees the narrowed lease before it is asked to clear");
  assert.equal(kinds.includes("remove_site"), false);
});

test("pending removals leave an adapter document that needs repair as it is", async () => {
  const futureAdapters = { format: 2, artifacts: [{ future: true }] };
  const state = install({
    captureEnabled: true,
    worker: pendingWorker([SITE.origin]),
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: String(current.revision),
        config: { ...current.config, adapters: futureAdapters }, config_issue: null,
        agent_status: AGENT_STATUS, adapter_issue: "unsupported adapter format 2",
        adapter_warnings: [],
        adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
      }),
    },
  });

  await openSettings();
  await until(() => state.pendingCleared, "pending removals to clear");

  // The invalid document can only be echoed unchanged, so there is nothing to
  // write; the worker keeps its block until a later fix drops the adapters.
  assert.equal(state.requests.some((request) => request.type === "update_config"), false);
  assert.equal(state.config.capture_enabled, true);
  assert.equal(state.workerRequests.some((request) => request.kind === "install_policy"), false);
});

test("a pending removal already absent from the host sends no update", async () => {
  const state = install({ worker: pendingWorker(["https://already-removed.example"]) });

  await openSettings();
  await until(() => state.pendingCleared, "pending removals to clear");

  assert.equal(state.requests.some((request) => request.type === "update_config"), false);
});

test("adapter policy problems and overlap warnings are visible in settings", async () => {
  install({
    adapters: [artifactFor(SITE.origin)],
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: "1", config: current.config, config_issue: null,
        agent_status: AGENT_STATUS, adapter_issue: "unsupported adapter format 2",
        adapter_warnings: [],
        adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
      }),
    },
  });
  await openSettings();
  assert.match(statusText(), /unsupported adapter format 2/);
  assert.equal(env.elements.get("pause-capture").disabled, false);

  install({
    adapters: [artifactFor(SITE.origin)],
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: "1", config: current.config, config_issue: null,
        agent_status: AGENT_STATUS, adapter_issue: null,
        adapter_warnings: [{ first_adapter: 0, first_rule: 0, second_adapter: 1, second_rule: 0 }],
        adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
      }),
    },
  });
  await openSettings();
  assert.match(statusText(), /1 adapter rule overlap needs review/);
});

const JIRA_MODAL = {
  type: "jira.issue",
  origin: "https://acme.atlassian.net",
  match: [
    { path: "/browse/{key:jira_key}" },
    { path: "/jira/{*}", query: { selectedIssue: "{key:jira_key}" } },
  ],
  id: "jira:{host}/{key}",
};

test("a strip_params entry an adapter reads is shown with a one-click fix", async () => {
  install({ adapters: [JIRA_MODAL], stripParams: ["utm_*", "selectedIssue", "fbclid"] });
  await openSettings();
  const notices = env.elements.get("strip-conflicts");
  assert.match(nodeText(notices), /selectedIssue/);
  assert.match(nodeText(notices), /jira\.issue on https:\/\/acme\.atlassian\.net/);
  assert.match(statusText(), /tracking-parameter entr/i);
  const [fix] = notices.querySelectorAll("button");
  assert.equal(fix.textContent, "Stop stripping selectedIssue");
  assert.equal(fix.disabled, false);

  const before = env.requests.filter((request) => request.type === "update_config").length;
  fix.click();
  await until(() => env.revision > 1 && idle(), "the fix to save");
  const updates = env.requests.filter((request) => request.type === "update_config");
  assert.equal(updates.length, before + 1);
  // A revision-checked save that removes only that entry.
  assert.equal(lastUpdate().expected_revision, "1");
  assert.deepEqual(lastUpdate().config.strip_params, ["utm_*", "fbclid"]);
  assert.deepEqual(lastUpdate().config.adapters, { format: 1, artifacts: [JIRA_MODAL] });
  assert.equal(lastUpdate().config.capture_enabled, true);
  assert.equal(lastUpdate().picker_token, null);
  assert.equal(notices.querySelectorAll("button").length, 0);
  assert.doesNotMatch(nodeText(notices), /selectedIssue/);
  assert.match(statusText(), /no longer strips selectedIssue/);
});

test("no strip_params notice without a conflict, and the fix waits for repair", async () => {
  install({ adapters: [JIRA_MODAL], stripParams: ["utm_*", "fbclid"] });
  await openSettings();
  assert.equal(env.elements.get("strip-conflicts").children.length, 0);

  install({
    adapters: [JIRA_MODAL],
    stripParams: ["selected*"],
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: "1", config: current.config, config_issue: "choose the folder again",
        agent_status: AGENT_STATUS, adapter_issue: null, adapter_warnings: [],
        adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
      }),
    },
  });
  await openSettings();
  const [fix] = env.elements.get("strip-conflicts").querySelectorAll("button");
  assert.equal(fix.textContent, "Stop stripping selected*");
  assert.equal(fix.disabled, true);
});

test("an unsupported adapter document can be removed without blocking unrelated controls", async () => {
  const state = install({
    captureEnabled: true,
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
        adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
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

test("resetting a host-only adapter key on an empty document clears the key before reporting success", async () => {
  // Models the host: an unknown [adapters] key never reaches the wire, and a
  // remove-all reset of an already empty document clears it only when the
  // update is an exact no-op.
  let hostOnlyKey = true;
  const issue = () => hostOnlyKey ? "unsupported adapter-table key future_mode" : null;
  const state = install({
    captureEnabled: true,
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: String(current.revision), config: current.config,
        config_issue: null, agent_status: AGENT_STATUS, adapter_issue: issue(),
        adapter_warnings: [],
        adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
      }),
      update_config: (request, current) => {
        const empty = request.config.adapters.format === 1 && request.config.adapters.artifacts.length === 0;
        const adaptersChanged = JSON.stringify(request.config.adapters) !== JSON.stringify(current.config.adapters);
        const noOp = JSON.stringify(request.config) === JSON.stringify(current.config);
        if (hostOnlyKey && empty && (adaptersChanged || noOp)) hostOnlyKey = false;
        current.config = request.config;
        current.revision += 1;
        return { type: "config_updated", revision: String(current.revision), config: current.config };
      },
    },
  });
  await openSettings();
  assert.equal(state.config.capture_enabled, true);
  assert.equal(env.elements.get("reset-adapters").disabled, false);

  env.elements.get("reset-adapters").click();
  await until(() => idle() && state.requests.some((request) => request.type === "update_config"),
    "the adapter reset");

  assert.equal(hostOnlyKey, false, "the reset actually clears the host-only key");
  assert.equal(state.config.capture_enabled, false);
  assert.deepEqual(state.config.adapters, { format: 1, artifacts: [] });
  assert.match(statusText(), /adapter rules removed/);
});

test("an adapter reset that leaves the issue in place does not report success", async () => {
  const state = install({
    host: {
      get_config: (_request, current) => ({
        type: "config_result", revision: String(current.revision), config: current.config,
        config_issue: null, agent_status: AGENT_STATUS,
        adapter_issue: "unsupported adapter-table key future_mode", adapter_warnings: [],
        adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
      }),
    },
  });
  await openSettings();
  env.elements.get("reset-adapters").click();
  await until(() => idle() && state.requests.some((request) => request.type === "update_config"),
    "the adapter reset");
  assert.doesNotMatch(statusText(), /adapter rules removed/);
  assert.match(statusText(), /future_mode/);
});

test("adapter workspace previews and saves a new adapter with one update", async () => {
  const saves = [];
  const state = install({
    host: { update_config: (request) => { saves.push(request); return null; } },
  });
  await openSettings();
  assert.equal(env.elements.get("new-adapter").disabled, false);

  typeSite("https://a.com");
  env.elements.get("new-adapter").click();
  assert.equal(env.elements.get("adapter-editor").hidden, false);
  assert.match(env.elements.get("adapter-json").value, /"origin": "https:\/\/a\.com"/);
  assert.equal(env.elements.get("save-adapter").disabled, false);
  assert.match(nodeText(env.elements.get("adapter-preview")), /All fixed non-artifact probes stay unmatched/);
  assert.match(nodeText(env.elements.get("adapter-preview")), /work\.item:a\.com\/123456/);

  env.elements.get("save-adapter").click();
  await until(() => state.config.adapters.artifacts.length === 1 && idle(), "the adapter save");
  assert.equal(saves.length, 1);
  assert.equal("consent_token" in saves[0], false);
  assert.equal(state.config.adapters.artifacts[0].type, "work.item");
  assert.match(statusText(), /adapter added/);
});

test("a Jira starter resolves the typed site address and enters preview before save", async () => {
  const site = { origin: "https://acme.atlassian.net", path_prefix: "/" };
  const state = install();
  await openSettings();
  assert.doesNotMatch(nodeText(env.elements.get("starter-gallery")), /Jira issues/);
  typeSite(site.origin);
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
  install();
  await openSettings();
  typeSite("https://github.com");
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
  install();
  await openSettings();
  typeSite("https://github.com");
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
        adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
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

test("a pull request or issue rule cannot be moved below its repository rule", async () => {
  const within = { type: "github.repo", id: "github.repo:{host}/{owner}/{repo}" };
  const scope = { owner: ["acme-corp"] };
  const pr = {
    type: "github.pr", origin: "https://github.com", scope, within,
    match: [{ path: "/{owner:name|lower}/{repo:name|lower}/pull/{number:int}" }],
    id: "github.pr:{host}/{owner}/{repo}/{number}",
  };
  const issue = {
    type: "github.issue", origin: "https://github.com", scope, within,
    match: [{ path: "/{owner:name|lower}/{repo:name|lower}/issues/{number:int}" }],
    id: "github.issue:{host}/{owner}/{repo}/{number}",
  };
  const repo = {
    type: "github.repo", origin: "https://github.com", scope,
    match: [{ path: "/{owner:name|lower}/{repo:name|lower}/{*}" }], id: within.id,
  };
  const state = install({ adapters: [pr, issue, repo] });
  await openSettings();
  const held = "A pull request or issue rule must stay ahead of its repository rule.";
  // Each card has Edit, Move up, Move down, Remove.
  const buttons = () => env.elements.get("adapter-list").querySelectorAll("button");
  assert.equal(buttons()[2].disabled, false, "PR and issue may swap");
  assert.equal(buttons()[6].disabled, true, "the issue cannot move below the repository");
  assert.equal(buttons()[6].title, held);
  assert.equal(buttons()[9].disabled, true, "the repository cannot move above the issue");

  // Even if the click arrives, nothing is saved and the reason is shown.
  buttons()[6].dispatch("click");
  await until(idle, "the held move");
  assert.equal(statusText(), held);
  assert.equal(state.requests.some((request) => request.type === "update_config"), false);

  buttons()[2].click();
  await until(() => state.config.adapters.artifacts[0].type === "github.issue" && idle(), "the allowed swap");
  assert.deepEqual(state.config.adapters.artifacts.map((adapter) => adapter.type),
    ["github.issue", "github.pr", "github.repo"]);
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

const FOLDER_ISSUE = "The selected home folder is unavailable or changed. Choose it again.";

test("a kept config with a folder issue needs a new folder pick, which repairs it", async () => {
  const state = install({ adapters: [artifactFor(SITE.origin)] });
  state.configIssue = FOLDER_ISSUE;
  await openSettings();
  assert.match(statusText(), /needs repair/);
  const [remove] = env.elements.get("sites-list").querySelectorAll("button");
  assert.equal(remove.disabled, true);
  assert.equal(env.elements.get("pause-capture").disabled, true);
  assert.equal(env.elements.get("resume-capture").disabled, true);
  typeSite("https://b.com");
  assert.equal(env.elements.get("new-adapter").disabled, true);

  env.elements.get("choose-folder").click();
  await until(() => state.config.storage?.root === "/vault" && idle(), "the repair to be saved");
  assert.match(statusText(), /^Capturing on sites with adapters\./);
  assert.deepEqual(state.config.adapters.artifacts.map((adapter) => adapter.origin), [SITE.origin]);
  assert.equal(env.elements.get("new-adapter").disabled, false);
});

test("saving settings sends one update_config and never confirm_config", async () => {
  const state = install();
  state.config = { ...state.config, storage: null };
  await openSettings();
  state.requests.length = 0;
  // The host has no confirm_config handler, so any call throws "Unexpected host request".
  env.elements.get("choose-folder").click();
  await until(() => state.config.storage?.root === "/vault" && idle(), "the folder to be saved");
  const hostTypes = () => state.requests.map((request) => request.type)
    .filter((type) => type !== "choose_folder" && type !== "hello");
  assert.deepEqual([...new Set(hostTypes())].sort(), ["get_config", "update_config"]);
  assert.equal(hostTypes().filter((type) => type === "update_config").length, 1);
  const lastUpdate = state.requests.findLast((request) => request.type === "update_config");
  assert.equal("consent_token" in lastUpdate, false);
});

test("choosing a folder saves it at once, leaving adapters and capture as they were", async () => {
  const saves = [];
  const state = install({
    host: { update_config: (request) => { saves.push(request); return null; } },
  });
  state.config = { ...state.config, storage: null };
  await openSettings();
  env.elements.get("choose-folder").click();
  await until(() => state.config.storage?.root === "/vault" && idle(), "the folder to be saved");
  assert.equal(saves.length, 1);
  assert.equal(saves[0].picker_token, "picker-1");
  assert.equal(saves[0].knowledge_base_token, null);
  assert.equal(env.requests.find((request) => request.type === "choose_folder").purpose, "notes_folder");
  assert.equal("consent_token" in saves[0], false);
  assert.equal(state.config.capture_enabled, false);
  assert.deepEqual(state.config.adapters.artifacts, []);
  assert.equal(env.elements.get("folder-path").textContent, "/vault");
  assert.match(statusText(), /^No site has an adapter yet\./);
});

test("choosing a home folder saves the fixed layout, never an older one", async () => {
  const saves = [];
  const state = install({
    host: { update_config: (request) => { saves.push(request); return null; } },
  });
  state.config = {
    ...state.config,
    storage: {
      ...state.config.storage, root: "/old", log_dir: "log", pages_dir: "pages", artifacts_dir: "brauser-artifacts",
      summaries_dir: null, knowledge_base_dir: "/kb",
    },
  };
  await openSettings();
  env.elements.get("choose-folder").click();
  await until(() => saves.length === 1 && idle(), "the home folder to be saved");
  assert.deepEqual(saves[0].config.storage, {
    root: "/vault", profile: "neutral", log_dir: "logs", pages_dir: "notes", later_dir: "later",
    artifacts_dir: "artifacts", summaries_dir: "summaries", knowledge_base_dir: "/kb",
  });
  const html = readFileSync(new URL("../options.html", import.meta.url), "utf8");
  for (const folder of ["notes/", "logs/", "artifacts/", "summaries/", "later/"]) {
    assert.ok(html.includes(`<code>${folder}</code>`), folder);
  }
  assert.ok(!html.includes("summaries-dir"));
});

test("a host restart drops a folder selection only the old host knew", async () => {
  const state = install({
    host: {
      // The first folder save fails, so the selection stays on the page.
      update_config: (request, current) => {
        if (current.connects === 1) return { type: "error", code: "internal", message: "could not save configuration" };
        return request.picker_token !== null && request.picker_token !== `picker-${current.connects}`
          ? { type: "error", code: "unauthorized", message: "folder selection token is unknown" }
          : null;
      },
    },
  });
  state.config = { ...state.config, storage: null };
  await openSettings();
  env.elements.get("choose-folder").click();
  await until(() => statusText() === "could not save configuration" && idle(), "the folder pick");
  assert.equal(env.elements.get("folder-path").textContent, "/vault");

  state.exitHost();
  assert.match(statusText(), /Choose the folder again/);
  assert.equal(env.elements.get("folder-path").textContent, "None selected");

  env.elements.get("choose-folder").click();
  await until(() => state.config.storage?.root === "/vault" && idle(), "the second folder pick to be saved");
  typeSite("https://a.com");
  env.elements.get("new-adapter").click();
  await saveOpenAdapter("the adapter save");
  assert.match(statusText(), /adapter added/);
  assert.equal(state.config.capture_enabled, true);
});

test("a folder selection the host did not save is saved with the next adapter", async () => {
  const state = install({
    host: {
      update_config: (request, current) => current.failedOnce ? null
        : (current.failedOnce = true, { type: "error", code: "internal", message: "could not save configuration" }),
    },
  });
  state.config = { ...state.config, storage: null };
  await openSettings();
  env.elements.get("choose-folder").click();
  await until(() => statusText() === "could not save configuration" && idle(), "the folder pick");
  typeSite("https://a.com");
  env.elements.get("new-adapter").click();
  await saveOpenAdapter("the adapter save");
  assert.equal(lastUpdate().picker_token, "picker-1");
  assert.equal(state.config.storage?.root, "/vault");
  assert.equal(state.config.capture_enabled, true);
});

test("choosing a knowledge base folder saves it with its own token", async () => {
  const state = install({
    host: {
      choose_folder: (request) => request.purpose === "knowledge_base"
        ? { type: "folder_chosen", path: "/kb", picker_token: "t" }
        : null,
    },
  });
  await openSettings();
  assert.equal(env.elements.get("kb-path").textContent, "Same as the home folder (/vault)");
  assert.equal(env.elements.get("kb-use-notes").disabled, true);
  env.elements.get("choose-kb").click();
  await until(() => state.config.storage?.knowledge_base_dir === "/kb" && idle(), "the knowledge base to be saved");
  const lastChoose = env.requests.findLast((request) => request.type === "choose_folder");
  assert.equal(lastChoose.purpose, "knowledge_base");
  assert.equal(lastUpdate().config.storage.knowledge_base_dir, "/kb");
  assert.equal(lastUpdate().config.storage.root, "/vault");
  assert.equal(lastUpdate().knowledge_base_token, "t");
  assert.equal(lastUpdate().picker_token, null);
  assert.equal(env.elements.get("kb-path").textContent, "/kb");
  assert.match(statusText(), /^Knowledge base folder saved\./);
});

test("Use home folder clears the knowledge base folder without a token", async () => {
  const state = install();
  state.config.storage.knowledge_base_dir = "/kb";
  await openSettings();
  assert.equal(env.elements.get("kb-path").textContent, "/kb");
  const button = env.elements.get("kb-use-notes");
  assert.equal(button.disabled, false);
  button.click();
  await until(() => state.config.storage?.knowledge_base_dir === null && idle(), "the knowledge base to follow the home folder");
  assert.equal(lastUpdate().knowledge_base_token, null);
  assert.equal(lastUpdate().picker_token, null);
  assert.equal(env.requests.some((request) => request.type === "choose_folder"), false);
});

test("a knowledge base folder needs a home folder first", async () => {
  const state = install();
  state.config = { ...state.config, storage: null };
  await openSettings();
  assert.equal(env.elements.get("choose-kb").disabled, true);
  assert.equal(env.elements.get("kb-use-notes").disabled, true);
  assert.equal(env.elements.get("kb-path").textContent, "Same as the home folder");
});

test("a home folder change keeps the chosen knowledge base folder", async () => {
  const state = install({
    host: {
      choose_folder: () => ({ type: "folder_chosen", path: "/other", picker_token: "notes" }),
    },
  });
  state.config.storage.knowledge_base_dir = "/kb";
  await openSettings();
  env.elements.get("choose-folder").click();
  await until(() => state.config.storage?.root === "/other" && idle(), "the home folder to be saved");
  assert.equal(lastUpdate().config.storage.knowledge_base_dir, "/kb");
  assert.equal(lastUpdate().knowledge_base_token, null);
  assert.equal(lastUpdate().picker_token, "notes");
});

const CODEX_FOUND = { harness: "codex", binary: "/opt/homebrew/bin/codex", version: "0.144.4", problem: null };
const CLAUDE_FOUND = { harness: "claude", binary: "/usr/local/bin/claude", version: "2.1.310", problem: null };
const SERVICE_DOWN = "The Brauser service is not installed. Reinstall Brauser, or for a development build run scripts/register-dev-host.mjs.";

// The fake host answers get_config at revision "r<n>", as the real host's
// revisions are opaque strings.
function opaqueRevision(state) {
  return {
    type: "config_result", revision: `r${state.revision}`, config: state.config,
    config_issue: null, agent_status: AGENT_STATUS, adapter_issue: null, adapter_warnings: [],
        adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
  };
}

test("detect lists both harnesses with versions and Test sends set_up_harness", async () => {
  const state = install({
    host: {
      get_config: (_request, current) => opaqueRevision(current),
      discover_harnesses: () => ({ type: "harnesses_discovered", harnesses: [CODEX_FOUND, CLAUDE_FOUND] }),
      set_up_harness: (request, current) => {
        current.config = {
          ...current.config,
          agent: {
            harness: request.harness, binary: CODEX_FOUND.binary, version: CODEX_FOUND.version,
            timeout_secs: request.timeout_secs, probed_at: "2026-10-27T10:00:00Z",
          },
          storage: { ...current.config.storage, summaries_dir: request.summaries_dir },
        };
        current.revision += 1;
        return { type: "config_updated", revision: `r${current.revision}`, config: current.config };
      },
    },
  });
  await openSettings();
  assert.equal(env.elements.get("harness-timeout").value, "10");
  assert.equal(env.elements.get("test-harness").disabled, true, "Test needs a detected harness");
  state.requests.length = 0;
  env.elements.get("detect-harnesses").click();
  await until(() => !env.elements.get("test-harness").disabled, "Test and use to become available");
  const listed = nodeText(env.elements.get("harness-list"));
  assert.match(listed, /Codex 0\.144\.4/);
  assert.match(listed, /Claude Code 2\.1\.310/);
  const radios = env.elements.get("harness-list").querySelectorAll("input");
  assert.deepEqual(radios.map((radio) => [radio.type, radio.value]), [["radio", "codex"], ["radio", "claude"]]);
  assert.equal(radios[0].checked, true, "the first usable harness is chosen");
  env.elements.get("test-harness").click();
  await until(() => statusText().startsWith("Codex is set up") && idle(), "setup to finish");
  const hostTypes = state.requests.map((request) => request.type);
  assert.ok(!hostTypes.includes("update_config"));
  assert.ok(state.requests.every((request) => !("harness_token" in request)));
  const discover = state.requests.find((request) => request.type === "discover_harnesses");
  assert.equal("expected_revision" in discover, false);
  const setUp = state.requests.find((request) => request.type === "set_up_harness");
  const { type: _type, protocol_version: _version, request_id: _id, ...lastSetUp } = setUp;
  // The host keeps summaries in the fixed summaries/ folder.
  assert.deepEqual(lastSetUp, {
    harness: "codex", timeout_secs: 600, summaries_dir: null, expected_revision: "r1",
  });
  // The page reloads the host config after setup.
  assert.ok(hostTypes.lastIndexOf("get_config") > hostTypes.indexOf("set_up_harness"));
  assert.equal(state.config.agent.harness, "codex");
});

test("a run time limit outside 1 to 60 minutes is not sent", async () => {
  const state = install({
    host: { discover_harnesses: () => ({ type: "harnesses_discovered", harnesses: [CODEX_FOUND, CLAUDE_FOUND] }) },
  });
  await openSettings();
  env.elements.get("detect-harnesses").click();
  await until(() => !env.elements.get("test-harness").disabled, "Test and use to become available");
  for (const minutes of ["0", "61", "2.5", ""]) {
    env.elements.get("harness-timeout").value = minutes;
    env.elements.get("harness-timeout").dispatch("input");
    env.elements.get("test-harness").click();
    await until(() => /whole number of minutes/.test(statusText()) && idle(), `refusal of ${minutes}`);
  }
  assert.ok(!state.requests.some((request) => request.type === "set_up_harness"));
});

test("agent.env names are shown and values never requested", async () => {
  const state = install({
    host: {
      get_agent_env: () => ({
        type: "agent_env_listed", names: ["AWS_PROFILE", "CLAUDE_CODE_USE_BEDROCK"], problem: null,
      }),
    },
  });
  await openSettings();
  await until(() => /AWS_PROFILE/.test(env.elements.get("agent-env-names").textContent), "agent.env names");
  const shown = env.elements.get("agent-env-names").textContent;
  assert.match(shown, /AWS_PROFILE, CLAUDE_CODE_USE_BEDROCK/);
  const listing = state.requests.filter((request) => request.type === "get_agent_env");
  assert.equal(listing.length, 1);
  const { type: _type, protocol_version: _version, request_id: _id, ...fields } = listing[0];
  assert.deepEqual(fields, {}, "the listing asks for nothing but names");
  assert.ok(!JSON.stringify(state.requests).includes("value"), "no request names a value");
});

test("an agent.env problem is shown in place of names", async () => {
  install({
    host: {
      get_agent_env: () => ({
        type: "agent_env_listed", names: [], problem: "agent.env must have mode 0600, not 0644",
      }),
    },
  });
  await openSettings();
  await until(() => /0600/.test(env.elements.get("agent-env-names").textContent), "agent.env problem");
  assert.match(env.elements.get("agent-env-names").textContent, /^agent\.env cannot be used: /);
});

test("reveal agent.env sends open_path agent_env", async () => {
  const state = install({
    host: { open_path: () => ({ type: "path_opened" }) },
  });
  await openSettings();
  await until(() => !env.elements.get("reveal-agent-env").disabled, "Reveal to become available");
  state.requests.length = 0;
  env.elements.get("reveal-agent-env").click();
  await until(() => statusText().startsWith("Showed agent.env in Finder") && idle(), "the reveal");
  const opened = state.requests.filter((request) => request.type === "open_path");
  assert.equal(opened.length, 1);
  assert.equal(opened[0].target, "agent_env");
  assert.equal(opened[0].run_id, null);
  // A reveal creates a missing file, so the names are read again after it.
  assert.ok(state.requests.findIndex((request) => request.type === "get_agent_env") >
    state.requests.findIndex((request) => request.type === "open_path"));
});

test("vocabulary edits save through update_config", async () => {
  const state = install();
  await openSettings();
  assert.equal(env.elements.get("vocab-types").value, "report");
  assert.equal(env.elements.get("vocab-categories").value, "news");
  env.elements.get("vocab-types").value = " report \n\nmemo\nReport\n  minutes  ";
  env.elements.get("vocab-types").dispatch("input");
  env.elements.get("vocab-categories").value = "news\nresearch paper\n";
  env.elements.get("vocab-categories").dispatch("input");
  const before = state.revision;
  env.elements.get("save-vocabulary").click();
  await until(() => state.revision > before && idle(), "the vocabulary save");
  const update = lastUpdate();
  assert.deepEqual(update.config.summary_vocabulary, {
    document_types: ["report", "memo", "minutes"],
    categories: ["news", "research paper"],
  });
  assert.equal(update.picker_token, null);
  assert.deepEqual({ ...update.config, summary_vocabulary: null },
    { ...configWith([]), summary_vocabulary: null }, "nothing else changes");
  assert.match(statusText(), /vocabulary saved/i);
  assert.equal(env.elements.get("vocab-types").value, "report\nmemo\nminutes");
});

test("an empty vocabulary list is not sent", async () => {
  const state = install();
  await openSettings();
  env.elements.get("vocab-categories").value = " \n \n";
  env.elements.get("vocab-categories").dispatch("input");
  env.elements.get("save-vocabulary").click();
  await until(() => /at least one category/.test(statusText()) && idle(), "the refusal");
  assert.ok(!state.requests.some((request) => request.type === "update_config"));
});

test("service_unavailable shows setup instructions", async () => {
  const { SERVICE_SETUP_INSTRUCTIONS } = await import("../dist/agent.js");
  const down = () => ({ type: "error", code: "service_unavailable", message: SERVICE_DOWN });
  install({ host: { get_agent_env: down, discover_harnesses: down } });
  await openSettings();
  const harnessState = () => env.elements.get("harness-state").textContent;
  await until(() => harnessState().includes(SERVICE_DOWN), "the listing's setup instructions");
  assert.ok(harnessState().startsWith(SERVICE_SETUP_INSTRUCTIONS));
  assert.ok(harnessState().endsWith(SERVICE_DOWN), "the host's message follows the instructions");
  env.elements.get("detect-harnesses").click();
  await until(() => idle() && /Could not detect harnesses/.test(statusText()), "detect to fail");
  assert.ok(harnessState().startsWith(SERVICE_SETUP_INSTRUCTIONS));
  // Other errors are not service setup problems.
  install({ host: { discover_harnesses: () => ({ type: "error", code: "internal", message: "boom" }) } });
  await openSettings();
  env.elements.get("detect-harnesses").click();
  await until(() => idle() && /Could not detect harnesses: boom/.test(statusText()), "detect to fail");
  assert.ok(!env.elements.get("harness-state").textContent.includes(SERVICE_SETUP_INSTRUCTIONS));
});

test("settings shows the exact setup warning", async () => {
  install();
  await openSettings();
  const { SETUP_WARNING } = await import("../dist/agent.js");
  assert.equal(env.elements.get("harness-warning").textContent, SETUP_WARNING);
  const html = readFileSync(new URL("../options.html", import.meta.url), "utf8");
  assert.ok(html.includes(`<p id="harness-warning">${SETUP_WARNING}</p>`));
  assert.ok(html.includes(">Test and use</button>"));
  assert.ok(html.includes(">Reveal agent.env in Finder</button>"));
  assert.ok(!html.includes("Set up harness"));
  for (const id of ["harness-offers", "harness-env", "setup-harness"]) {
    assert.ok(!html.includes(`id="${id}"`), id);
  }
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

// --- Capture Site advanced-editor handoff ----------------------------------

const { canonicalJson, DRAFT_HANDOFF_TTL_MS } = await import("../dist/capture-site-handoff.js");
const WIKI = "https://wiki.example";
const WIKI_TABLE = {
  type: "wiki.pages", origin: WIKI,
  match: [{ path: "/pages/{num:int}" }, { path: "/archive/{num:int}" }],
  id: "wiki.pages:{host}/{num}",
};
const LINEAR_TABLE = {
  type: "linear.issue", origin: "https://linear.app",
  match: [{ path: "/{workspace:name|lower}/issue/{key:jira_key}" }],
  id: "linear.issue:{host}/{workspace}/{key}",
};
const handoffKey = (id) => `captureSiteDraft:${id}`;

function handoff(id, adapters, overrides = {}) {
  return {
    id, origin: adapters[0].origin, created: Date.now(), adapters: structuredClone(adapters),
    baseline_rules: adapters.map((adapter) => adapter.match.map(canonicalJson)),
    visited: [{ shape: "/pages/{num}", count: 3 }], links: [{ shape: "/archive/{num}", count: 4 }],
    ...overrides,
  };
}

// The host's check_site_draft: `verdict(provenance, adapter, rule)` judges
// each rule; every scope choice is accepted.
function checkHost(verdict = () => ({ verdict: "kept", reason: null })) {
  return {
    check_site_draft: (request, state) => ({
      type: "site_draft_checked", revision: String(state.revision),
      rules: request.provenance.map((rules, a) => rules.map((provenance, r) => verdict(provenance, a, r))),
      scope: request.adapters.map(() => ({ state: "not_shared", resolved: "not_shared", reason: null })),
      preview: [],
    }),
  };
}

async function openHandoff(entry, options = {}) {
  const session = new Map([[handoffKey(entry.id), entry]]);
  install({ session, hash: `#draft=${entry.id}`, ...options });
  await openSettings();
  return session;
}

function editDraft(adapters) {
  env.elements.get("adapter-json").value = JSON.stringify(adapters, null, 2);
  env.elements.get("adapter-json").dispatch("input");
}

const checks = () => env.requests.filter((request) => request.type === "check_site_draft");

test("a handed-off draft opens in the adapter editor once and expires after 30 minutes", async () => {
  const entry = handoff("draft-1", [WIKI_TABLE]);
  const session = await openHandoff(entry);
  assert.equal(env.elements.get("adapter-editor").hidden, false);
  assert.match(env.elements.get("adapter-editor-heading").textContent, /wiki\.example/);
  assert.deepEqual(JSON.parse(env.elements.get("adapter-json").value), [WIKI_TABLE]);
  assert.equal(session.size, 0, "the entry is read once and deleted");
  assert.equal(env.elements.get("save-adapter").disabled, false);

  // A second load with the same ID finds nothing.
  install({ session, hash: "#draft=draft-1" });
  await openSettings();
  assert.equal(env.elements.get("adapter-editor").hidden, true);
  assert.match(statusText(), /expired or was already opened/);

  const expired = handoff("draft-2", [WIKI_TABLE], { created: Date.now() - DRAFT_HANDOFF_TTL_MS - 1 });
  const left = await openHandoff(expired);
  assert.equal(env.elements.get("adapter-editor").hidden, true);
  assert.equal(left.size, 0, "an expired entry is deleted");
  assert.match(statusText(), /expired or was already opened/);
  assert.equal(env.requests.some((request) => request.type === "update_config"), false);
});

test("editing one rule makes only that rule user-written", async () => {
  await openHandoff(handoff("draft-edit", [WIKI_TABLE]), { host: checkHost() });
  const edited = structuredClone(WIKI_TABLE);
  edited.match[1] = { path: "/archive/{num:int}/view" };
  editDraft([edited]);
  await saveOpenAdapter("the edited draft to save");
  const [check] = checks();
  assert.deepEqual(check.provenance, [["drafted", "user_written"]]);
  assert.deepEqual(check.adapters, [edited]);
  assert.deepEqual(check.scope, [null]);
  assert.deepEqual(check.visited, [{ shape: "/pages/{num}", count: 3 }]);
  assert.deepEqual(check.links, [{ shape: "/archive/{num}", count: 4 }]);
  assert.equal(check.expected_revision, "1");
  assert.deepEqual(lastUpdate().config.adapters.artifacts, [edited]);
  assert.equal(env.config.capture_enabled, true);
});

test("user-written rule failures only warn while unedited drafted failures block save", async () => {
  // A drafted rule the host now drops blocks the whole save.
  await openHandoff(handoff("draft-drop", [WIKI_TABLE]), { host: checkHost((provenance, _a, r) =>
    r === 1 ? { verdict: "dropped", reason: "draft_overlap" } : { verdict: "kept", reason: null }) });
  env.elements.get("save-adapter").click();
  await until(() => checks().length === 1 && idle(), "the refused check");
  assert.equal(env.requests.some((request) => request.type === "update_config"), false);
  assert.match(statusText(), /already captured by this site's rules/);
  assert.equal(env.elements.get("adapter-editor").hidden, false, "the editor stays open to fix it");

  // The same failure on the rule the user wrote only warns, and the full
  // edited document is saved.
  await openHandoff(handoff("draft-warn", [WIKI_TABLE]), { host: checkHost((provenance) =>
    provenance === "user_written" ? { verdict: "warned", reason: "draft_matches_probe" } : { verdict: "kept", reason: null }) });
  const edited = structuredClone(WIKI_TABLE);
  edited.match[1] = { path: "/archive/{num:int}/view" };
  editDraft([edited]);
  await saveOpenAdapter("the warned draft to save");
  assert.deepEqual(checks()[0].provenance, [["drafted", "user_written"]]);
  assert.deepEqual(lastUpdate().config.adapters.artifacts, [edited]);
  assert.match(statusText(), /saved/i);
  assert.match(statusText(), /wiki\.pages rule 2.*sign-in, search, dashboard or sharing page/);
});

test("adding a title or choosing scope in the advanced editor does not trip draft_forbidden_key", async () => {
  await openHandoff(handoff("draft-scope", [LINEAR_TABLE], {
    visited: [{ shape: "/{slug}/issue/{key}", count: 1 }], links: [],
  }), { host: checkHost() });
  const edited = { ...structuredClone(LINEAR_TABLE), scope: { workspace: ["acme"] }, title: "{title} · Linear" };
  editDraft([edited]);
  await saveOpenAdapter("the scoped draft to save");
  const [check] = checks();
  assert.deepEqual(check.adapters, [LINEAR_TABLE], "title, scope and unscoped are stripped");
  assert.deepEqual(check.provenance, [["drafted"]]);
  assert.deepEqual(check.scope, [{ values: ["acme"] }]);
  assert.deepEqual(lastUpdate().config.adapters.artifacts, [edited]);
});

test("an adapter with a hand-added path_case is user-written and its forbidden key only warns", async () => {
  await openHandoff(handoff("draft-case", [WIKI_TABLE]), { host: checkHost((provenance) =>
    provenance === "user_written" ? { verdict: "warned", reason: "draft_forbidden_key" } : { verdict: "dropped", reason: "draft_forbidden_key" }) });
  const edited = { ...structuredClone(WIKI_TABLE), path_case: "insensitive" };
  editDraft([edited]);
  await saveOpenAdapter("the hand-edited draft to save");
  const [check] = checks();
  assert.deepEqual(check.adapters, [edited], "a hand-added key stays in the checked table");
  assert.deepEqual(check.provenance, [["user_written", "user_written"]]);
  assert.deepEqual(lastUpdate().config.adapters.artifacts, [edited]);
  assert.match(statusText(), /it did not pass Brauser's safety checks/);
});

// GitHub PR and issue rules as a user configured them before repositories
// existed: scoped, with no `within`.
function existingGitHubRules(prOwners = ["a"], issueOwners = ["b"]) {
  return [
    {
      type: "github.pr", origin: "https://github.com", scope: { owner: prOwners },
      match: [{ path: "/{owner:name|lower}/{repo:name|lower}/pull/{number:int}" }],
      id: "github.pr:{host}/{owner}/{repo}/{number}",
    },
    {
      type: "github.issue", origin: "https://github.com", scope: { owner: issueOwners },
      match: [{ path: "/{owner:name|lower}/{repo:name|lower}/issues/{number:int}" }],
      id: "github.issue:{host}/{owner}/{repo}/{number}",
    },
  ];
}

const repoButton = () => env.elements.get("starter-gallery").querySelectorAll("button")
  .find((button) => button.dataset.starterKey === "github.repo");

test("GitHub gallery pre-fills organizations from existing GitHub rules", async () => {
  install({ adapters: existingGitHubRules(["a"], ["b"]) });
  await openSettings();
  const [scope] = env.elements.get("starter-gallery").querySelectorAll("input");
  assert.equal(scope.value, "a, b");

  // Once the user edits the field, a re-render keeps what they typed.
  scope.value = "c";
  scope.dispatch("input");
  typeSite("https://github.com");
  assert.equal(env.elements.get("starter-gallery").querySelectorAll("input")[0].value, "c");
});

test("a pull request starter links to repositories only when a repository rule exists", async () => {
  const prButton = () => env.elements.get("starter-gallery").querySelectorAll("button")
    .find((button) => button.dataset.starterKey === "github.pr");
  install();
  await openSettings();
  typeSite("https://github.com");
  const [scope] = env.elements.get("starter-gallery").querySelectorAll("input");
  scope.value = "acme-corp";
  scope.dispatch("input");
  prButton().click();
  assert.equal(JSON.parse(env.elements.get("adapter-json").value).within, undefined);

  const repository = {
    type: "github.repo", origin: "https://github.com", scope: { owner: ["acme-corp"] },
    match: [{ path: "/{owner:name|lower}/{repo:name|lower}/{*}" }],
    id: "github.repo:{host}/{owner}/{repo}",
  };
  install({ adapters: [repository] });
  await openSettings();
  prButton().click();
  assert.deepEqual(JSON.parse(env.elements.get("adapter-json").value).within,
    { type: "github.repo", id: "github.repo:{host}/{owner}/{repo}" });
});

test("github.repo is offered when only PR and issue rules exist", async () => {
  install({ adapters: existingGitHubRules(["acme-corp"], ["acme-corp"]) });
  await openSettings();
  const button = repoButton();
  assert.ok(button);
  assert.equal(button.textContent, "Review starter…");
  assert.equal(button.disabled, false);
});

test("adding repositories links existing PR and issue rules without changing their scope", async () => {
  const existing = existingGitHubRules(["a"], ["b"]);
  const saves = [];
  const state = install({
    adapters: existing,
    host: { update_config: (request) => { saves.push(request); return null; } },
  });
  await openSettings();
  repoButton().click();
  assert.equal(env.elements.get("adapter-editor").hidden, false);
  const draft = JSON.parse(env.elements.get("adapter-json").value);
  assert.equal(draft.type, "github.repo");
  assert.deepEqual(draft.scope, { owner: ["a", "b"] });
  assert.match(env.elements.get("adapter-validation").textContent,
    /Also links your existing pull request and issue rules to their repositories\./);

  env.elements.get("save-adapter").click();
  await until(() => state.config.adapters.artifacts.length === 3 && idle(), "the repository save");
  assert.equal(saves.length, 1);
  const [pr, issue, repo] = saves[0].config.adapters.artifacts;
  assert.deepEqual([pr.type, issue.type, repo.type], ["github.pr", "github.issue", "github.repo"]);
  const within = { type: "github.repo", id: "github.repo:{host}/{owner}/{repo}" };
  assert.deepEqual(pr, { ...existing[0], within });
  assert.deepEqual(issue, { ...existing[1], within });
  assert.deepEqual(pr.scope, { owner: ["a"] });
  assert.deepEqual(issue.scope, { owner: ["b"] });
});

test("a repository starter with already-linked rules adds no linking line", async () => {
  const within = { type: "github.repo", id: "github.repo:{host}/{owner}/{repo}" };
  install({ adapters: existingGitHubRules().map((adapter) => ({ ...adapter, within })) });
  await openSettings();
  repoButton().click();
  assert.equal(env.elements.get("adapter-editor").hidden, false);
  assert.match(env.elements.get("adapter-validation").textContent, /^Valid here\./);
  assert.doesNotMatch(env.elements.get("adapter-validation").textContent, /Also links/);
});

test("an existing rule that cannot be linked stays as it is and does not block repositories", async () => {
  const [, issue] = existingGitHubRules(["acme-corp"], ["acme-corp"]);
  // Hand-written: its ID has no {repo}, so a repository `within` cannot be built from it.
  const handWritten = {
    type: "github.pr", origin: "https://github.com", scope: { owner: ["acme-corp"] },
    match: [{ path: "/{owner:name|lower}/{_}/pull/{number:int}" }],
    id: "github.pr:{host}/{number}",
  };
  const saves = [];
  const state = install({
    adapters: [handWritten, issue],
    host: { update_config: (request) => { saves.push(request); return null; } },
  });
  await openSettings();
  repoButton().click();
  const validation = env.elements.get("adapter-validation").textContent;
  assert.match(validation, /^Valid here\./);
  assert.match(validation, /Also links your existing pull request and issue rules to their repositories\./);
  assert.match(validation, /Some existing pull request or issue rules can't be linked to repositories because they don't capture owner and repo; they stay as they are\./);
  assert.equal(env.elements.get("save-adapter").disabled, false);

  env.elements.get("save-adapter").click();
  await until(() => state.config.adapters.artifacts.length === 3 && idle(), "the repository save");
  assert.equal(saves.length, 1);
  const saved = saves[0].config.adapters.artifacts;
  assert.deepEqual(saved.find((adapter) => adapter.type === "github.pr"), handWritten);
  assert.deepEqual(saved.find((adapter) => adapter.type === "github.issue"),
    { ...issue, within: { type: "github.repo", id: "github.repo:{host}/{owner}/{repo}" } });
});

test("pre-filling organizations and rendering the gallery saves nothing", async () => {
  const state = install({ adapters: existingGitHubRules(["a"], ["b"]) });
  await openSettings();
  typeSite("https://github.com");
  assert.equal(env.elements.get("starter-gallery").querySelectorAll("input")[0].value, "a, b");
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(state.requests.some((request) => request.type === "update_config"), false);
});

const DAMAGED_RECORD = "Brauser's record of the adapters it has used (/cfg/adopted-contracts.json) is malformed. " +
  "Notes on pages the current adapters do not recognize are paused to protect note identity.";

test("settings names a damaged adopted-adapter record and offers to forget it", async () => {
  install({ adapters: [artifactFor("https://work.example")] });
  env.adoptedState = { issue: DAMAGED_RECORD, revision: "sha256:damaged", warnings: [], full: false };
  await openSettings();
  const el = (id) => env.elements.get(id);
  assert.equal(el("adopted-state").hidden, false);
  assert.equal(el("adopted-issue").textContent, DAMAGED_RECORD);
  assert.equal(el("forget-adopted").hidden, false);
  assert.equal(el("forget-adopted").disabled, false);
  // The rest of settings keeps working; this record only pauses some notes.
  assert.equal(el("new-adapter").disabled, false);
  assert.equal(el("pause-capture").disabled, false);
});

test("forgetting recorded adapters asks in the page first, then sends the shown revision", async () => {
  install({ adapters: [artifactFor("https://work.example")] });
  env.adoptedState = { issue: DAMAGED_RECORD, revision: "sha256:damaged", warnings: [], full: false };
  await openSettings();
  const el = (id) => env.elements.get(id);
  const forgets = () => env.requests.filter((request) => request.type === "forget_adopted_contracts");

  el("forget-adopted").click();
  assert.equal(el("forget-adopted-confirm").hidden, false);
  // The confirmation is in the page (no native dialog) and says what is lost.
  const html = readFileSync(new URL("../options.html", import.meta.url), "utf8");
  const confirmation = html.slice(html.indexOf('id="forget-adopted-confirm"'), html.indexOf('id="forget-adopted-yes"'));
  assert.match(confirmation, /back the record up/);
  assert.match(confirmation, /removed from config\.toml by hand/);
  assert.match(confirmation, /Use as artifact note/);
  assert.equal(forgets().length, 0, "nothing is forgotten before the in-page confirmation");
  el("forget-adopted-no").click();
  assert.equal(el("forget-adopted-confirm").hidden, true);
  assert.equal(forgets().length, 0);

  el("forget-adopted").click();
  el("forget-adopted-yes").click();
  await until(() => el("adopted-state").hidden, "the repaired record");
  assert.equal(forgets().length, 1);
  assert.equal(forgets()[0].expected_revision, "sha256:damaged");
  assert.equal(el("forget-adopted-confirm").hidden, true);
  assert.match(statusText(), /Recorded adapters forgotten/);
  assert.equal(env.requests.filter((request) => request.type === "update_config").length, 0);
  assert.deepEqual(env.permissionCalls, []);
});

test("a record that changed before forgetting is reread and shown, not forgotten", async () => {
  const changed = "Brauser's record of the adapters it has used (/cfg/adopted-contracts.json) " +
    "was written by a newer version of Brauser.";
  install({
    adapters: [artifactFor("https://work.example")],
    host: {
      forget_adopted_contracts: (_request, state) => {
        state.adoptedState = { issue: changed, revision: "sha256:newer", warnings: [], full: false };
        return { type: "error", code: "conflict", message: "the record of adopted adapters changed" };
      },
    },
  });
  env.adoptedState = { issue: DAMAGED_RECORD, revision: "sha256:damaged", warnings: [], full: false };
  await openSettings();
  const el = (id) => env.elements.get(id);
  el("forget-adopted").click();
  el("forget-adopted-yes").click();
  await until(() => el("adopted-issue").textContent === changed, "the reread record");
  assert.match(statusText(), /changed/);
  assert.equal(el("forget-adopted-confirm").hidden, true);
  assert.equal(el("forget-adopted").hidden, false);
  // A second confirmation uses the revision the user is now shown.
  el("forget-adopted").click();
  el("forget-adopted-yes").click();
  await until(() => env.requests.filter((request) => request.type === "forget_adopted_contracts").length === 2,
    "the second forget");
  assert.equal(env.requests.findLast((request) => request.type === "forget_adopted_contracts").expected_revision,
    "sha256:newer");
});

test("a full record's warning offers the forget repair with the same confirmation", async () => {
  const full = "Brauser's record of the adapters it has used (/cfg/adopted-contracts.json) is full " +
    "(1,024 adapters or 1 MiB), so 1 adapter in use is not recorded. Forget recorded adapters starts a fresh record.";
  install({ adapters: [artifactFor("https://work.example")] });
  env.adoptedState = { issue: null, revision: "sha256:full", warnings: [full], full: true };
  await openSettings();
  const el = (id) => env.elements.get(id);
  assert.equal(el("adopted-issue").hidden, true);
  assert.deepEqual(el("adopted-warnings").children.map((item) => item.textContent), [full]);
  assert.equal(el("forget-adopted").hidden, false);
  el("forget-adopted").click();
  assert.equal(el("forget-adopted-confirm").hidden, false);
  assert.equal(env.requests.filter((request) => request.type === "forget_adopted_contracts").length, 0);
  el("forget-adopted-yes").click();
  await until(() => el("adopted-state").hidden, "the fresh record");
  const forgets = env.requests.filter((request) => request.type === "forget_adopted_contracts");
  assert.equal(forgets.length, 1);
  assert.equal(forgets[0].expected_revision, "sha256:full");
});

test("recording warnings Forget cannot fix are shown as notices without the forget repair", async () => {
  const full = "Brauser's record of the adapters it has used (/cfg/adopted-contracts.json) could not be " +
    "updated: permission denied.";
  install({ adapters: [artifactFor("https://work.example")] });
  env.adoptedState = { issue: null, revision: "sha256:full", warnings: [full], full: false };
  await openSettings();
  const el = (id) => env.elements.get(id);
  assert.equal(el("adopted-state").hidden, false);
  assert.equal(el("adopted-issue").hidden, true);
  assert.deepEqual(el("adopted-warnings").children.map((item) => item.textContent), [full]);
  assert.equal(el("forget-adopted").hidden, true);
  assert.equal(el("new-adapter").disabled, false);
});

// Captured from the real host; the Rust native test verifies these replies
// against their source config.toml so the UI cannot invent a repair shape.
const ADAPTER_REPAIR_CASES = JSON.parse(readFileSync(
  new URL("../../host/tests/fixtures/adapter-repair-responses.json", import.meta.url), "utf8"));

for (const { name, response } of ADAPTER_REPAIR_CASES) {
  test(`host adapter repair reply reaches settings: ${name}`, async () => {
    const { isResponseShape } = await import("../dist/protocol-shape.js");
    assert.equal(isResponseShape(response), true);
    const { validateAdapterDocument } = await import("../dist/adapter-review.js");
    // Receiving a repair document never makes it valid outgoing policy.
    assert.equal(validateAdapterDocument(response.config.adapters) !== null, response.adapter_issue !== null);
    for (const malformed of [null, [], "adapter", 7]) {
      const bad = structuredClone(response);
      bad.config.adapters.artifacts = [malformed];
      assert.equal(isResponseShape(bad), false);
    }
    if (!response.adapter_issue) return;
    const state = install({ host: { get_config: (_request, current) => current.revision === 1
      ? response
      : { ...response, revision: String(current.revision), config: current.config, adapter_issue: null },
    } });
    await openSettings();
    assert.match(statusText(), /adapter configuration is invalid/);
    assert.doesNotMatch(statusText(), /protocol response is invalid|Native host unavailable/);
    assert.equal(env.elements.get("reset-adapters").disabled, false);
    env.elements.get("reset-adapters").click();
    await until(() => state.config.adapters.format === 1 &&
      state.config.adapters.artifacts.length === 0 && idle(), "repair reset");
    assert.match(statusText(), /adapter rules removed/);
  });
}

const ONEDRIVE = "https://cisco-my.sharepoint.com";
const SHARE_RULE = { path: "/(:w:|:x:|:p:)/(r|g)/(sites|personal)/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } };
function staleOneDrive() {
  return {
    type: "sharepoint.doc", origin: ONEDRIVE, path_case: "insensitive",
    match: [
      { path: "/sites/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } },
      { path: "/:w:/r/sites/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } },
      { path: "/personal/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } },
    ],
    id: "sharepoint:{host}/{doc}",
  };
}
const updateButtons = () => env.elements.get("adapter-list").querySelectorAll("button")
  .filter((button) => button.textContent === "Update to current rules");

test("a stale starter adapter offers Update to current rules; an up-to-date one does not", async () => {
  install({ adapters: [staleOneDrive(), artifactFor("https://a.com")] });
  await openSettings();
  assert.equal(updateButtons().length, 1);
  const [card] = env.elements.get("adapter-list").children;
  const actions = card.children.find((child) => child.className === "adapter-actions");
  assert.deepEqual(actions.children.slice(0, 2).map((button) => button.textContent),
    ["Edit and preview", "Update to current rules"]);

  install({ adapters: [{ ...staleOneDrive(), match: [...staleOneDrive().match, SHARE_RULE] }] });
  await openSettings();
  assert.equal(updateButtons().length, 0);
});

const updatedOneDrive = (saved) => {
  const [sites, , personal, ...rest] = saved.match;
  return { ...saved, match: [sites, personal, ...rest, SHARE_RULE] };
};

test("Update to current rules reviews the replaced rule and saves it only on Save", async () => {
  const saved = { ...staleOneDrive(), title: "{title}.docx" };
  const saves = [];
  const state = install({
    adapters: [saved],
    host: { update_config: (request) => { saves.push(request); return null; } },
  });
  await openSettings();
  updateButtons()[0].click();
  assert.equal(env.elements.get("adapter-editor").hidden, false);
  assert.deepEqual(JSON.parse(env.elements.get("adapter-json").value), updatedOneDrive(saved));
  const review = env.elements.get("adapter-editor-review");
  assert.equal(review.hidden, false);
  const text = nodeText(review);
  assert.match(text, /Replaces 1 rule with 1 rule from the current OneDrive for work documents starter:/);
  assert.ok(text.includes("Adds /(:w:|:x:|:p:)/(r|g)/(sites|personal)/{_}/_layouts/15/Doc.aspx · query sourcedoc={doc:uuid}"), text);
  assert.ok(text.includes("Removes /:w:/r/sites/{_}/_layouts/15/Doc.aspx · query sourcedoc={doc:uuid}"), text);
  // The preview starts on the new rule, and that example matches it.
  const example = env.elements.get("adapter-preview-url").value;
  assert.match(example, /^https:\/\/cisco-my\.sharepoint\.com\/:w:\/r\/sites\/example\/_layouts\/15\/Doc\.aspx\?sourcedoc=/);
  assert.match(nodeText(env.elements.get("adapter-preview")), /Example: [^ ]+ — Matches sharepoint\.doc rule 3 · sharepoint:cisco-my\.sharepoint\.com\//);
  assert.equal(saves.length, 0, "opening the review saves nothing");

  await saveOpenAdapter("the starter update");
  assert.equal(saves.length, 1);
  const [adapter] = saves[0].config.adapters.artifacts;
  assert.deepEqual(adapter, updatedOneDrive(saved));
  assert.deepEqual(state.config.adapters.artifacts, [adapter]);
  assert.equal(env.elements.get("adapter-editor").hidden, true);
  assert.equal(updateButtons().length, 0, "an up-to-date adapter offers no update");

  // Editing another adapter afterwards shows no stale review line.
  env.elements.get("adapter-list").querySelectorAll("button")[0].click();
  assert.equal(env.elements.get("adapter-editor-review").hidden, true);
});

test("a starter update the host refuses as a retired-contract change explains itself", async () => {
  install({
    adapters: [staleOneDrive()],
    host: { update_config: () => ({
      type: "error", code: "invalid_config",
      message: "the artifact type or id template for adapter \"x\" on \"y\" cannot change until artifact migrations are available: recorded adapter",
    }) },
  });
  await openSettings();
  updateButtons()[0].click();
  env.elements.get("save-adapter").click();
  await until(() => statusText().startsWith("This update") && idle(), "the refused save");
  assert.equal(statusText(), "This update overlaps an adapter you used before; edit it by hand in the editor.");
  assert.equal(env.elements.get("adapter-editor").hidden, false, "the editor stays open for a hand edit");
});
