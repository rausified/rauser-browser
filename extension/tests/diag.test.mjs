import assert from "node:assert/strict";
import test from "node:test";

// Run after build:extension.
const {
  DIAG_MAX_BYTES, DIAG_MAX_ENTRIES, DIAG_STORAGE_KEY, DiagLog, capEntries, diag, diagOrigin,
  diagnosticsText, initDiag, serializeError,
} = await import("../dist/diag.js");
const { HostClient, HostError, PROTOCOL_VERSION } = await import("../dist/native.js");

// Text that stands for page content; no diagnostic may ever hold it.
const SENTINEL = "SENTINEL-page-text-4b1d";

function fakeStorage({ failSet = false, failAccess = false } = {}) {
  const values = new Map();
  const storage = {
    values,
    sets: 0,
    async get(key) {
      return values.has(key) ? { [key]: structuredClone(values.get(key)) } : {};
    },
    async set(items) {
      storage.sets += 1;
      if (failSet) throw new Error("QUOTA_BYTES quota exceeded");
      for (const [name, value] of Object.entries(items)) values.set(name, structuredClone(value));
    },
    async remove(key) { values.delete(key); },
    async setAccessLevel() {
      if (failAccess) throw new Error("not allowed");
    },
  };
  return storage;
}

function logger(storage, options = {}) {
  return new DiagLog({ ctx: "test", storage: () => storage, console: null, flushMs: 60_000, ...options });
}

function entry(level, event, extra = 0) {
  return { ts: "2026-10-28T00:00:00.000Z", level, ctx: "test", event, ...(extra ? { fields: { pad: "x".repeat(extra) } } : {}) };
}

test("a URL is reduced to its origin", () => {
  assert.equal(diagOrigin("https://jira.example.com:8443/browse/PROJ-1?q=secret#frag"), "https://jira.example.com:8443");
  assert.equal(diagOrigin("http://user:pass@host.example/path"), "http://host.example");
  assert.equal(diagOrigin("about:blank"), "about:");
  assert.equal(diagOrigin(`data:text/plain,${SENTINEL}`), "data:");
  assert.equal(diagOrigin("not a url"), "invalid");
  assert.equal(diagOrigin(""), "none");
  assert.equal(diagOrigin(undefined), "none");
});

test("an error keeps its name, message, and code but not its stack", () => {
  assert.deepEqual(serializeError(new TypeError("bad thing")), { name: "TypeError", message: "bad thing" });
  assert.deepEqual(serializeError(new HostError("timeout", "Native host did not respond in time")),
    { name: "HostError", message: "Native host did not respond in time", code: "timeout" });
  const withStack = serializeError(new Error("boom"), { stack: true });
  assert.match(withStack.stack, /boom/);
  assert.deepEqual(serializeError("plain"), { name: "string", message: "plain" });
  assert.equal(serializeError(new Error("y".repeat(5_000))).message.length, 1_001);
});

test("field values are reduced to primitives and errors", async () => {
  const storage = fakeStorage();
  const log = logger(storage);
  log.warn("something failed", {
    error: new Error("disk full"), count: 3, ok: false, none: null, skipped: undefined,
    page: { text: SENTINEL }, list: [SENTINEL], nan: Number.NaN,
  });
  const [kept] = await log.entries();
  assert.equal(kept.level, "warn");
  assert.equal(kept.ctx, "test");
  assert.equal(kept.event, "something failed");
  assert.deepEqual(kept.fields, {
    error: { name: "Error", message: "disk full" }, count: 3, ok: false, none: null,
    page: "[object]", list: "[array]", nan: "NaN",
  });
  assert.equal(JSON.stringify([...storage.values]).includes(SENTINEL), false);
});

test("entries reach storage in one batched write, not one per event", async () => {
  const storage = fakeStorage();
  const log = logger(storage);
  for (let index = 0; index < 100; index += 1) log.debug("native request", { request_id: `r${index}` });
  await new Promise((resolve) => setTimeout(resolve, 5));
  assert.equal(storage.sets, 0, "nothing is written before the flush interval");
  await log.flush();
  assert.equal(storage.sets, 1);
  assert.equal(storage.values.get(DIAG_STORAGE_KEY).length, 100);
  await log.flush();
  assert.equal(storage.sets, 1, "a flush with nothing pending writes nothing");
});

test("the flush timer writes pending entries once", async () => {
  const storage = fakeStorage();
  const log = logger(storage, { flushMs: 5 });
  log.info("one");
  log.info("two");
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.equal(storage.sets, 1);
  assert.deepEqual(storage.values.get(DIAG_STORAGE_KEY).map((item) => item.event), ["one", "two"]);
});

test("the buffer is capped by count and bytes, dropping debug entries first", () => {
  const many = Array.from({ length: DIAG_MAX_ENTRIES + 20 }, (_, index) => entry("info", `e${index}`));
  const capped = capEntries(many);
  assert.equal(capped.length, DIAG_MAX_ENTRIES);
  assert.equal(capped[0].event, "e20", "the oldest go first");

  const mixed = [entry("warn", "old warning"), entry("debug", "d1"), entry("debug", "d2"), entry("info", "newest")];
  assert.deepEqual(capEntries(mixed, 3).map((item) => item.event), ["old warning", "d2", "newest"]);
  assert.deepEqual(capEntries(mixed, 1).map((item) => item.event), ["newest"]);

  const big = Array.from({ length: 10 }, (_, index) => entry("info", `b${index}`, 50_000));
  const byBytes = capEntries(big);
  assert.ok(byBytes.length < 10);
  assert.ok(JSON.stringify(byBytes).length <= DIAG_MAX_BYTES);
  assert.equal(byBytes.at(-1).event, "b9");
});

test("persisted entries stay capped across flushes", async () => {
  const storage = fakeStorage();
  const log = logger(storage);
  for (let round = 0; round < 3; round += 1) {
    for (let index = 0; index < 300; index += 1) log.info("tick", { round, index });
    await log.flush();
  }
  const stored = storage.values.get(DIAG_STORAGE_KEY);
  assert.equal(stored.length, DIAG_MAX_ENTRIES);
  assert.deepEqual(stored.at(-1).fields, { round: 2, index: 299 });
});

test("storage failures are swallowed", async () => {
  const failing = fakeStorage({ failSet: true });
  const log = logger(failing);
  log.error("first");
  await log.flush();
  assert.equal(failing.sets, 1);
  assert.deepEqual(await log.entries(), []);

  const untrusted = fakeStorage({ failAccess: true });
  const guarded = logger(untrusted);
  guarded.error("held back");
  await guarded.flush();
  assert.equal(untrusted.sets, 0, "nothing is written without trusted-only access");

  const throwing = new DiagLog({
    storage: () => { throw new Error("no storage"); }, console: null, flushMs: 60_000,
  });
  throwing.warn("still fine");
  await throwing.flush();

  const noisyConsole = { debug() { throw new Error("x"); }, info() {}, warn() {}, error() {} };
  const quiet = logger(fakeStorage(), { console: noisyConsole });
  assert.doesNotThrow(() => quiet.debug("console failure"));
});

test("the console copy has the Brauser prefix and the entry's level", () => {
  const calls = [];
  const sink = Object.fromEntries(["debug", "info", "warn", "error"].map((level) =>
    [level, (...args) => calls.push([level, ...args])]));
  const log = logger(fakeStorage(), { console: sink });
  log.warn("host sync failed", { error: new Error("offline") });
  log.info("ready");
  assert.deepEqual(calls, [
    ["warn", "Brauser host sync failed", { error: { name: "Error", message: "offline" } }],
    ["info", "Brauser ready"],
  ]);
});

test("clear forgets persisted and pending entries", async () => {
  const storage = fakeStorage();
  const log = logger(storage);
  log.info("persisted");
  await log.flush();
  log.info("pending");
  await log.clear();
  assert.equal(storage.values.has(DIAG_STORAGE_KEY), false);
  assert.deepEqual(await log.entries(), []);
});

test("the copied report starts with a header line", () => {
  const text = diagnosticsText(
    { extension_version: "0.1.0", build_id: "abc123", protocol_version: 8 },
    [entry("info", "one"), entry("warn", "two")],
    new Date("2026-10-28T12:00:00.000Z"),
  );
  const lines = text.trimEnd().split("\n").map((line) => JSON.parse(line));
  assert.deepEqual(lines[0], {
    type: "brauser_diagnostics", exported_at: "2026-10-28T12:00:00.000Z",
    extension_version: "0.1.0", build_id: "abc123", protocol_version: 8, entries: 2,
  });
  assert.deepEqual(lines.slice(1).map((line) => line.event), ["one", "two"]);
});

test("uncaught errors and unhandled rejections are logged at error", async () => {
  const storage = fakeStorage();
  globalThis.chrome = { storage: { local: storage } };
  const listeners = new Map();
  globalThis.addEventListener = (type, listener) => listeners.set(type, listener);
  try {
    await diag.clear();
    initDiag("panel");
    listeners.get("error")({ error: new RangeError("out of range"), message: "Uncaught RangeError" });
    listeners.get("unhandledrejection")({ reason: new HostError("disconnected", "gone") });
    const entries = await diag.entries();
    assert.deepEqual(entries.map(({ level, ctx, event, fields }) => ({ level, ctx, event, fields })), [
      { level: "error", ctx: "panel", event: "uncaught error", fields: { error: { name: "RangeError", message: "out of range" } } },
      { level: "error", ctx: "panel", event: "unhandled rejection", fields: { error: { name: "HostError", message: "gone", code: "disconnected" } } },
    ]);
    assert.equal(entries.some((item) => "stack" in (item.fields?.error ?? {})), false);
  } finally {
    delete globalThis.addEventListener;
  }
});

// A scripted host for HostClient: `answer` decides each request's reply.
function installHost(answer, { lastError } = {}) {
  const storage = fakeStorage();
  const ports = [];
  globalThis.chrome = {
    storage: { local: storage },
    runtime: {
      lastError: undefined,
      connectNative() {
        const messages = [];
        const disconnects = [];
        const port = {
          onMessage: { addListener(listener) { messages.push(listener); } },
          onDisconnect: { addListener(listener) { disconnects.push(listener); } },
          postMessage(request) {
            const reply = answer(request);
            if (reply === "drop") {
              setTimeout(() => {
                globalThis.chrome.runtime.lastError = lastError ? { message: lastError } : undefined;
                for (const listener of disconnects) listener();
                globalThis.chrome.runtime.lastError = undefined;
              }, 0);
            } else if (reply) {
              setTimeout(() => {
                for (const listener of messages) {
                  listener({ ...reply, protocol_version: PROTOCOL_VERSION, request_id: request.request_id });
                }
              }, 0);
            }
          },
          disconnect() {},
        };
        ports.push(port);
        return port;
      },
    },
  };
  return storage;
}

const fields = (item) => item.fields ?? {};

test("every native request is logged with its request_id, kind, outcome, and time, never its content", async () => {
  installHost((request) => {
    if (request.type === "hello") return { type: "hello_result", host_version: "0.0.0", configured: true, config_issue: null };
    if (request.type === "save_note") return { type: "error", code: "invalid_request", message: "Note is too large" };
    return null;
  });
  await diag.clear();
  const host = new HostClient();
  await host.call({ type: "hello", protocol_version: PROTOCOL_VERSION, request_id: "req-ok" }, "hello_result");
  await assert.rejects(host.callAny({
    type: "save_note", protocol_version: PROTOCOL_VERSION, request_id: "req-error",
    url: `https://notes.example/page?text=${SENTINEL}`, title: SENTINEL, body: SENTINEL,
  }, ["note_saved"]), HostError);
  await assert.rejects(host.call({
    type: "get_run", protocol_version: PROTOCOL_VERSION, request_id: "req-timeout", run_id: "run-7",
  }, "run_status", 5), (error) => error.code === "timeout");
  host.disconnect();

  const entries = await diag.entries();
  const byId = (id) => entries.find((item) => fields(item).request_id === id);
  const ok = byId("req-ok");
  assert.equal(ok.level, "debug");
  assert.equal(ok.event, "native request");
  assert.equal(fields(ok).kind, "hello");
  assert.equal(fields(ok).outcome, "ok");
  assert.equal(typeof fields(ok).elapsed_ms, "number");

  const failed = byId("req-error");
  assert.equal(failed.level, "info");
  assert.equal(failed.event, "native request failed");
  assert.equal(fields(failed).kind, "save_note");
  assert.equal(fields(failed).outcome, "invalid_request");
  assert.equal(fields(failed).error, "Note is too large");

  const timedOut = byId("req-timeout");
  assert.equal(timedOut.level, "warn");
  assert.equal(fields(timedOut).outcome, "timeout");
  assert.equal(fields(timedOut).run_id, "run-7");

  assert.equal(JSON.stringify(entries).includes(SENTINEL), false, "no request content is logged");
  assert.equal(JSON.stringify(entries).includes("notes.example"), false, "no request URL is logged");
});

test("a host Chrome cannot find is logged at warn, as are its requests", async () => {
  installHost(() => "drop", { lastError: "Specified native messaging host not found." });
  await diag.clear();
  const host = new HostClient();
  await assert.rejects(host.call({ type: "hello", protocol_version: PROTOCOL_VERSION, request_id: "req-missing" }, "hello_result"),
    (error) => error.code === "disconnected");
  host.disconnect();
  const entries = await diag.entries();
  const missing = entries.find((item) => item.event === "native host not found");
  assert.equal(missing.level, "warn");
  assert.deepEqual(fields(missing), { answered: false, pending: 1, error: "Specified native messaging host not found." });
  const request = entries.find((item) => fields(item).request_id === "req-missing");
  assert.equal(request.level, "warn");
  assert.equal(fields(request).outcome, "disconnected");
});

test("a host that exits mid-request is logged as disconnected", async () => {
  let calls = 0;
  installHost(() => {
    calls += 1;
    return calls === 1 ? { type: "hello_result", host_version: "0.0.0", configured: true, config_issue: null } : "drop";
  });
  await diag.clear();
  const host = new HostClient();
  await host.call({ type: "hello", protocol_version: PROTOCOL_VERSION, request_id: "req-first" }, "hello_result");
  await assert.rejects(host.call({ type: "get_config", protocol_version: PROTOCOL_VERSION, request_id: "req-dropped" }, "config_result"));
  host.disconnect();
  const entries = await diag.entries();
  const dropped = entries.find((item) => item.event === "native host disconnected");
  assert.equal(dropped.level, "warn");
  assert.equal(fields(dropped).answered, true);
  assert.equal(fields(dropped).pending, 1);
});

test("a failed harness setup is logged by its code only: its message can quote the probe's stderr", async () => {
  const PROBE_STDERR = "sentinel-probe-stderr-77a2";
  installHost((request) => request.type === "set_up_harness"
    ? { type: "error", code: "invalid_config", message: `Codex failed its test run (${PROBE_STDERR}); summaries stay off` }
    : null);
  await diag.clear();
  const host = new HostClient();
  await assert.rejects(host.callAny({
    type: "set_up_harness", protocol_version: PROTOCOL_VERSION, request_id: "req-setup",
    expected_revision: "r1", harness: "codex",
  }, ["config_updated"]), (error) => error instanceof HostError && error.message.includes(PROBE_STDERR));
  host.disconnect();

  const entries = await diag.entries();
  const setup = entries.find((item) => fields(item).request_id === "req-setup");
  assert.equal(fields(setup).outcome, "invalid_config");
  assert.equal(fields(setup).error, undefined);
  assert.equal(JSON.stringify(entries).includes(PROBE_STDERR), false);
});
