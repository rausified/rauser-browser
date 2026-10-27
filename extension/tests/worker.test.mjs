import assert from "node:assert/strict";
import test from "node:test";

const { finishHostPause, withLatestHostState } = await import("../dist/coordination.js");
const { MAX_QUEUE_BYTES, PolicyLeaseCache, localTimestamp } = await import("../dist/model.js");
const { compileDocument } = await import("../dist/adapter-templates.js");
const { storageKey } = await import("../dist/brand.js");
const { isConfig, isResponseShape } = await import("../dist/protocol-shape.js");

// Run after build:extension. The mock copies storage values as Chrome does,
// which catches accidental reliance on mutating an object returned by get().
const values = new Map();
const sessionValues = new Map();
const tabs = new Map();
let tabReads = 0;
let onMessage;
let onCommitted;
let onHistoryStateUpdated;
let onTabUpdated;
let onTabActivated;
let onTabRemoved;
let onTabReplaced;
let onCreatedNavigationTarget;
let onWindowFocusChanged;
let onIdleStateChanged;
let frameGate = null;
let frameEntered = null;
let onAlarm;
let onStartup;
let onInstalled;
const alarms = new Map();
// Every native port the code under test opened, newest last.
const ports = [];
let badgeText = "";

function storageArea(entries) {
  return {
    async get(key) {
      const result = {};
      for (const name of Array.isArray(key) ? key : [key]) {
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
    async setAccessLevel() {},
  };
}

function fakePort() {
  const messageListeners = [];
  const disconnectListeners = [];
  const port = {
    sent: [],
    closed: false,
    postMessage(message) {
      if (port.closed) throw new Error("Attempting to use a disconnected port object");
      port.sent.push(message);
    },
    disconnect() { port.closed = true; },
    onMessage: { addListener(listener) { messageListeners.push(listener); } },
    onDisconnect: { addListener(listener) { disconnectListeners.push(listener); } },
    // Deliver a host frame, as Chrome would.
    reply(message) { for (const listener of messageListeners) listener(message); },
    // The host process exited.
    drop() {
      port.closed = true;
      for (const listener of disconnectListeners) listener();
    },
  };
  ports.push(port);
  return port;
}

globalThis.chrome = {
  action: {
    async setBadgeText({ text }) { badgeText = text; },
    async setBadgeBackgroundColor() {},
  },
  runtime: {
    id: "test-extension",
    getURL: (path) => `chrome-extension://test-extension/${path}`,
    onMessage: { addListener(listener) { onMessage = listener; } },
    onInstalled: { addListener(listener) { onInstalled = listener; } },
    onStartup: { addListener(listener) { onStartup = listener; } },
    connectNative: () => fakePort(),
  },
  alarms: {
    async create(name, info) { alarms.set(name, { name, ...info }); },
    async get(name) { return alarms.get(name); },
    onAlarm: { addListener(listener) { onAlarm = listener; } },
  },
  contextMenus: {
    create() {},
    removeAll(callback) { callback?.(); },
    onClicked: { addListener() {} },
  },
  storage: {
    local: storageArea(values),
    session: storageArea(sessionValues),
  },
  // Chrome grants every HTTP(S) site at install. The worker never asks
  // Chrome about a permission and never listens for permission changes.
  permissions: {
    async contains() { throw new Error("the worker must not check Chrome permissions"); },
  },
  tabs: {
    async get(tabId) {
      tabReads += 1;
      if (!tabs.has(tabId)) throw new Error("No tab");
      return structuredClone(tabs.get(tabId));
    },
    onUpdated: { addListener(listener) { onTabUpdated = listener; } },
    onActivated: { addListener(listener) { onTabActivated = listener; } },
    onRemoved: { addListener(listener) { onTabRemoved = listener; } },
    onReplaced: { addListener(listener) { onTabReplaced = listener; } },
    async query() { return [...tabs.values()].map((tab) => structuredClone(tab)); },
  },
  webNavigation: {
    onCommitted: { addListener(listener) { onCommitted = listener; } },
    onHistoryStateUpdated: { addListener(listener) { onHistoryStateUpdated = listener; } },
    onCreatedNavigationTarget: { addListener(listener) { onCreatedNavigationTarget = listener; } },
    async getFrame({ tabId }) {
      const tab = tabs.get(tabId);
      const result = tab?.url ? { url: tab.url } : null;
      frameEntered?.();
      if (frameGate) await frameGate;
      return result;
    },
  },
  windows: {
    WINDOW_ID_NONE: -1,
    async getLastFocused() { return { id: 1, focused: true }; },
    onFocusChanged: { addListener(listener) { onWindowFocusChanged = listener; } },
  },
  idle: {
    setDetectionInterval() {},
    async queryState() { return "active"; },
    onStateChanged: { addListener(listener) { onIdleStateChanged = listener; } },
  },
  sidePanel: { async setPanelBehavior() {} },
};

const { queueActivityMapRebuild, rebuildActivityMap } = await import("../dist/worker.js");
const { HostClient, HostError, PROTOCOL_VERSION } = await import("../dist/native.js");
const { ConfigSession } = await import("../dist/settings.js");

function send(message) {
  return new Promise((resolve, reject) => {
    const accepted = onMessage(message, {
      id: chrome.runtime.id,
      url: chrome.runtime.getURL("panel.html"),
    }, (reply) => {
      if (reply.ok) resolve(reply.value);
      else reject(new Error(reply.error));
    });
    if (!accepted) reject(new Error("Worker did not accept the panel message"));
  });
}

// Listener work is serialized with panel messages, so a cheap message that
// performs no permission check waits for earlier navigation work to finish.
async function flush() {
  await send({ kind: "get_pending_ids" });
}

function artifact(origin, extra = {}) {
  return {
    type: "test.item", origin,
    match: [{ path: "/items/{id:token}" }],
    id: "test:{host}/{id}",
    ...extra,
  };
}

function lease(origin) {
  return {
    revision: `host-revision:${origin}`,
    expires_at: Date.now() + 60_000,
    capture_enabled: true,
    sites: [{ origin, path_prefix: "/" }],
    adapters: { format: 1, artifacts: [artifact(origin)] },
  };
}

test("a restored policy compiles once per revision while every install validates", () => {
  let compilations = 0;
  const policies = new PolicyLeaseCache((adapters) => {
    compilations += 1;
    return compileDocument(adapters);
  });
  const first = lease("https://compiled.example");

  assert.equal(policies.restore(first), first);
  assert.ok(policies.compiledFor(first));
  assert.equal(policies.restore(structuredClone(first)).revision, first.revision);
  assert.equal(compilations, 1);

  const next = { ...structuredClone(first), revision: "next-revision" };
  assert.equal(policies.restore(next), next);
  assert.equal(compilations, 2);

  assert.equal(policies.install(structuredClone(next)).revision, next.revision);
  assert.equal(compilations, 3);
  const invalid = structuredClone(next);
  invalid.adapters.artifacts[0].match = [{ path: "/items/{id:missing}" }];
  assert.equal(policies.install(invalid), null);
  assert.equal(compilations, 4);
});

function visit(id, url) {
  return {
    event: {
      event_id: id,
      url,
      title: null,
      occurred_at: "2026-09-28T00:00:00Z",
      incognito: false,
    },
    dedupe_key: id,
    attempts: 0,
    retry_after: 0,
  };
}

function activityVisit(id, url, extra = {}) {
  return {
    event: {
      event_id: id,
      artifact_id: "test:pressure.example/shared",
      artifact_type: "test.item",
      url,
      title: null,
      occurred_at: "2026-10-15T10:00:00+00:00",
      incognito: false,
      opens: 1,
      focused_seconds: 0,
      background_seconds: 0,
      modes: [],
      trails: [],
      refs: [],
      ...extra,
    },
    dedupe_key: id,
    attempts: 0,
    retry_after: 0,
    dispatched: false,
  };
}

test("config repair suspends capture without losing visits, and discard drops only the chosen ones", async () => {
  values.clear();
  sessionValues.clear();
  const firstOrigin = "https://first.example";
  const secondOrigin = "https://second.example";
  values.set(storageKey("queue_v1"), {
    version: 1,
    items: [visit("first", `${firstOrigin}/items/a`), visit("second", `${secondOrigin}/items/b`)],
    overflow_count: 3,
    rejected_count: 2,
    last_error: null,
  });
  values.set(storageKey("policy_v1"), {
    revision: "host-revision",
    expires_at: Date.now() + 60_000,
    capture_enabled: true,
    sites: [
      { origin: firstOrigin, path_prefix: "/" },
      { origin: secondOrigin, path_prefix: "/" },
    ],
    adapters: { format: 1, artifacts: [artifact(firstOrigin), artifact(secondOrigin)] },
  });

  const suspended = await send({ kind: "suspend_policy" });
  assert.equal(suspended.queued, 2);
  assert.equal(suspended.policy_expires_at, null);
  assert.equal(values.get(storageKey("policy_v1")), null);
  assert.deepEqual(await send({ kind: "get_pending" }), []);

  const kept = await send({ kind: "get_status" });
  assert.equal(kept.queued, 2);
  assert.equal(values.get(storageKey("queue_v1")).items.length, 2);

  const ids = await send({ kind: "get_pending_ids" });
  assert.deepEqual(ids, ["first", "second"]);
  const nextQueue = values.get(storageKey("queue_v1"));
  nextQueue.items.push(visit("later", `${secondOrigin}/items/later`));
  values.set(storageKey("queue_v1"), nextQueue);
  const discarded = await send({ kind: "discard_pending", event_ids: ids });
  assert.equal(discarded.queued, 1);
  assert.equal(discarded.overflow_count, 3);
  assert.equal(discarded.rejected_count, 2);
  assert.equal(values.get(storageKey("queue_v1")).items[0].event.event_id, "later");
});

test("a newer local pause cannot be resumed by an older enable confirmation", async () => {
  values.clear();
  sessionValues.clear();
  const candidateLease = lease("https://first.example");

  const previous = await send({ kind: "get_status" });
  const paused = await send({ kind: "pause_capture" });
  assert.equal(paused.pause_pending, true);
  assert.notEqual(paused.pause_token, previous.pause_token);

  const stale = await send({
    kind: "install_policy",
    lease: candidateLease,
    resume_after_confirmation: true,
    resume_after_pause_token: previous.pause_token,
  });
  assert.equal(stale.pause_pending, true);
  assert.equal(stale.policy_expires_at, null);

  const resumed = await send({
    kind: "install_policy",
    lease: candidateLease,
    resume_after_confirmation: true,
    resume_after_pause_token: paused.pause_token,
  });
  assert.equal(resumed.pause_pending, false);
  assert.equal(resumed.policy_expires_at, candidateLease.expires_at);
});

test("a delayed lease refresh reads the host after a site removal commits", async () => {
  let tail = Promise.resolve();
  const lock = async (action) => {
    const prior = tail;
    let release;
    tail = new Promise((resolve) => { release = resolve; });
    await prior;
    try { return await action(); }
    finally { release(); }
  };
  let host = { revision: "r1", sites: ["https://removed.example"] };
  let enteredRemoval;
  const removalStarted = new Promise((resolve) => { enteredRemoval = resolve; });
  let releaseRemoval;
  const removalGate = new Promise((resolve) => { releaseRemoval = resolve; });
  const removing = lock(async () => {
    host = { revision: "r2", sites: [] };
    enteredRemoval();
    await removalGate;
  });
  await removalStarted;
  const installed = [];
  const refreshing = withLatestHostState(
    lock,
    async () => structuredClone(host),
    async (state) => { installed.push(state); },
  );
  releaseRemoval();
  await Promise.all([removing, refreshing]);
  assert.deepEqual(installed, [{ revision: "r2", sites: [] }]);
});

test("pause re-reads a conflicting host revision before clearing the local block", async () => {
  let host = { revision: "r2", config: { capture_enabled: true }, config_issue: null };
  let localPauses = 0;
  let saves = 0;
  let installedDisabled = 0;
  await finishHostPause(
    async () => structuredClone(host),
    async () => { localPauses += 1; },
    async (snapshot) => {
      saves += 1;
      if (saves === 1) {
        host = { ...host, revision: "r3" };
        throw new Error("conflict");
      }
      assert.equal(snapshot.revision, "r3");
      host = { ...snapshot, config: { capture_enabled: false } };
    },
    async () => { installedDisabled += 1; },
    (error) => error instanceof Error && error.message === "conflict",
  );
  assert.equal(localPauses, 2);
  assert.equal(saves, 2);
  assert.equal(installedDisabled, 0);
  assert.equal(host.config.capture_enabled, false);
});

test("pause can disable capture while adapter policy is invalid", async () => {
  const host = {
    revision: "r2", config: { capture_enabled: true }, config_issue: null,
    adapter_issue: "unsupported adapter format 2",
  };
  let saved = false;
  await finishHostPause(
    async () => structuredClone(host),
    async () => undefined,
    async (snapshot) => {
      assert.equal(snapshot.adapter_issue, "unsupported adapter format 2");
      saved = true;
    },
    async () => assert.fail("enabled capture must be saved as disabled"),
    () => false,
  );
  assert.equal(saved, true);
});

test("a matching adapter is captured without any permission check", async () => {
  values.clear();
  sessionValues.clear();
  tabs.clear();
  const origin = "https://no-check.example";
  chrome.permissions.contains = () => { throw new Error("no permission checks"); };
  values.set(storageKey("policy_v1"), lease(origin));
  tabs.set(41, { id: 41, url: `${origin}/items/open`, title: "Open", incognito: false });
  onCommitted({
    tabId: 41, frameId: 0, documentId: "doc-no-check", documentLifecycle: "active",
    url: `${origin}/items/open`, timeStamp: Date.now() - 2 * 60 * 60_000,
  });
  await flush();
  assert.equal((await send({ kind: "get_status" })).queued, 1);
});

test("a capture-disabled lease clears the queue and blocks capture", async () => {
  values.clear();
  sessionValues.clear();
  tabs.clear();
  const origin = "https://disabled.example";
  values.set(storageKey("policy_v1"), { ...lease(origin), capture_enabled: false });
  values.set(storageKey("queue_v1"), {
    version: 1, items: [visit("old", `${origin}/items/old`)],
    overflow_count: 0, rejected_count: 0, last_error: null,
  });
  const status = await send({ kind: "get_status" });
  assert.equal(status.queued, 0);
  tabs.set(42, { id: 42, url: `${origin}/items/new`, title: "New", incognito: false });
  onCommitted({
    tabId: 42, frameId: 0, documentId: "doc-disabled", documentLifecycle: "active",
    url: `${origin}/items/new`, timeStamp: Date.now(),
  });
  await flush();
  assert.equal((await send({ kind: "get_activity" })).length, 0);
  assert.equal((await send({ kind: "get_status" })).queued, 0);
});

test("worker status carries no revocation fields", async () => {
  values.clear();
  sessionValues.clear();
  const s = await send({ kind: "get_status" });
  for (const k of ["revoked_origins", "locally_removed_sites", "navigation_ready"]) assert.equal(k in s, false);
  assert.deepEqual(s.pending_removals, []);
});

test("retired revocation requests are refused", async () => {
  for (const kind of ["remove_site", "ack_reenabled_origin", "ack_revocations"]) {
    await assert.rejects(send({ kind, origin: "https://a.example", origins: [], revision: "r1" }),
      /Unknown worker request/);
  }
});

test("pending local removals survive upgrade until the host drops those adapters", async () => {
  values.clear();
  sessionValues.clear();
  tabs.clear();
  const removedOrigin = "https://a.example";
  const revokedOrigin = "https://b.example";
  const keptOrigin = "https://c.example";
  values.set(storageKey("locally_removed_sites_v1"), [{ origin: removedOrigin, path_prefix: "/" }]);
  values.set(storageKey("revocations_v1"), [revokedOrigin, removedOrigin]);
  values.set(storageKey("note_origins_v1"), [removedOrigin]);
  values.set(storageKey("policy_v1"), {
    ...lease(removedOrigin),
    sites: [{ origin: removedOrigin, path_prefix: "/" }, { origin: keptOrigin, path_prefix: "/" }],
    adapters: { format: 1, artifacts: [artifact(removedOrigin), artifact(keptOrigin)] },
  });

  onInstalled({ reason: "update" });
  await flush();
  assert.equal(values.has(storageKey("note_origins_v1")), false);
  assert.equal(values.has(storageKey("locally_removed_sites_v1")), false);
  assert.equal(values.has(storageKey("revocations_v1")), false);
  assert.deepEqual(values.get(storageKey("pending_site_removals_v1")), [removedOrigin, revokedOrigin]);

  tabs.set(43, { id: 43, url: `${removedOrigin}/items/blocked`, title: "Blocked", incognito: false });
  onCommitted({
    tabId: 43, frameId: 0, documentId: "doc-pending", documentLifecycle: "active",
    url: `${removedOrigin}/items/blocked`, timeStamp: Date.now() - 2 * 60 * 60_000,
  });
  await flush();
  const blocked = await send({ kind: "get_status" });
  assert.equal(blocked.queued, 0);
  assert.equal(blocked.live_activity, 0);
  assert.deepEqual(blocked.pending_removals, [removedOrigin, revokedOrigin]);

  // Unrelated adapters in the same lease keep capturing.
  tabs.set(44, { id: 44, url: `${keptOrigin}/items/kept`, title: "Kept", incognito: false });
  onCommitted({
    tabId: 44, frameId: 0, documentId: "doc-kept", documentLifecycle: "active",
    url: `${keptOrigin}/items/kept`, timeStamp: Date.now() - 2 * 60 * 60_000,
  });
  await flush();
  assert.equal((await send({ kind: "get_status" })).queued, 1);

  await send({ kind: "clear_pending_removals" });
  assert.deepEqual(values.get(storageKey("pending_site_removals_v1")), [removedOrigin, revokedOrigin]);

  // An earlier build narrowed its stored lease locally, so a stored lease the
  // host never sent proves nothing about the host config.
  values.set(storageKey("policy_v1"), { ...lease(keptOrigin), revision: "narrowed-locally" });
  await send({ kind: "clear_pending_removals" });
  assert.deepEqual(values.get(storageKey("pending_site_removals_v1")), [removedOrigin, revokedOrigin]);

  // An install of the old lease still holds the removed origin back.
  await send({
    kind: "install_policy", lease: { ...lease(removedOrigin), revision: "still-there" },
    resume_after_confirmation: false, resume_after_pause_token: null,
  });
  await send({ kind: "clear_pending_removals" });
  assert.deepEqual(values.get(storageKey("pending_site_removals_v1")), [removedOrigin, revokedOrigin]);
  assert.equal(values.get(storageKey("policy_v1")).adapters.artifacts.length, 1,
    "the received lease is stored as the host sent it");

  await send({
    kind: "install_policy", lease: lease(keptOrigin),
    resume_after_confirmation: false, resume_after_pause_token: null,
  });
  const cleared = await send({ kind: "clear_pending_removals" });
  assert.equal(values.has(storageKey("pending_site_removals_v1")), false);
  assert.deepEqual(cleared.pending_removals, []);
});

test("a committed page updates the live hourly activity title before freezing", async () => {
  values.clear();
  sessionValues.clear();
  tabs.clear();
  const origin = "https://title.example";
  values.set(storageKey("policy_v1"), lease(origin));
  tabs.set(7, { id: 7, url: `${origin}/items/a`, title: "Previous page", incognito: false });

  onCommitted({
    tabId: 7, frameId: 0, documentId: "doc-1", documentLifecycle: "active",
    url: `${origin}/items/a`, timeStamp: Date.now(),
  });
  await flush();
  let [live] = await send({ kind: "get_activity" });
  assert.equal(live.title, null);
  assert.equal(live.opens, 1);
  assert.match(live.hour_start, /^\d{4}-\d{2}-\d{2}T\d{2}:00:00[+-]\d{2}:\d{2}$/);
  assert.equal(values.get(storageKey("queue_v1")), undefined,
    "the current hour is not delivered before it is complete");

  onTabUpdated(7, { title: "x".repeat(400) }, { id: 7, url: `${origin}/items/a#part`, incognito: false });
  await flush();
  [live] = await send({ kind: "get_activity" });
  assert.equal(live.title, "x".repeat(300));

  tabs.set(8, { id: 8, url: `${origin}/items/a`, title: "Other tab", incognito: false });
  onTabUpdated(8, { title: "Other tab" }, { id: 8, url: `${origin}/items/a`, incognito: false });
  await flush();
  [live] = await send({ kind: "get_activity" });
  assert.equal(live.title, "x".repeat(300));
});

test("a stored visit without a dispatch flag is never patched", async () => {
  values.clear();
  sessionValues.clear();
  const origin = "https://legacy.example";
  values.set(storageKey("policy_v1"), lease(origin));
  values.set(storageKey("queue_v1"), {
    version: 1,
    items: [{ ...visit("legacy", `${origin}/items/a`), tab_id: 3 }],
    overflow_count: 0,
    rejected_count: 0,
    last_error: null,
  });
  onTabUpdated(3, { title: "Late title" }, { id: 3, url: `${origin}/items/a`, incognito: false });
  await flush();
  assert.equal(values.get(storageKey("queue_v1")).items[0].event.title, null);
  const status = await send({ kind: "get_status" });
  assert.equal(status.retry_error, null);
});

test("a navigation outside every enabled site skips tab reads", async () => {
  values.clear();
  sessionValues.clear();
  tabs.clear();
  values.set(storageKey("policy_v1"), lease("https://enabled.example"));
  tabs.set(4, { id: 4, url: "https://other.example/items/x", title: "Other", incognito: false });
  const reads = tabReads;
  onCommitted({
    tabId: 4, frameId: 0, documentId: "doc-2", documentLifecycle: "active",
    url: "https://other.example/items/x", timeStamp: Date.now(),
  });
  await flush();
  assert.equal(tabReads, reads);
  assert.equal(values.get(storageKey("queue_v1")), undefined);

  tabs.set(4, { id: 4, url: "https://enabled.example/items/y", title: "Old", incognito: false });
  onCommitted({
    tabId: 4, frameId: 0, documentId: "doc-3", documentLifecycle: "active",
    url: "https://enabled.example/items/y", timeStamp: Date.now(),
  });
  await flush();
  assert.equal((await send({ kind: "get_activity" })).length, 1);
});

test("matching navigations never serialize the adapter document", async () => {
  values.clear();
  sessionValues.clear();
  tabs.clear();
  const origin = "https://cached-policy.example";
  values.set(storageKey("policy_v1"), lease(origin));
  tabs.set(31, { id: 31, url: `${origin}/items/first`, title: "First", incognito: false });
  tabs.set(32, { id: 32, url: `${origin}/items/second`, title: "Second", incognito: false });

  const stringify = JSON.stringify;
  let adapterSerializations = 0;
  JSON.stringify = function (value, replacer, space) {
    if (value && typeof value === "object" && value.format === 1 && Array.isArray(value.artifacts)) {
      adapterSerializations += 1;
    }
    return stringify.call(JSON, value, replacer, space);
  };
  try {
    onCommitted({
      tabId: 31, frameId: 0, documentId: "doc-cache-1", documentLifecycle: "active",
      url: `${origin}/items/first`, timeStamp: Date.now(),
    });
    await flush();
    onCommitted({
      tabId: 32, frameId: 0, documentId: "doc-cache-2", documentLifecycle: "active",
      url: `${origin}/items/second`, timeStamp: Date.now(),
    });
    await flush();
  } finally {
    JSON.stringify = stringify;
  }

  assert.equal(adapterSerializations, 0);
  assert.equal((await send({ kind: "get_activity" })).length, 2);
});

test("an expired policy install cannot replace the active revision's compiled adapter", async () => {
  values.clear();
  sessionValues.clear();
  tabs.clear();
  const origin = "https://expired-install.example";
  const active = lease(origin);
  values.set(storageKey("policy_v1"), active);
  await send({ kind: "get_status" });

  const expired = {
    ...structuredClone(active),
    expires_at: Date.now() - 1,
    adapters: {
      format: 1,
      artifacts: [artifact(origin, { match: [{ path: "/private/{id:token}" }] })],
    },
  };
  await assert.rejects(send({
    kind: "install_policy",
    lease: expired,
    resume_after_confirmation: false,
    resume_after_pause_token: null,
  }), /invalid or expired/);

  tabs.set(33, { id: 33, url: `${origin}/private/secret`, title: "Private", incognito: false });
  onCommitted({
    tabId: 33, frameId: 0, documentId: "doc-expired-install", documentLifecycle: "active",
    url: `${origin}/private/secret`, timeStamp: Date.now(),
  });
  await flush();

  assert.equal((await send({ kind: "get_activity" })).length, 0);
  assert.deepEqual(values.get(storageKey("policy_v1")), active);
});

test("adapter rules and scope fail closed before buffering", async () => {
  values.clear();
  sessionValues.clear();
  tabs.clear();
  const origin = "https://scoped.example";
  values.set(storageKey("policy_v1"), {
    ...lease(origin),
    adapters: {
      format: 1,
      artifacts: [{
        type: "test.item", origin,
        match: [{ path: "/teams/{owner:name|lower}/items/{id:token}" }],
        id: "test:{host}/{owner}/{id}",
        scope: { owner: ["acme"] },
      }],
    },
  });
  const initialReads = tabReads;

  tabs.set(20, { id: 20, url: `${origin}/search`, title: "Search", incognito: false });
  onCommitted({
    tabId: 20, frameId: 0, documentId: "not-artifact", documentLifecycle: "active",
    url: `${origin}/search`, timeStamp: Date.now(),
  });
  tabs.set(21, { id: 21, url: `${origin}/teams/personal/items/7`, title: "Personal", incognito: false });
  onCommitted({
    tabId: 21, frameId: 0, documentId: "out-of-scope", documentLifecycle: "active",
    url: `${origin}/teams/personal/items/7`, timeStamp: Date.now(),
  });
  await flush();
  assert.equal(tabReads, initialReads, "unrecognized URLs never expose tab metadata");
  assert.equal(values.get(storageKey("queue_v1")), undefined);
  assert.equal((await send({ kind: "get_activity" })).length, 0);

  tabs.set(22, { id: 22, url: `${origin}/teams/acme/items/7`, title: "Work", incognito: false });
  onCommitted({
    tabId: 22, frameId: 0, documentId: "recognized", documentLifecycle: "active",
    url: `${origin}/teams/acme/items/7`, timeStamp: Date.now(),
  });
  await flush();
  assert.equal((await send({ kind: "get_activity" })).length, 1);
});

test("worker-start map rebuild restores recognized tabs without counting opens", async () => {
  values.clear();
  sessionValues.clear();
  tabs.clear();
  const origin = "https://rebuild.example";
  values.set(storageKey("policy_v1"), lease(origin));
  tabs.set(30, {
    id: 30, windowId: 1, active: true, status: "loading",
    url: `${origin}/items/restored`, title: "Restored", incognito: false,
  });

  await rebuildActivityMap();
  const session = sessionValues.get(storageKey("activity_session_v1"));
  assert.equal(session.tabs[30].artifact_id, "test:rebuild.example/restored");
  assert.equal(session.active_by_window[1], 30);
  assert.deepEqual(values.get(storageKey("activity_totals_v1")).totals, [],
    "rebuild maps attention but is not a cold open");

  tabs.get(30).url = `${origin}/items/different`;
  await rebuildActivityMap();
  assert.equal(sessionValues.get(storageKey("activity_session_v1")).tabs[30].artifact_id,
    "test:rebuild.example/restored", "an existing map entry is not overwritten");

  sessionValues.clear();
  tabs.get(30).url = `${origin}/items/restored`;
  let releaseFrame;
  frameGate = new Promise((resolve) => { releaseFrame = resolve; });
  const entered = new Promise((resolve) => { frameEntered = resolve; });
  const rebuilding = queueActivityMapRebuild();
  await entered;
  tabs.get(30).url = `${origin}/items/committed`;
  onCommitted({
    tabId: 30, frameId: 0, documentId: "commit-during-rebuild", documentLifecycle: "active",
    url: `${origin}/items/committed`, timeStamp: Date.now(), transitionType: "link",
  });
  releaseFrame();
  frameGate = null;
  frameEntered = null;
  await rebuilding;
  await flush();
  assert.equal(sessionValues.get(storageKey("activity_session_v1")).tabs[30].artifact_id,
    "test:rebuild.example/committed", "a queued commit overwrites the rebuilt entry");

  values.set(storageKey("pending_site_removals_v1"), [origin]);
  sessionValues.clear();
  await rebuildActivityMap();
  assert.deepEqual(sessionValues.get(storageKey("activity_session_v1"))?.tabs ?? {}, {},
    "an origin with a pending site removal is never mapped");
});

test("worker-start map rebuild settles activity restored from storage.session", async () => {
  values.clear();
  sessionValues.clear();
  tabs.clear();
  const origin = "https://worker-wake.example";
  const start = new Date("2026-10-16T10:15:00Z").getTime();
  const now = start + 15_000;
  values.set(storageKey("policy_v1"), {
    ...lease(origin), expires_at: now + 60_000,
  });
  values.set(storageKey("activity_totals_v1"), {
    version: 1,
    totals: [{
      event_id: "00000000-0000-4000-8000-000000000001",
      artifact_id: "test:worker-wake.example/a", artifact_type: "test.item",
      // Totals are keyed by local hour, so derive it rather than hardcoding UTC.
      hour_start: `${localTimestamp(new Date(start)).slice(0, 13)}:00:00${localTimestamp(new Date(start)).slice(19)}`,
      url: `${origin}/items/a`, title: "A",
      opens: 1, focused_ms: 0, background_ms: 0, modes: [], trails: [], refs: [],
    }],
  });
  sessionValues.set(storageKey("activity_session_v1"), {
    version: 1,
    tabs: {
      30: {
        artifact_id: "test:worker-wake.example/a", artifact_type: "test.item",
        url: `${origin}/items/a`, title: "A", mode: null, window_id: 1,
      },
    },
    pending_sources: {}, active_by_window: { 1: 30 }, last_focused_window: 1,
    chrome_focused: true, idle: false, measured_at: start,
  });
  tabs.set(30, {
    id: 30, windowId: 1, active: true, status: "complete",
    url: `${origin}/items/a`, title: "A", incognito: false,
  });

  const realNow = Date.now;
  Date.now = () => now;
  try {
    await rebuildActivityMap();
  } finally {
    Date.now = realNow;
  }

  const [total] = values.get(storageKey("activity_totals_v1")).totals;
  assert.equal(total.focused_ms, 15_000,
    "ordinary service-worker suspension must not discard restored dwell time");
  assert.equal(sessionValues.get(storageKey("activity_session_v1")).measured_at, now);
});

test("a successful ack clears retry state but keeps notices until dismissed", async () => {
  values.clear();
  sessionValues.clear();
  const origin = "https://ack.example";
  values.set(storageKey("policy_v1"), lease(origin));
  values.set(storageKey("queue_v1"), {
    version: 1,
    items: [visit("one", `${origin}/items/1`), visit("two", `${origin}/items/2`)],
    overflow_count: 2,
    rejected_count: 1,
    last_error: "Visit queue is full; newer visits were not buffered",
  });

  const retrying = await send({
    kind: "ack_visit", event_id: "one", outcome: "retryable", reason: "Host busy",
  });
  assert.equal(retrying.retry_error, "Host busy");
  assert.equal(retrying.last_error, "Visit queue is full; newer visits were not buffered");

  const partial = await send({ kind: "ack_visit", event_id: "two", outcome: "persisted", reason: null });
  assert.equal(partial.retry_error, "Host busy");

  const saved = await send({ kind: "ack_visit", event_id: "one", outcome: "persisted", reason: null });
  assert.equal(saved.retry_error, null);
  assert.equal(saved.last_error, "Visit queue is full; newer visits were not buffered");
  assert.equal(saved.overflow_count, 2);
  assert.equal(saved.rejected_count, 1);

  const cleared = await send({ kind: "clear_notices" });
  assert.equal(cleared.last_error, null);
  assert.equal(cleared.overflow_count, 0);
  assert.equal(cleared.rejected_count, 0);
  assert.equal(values.get(storageKey("queue_v1")).last_error, null);
});

test("visit timestamps use local time with a numeric UTC offset", () => {
  const previous = process.env.TZ;
  const instant = new Date("2026-09-28T03:04:05.678Z");
  try {
    process.env.TZ = "UTC";
    assert.equal(localTimestamp(instant), "2026-09-28T03:04:05+00:00");
    process.env.TZ = "America/New_York";
    assert.equal(localTimestamp(instant), "2026-09-27T23:04:05-04:00");
    process.env.TZ = "Asia/Kolkata";
    assert.equal(localTimestamp(instant), "2026-09-28T08:34:05+05:30");
    assert.equal(localTimestamp(new Date("0999-01-02T00:00:00Z")).length, 25);
  } finally {
    if (previous === undefined) delete process.env.TZ;
    else process.env.TZ = previous;
  }
});

test("worker accepts messages only from the side panel and settings page", async () => {
  for (const page of ["panel.html", "options.html"]) {
    const reply = await new Promise((resolve) => {
      const accepted = onMessage({ kind: "get_status" }, {
        id: chrome.runtime.id, url: chrome.runtime.getURL(page),
      }, resolve);
      assert.equal(accepted, true, page);
    });
    assert.equal(reply.ok, true, page);
  }
  for (const sender of [
    { id: chrome.runtime.id, url: chrome.runtime.getURL("other.html") },
    { id: chrome.runtime.id, url: "https://example.com/options.html" },
    { id: "another-extension", url: "chrome-extension://another-extension/options.html" },
    { id: chrome.runtime.id },
  ]) {
    let responded = false;
    const accepted = onMessage({ kind: "get_status" }, sender, () => { responded = true; });
    assert.equal(accepted, false, JSON.stringify(sender));
    assert.equal(responded, false, JSON.stringify(sender));
  }
});

test("an SPA route change accepts the title update into its live hour", async () => {
  values.clear();
  sessionValues.clear();
  tabs.clear();
  const origin = "https://spa.example";
  values.set(storageKey("policy_v1"), lease(origin));
  // pushState has run, but the app has not set the new route's title yet.
  tabs.set(9, { id: 9, url: `${origin}/items/b`, title: "Route A", incognito: false });

  onHistoryStateUpdated({
    tabId: 9, frameId: 0, documentId: "doc-spa", documentLifecycle: "active",
    url: `${origin}/items/b`, timeStamp: Date.now(),
  });
  await flush();
  let [live] = await send({ kind: "get_activity" });
  assert.equal(live.title, "Route A");
  assert.deepEqual(await send({ kind: "get_pending" }), []);

  onTabUpdated(9, { title: "Route B" }, { id: 9, url: `${origin}/items/b`, incognito: false });
  await flush();
  [live] = await send({ kind: "get_activity" });
  assert.equal(live.title, "Route B");

  // Some SPAs set document.title before pushState. Chrome's title event can
  // still carry the prior URL; it must not retitle that prior artifact, while
  // the following history event may take the already-current tab title.
  tabs.set(9, { id: 9, windowId: 1, url: `${origin}/items/c`, title: "Route C", incognito: false });
  onTabUpdated(9, { title: "Route C" }, { id: 9, url: `${origin}/items/b`, incognito: false });
  onHistoryStateUpdated({
    tabId: 9, frameId: 0, documentId: "doc-spa", documentLifecycle: "active",
    url: `${origin}/items/c`, timeStamp: Date.now(), transitionType: "link",
  });
  await flush();
  const totals = await send({ kind: "get_activity" });
  assert.equal(totals.find((total) => total.artifact_id.endsWith("/b")).title, "Route B");
  assert.equal(totals.find((total) => total.artifact_id.endsWith("/c")).title, "Route C");
});

test("a retry notice clears once nothing is left to retry, and on dismissal", async () => {
  values.clear();
  sessionValues.clear();
  const origin = "https://retry.example";
  const setUp = (ids) => {
    values.set(storageKey("policy_v1"), lease(origin));
    values.set(storageKey("queue_v1"), {
      version: 1,
      items: ids.map((id) => visit(id, `${origin}/items/${id}`)),
      overflow_count: 0,
      rejected_count: 0,
      last_error: null,
    });
  };
  const retry = (id) => send({ kind: "ack_visit", event_id: id, outcome: "retryable", reason: "Host busy" });

  setUp(["one"]);
  assert.equal((await retry("one")).retry_error, "Host busy");
  const rejected = await send({ kind: "ack_visit", event_id: "one", outcome: "rejected", reason: "No" });
  assert.equal(rejected.queued, 0);
  assert.equal(rejected.retry_error, null);

  setUp(["two"]);
  await retry("two");
  assert.equal((await send({ kind: "discard_pending", event_ids: ["two"] })).retry_error, null);

  setUp(["four"]);
  await retry("four");
  assert.equal((await send({ kind: "pause_capture" })).retry_error, null);
  values.delete(storageKey("pause_pending_v1"));

  setUp(["five"]);
  await retry("five");
  const dismissed = await send({ kind: "clear_notices" });
  assert.equal(dismissed.queued, 1);
  assert.equal(dismissed.retry_error, null);
});

function helloResult(requestId) {
  return {
    type: "hello_result", protocol_version: PROTOCOL_VERSION, request_id: requestId,
    host_version: "0.0.0", configured: true, config_issue: null,
  };
}

function hello() {
  return {
    type: "hello", protocol_version: PROTOCOL_VERSION, request_id: crypto.randomUUID(),
  };
}

test("a host client reconnects lazily after the host exits", async () => {
  ports.length = 0;
  const client = new HostClient();
  let changes = 0;
  client.onStateChange(() => { changes += 1; });
  const first = ports[0];
  const answered = client.call(hello(), "hello_result");
  first.reply(helloResult(first.sent[0].request_id));
  await answered;
  assert.equal(client.connected, true);

  const inFlight = client.call(hello(), "hello_result");
  first.drop();
  await assert.rejects(inFlight, (error) => error instanceof HostError && error.code === "disconnected");
  // The next call starts a fresh host, so the page is still usable.
  assert.equal(client.connected, true);

  const retried = client.call(hello(), "hello_result");
  assert.equal(ports.length, 2);
  const second = ports[1];
  // A late frame on the dead port must not settle the new request.
  first.reply({ ...helloResult(second.sent[0].request_id), host_version: "stale" });
  second.reply(helloResult(second.sent[0].request_id));
  assert.equal((await retried).host_version, "0.0.0");
  assert.equal(client.connected, true);
  assert.ok(changes >= 1);

  client.disconnect();
  assert.equal(client.connected, false);
  await assert.rejects(client.call(hello(), "hello_result"),
    (error) => error instanceof HostError && error.code === "disconnected");
  assert.equal(ports.length, 2, "an explicitly closed client stays closed");
});

test("a host that cannot be started reports the session disconnected", async () => {
  ports.length = 0;
  let changes = 0;
  const session = new ConfigSession(() => { changes += 1; });
  session.config = {
    storage: null, capture_enabled: false, sites: [], strip_params: [],
  };
  session.revision = "r1";
  assert.equal(session.connected, true);

  // The host was removed, so Chrome drops the port before any reply.
  const pending = session.hello();
  ports[0].drop();
  await assert.rejects(pending, (error) => error instanceof HostError && error.code === "disconnected");
  assert.equal(session.connected, false);
  assert.ok(changes >= 1, "the page is told to redraw its controls");

  // Once the host is back, the next call reconnects and the session recovers.
  const recovered = session.hello();
  const port = ports.at(-1);
  port.reply(helloResult(port.sent[0].request_id));
  await recovered;
  assert.equal(session.connected, true);
  session.host.disconnect();
});

test("a post-save folder issue suspends policy instead of installing a lease", async () => {
  values.clear();
  sessionValues.clear();
  ports.length = 0;
  const origin = "https://changed-folder.example";
  const saved = configResult({ request_id: "fixture" }, origin).config;
  const session = new ConfigSession(() => undefined);
  session.config = structuredClone(saved);
  session.revision = "r1";
  const previousSendMessage = chrome.runtime.sendMessage;
  chrome.runtime.sendMessage = async (request) => ({ ok: true, value: await send(request) });

  try {
    const saving = session.saveConfig(structuredClone(saved), null);
    const port = ports[0];
    let request = await sentOn(port, 1);
    assert.equal(request.type, "update_config");
    assert.equal("consent_token" in request, false);
    port.reply({
      type: "config_updated", protocol_version: PROTOCOL_VERSION, request_id: request.request_id,
      revision: "r2", config: saved,
    });
    request = await sentOn(port, 2);
    assert.equal(request.type, "get_config");
    port.reply(configResult(request, origin, {
      revision: "r2",
      config: saved,
      config_issue: "The selected notes folder is unavailable or changed.",
    }));
    await saving;

    assert.equal(values.get(storageKey("policy_v1")), null);
    assert.match(session.configIssue, /unavailable or changed/);
  } finally {
    chrome.runtime.sendMessage = previousSendMessage;
    session.host.disconnect();
  }
});

test("unaddressed host errors and version mismatches reach the caller", async () => {
  ports.length = 0;
  const client = new HostClient();
  const port = ports[0];

  const skewed = client.call(hello(), "hello_result");
  port.reply({
    type: "error", protocol_version: PROTOCOL_VERSION + 1, request_id: port.sent[0].request_id,
    code: "unsupported_protocol_version", message: "extension and host protocol versions differ",
  });
  await assert.rejects(skewed,
    (error) => error instanceof HostError && error.code === "unsupported_protocol_version");

  const skewedResult = client.call(hello(), "hello_result");
  port.reply({ ...helloResult(port.sent[1].request_id), protocol_version: PROTOCOL_VERSION + 1 });
  await assert.rejects(skewedResult,
    (error) => error instanceof HostError && error.code === "invalid_response");

  // The host cannot read an oversized frame's request_id, so it answers with
  // an empty one and closes the connection.
  const oversized = client.call(hello(), "hello_result");
  port.reply({
    type: "error", protocol_version: PROTOCOL_VERSION, request_id: "",
    code: "message_too_large", message: "native message exceeds the 4 MiB inbound limit",
  });
  port.drop();
  await assert.rejects(oversized,
    (error) => error instanceof HostError && error.code === "message_too_large");
  client.disconnect();
});

test("host config shape requires every v6 agent and adapter key and no retired privacy key", () => {
  const config = {
    storage: {
      root: "/notes", profile: "neutral", log_dir: "log", pages_dir: "pages", later_dir: "later", artifacts_dir: "artifacts",
      summaries_dir: null,
    },
    capture_enabled: false, sites: [], strip_params: [],
    agent: null,
    adapters: { format: 1, artifacts: [] },
  };
  assert.equal(isConfig(config), true);
  const agent = {
    harness_id: "claude-code", adapter: "claude_code", binary: "/usr/local/bin/claude",
    args: ["-p", "{prompt}"], env_allow: [], timeout_secs: 120,
  };
  assert.equal(isConfig({ ...config, agent }), true);
  for (const key of ["agent", "adapters"]) {
    const without = { ...config };
    delete without[key];
    assert.equal(isConfig(without), false, key);
  }
  assert.equal(isConfig({ ...config, agent_denylist: ["bank.example"] }), false);
  assert.equal(isConfig({ ...config, agent_denylist_confirmed: false }), false);
  assert.equal(isConfig({ ...config, log_incognito: false }), false);
  assert.equal(isConfig({ ...config, agent: { ...agent, adapter: "claude-code" } }), false);
  // The host always sends summaries_dir, as null until a folder is chosen.
  const { summaries_dir: _omitted, ...m1Storage } = config.storage;
  assert.equal(isConfig({ ...config, storage: m1Storage }), false);
  assert.equal(isConfig({ ...config, storage: { ...m1Storage, summaries_dir: 7 } }), false);

  const result = {
    type: "config_result", revision: "missing", config, config_issue: null,
    adapter_issue: null, adapter_warnings: [],
  };
  assert.equal(isResponseShape(result), false);
  const agentStatus = { state: "not_set_up", harness_version: null, message: "Set up" };
  assert.equal(isResponseShape({ ...result, agent_status: agentStatus }), true);
  assert.equal(isResponseShape({ ...result, agent_status: { ...agentStatus, state: "on" } }), false);
  assert.equal(isResponseShape({
    type: "agent_checked", harness_id: "claude-code", harness_version: "2.1.0",
  }), true);

  // Harness setup: every offer key is present, nullable ones as null.
  const offer = {
    adapter: "claude_code", harness_id: "claude-code",
    binary: "/usr/local/bin/claude", real_path: "/opt/claude/bin/claude", version: "2.1.284",
    args: ["-p", "{prompt}"], env_required: ["HOME", "PATH"],
    env_optional: [{ name: "ANTHROPIC_API_KEY", present: true }], refusal: null,
  };
  const refused = {
    ...offer, adapter: "codex", harness_id: "codex", real_path: null,
    version: null, env_optional: [], refusal: "codex --version output was not recognized",
  };
  const discovered = { type: "harnesses_discovered", offers: [offer, refused] };
  assert.equal(isResponseShape(discovered), true);
  assert.equal(isResponseShape({ ...discovered, offers: [] }), true);
  assert.equal(isResponseShape({ ...discovered, offers: null }), false);
  for (const key of Object.keys(offer)) {
    const without = { ...offer };
    delete without[key];
    assert.equal(isResponseShape({ ...discovered, offers: [without] }), false, key);
  }
  assert.equal(isResponseShape({ ...discovered, offers: [{ ...offer, adapter: "generic" }] }), false);
  assert.equal(isResponseShape({
    ...discovered, offers: [{ ...offer, env_optional: ["ANTHROPIC_API_KEY"] }],
  }), false);
  assert.equal(isResponseShape({
    ...discovered, offers: [{ ...offer, env_optional: [{ name: "X", present: "yes" }] }],
  }), false);

  // Harness setup answers config_updated; the v5 token response is gone.
  const setUp = { type: "harness_setup_confirmed", harness_token: "t", config, summary: "Harness" };
  assert.equal(isResponseShape(setUp), false);
});

// --- Background sync: the worker, not the panel, starts the host -----------

const { BINARY_NAME } = await import("../dist/brand.js");
const SYNC_ALARM = `${BINARY_NAME}-sync`;
const DELIVER_ALARM = `${BINARY_NAME}-deliver`;
const HOUR_MS = 60 * 60_000;

async function until(check, label) {
  for (let attempt = 0; attempt < 400; attempt += 1) {
    const value = await check();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  throw new Error(`Timed out waiting for ${label}`);
}

const sentOn = (port, count) => until(() => port.sent.length >= count && port.sent[count - 1], `host request ${count}`);

function batchRecorded(request, outcome = "persisted", reason = null, overflowRecorded = false) {
  return {
    type: "batch_recorded", protocol_version: PROTOCOL_VERSION, request_id: request.request_id,
    batch_id: request.batch_id, overflow_recorded: overflowRecorded,
    results: request.events.map((item) => ({
      event_id: (item.activity ?? item.visit).event_id, outcome, reason,
      relative_path: outcome === "persisted" ? "log/day.md" : null,
    })),
  };
}

function configResult(request, origin, overrides = {}) {
  return {
    type: "config_result", protocol_version: PROTOCOL_VERSION, request_id: request.request_id,
    revision: `host-revision:${origin}`,
    config: {
      storage: {
        root: "/notes", profile: "neutral", log_dir: "log", pages_dir: "pages", later_dir: "later", artifacts_dir: "artifacts",
        summaries_dir: null,
      },
      capture_enabled: true, sites: [{ origin, path_prefix: "/" }], strip_params: [],
      agent: null,
      adapters: { format: 1, artifacts: [artifact(origin)] },
    },
    config_issue: null,
    agent_status: { state: "not_set_up", harness_version: null, message: "Set up" },
    adapter_issue: null,
    adapter_warnings: [],
    ...overrides,
  };
}

function resetSync(origin, expiresIn) {
  values.clear();
  sessionValues.clear();
  tabs.clear();
  alarms.clear();
  ports.length = 0;
  badgeText = "";
  values.set(storageKey("policy_v1"), { ...lease(origin), expires_at: Date.now() + expiresIn });
}

function exactPattern(origin) {
  return `${origin}:443/*`;
}

function queueWith(...items) {
  values.set(storageKey("queue_v1"), {
    version: 1, items, overflow_count: 0, overflow_first_at: null, overflow_last_at: null,
    rejected_count: 0, last_error: null, retry_error: null,
  });
}

test("the worker registers its periodic sync alarm and syncs on browser startup", () => {
  assert.equal(typeof onAlarm, "function");
  assert.equal(typeof onStartup, "function");
});

test("with no extension page open, the worker freezes and delivers a completed activity hour", async () => {
  const origin = "https://background.example";
  resetSync(origin, 2 * HOUR_MS);
  tabs.set(9, { id: 9, url: `${origin}/items/issue`, title: "Issue", incognito: false });
  onCommitted({
    tabId: 9, frameId: 0, documentId: "doc-bg", documentLifecycle: "active",
    url: `${origin}/items/issue`, timeStamp: Date.now() - 2 * HOUR_MS,
  });
  await flush();
  assert.ok(alarms.has(DELIVER_ALARM), "a capture schedules a delivery");
  assert.equal(ports.length, 0, "the current event does not start a host directly");
  onAlarm({ name: DELIVER_ALARM });
  const port = await until(() => ports[0], "the worker to start the host");
  const request = await sentOn(port, 1);
  assert.equal(request.type, "record_batch");
  assert.equal(request.events[0].kind, "activity");
  assert.equal(request.events[0].activity.url, `${origin}/items/issue`);
  assert.equal(request.events[0].activity.opens, 1);
  port.reply(batchRecorded(request));
  await until(() => values.get(storageKey("queue_v1")).items.length === 0, "the queue to drain");
  await until(() => port.closed, "the host connection to close");
  assert.equal((await send({ kind: "get_status" })).host_error, null);
});

test("a status wake hands off completed hours before reporting live activity", async () => {
  const origin = "https://status-boundary.example";
  resetSync(origin, 2 * HOUR_MS);
  tabs.set(10, { id: 10, url: `${origin}/items/old-hour`, title: "Old hour", incognito: false });
  onCommitted({
    tabId: 10, frameId: 0, documentId: "doc-status", documentLifecycle: "active",
    url: `${origin}/items/old-hour`, timeStamp: Date.now() - 2 * HOUR_MS,
  });
  await flush();

  const status = await send({ kind: "get_status" });
  assert.equal(status.live_activity, 0);
  assert.equal(status.queued, 1);
  assert.equal(values.get(storageKey("queue_v1")).items[0].event.artifact_id,
    "test:status-boundary.example/old-hour");
});

test("the worker leaves the host alone when there is nothing to send or renew", async () => {
  const origin = "https://idle.example";
  resetSync(origin, 2 * HOUR_MS);
  onAlarm({ name: SYNC_ALARM });
  await send({ kind: "sync_now" });
  assert.equal(ports.length, 0);
});

test("the worker renews a lease near expiry from the host config", async () => {
  const origin = "https://renew.example";
  resetSync(origin, 60_000);
  onAlarm({ name: SYNC_ALARM });
  const port = await until(() => ports[0], "the worker to start the host");
  const request = await sentOn(port, 1);
  assert.equal(request.type, "get_config");
  port.reply(configResult(request, origin));
  const renewed = await until(() => {
    const policy = values.get(storageKey("policy_v1"));
    return policy.expires_at > Date.now() + 23 * HOUR_MS && policy;
  }, "a renewed lease");
  assert.deepEqual(renewed.sites, [{ origin, path_prefix: "/" }]);
  await until(() => port.closed, "the host connection to close");
});

test("a renewal that finds a host config issue suspends capture but keeps visits", async () => {
  const origin = "https://issue.example";
  resetSync(origin, 60_000);
  queueWith({ ...visit("kept", `${origin}/items/a`), retry_after: Date.now() + HOUR_MS });
  onAlarm({ name: SYNC_ALARM });
  const port = await until(() => ports[0], "the worker to start the host");
  port.reply(configResult(await sentOn(port, 1), origin, { config_issue: "config.toml is newer" }));
  await until(() => values.get(storageKey("policy_v1")) === null, "the lease to be suspended");
  assert.equal(values.get(storageKey("queue_v1")).items.length, 1);
});

test("a renewal that finds an adapter policy issue suspends capture but keeps visits", async () => {
  const origin = "https://adapter-issue.example";
  resetSync(origin, 60_000);
  queueWith({ ...visit("kept-adapter", `${origin}/items/a`), retry_after: Date.now() + HOUR_MS });
  onAlarm({ name: SYNC_ALARM });
  const port = await until(() => ports[0], "the worker to start the host");
  port.reply(configResult(await sentOn(port, 1), origin, {
    adapter_issue: "unsupported_format",
  }));
  await until(() => values.get(storageKey("policy_v1")) === null, "the lease to be suspended");
  assert.equal(values.get(storageKey("queue_v1")).items.length, 1);
});

test("a host that cannot start is reported, and visits wait to retry", async () => {
  const origin = "https://down.example";
  resetSync(origin, 60_000);
  queueWith(visit("waiting", `${origin}/items/a`));
  onAlarm({ name: SYNC_ALARM });
  const first = await until(() => ports[0], "the worker to start the host");
  await sentOn(first, 1);
  first.drop();
  // The lease is still valid, so delivery is tried on a fresh host process.
  const second = await until(() => ports[1], "a second host process");
  await sentOn(second, 1);
  second.drop();
  const status = await until(async () => {
    const current = await send({ kind: "get_status" });
    return current.host_error && current;
  }, "the host error");
  assert.match(status.host_error, /disconnected/i);
  assert.equal(status.queued, 1);
  const [item] = values.get(storageKey("queue_v1")).items;
  assert.equal(item.attempts, 1);
  assert.ok(item.retry_after > Date.now(), "the failed visit backs off");
  assert.match(status.retry_error, /disconnected/i);
});

test("a delivery the host never answers is reported even when no renewal was due", async () => {
  const origin = "https://gone.example";
  resetSync(origin, 12 * HOUR_MS);
  queueWith(visit("unsent", `${origin}/items/a`));
  onAlarm({ name: SYNC_ALARM });
  const port = await until(() => ports[0], "the worker to start the host");
  assert.equal((await sentOn(port, 1)).type, "record_batch");
  port.drop();
  const status = await until(async () => {
    const current = await send({ kind: "get_status" });
    return current.host_error && current;
  }, "the host error");
  // Joining the sync lets it finish before the next test starts one.
  await send({ kind: "sync_now" });
  assert.match(status.host_error, /disconnected/i);
  assert.equal(ports.length, 1, "no renewal was due, so only delivery started the host");
});

test("after the lease lapsed, browsing sites that are not enabled starts no host", async () => {
  const origin = "https://enabled.example";
  resetSync(origin, -1_000);
  tabs.set(12, { id: 12, url: "https://elsewhere.example/", title: "Elsewhere", incognito: false });
  onCommitted({
    tabId: 12, frameId: 0, documentId: "doc-elsewhere", documentLifecycle: "active",
    url: "https://elsewhere.example/", timeStamp: Date.now(),
  });
  await flush();
  assert.equal(ports.length, 0);
});

test("after the lease lapsed, the first navigation renews it and is still captured", async () => {
  const origin = "https://morning.example";
  resetSync(origin, -1_000);
  tabs.set(11, { id: 11, url: `${origin}/items/first`, title: "First", incognito: false });
  onCommitted({
    tabId: 11, frameId: 0, documentId: "doc-morning", documentLifecycle: "active",
    url: `${origin}/items/first`, timeStamp: Date.now(),
  });
  const port = await until(() => ports[0], "the worker to start the host");
  const request = await sentOn(port, 1);
  assert.equal(request.type, "get_config");
  port.reply(configResult(request, origin));
  const [live] = await until(async () => {
    const totals = await send({ kind: "get_activity" });
    return totals.length === 1 && totals;
  }, "the activity total to be retained");
  assert.equal(live.url, `${origin}/items/first`);
});

test("an older cached lease renews once instead of permanently disabling background capture", async () => {
  const origin = "https://upgrade.example";
  resetSync(origin, 2 * HOUR_MS);
  const legacy = values.get(storageKey("policy_v1"));
  delete legacy.adapters;
  values.set(storageKey("policy_v1"), legacy);
  tabs.set(13, { id: 13, url: `${origin}/items/migrated`, title: "Migrated", incognito: false });

  onCommitted({
    tabId: 13, frameId: 0, documentId: "doc-upgrade", documentLifecycle: "active",
    url: `${origin}/items/migrated`, timeStamp: Date.now(),
  });
  const port = await until(() => ports[0], "the worker to renew an older lease");
  const request = await sentOn(port, 1);
  assert.equal(request.type, "get_config");
  port.reply(configResult(request, origin));
  const [live] = await until(async () => {
    const totals = await send({ kind: "get_activity" });
    return totals.length === 1 && totals;
  }, "the first post-upgrade artifact activity");
  assert.equal(live.url, `${origin}/items/migrated`);
  assert.equal(values.get(storageKey("policy_v1")).adapters.artifacts.length, 1);
});

test("an older cached lease does not start the host for an unrelated navigation", async () => {
  const origin = "https://upgrade-policy.example";
  resetSync(origin, 2 * HOUR_MS);
  const legacy = values.get(storageKey("policy_v1"));
  delete legacy.adapters;
  values.set(storageKey("policy_v1"), legacy);
  tabs.set(14, { id: 14, url: "https://elsewhere.example/", title: "Elsewhere", incognito: false });

  onCommitted({
    tabId: 14, frameId: 0, documentId: "doc-upgrade-elsewhere", documentLifecycle: "active",
    url: "https://elsewhere.example/", timeStamp: Date.now(),
  });
  await flush();
  assert.equal(ports.length, 0);
});

test("disabled or malformed cached policy does not cause background host wakeups", async () => {
  const origin = "https://disabled-upgrade.example";
  resetSync(origin, 2 * HOUR_MS);
  const legacy = values.get(storageKey("policy_v1"));
  delete legacy.adapters;
  legacy.capture_enabled = false;
  values.set(storageKey("policy_v1"), legacy);
  onAlarm({ name: SYNC_ALARM });
  await flush();
  assert.equal(ports.length, 0);

  values.set(storageKey("policy_v1"), { corrupt: true });
  onAlarm({ name: SYNC_ALARM });
  await flush();
  assert.equal(ports.length, 0);
});

test("a local pause still blocks migration of an older cached lease", async () => {
  const origin = "https://paused-upgrade.example";
  resetSync(origin, 2 * HOUR_MS);
  const legacy = values.get(storageKey("policy_v1"));
  delete legacy.adapters;
  values.set(storageKey("policy_v1"), legacy);
  values.set(storageKey("pause_pending_v1"), true);
  await send({ kind: "sync_now" });
  assert.equal(ports.length, 0);
});

test("the panel's send-now request waits for the worker's delivery", async () => {
  const origin = "https://now.example";
  resetSync(origin, 2 * HOUR_MS);
  queueWith(visit("now", `${origin}/items/a`));
  const answered = send({ kind: "sync_now" });
  const port = await until(() => ports[0], "the worker to start the host");
  port.reply(batchRecorded(await sentOn(port, 1)));
  const status = await answered;
  assert.equal(status.queued, 0);
});

test("one native batch drains multiple records and durably acknowledges overflow", async () => {
  const origin = "https://batch.example";
  resetSync(origin, 2 * HOUR_MS);
  queueWith(visit("first", `${origin}/items/a`), visit("second", `${origin}/items/b`));
  const queue = values.get(storageKey("queue_v1"));
  queue.overflow_count = 4;
  queue.overflow_first_at = "2026-10-16T08:00:00+00:00";
  queue.overflow_last_at = "2026-10-16T11:00:00+00:00";
  values.set(storageKey("queue_v1"), queue);

  const answered = send({ kind: "sync_now" });
  const port = await until(() => ports[0], "the batched host delivery");
  const request = await sentOn(port, 1);
  assert.equal(request.type, "record_batch");
  assert.equal(request.events.length, 2);
  assert.deepEqual(request.overflow, {
    count: 4,
    first_occurred_at: "2026-10-16T08:00:00+00:00",
    last_occurred_at: "2026-10-16T11:00:00+00:00",
  });
  port.reply(batchRecorded(request, "persisted", null, true));
  const status = await answered;
  assert.equal(status.queued, 0);
  assert.equal(status.overflow_count, 0);
  assert.equal(status.overflow_first_at, null);
  assert.equal(status.overflow_last_at, null);
});

test("a lost acknowledgement replays the exact overflow snapshot", async () => {
  const origin = "https://overflow-retry.example";
  resetSync(origin, 2 * HOUR_MS);
  queueWith(visit("retry", `${origin}/items/a`));
  const queue = values.get(storageKey("queue_v1"));
  queue.overflow_count = 4;
  queue.overflow_first_at = "2026-10-16T08:00:00+00:00";
  queue.overflow_last_at = "2026-10-16T11:00:00+00:00";
  values.set(storageKey("queue_v1"), queue);

  const firstAnswer = send({ kind: "sync_now" });
  const firstPort = await until(() => ports[0], "the first overflow delivery");
  const firstRequest = await sentOn(firstPort, 1);
  const whileWriting = values.get(storageKey("queue_v1"));
  whileWriting.overflow_count = 6;
  whileWriting.overflow_last_at = "2026-10-16T13:00:00+00:00";
  values.set(storageKey("queue_v1"), whileWriting);
  firstPort.drop();
  await firstAnswer;

  const retryable = values.get(storageKey("queue_v1"));
  retryable.items[0].retry_after = 0;
  values.set(storageKey("queue_v1"), retryable);
  const secondAnswer = send({ kind: "sync_now" });
  const secondPort = await until(() => ports[1], "the replayed overflow delivery");
  const replay = await sentOn(secondPort, 1);
  assert.equal(replay.batch_id, firstRequest.batch_id);
  assert.deepEqual(replay.overflow, firstRequest.overflow,
    "new drops must not change an idempotent batch replay");
  secondPort.reply(batchRecorded(replay, "persisted", null, true));

  const status = await secondAnswer;
  assert.equal(status.queued, 0);
  assert.equal(status.overflow_count, 2);
  assert.equal(status.overflow_first_at, "2026-10-16T11:00:00+00:00");
  assert.equal(status.overflow_last_at, "2026-10-16T13:00:00+00:00");
});

test("dismissing an in-flight overflow cannot erase later drops", async () => {
  const origin = "https://overflow-dismiss.example";
  resetSync(origin, 2 * HOUR_MS);
  queueWith(visit("writing", `${origin}/items/a`));
  const queue = values.get(storageKey("queue_v1"));
  queue.overflow_count = 4;
  queue.overflow_first_at = "2026-10-16T08:00:00+00:00";
  queue.overflow_last_at = "2026-10-16T11:00:00+00:00";
  values.set(storageKey("queue_v1"), queue);

  const answered = send({ kind: "sync_now" });
  const port = await until(() => ports[0], "the in-flight overflow delivery");
  const request = await sentOn(port, 1);
  await send({ kind: "clear_notices" });

  const afterDismiss = values.get(storageKey("queue_v1"));
  afterDismiss.overflow_count = 2;
  afterDismiss.overflow_first_at = "2026-10-16T12:00:00+00:00";
  afterDismiss.overflow_last_at = "2026-10-16T13:00:00+00:00";
  values.set(storageKey("queue_v1"), afterDismiss);
  port.reply(batchRecorded(request, "persisted", null, true));

  const status = await answered;
  assert.equal(status.overflow_count, 2);
  assert.equal(status.overflow_first_at, "2026-10-16T12:00:00+00:00");
  assert.equal(status.overflow_last_at, "2026-10-16T13:00:00+00:00");
});

test("queue pressure turns on the toolbar badge at eighty percent", async () => {
  const origin = "https://pressure.example";
  resetSync(origin, 2 * HOUR_MS);
  queueWith(...Array.from({ length: 206 }, (_, index) =>
    visit(`pressure-${index}`, `${origin}/items/${index}`)));
  await send({ kind: "discard_pending", event_ids: ["pressure-205"] });
  assert.equal(badgeText, "!");
  await send({ kind: "discard_pending", event_ids:
    Array.from({ length: 205 }, (_, index) => `pressure-${index}`) });
  assert.equal(badgeText, "");
});

test("pressure coalescing never grows the queue past its byte cap", async () => {
  const origin = "https://pressure.example";
  resetSync(origin, 2 * HOUR_MS);
  const existing = activityVisit(
    "00000000-0000-4000-8000-000000000001", `${origin}/items/shared`);
  const padding = "x".repeat(1_850);
  queueWith(existing, ...Array.from({ length: 204 }, (_, index) =>
    visit(`filler-${index}`, `${origin}/items/filler-${index}?padding=${padding}`)));
  const before = values.get(storageKey("queue_v1"));
  const beforeBytes = new TextEncoder().encode(JSON.stringify(before.items)).length;
  assert.ok(beforeBytes < MAX_QUEUE_BYTES);

  const evidence = `${origin}/items/shared?evidence=${"y".repeat(7_800)}`;
  values.set(storageKey("activity_totals_v1"), {
    version: 1,
    totals: [{
      event_id: "00000000-0000-4000-8000-000000000002",
      artifact_id: "test:pressure.example/shared", artifact_type: "test.item",
      hour_start: "2026-10-15T10:00:00+00:00",
      url: `${origin}/items/shared`, title: "A".repeat(300),
      opens: 1, focused_ms: 1_000, background_ms: 0,
      modes: ["view", "edit", "review"].map((mode) => ({ mode, url: evidence })),
      trails: Array.from({ length: 10 }, (_, index) => ({
        from_artifact_id: `test:pressure.example/source-${index}`,
        from_url: `${evidence}&trail=${index}`,
        transition: "link",
      })),
      refs: Array.from({ length: 20 }, (_, index) => ({
        artifact_type: "test.item", value: `${index}-${"r".repeat(500)}`,
      })),
    }],
  });

  const status = await send({ kind: "get_status" });
  const after = values.get(storageKey("queue_v1"));
  assert.equal(status.overflow_count, 1);
  assert.equal(after.items.length, 205);
  assert.equal(after.items[0].event.opens, 1,
    "a refused merge leaves the original durable event unchanged");
  assert.ok(new TextEncoder().encode(JSON.stringify(after.items)).length <= MAX_QUEUE_BYTES);
});
