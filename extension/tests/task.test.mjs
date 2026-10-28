import assert from "node:assert/strict";
import test from "node:test";

// Run after build:extension.
const { EXTRACT_LIMITS } = await import("../dist/extract.js");
const {
  TASK_INPUT_MAX_UTF16_UNITS, TASK_MAY_HAVE_STARTED_MESSAGE, TASK_NEEDS_HARNESS_MESSAGE, TASK_PAGE_CHANGED_MESSAGE,
  TASK_UNREADABLE_MESSAGE, cleanInstruction, startTask, taskFailureText,
} = await import("../dist/task.js");
const { HostClient, HostError, PROTOCOL_VERSION } = await import("../dist/native.js");
const { DIAG_FLUSH_MS, DIAG_STORAGE_KEY, diag } = await import("../dist/diag.js");
const { SERVICE_SETUP_INSTRUCTIONS } = await import("../dist/agent.js");
const { storageKey } = await import("../dist/brand.js");

const MAX_INBOUND_BYTES = 4 * 1024 * 1024;
const RUNS_MAY_EXIST_KEY = storageKey("runs_may_exist_v1");
const TAB = { id: 4, url: "https://read.example/article", title: "Tab title" };
const NOW = new Date("2026-10-28T10:00:00Z");

const framePage = (fields) => ({
  title: "Page title", content: "", selection: null, truncated: false, readable: true, href: TAB.url, ...fields,
});

// A queued task as the service answers run_started.
const RUN = {
  run_id: "00000000-0000-4000-8000-000000000090", state: "queued", harness: "codex",
  record_key: { kind: "url", value: "https://read.example/article" }, written_record_key: null,
  url: "https://read.example/article", title: "Page title", cwd: "/Users/me/Notes",
  created_at: "2026-10-28T10:00:00.000000000Z", started_at: null,
  finished_at: null, error: null, error_kind: null, thread_id: null, session_id: null,
  summary_path: null, stream_bytes: 0, can_retry_save: false, answer: null,
  kind: "task", instruction: "Do it", task_report: null,
};

// Deps whose executeScript answers the extractor with one top frame, and
// whose host records each request and the storage writes around it.
function taskDeps({ page = framePage({ content: "Body text" }), answer } = {}) {
  const events = [];
  const requests = [];
  const deps = {
    scripting: {
      async executeScript(injection) {
        events.push(injection.files ? "readability" : "extract");
        if (injection.files) return [];
        return [{ frameId: 0, result: page }];
      },
    },
    host: {
      async call(request, expected) {
        events.push(`call:${request.type}`);
        requests.push({ request, expected });
        if (answer) return answer(request);
        return { type: "run_started", run: RUN, existing: false };
      },
    },
    storage: {
      async set(items) { events.push(`set:${Object.keys(items).join(",")}`); },
    },
    now: () => NOW,
  };
  return { deps, events, requests };
}

const byteLength = (value) => new TextEncoder().encode(JSON.stringify(value)).length;

test("startTask sends start_task with the page and instruction and no kind or record key", async () => {
  const { deps, requests } = taskDeps({ page: framePage({ content: "Body text", selection: "picked" }) });
  const run = await startTask(TAB, "Draft a reply to this thread", deps);
  assert.deepEqual(run, RUN);
  assert.equal(requests.length, 1);
  const { request, expected } = requests[0];
  assert.equal(expected, "run_started");
  assert.equal(request.type, "start_task");
  assert.equal(request.protocol_version, PROTOCOL_VERSION);
  assert.equal(typeof request.request_id, "string");
  assert.equal(request.url, TAB.url);
  assert.equal(request.title, "Page title");
  assert.equal(request.content, "Body text");
  assert.equal(request.selection, "picked");
  assert.equal(request.instruction, "Draft a reply to this thread");
  assert.equal(request.harness, null);
  assert.match(request.requested_at, /^2026-10-28T\d\d:\d\d:00[+-]\d\d:\d\d$/);
  assert.deepEqual(Object.keys(request).sort(), [
    "content", "harness", "instruction", "protocol_version", "request_id", "requested_at", "selection", "title",
    "type", "url",
  ]);
  for (const key of ["kind", "record_key", "cwd", "prompt", "schema"]) assert.equal(key in request, false, key);
});

test("startTask replaces lone surrogates and control characters and sends the trimmed text", async () => {
  const { deps, requests } = taskDeps();
  const raw = "  \u0085\tFirst\uD800 line\r\nsecond\u000Bthird\u001Bfourth\u0000\u007F\u009F\n\t  ";
  await startTask(TAB, raw, deps);
  const sent = requests[0].request.instruction;
  assert.equal(sent, "First\uFFFD line \nsecond third fourth");
  assert.equal(sent, cleanInstruction(raw));
  // Nothing the host's check_task_instruction refuses is left.
  assert.doesNotMatch(sent.replace(/[\n\t]/g, ""), /\p{Cc}/u);
  assert.doesNotMatch(sent, /[\uD800-\uDFFF]/);
  assert.equal(cleanInstruction("😀 keep pairs"), "😀 keep pairs");
  assert.equal(cleanInstruction("a\tb\nc"), "a\tb\nc");
});

test("startTask sends nothing when cleaning leaves the instruction empty", async () => {
  for (const raw of ["", "   ", "\n\t\n", "\r\u0085\u000B\u001B", " \u0000 "]) {
    const { deps, events } = taskDeps();
    assert.equal(await startTask(TAB, raw, deps), null, JSON.stringify(raw));
    assert.deepEqual(events, [], JSON.stringify(raw));
  }
});

test("startTask marks runs may exist before and after start_task", async () => {
  const started = taskDeps();
  await startTask(TAB, "Do it", started.deps);
  const mark = `set:${RUNS_MAY_EXIST_KEY}`;
  assert.deepEqual(started.events, [mark, "readability", "extract", "call:start_task", mark]);

  // A refused start was still marked first, so a panel closed mid-start
  // looks for it.
  const refused = taskDeps({ answer: () => { throw new HostError("conflict", "Four tasks are already waiting or running. Wait for one to finish."); } });
  await assert.rejects(startTask(TAB, "Do it", refused.deps), (error) => error.code === "conflict");
  assert.deepEqual(refused.events, [mark, "readability", "extract", "call:start_task"]);
});

test("startTask refuses an unreadable or moved page without sending anything", async () => {
  const empty = taskDeps({ page: framePage({ content: "  " }) });
  await assert.rejects(startTask(TAB, "Do it", empty.deps), (error) => error.code === "unreadable");
  assert.equal(empty.requests.length, 0);
  const moved = taskDeps({ page: framePage({ content: "Body", href: "https://read.example/other" }) });
  await assert.rejects(startTask(TAB, "Do it", moved.deps), (error) => error.code === "page_changed");
  assert.equal(moved.requests.length, 0);
});

test("a maximal page plus the worst-case instruction fits the frame", async () => {
  // Content filling the whole extraction budget, at three UTF-8 bytes a
  // character, and no selection.
  const content = "界".repeat(Math.floor((EXTRACT_LIMITS.maxRequestBytes - 2) / 3));
  assert.ok(byteLength(content) <= EXTRACT_LIMITS.maxRequestBytes);
  // TASK_INPUT_MAX_UTF16_UNITS units of the inputs that encode worst in JSON
  // before cleaning: control characters (six-byte escapes), lone surrogates
  // (six-byte escapes), quotes and backslashes, and three-byte characters.
  const units = ["\u0001", "\uD800", "\"", "\\", "界", "\u0085"];
  let raw = "x";
  for (let index = 0; raw.length < TASK_INPUT_MAX_UTF16_UNITS - 1; index += 1) raw += units[index % units.length];
  raw += "x";
  assert.equal(raw.length, TASK_INPUT_MAX_UTF16_UNITS);
  const longTab = { ...TAB, url: `https://read.example/${"a".repeat(8000)}` };
  const { deps, requests } = taskDeps({
    page: framePage({ title: "界".repeat(EXTRACT_LIMITS.maxTitleChars), content, href: longTab.url }),
  });
  await startTask(longTab, raw, deps);
  const { request } = requests[0];
  // Once cleaned, no unit encodes to more than three bytes of JSON.
  assert.ok(byteLength(request.instruction) <= 3 * TASK_INPUT_MAX_UTF16_UNITS + 2);
  const frame = JSON.stringify({ ...request, request_id: "x".repeat(128) });
  assert.ok(new TextEncoder().encode(frame).length < MAX_INBOUND_BYTES);
});

test("taskFailureText maps each error", () => {
  const settings = (text) => ({ text, openSettings: true, uncertain: false });
  const plain = (text) => ({ text, openSettings: false, uncertain: false });
  // The answer was lost after start_task may have reached the service.
  const uncertain = { text: TASK_MAY_HAVE_STARTED_MESSAGE, openSettings: false, uncertain: true };
  const finishSetup = settings("Finish setup in Brauser settings to run tasks (home folder, knowledge base folder and agent).");
  const repair = settings("Brauser's settings need repair before a task can run.");
  const cases = [
    [new HostError("unreadable", "Brauser can't read this page's content."), plain(TASK_UNREADABLE_MESSAGE)],
    [new HostError("page_changed", "The page changed before it could be read. Summarize it again."),
      plain(TASK_PAGE_CHANGED_MESSAGE)],
    [new HostError("message_too_large", "This page is too large to send with a task."),
      plain("This page is too large to send with a task.")],
    [new HostError("message_too_large", "Message is too large"), plain("This page is too large to send with a task.")],
    [new HostError("service_unavailable", "The service did not start."),
      plain(`${SERVICE_SETUP_INSTRUCTIONS} The service did not start.`)],
    [new HostError("conflict", "Four tasks are already waiting or running. Wait for one to finish."),
      plain("Four tasks are already waiting or running. Wait for one to finish.")],
    // not_configured is mapped by code, whatever the summary-worded text.
    [new HostError("not_configured", "Choose a notes folder before summarizing."), finishSetup],
    [new HostError("not_configured", "The knowledge base folder changed or is missing; choose it again in settings."), finishSetup],
    [new HostError("not_configured", "Set up a summary harness in settings first."), finishSetup],
    [new HostError("not_configured", "Claude Code is not the harness set up in settings."), finishSetup],
    [new HostError("not_configured", "No AI harness is set up for summaries."), finishSetup],
    [new HostError("invalid_config", "agent.env line 3 is not NAME=value"), repair],
    [new HostError("invalid_config", "The configuration needs repair before summaries can run."), repair],
    [new HostError("invalid_request", "invalid or unsupported request JSON"),
      plain("Rebuild the Brauser host to use the task bar.")],
    [new HostError("invalid_request", "instruction contains a control character"),
      plain("Could not start the task: instruction contains a control character")],
    [new HostError("invalid_request", "invalid or unsupported request JSON!"),
      plain("Could not start the task: invalid or unsupported request JSON!")],
    [new HostError("timeout", "Native host did not respond in time"), uncertain],
    [new HostError("disconnected", "Native host has exited."), uncertain],
    [new HostError("invalid_response", "Native host returned run_fetched instead of run_started"), uncertain],
    // The host's own lost answer (EXCHANGE_TIMEOUT, or a service that closed
    // the connection) and its unusable reply.
    [new HostError("service_unavailable", "The Brauser service stopped before answering. Try again in a moment."), uncertain],
    [new HostError("internal", "The Brauser service sent an unexpected reply."), uncertain],
    [new HostError("internal", "task broke"), plain("Could not start the task: task broke")],
    [new Error("boom"), plain("Could not start the task: boom")],
    ["odd", plain("Could not start the task: odd")],
  ];
  for (const [error, expected] of cases) {
    assert.deepEqual(taskFailureText(error), expected, `${error?.code ?? ""} ${error?.message ?? error}`);
  }
  for (const [error, { text }] of cases) {
    if (error?.code !== "service_unavailable") assert.doesNotMatch(text, /summar/i);
  }
  assert.equal(TASK_NEEDS_HARNESS_MESSAGE, "Set up an agent (Codex or Claude Code) in settings to run tasks.");
  assert.equal(TASK_UNREADABLE_MESSAGE, "Brauser could not read this page for a task.");
  assert.equal(TASK_MAY_HAVE_STARTED_MESSAGE, "The task may have started — check Tasks before running it again.");
  assert.equal(TASK_INPUT_MAX_UTF16_UNITS, 4000);
});

// A scripted native host for a real HostClient, so its request log runs.
function installHost(answer) {
  const values = new Map();
  const storage = {
    values,
    async get(key) { return values.has(key) ? { [key]: structuredClone(values.get(key)) } : {}; },
    async set(items) { for (const [name, value] of Object.entries(items)) values.set(name, structuredClone(value)); },
    async remove(key) { values.delete(key); },
    async setAccessLevel() {},
  };
  globalThis.chrome = {
    storage: { local: storage },
    runtime: {
      lastError: undefined,
      connectNative() {
        const listeners = [];
        return {
          onMessage: { addListener(listener) { listeners.push(listener); } },
          onDisconnect: { addListener() {} },
          postMessage(request) {
            const reply = answer(request);
            setTimeout(() => {
              for (const listener of listeners) {
                listener({ ...reply, protocol_version: PROTOCOL_VERSION, request_id: request.request_id });
              }
            }, 0);
          },
          disconnect() {},
        };
      },
    },
  };
  return storage;
}

test("startTask keeps the instruction out of diagnostics", async () => {
  const SENTINEL = "SENTINEL-task-instruction-9c3e";
  const TITLE = "SENTINEL-page-title-51aa";
  let calls = 0;
  const storage = installHost(() => {
    calls += 1;
    if (calls === 1) return { type: "run_started", run: { ...RUN, instruction: SENTINEL, title: TITLE }, existing: false };
    if (calls === 2) return { type: "error", code: "conflict", message: "Four tasks are already waiting or running. Wait for one to finish." };
    return { type: "error", code: "invalid_request", message: "invalid or unsupported request JSON" };
  });
  await diag.clear();
  const host = new HostClient();
  const deps = {
    ...taskDeps({ page: framePage({ title: TITLE, content: `Body ${SENTINEL}` }) }).deps,
    host,
    storage,
  };
  await startTask(TAB, `Please ${SENTINEL}`, deps);
  await assert.rejects(startTask(TAB, `Again ${SENTINEL}`, deps), (error) => error.code === "conflict");
  await assert.rejects(startTask(TAB, `Third ${SENTINEL}`, deps), (error) => error.code === "invalid_request");
  host.disconnect();
  await new Promise((resolve) => setTimeout(resolve, DIAG_FLUSH_MS + 200));
  const stored = storage.values.get(DIAG_STORAGE_KEY);
  assert.ok(Array.isArray(stored), "the ring buffer was written");
  const starts = stored.filter((entry) => entry.fields?.kind === "start_task");
  assert.equal(starts.length, 3);
  assert.deepEqual(starts.map((entry) => entry.fields.outcome), ["ok", "conflict", "invalid_request"]);
  const text = JSON.stringify(stored);
  assert.equal(text.includes(SENTINEL), false, "no instruction in diagnostics");
  assert.equal(text.includes(TITLE), false, "no page title in diagnostics");
});
