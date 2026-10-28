import assert from "node:assert/strict";
import test, { mock } from "node:test";

// Run after build:extension. The run card gets a fake DOM, a scripted host,
// and mocked timers, so each poll happens when a test ticks the clock.
const {
  MAX_RESTORED_FINISHED, POLL_MS, RESTORE_FINISHED_WITHIN_MS, RunCards, answerLine, dismissedIds, elapsedText,
  markRunsMayExist, rememberDismissed, restorable, restoreRunCards, resumeCommand, runsMayExist, stateText,
} = await import("../dist/run-card.js");
const { HostError } = await import("../dist/native.js");
const { SERVICE_SETUP_INSTRUCTIONS } = await import("../dist/agent.js");
const { storageKey } = await import("../dist/brand.js");
const { RunProgress } = await import("../dist/run-card.js");

const DISMISSED_KEY = storageKey("dismissed_runs_v1");

class FakeElement {
  constructor(tagName) {
    this.tagName = tagName.toUpperCase();
    this.className = "";
    this.ownText = "";
    this.title = "";
    this.type = "";
    this.hidden = false;
    this.disabled = false;
    this.children = [];
    this.dataset = {};
    this.listeners = new Map();
  }
  get textContent() {
    return [this.ownText, ...this.children.map((child) => child.textContent)].join("");
  }
  set textContent(value) {
    this.ownText = String(value);
    this.children = [];
  }
  set innerHTML(_value) {
    throw new Error("The run card must never render HTML");
  }
  setAttribute() {}
  addEventListener(type, listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }
  click() {
    if (this.disabled) return;
    for (const listener of this.listeners.get("click") ?? []) listener({ type: "click", target: this });
  }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = children; }
}

function walk(root, visit) {
  for (const child of root.children) {
    visit(child);
    walk(child, visit);
  }
}

function buttonTexts(root) {
  const texts = [];
  walk(root, (child) => { if (child.tagName === "BUTTON" && !child.hidden) texts.push(child.textContent); });
  return texts;
}

function button(root, text) {
  let found = null;
  walk(root, (child) => {
    if (!found && child.tagName === "BUTTON" && !child.hidden && child.textContent === text) found = child;
  });
  assert.ok(found, `no ${text} button in ${JSON.stringify(buttonTexts(root))}`);
  return found;
}

function cards(container) {
  return container.children.filter((child) => child.className.split(" ").includes("run-card"));
}

function storageArea(entries = new Map()) {
  return {
    entries,
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
  };
}

const NOW = Date.parse("2026-10-27T16:00:00Z");

function view(overrides = {}) {
  return {
    run_id: "00000000-0000-4000-8000-000000000040", state: "running", harness: "codex",
    record_key: { kind: "url", value: "https://read.example/a" }, written_record_key: null,
    url: "https://read.example/a", title: "A readable article", cwd: "/Users/me/Notes",
    created_at: "2026-10-27T15:59:00.000000000Z", started_at: "2026-10-27T15:59:30.000000000Z",
    finished_at: null, error: null, error_kind: null, thread_id: null, session_id: null,
    summary_path: null, stream_bytes: 0, can_retry_save: false, answer: null,
    ...overrides,
  };
}

const ANSWER = {
  document_type: "report", category: "news", importance: "high", importance_reason: "Ships Friday",
};

// `reply(request)` answers each host call; a HostError it throws (or returns)
// is the host's error answer.
function setup({ reply = () => null, visible = true, storage = storageArea(), summarySaved, extra = {} } = {}) {
  globalThis.document = { createElement: (tag) => new FakeElement(tag) };
  const requests = [];
  const copied = [];
  const state = { visible, settingsOpened: 0 };
  const host = {
    async call(request, expected) {
      requests.push(structuredClone(request));
      const answer = await reply(request);
      if (answer instanceof Error) throw answer;
      if (!answer) throw new HostError("internal", `unexpected ${request.type}`);
      assert.equal(answer.type, expected);
      return answer;
    },
  };
  const container = new FakeElement("div");
  const runCards = new RunCards(container, {
    host,
    now: () => NOW,
    copy: async (text) => { copied.push(text); },
    visible: () => state.visible,
    storage,
    openSettings: () => { state.settingsOpened += 1; },
    ...(summarySaved ? { summarySaved } : {}),
    ...extra,
  });
  return { container, runCards, requests, copied, state, storage, sent: (type) => requests.filter((r) => r.type === type) };
}

async function settle() {
  for (let i = 0; i < 20; i += 1) await new Promise((resolve) => setImmediate(resolve));
}

async function tick(ms = POLL_MS) {
  mock.timers.tick(ms);
  await settle();
}

test.beforeEach(() => mock.timers.enable({ apis: ["setTimeout"] }));
test.afterEach(() => mock.timers.reset());

test("elapsedText counts from start to finish", () => {
  assert.equal(elapsedText(view(), NOW), "30s");
  assert.equal(elapsedText(view({ finished_at: "2026-10-27T16:00:35.500000000Z" }), NOW + 3_600_000), "1m 05s");
  assert.equal(elapsedText(view({ started_at: "2026-10-27T14:57:30Z" }), NOW), "1h 02m");
  assert.equal(elapsedText(view({ state: "queued", started_at: null }), NOW), "");
});

test("resume command quotes the knowledge base path", () => {
  assert.equal(
    resumeCommand("/Users/me/My Notes/it's", "1b4e28ba-2fa1-11d2-883f-0016d3cca427"),
    "cd '/Users/me/My Notes/it'\\''s' && claude --resume 1b4e28ba-2fa1-11d2-883f-0016d3cca427",
  );
  // A session id the harness reported with shell characters is quoted too.
  assert.equal(resumeCommand("/kb", "a;rm -rf ~"), "cd '/kb' && claude --resume 'a;rm -rf ~'");
});

test("answer line shows type, category and importance", () => {
  assert.equal(answerLine(ANSWER), "Type: report · Category: news · Importance: high — Ships Friday");
  assert.equal(stateText(view({ state: "queued" })), "Queued");
  assert.equal(stateText(view()), "Running");
  assert.equal(stateText(view({ state: "succeeded" })), "Summarized");
  assert.equal(stateText(view({ state: "failed", error: "The harness exited." })), "Failed: The harness exited.");
  assert.equal(stateText(view({ state: "canceled" })), "Canceled");
});

test("stateText says Done for a task, Drafted for a site draft, Summarized for a summary", () => {
  assert.equal(stateText(view({ state: "succeeded", kind: "task", instruction: "Go" })), "Done");
  assert.equal(stateText(view({ state: "succeeded", kind: "site_draft" })), "Drafted");
  assert.equal(stateText(view({ state: "succeeded", kind: "summary" })), "Summarized");
  assert.equal(stateText(view({ state: "succeeded" })), "Summarized");
  assert.equal(stateText(view({ state: "running", kind: "task", instruction: "Go" })), "Running");
});

test("polling appends stream text as text and stops after success", async () => {
  const pages = [
    { run: view(), stream_text: "<b>Hello", next_offset: 8, more: true },
    { run: view(), stream_text: " world", next_offset: 14, more: false },
    { run: view({ state: "succeeded", finished_at: "2026-10-27T16:00:10Z", answer: ANSWER }), stream_text: "!", next_offset: 15, more: false },
  ];
  const page = setup({
    reply: (request) => request.type === "get_run" ? { type: "run_fetched", ...pages.shift() } : null,
  });
  page.runCards.show(view());
  await settle();
  // One poll follows `more` to the end of the stream.
  assert.deepEqual(page.sent("get_run").map((r) => [r.run_id, r.since_offset]),
    [[view().run_id, 0], [view().run_id, 8]]);
  const [card] = cards(page.container);
  assert.match(card.textContent, /Running/);
  assert.match(card.textContent, /A readable article/);

  await tick();
  assert.deepEqual(page.sent("get_run").map((r) => r.since_offset), [0, 8, 14]);
  let stream = null;
  walk(card, (child) => { if (child.tagName === "PRE") stream = child; });
  assert.equal(stream.textContent, "<b>Hello world!");
  assert.deepEqual(stream.children, []);
  assert.match(card.textContent, /Summarized/);
  assert.match(card.textContent, /Type: report · Category: news · Importance: high — Ships Friday/);
  assert.ok(!buttonTexts(card).includes("Cancel"));

  await tick(POLL_MS * 5);
  assert.equal(page.sent("get_run").length, 3);
  page.runCards.dispose();
});

test("polling pauses while the panel is hidden", async () => {
  const page = setup({
    visible: false,
    reply: (request) => request.type === "get_run"
      ? { type: "run_fetched", run: view(), stream_text: "", next_offset: 0, more: false }
      : null,
  });
  page.runCards.show(view());
  await settle();
  await tick(POLL_MS * 3);
  assert.equal(page.sent("get_run").length, 0);
  page.state.visible = true;
  await tick();
  assert.equal(page.sent("get_run").length, 1);
  await tick();
  assert.equal(page.sent("get_run").length, 2);
  page.runCards.dispose();
  await tick(POLL_MS * 3);
  assert.equal(page.sent("get_run").length, 2);
});

test("cancel sends cancel_run and shows Canceled", async () => {
  let current = view();
  const page = setup({
    reply: (request) => {
      if (request.type === "get_run") return { type: "run_fetched", run: current, stream_text: "", next_offset: 0, more: false };
      if (request.type === "cancel_run") {
        current = view({ state: "canceled", finished_at: "2026-10-27T16:00:01Z", error: "Canceled.", error_kind: "canceled" });
        return { type: "run_updated", run: current };
      }
      return null;
    },
  });
  page.runCards.show(view({ state: "queued", started_at: null }));
  await settle();
  const [card] = cards(page.container);
  button(card, "Cancel").click();
  await settle();
  assert.deepEqual(page.sent("cancel_run").map((r) => r.run_id), [view().run_id]);
  assert.match(card.textContent, /Canceled/);
  assert.ok(!buttonTexts(card).includes("Cancel"));
  // One last fetch after the terminal state, then no more.
  const fetched = page.sent("get_run").length;
  await tick(POLL_MS * 4);
  assert.equal(page.sent("get_run").length, fetched + 1);
  page.runCards.dispose();
});

test("codex run offers Open in Codex; claude run offers Copy resume command", async () => {
  const codex = view({ state: "succeeded", thread_id: "thread-1", answer: ANSWER, finished_at: "2026-10-27T16:00:00Z" });
  const claude = view({
    run_id: "00000000-0000-4000-8000-000000000041", state: "succeeded", harness: "claude",
    session_id: "session-1", cwd: "/Users/me/My Notes", answer: ANSWER, finished_at: "2026-10-27T16:00:00Z",
  });
  const noCwd = view({ run_id: "00000000-0000-4000-8000-000000000042", harness: "claude", session_id: "session-2", cwd: null });
  const page = setup({
    reply: (request) => {
      if (request.type === "get_run") {
        const run = [codex, claude, noCwd].find((candidate) => candidate.run_id === request.run_id);
        return { type: "run_fetched", run, stream_text: "", next_offset: 0, more: false };
      }
      if (request.type === "open_path") return { type: "path_opened" };
      return null;
    },
  });
  page.runCards.show(codex);
  page.runCards.show(claude);
  page.runCards.show(noCwd);
  await settle();
  const [codexCard, claudeCard, noCwdCard] = [codex, claude, noCwd].map((run) => {
    const found = cards(page.container).find((card) => card.dataset.runId === run.run_id);
    assert.ok(found, `no card for ${run.run_id}`);
    return found;
  });

  assert.ok(buttonTexts(codexCard).includes("Open in Codex"));
  assert.ok(!buttonTexts(codexCard).includes("Copy resume command"));
  button(codexCard, "Open in Codex").click();
  await settle();
  assert.deepEqual(page.sent("open_path").map((r) => [r.target, r.run_id]), [["codex_thread", codex.run_id]]);

  assert.ok(buttonTexts(claudeCard).includes("Copy resume command"));
  assert.ok(!buttonTexts(claudeCard).includes("Open in Codex"));
  button(claudeCard, "Copy resume command").click();
  await settle();
  assert.deepEqual(page.copied, ["cd '/Users/me/My Notes' && claude --resume session-1"]);

  assert.ok(!buttonTexts(noCwdCard).includes("Copy resume command"));
  page.runCards.dispose();
});

test("a save failure offers Retry save and retrying resumes polling", async () => {
  const failed = view({
    state: "failed", finished_at: "2026-10-27T16:00:00Z", error: "The notes folder is not mounted.",
    error_kind: "save_failed", can_retry_save: true,
  });
  const saved = view({ state: "succeeded", finished_at: "2026-10-27T16:00:20Z", answer: ANSWER });
  let current = failed;
  const page = setup({
    reply: (request) => {
      if (request.type === "get_run") return { type: "run_fetched", run: current, stream_text: "", next_offset: 0, more: false };
      if (request.type === "retry_save") {
        current = saved;
        return { type: "run_updated", run: saved };
      }
      return null;
    },
  });
  page.runCards.show(failed);
  await settle();
  const [card] = cards(page.container);
  assert.match(card.textContent, /Failed: The notes folder is not mounted\./);
  assert.ok(buttonTexts(card).includes("Copy"));
  const before = page.sent("get_run").length;
  button(card, "Retry save").click();
  await settle();
  assert.deepEqual(page.sent("retry_save").map((r) => r.run_id), [failed.run_id]);
  assert.match(card.textContent, /Summarized/);
  assert.ok(!buttonTexts(card).includes("Retry save"));
  await tick();
  assert.equal(page.sent("get_run").length, before + 1);
  page.runCards.dispose();
});

test("other failures offer no Retry save but keep Copy", async () => {
  const failed = view({
    state: "failed", finished_at: "2026-10-27T16:00:00Z", error: "The answer was not valid JSON.",
    error_kind: "invalid_output", can_retry_save: false,
  });
  const silent = view({ ...failed, run_id: "00000000-0000-4000-8000-000000000043", error: "The harness exited." });
  const page = setup({
    reply: (request) => request.type === "get_run"
      ? { type: "run_fetched", run: request.run_id === failed.run_id ? failed : silent, stream_text: request.run_id === failed.run_id ? "partial text" : "", next_offset: 12, more: false }
      : null,
  });
  page.runCards.show(failed);
  page.runCards.show(silent);
  await settle();
  const all = cards(page.container);
  assert.equal(all.length, 2);
  for (const card of all) {
    assert.ok(!buttonTexts(card).includes("Retry save"));
    assert.ok(buttonTexts(card).includes("Copy"));
  }
  const textCard = all.find((card) => card.textContent.includes("not valid JSON"));
  const errorCard = all.find((card) => card.textContent.includes("harness exited"));
  button(textCard, "Copy").click();
  button(errorCard, "Copy").click();
  await settle();
  assert.deepEqual(page.copied, ["partial text", "The harness exited."]);
  page.runCards.dispose();
});

test("restore shows in-flight runs and at most 5 recent finished, skipping dismissed and older than a day", async () => {
  const id = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
  const minutesAgo = (m) => new Date(NOW - m * 60_000).toISOString();
  const finished = (n, minutes) => view({ run_id: id(n), state: "succeeded", answer: ANSWER, finished_at: minutesAgo(minutes) });
  const runs = [
    view({ run_id: id(1), state: "running" }),
    view({ run_id: id(2), state: "queued", started_at: null }),
    finished(3, 1), finished(4, 2), finished(5, 3), finished(6, 4),
    finished(7, 5), finished(8, 6), finished(9, 7),
    finished(10, 24 * 60 + 1),
  ];
  const dismissed = new Set([id(4)]);
  assert.equal(MAX_RESTORED_FINISHED, 5);
  assert.equal(RESTORE_FINISHED_WITHIN_MS, 24 * 60 * 60 * 1000);
  assert.deepEqual(restorable(runs, NOW, dismissed).map((run) => run.run_id),
    [id(1), id(2), id(3), id(5), id(6), id(7), id(8)]);

  const storage = storageArea(new Map([[DISMISSED_KEY, [id(4)]]]));
  const page = setup({
    storage,
    reply: (request) => {
      if (request.type === "list_runs") return { type: "runs_listed", runs };
      if (request.type === "get_run") {
        return { type: "run_fetched", run: runs.find((run) => run.run_id === request.run_id), stream_text: "", next_offset: 0, more: false };
      }
      return null;
    },
  });
  await page.runCards.restore();
  await settle();
  assert.equal(cards(page.container).length, 7);
  // Restoring again shows each run once.
  await page.runCards.restore();
  await settle();
  assert.equal(cards(page.container).length, 7);

  // Dismiss removes a finished card and remembers it, keeping the newest 1000.
  storage.entries.set(DISMISSED_KEY, Array.from({ length: 1000 }, (_, n) => id(1000 + n)));
  const card = cards(page.container).find((candidate) => candidate.textContent.includes("Summarized"));
  assert.ok(!buttonTexts(cards(page.container)[0]).includes("Dismiss"), "an in-flight card has no Dismiss");
  button(card, "Dismiss").click();
  await settle();
  assert.equal(cards(page.container).length, 6);
  const stored = storage.entries.get(DISMISSED_KEY);
  assert.equal(stored.length, 1000);
  assert.equal(stored.at(-1), id(3));
  assert.ok(!stored.includes(id(1000)));
  page.runCards.dispose();
});

test("rememberDismissed keeps the newest 1000", async () => {
  const id = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
  const storage = storageArea(new Map([[DISMISSED_KEY, Array.from({ length: 1000 }, (_, n) => id(n))]]));
  await rememberDismissed(storage, id(1000));
  const stored = storage.entries.get(DISMISSED_KEY);
  assert.equal(stored.length, 1000);
  assert.equal(stored.at(-1), id(1000));
  assert.ok(!stored.includes(id(0)));
  assert.deepEqual(await dismissedIds(storage), stored);
});

test("dismissing two cards at once remembers both", async () => {
  const id = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
  const runs = [1, 2].map((n) => view({
    run_id: id(n), state: "succeeded", answer: ANSWER, finished_at: new Date(NOW - n * 60_000).toISOString(),
  }));
  const storage = storageArea(new Map([[DISMISSED_KEY, [id(9)]]]));
  const page = setup({
    storage,
    reply: (request) => {
      if (request.type === "list_runs") return { type: "runs_listed", runs };
      if (request.type === "get_run") {
        return { type: "run_fetched", run: runs.find((run) => run.run_id === request.run_id), stream_text: "", next_offset: 0, more: false };
      }
      return null;
    },
  });
  await page.runCards.restore();
  await settle();
  assert.equal(cards(page.container).length, 2);
  // Both clicks land before either storage read completes.
  const [first, second] = cards(page.container);
  button(first, "Dismiss").click();
  button(second, "Dismiss").click();
  await settle();
  assert.equal(cards(page.container).length, 0);
  assert.deepEqual([...storage.entries.get(DISMISSED_KEY)].sort(), [id(1), id(2), id(9)]);
  page.runCards.dispose();
});

test("service_unavailable shows setup instructions", async () => {
  const message = "The Brauser service did not answer.";
  const page = setup({
    reply: (request) => request.type === "list_runs" ? new HostError("service_unavailable", message) : null,
  });
  await page.runCards.restore();
  assert.ok(page.container.textContent.includes(SERVICE_SETUP_INSTRUCTIONS));
  assert.ok(page.container.textContent.includes(message));
  button(page.container, "Open settings").click();
  assert.equal(page.state.settingsOpened, 1);
  page.runCards.dispose();
});

const SAVED = {
  summarized_at: "2026-10-27T16:00:10.000000000Z", document_type: "report", category: "news",
  importance: "high", summary_path: "Summaries/read.example/a.summary.md", harness: "codex",
};

test("a succeeded card shows the saved summary line and opens it", async () => {
  const running = view();
  const saved = view({
    state: "succeeded", finished_at: "2026-10-27T16:00:10Z", answer: ANSWER,
    summary_path: SAVED.summary_path, written_record_key: running.record_key,
  });
  let current = running;
  const page = setup({
    reply: (request) => {
      if (request.type === "get_run") return { type: "run_fetched", run: current, stream_text: "", next_offset: 0, more: false };
      if (request.type === "get_artifact") return { type: "artifact_result", recognized: false, artifact: null, summary: SAVED };
      if (request.type === "open_summary") return { type: "summary_opened", relative_path: SAVED.summary_path };
      return null;
    },
  });
  page.runCards.show(running);
  await settle();
  const [card] = cards(page.container);
  assert.equal(page.sent("get_artifact").length, 0);
  assert.ok(!buttonTexts(card).includes("Open summary"));

  current = saved;
  await tick();
  assert.deepEqual(page.sent("get_artifact").map((r) => [r.url, r.title]), [[saved.url, saved.title]]);
  assert.match(card.textContent, /report · news · high importance/);
  // Agent text is set as text, never as child nodes or HTML.
  let line = null;
  walk(card, (child) => { if (child.className === "run-card-summary") line = child; });
  assert.ok(line && !line.hidden);
  assert.deepEqual(line.children, []);

  button(card, "Open summary").click();
  await settle();
  // By the file the run wrote, not only its URL.
  assert.deepEqual(page.sent("open_summary").map((r) => [r.url, r.summary_path]), [[saved.url, SAVED.summary_path]]);
  assert.ok(!card.textContent.includes("Could not open"));

  // Once per saved file: later updates and polls ask nothing more.
  page.runCards.show(saved);
  await tick(POLL_MS * 4);
  assert.equal(page.sent("get_artifact").length, 1);
  page.runCards.dispose();
});

test("a run seen succeeding tells the panel once, so its page summary refreshes", async () => {
  const running = view();
  const saved = view({
    state: "succeeded", finished_at: "2026-10-27T16:00:10Z", answer: ANSWER,
    summary_path: SAVED.summary_path, written_record_key: running.record_key,
  });
  let current = running;
  const told = [];
  const page = setup({
    summarySaved: (run) => told.push(run.run_id),
    reply: (request) => {
      if (request.type === "get_run") return { type: "run_fetched", run: current, stream_text: "", next_offset: 0, more: false };
      if (request.type === "get_artifact") return { type: "artifact_result", recognized: false, artifact: null, summary: SAVED };
      return null;
    },
  });
  page.runCards.show(running);
  await settle();
  assert.deepEqual(told, []);
  current = saved;
  await tick();
  assert.deepEqual(told, [saved.run_id]);
  page.runCards.show(saved);
  await tick(POLL_MS * 3);
  assert.deepEqual(told, [saved.run_id]);
  // A card restored already succeeded changes nothing on the page.
  const other = view({ ...saved, run_id: "00000000-0000-4000-8000-000000000041" });
  page.runCards.show(other);
  await tick();
  assert.deepEqual(told, [saved.run_id]);
  page.runCards.dispose();
});

test("a save failure no retry can fix offers no Retry save but opens the saved file", async () => {
  const failed = view({
    state: "failed", finished_at: "2026-10-27T16:00:10Z", error_kind: "save_failed",
    error: "The day's daily log is full, so this summary cannot be logged. The summary file was saved.",
    summary_path: SAVED.summary_path, can_retry_save: false, answer: ANSWER,
  });
  const page = setup({
    reply: (request) => {
      if (request.type === "get_run") return { type: "run_fetched", run: failed, stream_text: "", next_offset: 0, more: false };
      if (request.type === "open_summary") return { type: "summary_opened", relative_path: SAVED.summary_path };
      return null;
    },
  });
  page.runCards.show(failed);
  await tick();
  const [card] = cards(page.container);
  assert.ok(!buttonTexts(card).includes("Retry save"));
  // No log event, so no saved line is asked for.
  assert.equal(page.sent("get_artifact").length, 0);
  button(card, "Open summary").click();
  await settle();
  // By the file the run wrote, not only its URL.
  assert.deepEqual(page.sent("open_summary").map((r) => [r.url, r.summary_path]), [[failed.url, SAVED.summary_path]]);
  page.runCards.dispose();
});

test("a succeeded card without summary_path makes no get_artifact call", async () => {
  const done = view({ state: "succeeded", finished_at: "2026-10-27T16:00:10Z", answer: ANSWER });
  const page = setup({
    reply: (request) => request.type === "get_run"
      ? { type: "run_fetched", run: done, stream_text: "", next_offset: 0, more: false }
      : null,
  });
  page.runCards.show(done);
  await tick(POLL_MS * 3);
  const [card] = cards(page.container);
  assert.equal(page.sent("get_artifact").length, 0);
  assert.ok(!buttonTexts(card).includes("Open summary"));
  page.runCards.dispose();
});

test("a restored card with an untitled page asks with a null title, and an open failure shows in the card", async () => {
  const done = view({
    state: "succeeded", finished_at: "2026-10-27T16:00:10Z", answer: ANSWER, title: "",
    summary_path: SAVED.summary_path,
  });
  const page = setup({
    visible: false,
    reply: (request) => {
      if (request.type === "get_artifact") return { type: "artifact_result", recognized: false, artifact: null, summary: SAVED };
      if (request.type === "open_summary") return new HostError("conflict", "The file at the summary path is not a Brauser summary.");
      return null;
    },
  });
  page.runCards.show(done);
  await settle();
  // The card asks once even while the panel is hidden, since it does not poll for it.
  assert.deepEqual(page.sent("get_artifact").map((r) => [r.url, r.title]), [[done.url, null]]);
  const [card] = cards(page.container);
  assert.match(card.textContent, /report · news · high importance/);
  button(card, "Open summary").click();
  await settle();
  assert.match(card.textContent, /Could not open the summary: The file at the summary path is not a Brauser summary\./);
  page.runCards.dispose();
});

test("a failed get_artifact keeps Open summary and shows no saved line", async () => {
  const done = view({
    state: "succeeded", finished_at: "2026-10-27T16:00:10Z", answer: ANSWER, summary_path: SAVED.summary_path,
  });
  const page = setup({
    reply: (request) => {
      if (request.type === "get_run") return { type: "run_fetched", run: done, stream_text: "", next_offset: 0, more: false };
      if (request.type === "get_artifact") return new HostError("internal", "The index is busy.");
      return null;
    },
  });
  page.runCards.show(done);
  await settle();
  const [card] = cards(page.container);
  assert.deepEqual(page.sent("get_artifact").map((r) => [r.url, r.title]), [[done.url, done.title]]);
  assert.ok(!card.textContent.includes("importance"));
  assert.ok(buttonTexts(card).includes("Open summary"));
  // One retry on a later poll, then no more.
  await tick();
  assert.equal(page.sent("get_artifact").length, 2);
  await tick(POLL_MS * 4);
  assert.equal(page.sent("get_artifact").length, 2);
  assert.ok(!card.textContent.includes("importance"));
  page.runCards.dispose();
});

test("a get_artifact that fails once is retried on a later poll and shows the saved line", async () => {
  const done = view({
    state: "succeeded", finished_at: "2026-10-27T16:00:10Z", answer: ANSWER, summary_path: SAVED.summary_path,
  });
  const replies = [
    new HostError("internal", "The index is busy."),
    { type: "artifact_result", recognized: false, artifact: null, summary: SAVED },
  ];
  const page = setup({
    reply: (request) => {
      if (request.type === "get_run") return { type: "run_fetched", run: done, stream_text: "", next_offset: 0, more: false };
      if (request.type === "get_artifact") return replies.shift();
      return null;
    },
  });
  page.runCards.show(done);
  await settle();
  const [card] = cards(page.container);
  assert.equal(page.sent("get_artifact").length, 1);
  assert.ok(!card.textContent.includes("importance"));
  await tick();
  assert.equal(page.sent("get_artifact").length, 2);
  assert.match(card.textContent, /report · news · high importance/);
  await tick(POLL_MS * 4);
  assert.equal(page.sent("get_artifact").length, 2);
  page.runCards.dispose();
});

test("a finished run's multi-page stream keeps fetching through a transient failure", async () => {
  const done = view({ state: "succeeded", finished_at: "2026-10-27T16:00:10Z", answer: ANSWER });
  const replies = [
    { type: "run_fetched", run: done, stream_text: "first ", next_offset: 6, more: true },
    new HostError("internal", "the host restarted"),
    { type: "run_fetched", run: done, stream_text: "second", next_offset: 12, more: false },
  ];
  const page = setup({ reply: (request) => request.type === "get_run" ? replies.shift() : null });
  page.runCards.show(done);
  await settle();
  const [card] = cards(page.container);
  assert.match(card.textContent, /Could not refresh this run: the host restarted/);
  // The terminal page with more stream is not the final fetch: polling goes on.
  await tick();
  assert.deepEqual(page.sent("get_run").map((r) => r.since_offset), [0, 6, 6]);
  let stream = null;
  walk(card, (child) => { if (child.tagName === "PRE") stream = child; });
  assert.equal(stream.textContent, "first second");
  assert.doesNotMatch(card.textContent, /Could not refresh/);
  await tick(POLL_MS * 4);
  assert.equal(page.sent("get_run").length, 3);
  page.runCards.dispose();
});

test("a shown run records that runs may exist; an empty list_runs clears it", async () => {
  const storage = storageArea();
  let runs = [];
  const page = setup({
    storage,
    reply: (request) => {
      if (request.type === "list_runs") return { type: "runs_listed", runs };
      if (request.type === "get_run") return { type: "run_fetched", run: view(), stream_text: "", next_offset: 0, more: false };
      return null;
    },
  });
  assert.equal(await runsMayExist(storage), false);
  await markRunsMayExist(storage, NOW);
  assert.equal(await runsMayExist(storage), true);
  // Nothing active or undismissed: the record goes.
  await page.runCards.restore();
  await settle();
  assert.equal(await runsMayExist(storage), false);

  // A restored card keeps it, and so does a card shown in this panel.
  runs = [view()];
  await page.runCards.restore();
  await settle();
  assert.equal(await runsMayExist(storage), true);
  page.runCards.dispose();

  const other = setup({
    storage: storageArea(),
    reply: (request) => request.type === "get_run"
      ? { type: "run_fetched", run: view(), stream_text: "", next_offset: 0, more: false }
      : null,
  });
  other.runCards.show(view());
  await settle();
  assert.equal(await runsMayExist(other.storage), true);
  other.runCards.dispose();
});

test("a run recorded while list_runs is in flight is not cleared", async () => {
  const storage = storageArea();
  let release;
  const page = setup({
    storage,
    reply: (request) => request.type === "list_runs"
      ? new Promise((resolve) => { release = resolve; })
      : null,
  });
  const restoring = page.runCards.restore();
  await settle();
  // The worker's shortcut starts a run after the list was read.
  await markRunsMayExist(storage, NOW + 1);
  release({ type: "runs_listed", runs: [] });
  await restoring;
  await settle();
  assert.equal(await runsMayExist(storage), true);
  page.runCards.dispose();
});

test("cards show only for the followed page", async () => {
  const other = view({
    run_id: "00000000-0000-4000-8000-000000000041", url: "https://read.example/b", title: "Another page",
  });
  const page = setup({
    reply: (request) => request.type === "get_run"
      ? { type: "run_fetched", run: request.run_id === other.run_id ? other : view(), stream_text: "", next_offset: 0, more: false }
      : null,
  });
  page.runCards.follow("https://read.example/a#section");
  page.runCards.show(view());
  page.runCards.show(other);
  await settle();
  const shown = () => cards(page.container).filter((card) => !card.hidden).map((card) => card.dataset.runId);
  // A card for another page is kept, hidden, so it is back when its page is.
  assert.equal(cards(page.container).length, 2);
  assert.deepEqual(shown(), [view().run_id]);
  page.runCards.follow("https://read.example/b");
  assert.deepEqual(shown(), [other.run_id]);
  // A page that is not a web page shows no card.
  page.runCards.follow(null);
  assert.deepEqual(shown(), []);
  page.runCards.dispose();
});

test("panel.css hides a hidden run card despite its display rule", async () => {
  // `.run-card { display: grid }` outranks Chrome's [hidden] style, so the
  // card itself needs its own rule or follow() hides nothing on screen.
  const { readFile } = await import("node:fs/promises");
  const css = await readFile(new URL("../panel.css", import.meta.url), "utf8");
  assert.match(css, /\.run-card\[hidden\][^{]*\{\s*display:\s*none;/);
});

const SITE_DRAFT = {
  kind: "site_draft", url: "https://wiki.example.com", title: "Capture Site: wiki.example.com",
  record_key: { kind: "url", value: "https://wiki.example.com" }, cwd: "/Users/me/Library/Application Support/Brauser/runs/x",
};

const ANSWER_DRAFT = { adapters: [{ type: "wiki.pages", origin: "https://wiki.example.com", match: [{ path: "/pages/{n:int}" }], id: "wiki.pages:{host}/{n}" }] };

test("restored summary cards skip site-draft runs", async () => {
  const id = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
  const runs = [
    view({ run_id: id(1), state: "running" }),
    view({ run_id: id(2), state: "running", ...SITE_DRAFT }),
    view({ run_id: id(3), state: "succeeded", answer: ANSWER, finished_at: "2026-10-27T15:59:50Z", kind: "summary" }),
    view({ run_id: id(4), state: "succeeded", finished_at: "2026-10-27T15:59:55Z", ...SITE_DRAFT, site_draft: ANSWER_DRAFT }),
    view({ run_id: id(5), state: "queued", started_at: null, ...SITE_DRAFT }),
  ];
  assert.deepEqual(restorable(runs, NOW, new Set()).map((run) => run.run_id), [id(1), id(3)]);
  const page = setup({
    reply: (request) => {
      if (request.type === "list_runs") return { type: "runs_listed", runs };
      if (request.type === "get_run") {
        return { type: "run_fetched", run: runs.find((run) => run.run_id === request.run_id), stream_text: "", next_offset: 0, more: false };
      }
      return null;
    },
  });
  await page.runCards.restore();
  await settle();
  assert.deepEqual(cards(page.container).map((card) => card.dataset.runId), [id(1), id(3)]);
  assert.ok(page.sent("get_run").every((request) => request.run_id === id(1) || request.run_id === id(3)));
  page.runCards.dispose();
});

function progressSetup({ reply, visible = true, run = view({ state: "queued", started_at: null, ...SITE_DRAFT }) }) {
  globalThis.document = { createElement: (tag) => new FakeElement(tag) };
  const requests = [];
  const copied = [];
  const finished = [];
  const state = { visible };
  const host = {
    async call(request, expected) {
      requests.push(structuredClone(request));
      const answer = await reply(request);
      if (answer instanceof Error) throw answer;
      if (!answer) throw new HostError("internal", `unexpected ${request.type}`);
      assert.equal(answer.type, expected);
      return answer;
    },
  };
  const progress = new RunProgress(run, {
    host, now: () => NOW, copy: async (text) => { copied.push(text); }, visible: () => state.visible,
  }, (done) => finished.push(structuredClone(done)));
  return { progress, requests, copied, finished, state, sent: (type) => requests.filter((r) => r.type === type) };
}

test("run progress streams text as text, cancels, and reports the finished run once", async () => {
  const running = view({ ...SITE_DRAFT, thread_id: "thread-9" });
  const pages = [
    { run: running, stream_text: "<img src=x onerror=alert(1)>", next_offset: 28, more: false },
    { run: view({ ...SITE_DRAFT, thread_id: "thread-9", state: "succeeded", finished_at: "2026-10-27T16:00:20Z", site_draft: ANSWER_DRAFT }), stream_text: " done", next_offset: 33, more: false },
  ];
  const page = progressSetup({
    reply: (request) => request.type === "get_run" ? { type: "run_fetched", ...pages.shift() } : null,
  });
  page.progress.start();
  await settle();
  const root = page.progress.element;
  assert.equal(root.dataset.runId, running.run_id);
  assert.match(root.textContent, /Running · 30s/);
  let stream = null;
  walk(root, (child) => { if (child.tagName === "PRE") stream = child; });
  assert.equal(stream.textContent, "<img src=x onerror=alert(1)>");
  assert.deepEqual(stream.children, []);
  // Only progress controls: no answer, Open summary, Retry save or Dismiss.
  assert.deepEqual(buttonTexts(root), ["Cancel", "Open in Codex"]);
  assert.deepEqual(page.finished, []);

  await tick();
  assert.match(root.textContent, /Drafted/);
  assert.doesNotMatch(root.textContent, /Summarized|Type:/);
  assert.equal(stream.textContent, "<img src=x onerror=alert(1)> done");
  assert.deepEqual(buttonTexts(root), ["Open in Codex"]);
  assert.equal(page.finished.length, 1);
  assert.equal(page.finished[0].state, "succeeded");
  assert.deepEqual(page.finished[0].site_draft, ANSWER_DRAFT);
  // Finished is reported once, and polling stops.
  await tick(POLL_MS * 4);
  assert.equal(page.finished.length, 1);
  assert.equal(page.sent("get_run").length, 2);
  page.progress.dispose();

  // Cancel: cancel_run, then one final fetch, then finished once as canceled.
  let current = view({ ...SITE_DRAFT, harness: "claude", session_id: "s-1" });
  const canceling = progressSetup({
    run: current,
    reply: (request) => {
      if (request.type === "get_run") return { type: "run_fetched", run: current, stream_text: "", next_offset: 0, more: false };
      if (request.type === "cancel_run") {
        current = view({ ...SITE_DRAFT, harness: "claude", session_id: "s-1", state: "canceled", finished_at: "2026-10-27T16:00:01Z", error: "Canceled.", error_kind: "canceled" });
        return { type: "run_updated", run: current };
      }
      return null;
    },
  });
  canceling.progress.start();
  await settle();
  assert.deepEqual(buttonTexts(canceling.progress.element), ["Cancel", "Copy resume command"]);
  button(canceling.progress.element, "Copy resume command").click();
  await settle();
  assert.deepEqual(canceling.copied, [`cd '${SITE_DRAFT.cwd}' && claude --resume s-1`]);
  button(canceling.progress.element, "Cancel").click();
  await settle();
  assert.deepEqual(canceling.sent("cancel_run").map((r) => r.run_id), [current.run_id]);
  assert.match(canceling.progress.element.textContent, /Canceled/);
  assert.deepEqual(canceling.finished, []);
  await tick();
  assert.deepEqual(canceling.finished.map((run) => run.state), ["canceled"]);
  await tick(POLL_MS * 3);
  assert.equal(canceling.finished.length, 1);
  canceling.progress.dispose();

  // close() cancels an unfinished run and stops, reporting nothing.
  const closing = progressSetup({
    reply: (request) => {
      if (request.type === "get_run") return { type: "run_fetched", run: running, stream_text: "", next_offset: 0, more: false };
      if (request.type === "cancel_run") return { type: "run_updated", run: view({ ...SITE_DRAFT, state: "canceled", finished_at: "2026-10-27T16:00:01Z" }) };
      return null;
    },
  });
  closing.progress.start();
  await settle();
  await closing.progress.close();
  await settle();
  assert.deepEqual(closing.sent("cancel_run").map((r) => r.run_id), [running.run_id]);
  const fetched = closing.sent("get_run").length;
  await tick(POLL_MS * 3);
  assert.equal(closing.sent("get_run").length, fetched);
  assert.deepEqual(closing.finished, []);
  // close() on a finished run sends nothing.
  const done = progressSetup({
    run: view({ ...SITE_DRAFT, state: "failed", finished_at: "2026-10-27T16:00:01Z", error: "bad", error_kind: "invalid_output" }),
    reply: (request) => request.type === "get_run"
      ? { type: "run_fetched", run: view({ ...SITE_DRAFT, state: "failed", finished_at: "2026-10-27T16:00:01Z", error: "bad", error_kind: "invalid_output" }), stream_text: "", next_offset: 0, more: false }
      : null,
  });
  done.progress.start();
  await settle();
  assert.deepEqual(done.finished.map((run) => run.state), ["failed"]);
  assert.match(done.progress.element.textContent, /Failed: bad/);
  await done.progress.close();
  assert.deepEqual(done.sent("cancel_run"), []);
});

const TASK = { kind: "task", instruction: "List every link on this page", task_report: null };

function taskView(overrides = {}) {
  return view({ ...TASK, ...overrides });
}

function byClass(root, name) {
  let found = null;
  walk(root, (child) => { if (!found && child.className.split(" ").includes(name)) found = child; });
  return found;
}

test("restorable keeps summary runs only, never tasks or site drafts", () => {
  const id = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
  const runs = [
    view({ run_id: id(1), state: "running" }),
    view({ run_id: id(2), state: "running", kind: "summary" }),
    taskView({ run_id: id(3), state: "running" }),
    view({ run_id: id(4), state: "running", ...SITE_DRAFT }),
    taskView({ run_id: id(5), state: "succeeded", finished_at: "2026-10-27T15:59:00Z" }),
    view({ run_id: id(6), state: "succeeded", finished_at: "2026-10-27T15:59:10Z", ...SITE_DRAFT }),
    view({ run_id: id(7), state: "succeeded", answer: ANSWER, finished_at: "2026-10-27T15:59:20Z" }),
    taskView({ run_id: id(8), state: "failed", error: "x", finished_at: "2026-10-27T15:59:30Z" }),
  ];
  assert.deepEqual(restorable(runs, NOW, new Set()).map((run) => run.run_id), [id(1), id(2), id(7)]);
  // A kind the panel does not know is not a summary.
  const unknown = [view({ run_id: id(9), state: "running", kind: "other" })];
  assert.deepEqual(restorable(unknown, NOW, new Set()), []);
});

function restoreSetup({ reply, storage = storageArea() }) {
  globalThis.document = { createElement: (tag) => new FakeElement(tag) };
  const requests = [];
  const state = { settingsOpened: 0 };
  const host = {
    async call(request, expected) {
      requests.push(structuredClone(request));
      const answer = await reply(request);
      if (answer instanceof Error) throw answer;
      if (!answer) throw new HostError("internal", `unexpected ${request.type}`);
      assert.equal(answer.type, expected);
      return answer;
    },
  };
  const deps = {
    host, now: () => NOW, copy: async () => {}, visible: () => true, storage,
    openSettings: () => { state.settingsOpened += 1; },
  };
  const summaryArea = new FakeElement("div");
  const noticeArea = new FakeElement("div");
  const summaries = new RunCards(summaryArea, deps);
  return {
    summaryArea, noticeArea, summaries, storage, state, deps,
    sent: (type) => requests.filter((r) => r.type === type),
    restore: (options) => restoreRunCards([summaries], noticeArea, deps, options),
    dispose: () => summaries.dispose(),
  };
}

test("summary restore asks list_runs for kind summary, and clears runsMayExist only when nothing shows", async () => {
  let runs = [view({ state: "running" })];
  const reply = (request) => {
    if (request.type === "list_runs") return { type: "runs_listed", runs, more: false };
    if (request.type === "get_run") {
      return { type: "run_fetched", run: runs.find((run) => run.run_id === request.run_id) ?? view(), stream_text: "", next_offset: 0, more: false };
    }
    return null;
  };
  const page = restoreSetup({ reply });
  await markRunsMayExist(page.storage, NOW - 1);
  await page.restore();
  await settle();
  const [listed] = page.sent("list_runs");
  assert.equal(page.sent("list_runs").length, 1);
  assert.equal(listed.kind, "summary");
  assert.equal("before" in listed, false);
  assert.equal(cards(page.summaryArea).length, 1);
  assert.equal(await runsMayExist(page.storage), true);
  page.dispose();

  // Nothing to show: the record goes, after one list_runs.
  runs = [];
  const empty = restoreSetup({ reply });
  await markRunsMayExist(empty.storage, NOW - 1);
  await empty.restore();
  await settle();
  assert.equal(empty.sent("list_runs").length, 1);
  assert.equal(await runsMayExist(empty.storage), false);
  empty.dispose();

  // Nothing to show, but the caller knows of an active task: the record stays.
  const kept = restoreSetup({ reply });
  await markRunsMayExist(kept.storage, NOW - 1);
  await kept.restore({ keepRecord: true });
  await settle();
  assert.equal(await runsMayExist(kept.storage), true);
  kept.dispose();
});

test("restoreRunCards shows one neutral notice in the named area when listing fails", async () => {
  const message = "The Brauser service did not answer.";
  let failure = new HostError("service_unavailable", message);
  const page = restoreSetup({
    reply: (request) => {
      if (request.type === "list_runs") return failure ?? { type: "runs_listed", runs: [] };
      return null;
    },
  });
  await page.restore();
  const notices = (area) => area.children.filter((child) => child.className.split(" ").includes("run-notice"));
  assert.equal(notices(page.summaryArea).length, 0);
  assert.equal(notices(page.noticeArea).length, 1);
  assert.ok(page.noticeArea.textContent.includes(SERVICE_SETUP_INSTRUCTIONS));
  assert.ok(page.noticeArea.textContent.includes(message));
  button(page.noticeArea, "Open settings").click();
  assert.equal(page.state.settingsOpened, 1);

  // Another failure replaces it, neutrally worded, with no settings button.
  failure = new HostError("internal", "disk full");
  await page.restore();
  assert.equal(notices(page.noticeArea).length, 1);
  assert.equal(page.noticeArea.textContent, "Could not list agent runs: disk full");
  assert.equal(page.summaryArea.textContent, "");
  assert.ok(!buttonTexts(page.noticeArea).includes("Open settings"));

  // An older host refuses the kind field: it needs rebuilding.
  failure = new HostError("invalid_request", "invalid or unsupported request JSON");
  await page.restore();
  assert.equal(notices(page.noticeArea).length, 1);
  assert.equal(page.noticeArea.textContent, "Rebuild the Brauser host to see task history.");

  // A list that works clears it.
  failure = null;
  await page.restore();
  assert.equal(notices(page.noticeArea).length, 0);
  page.dispose();

  // The single-area restore uses the same neutral words.
  const single = setup({
    reply: (request) => request.type === "list_runs" ? new HostError("internal", "disk full") : null,
  });
  await single.runCards.restore();
  assert.equal(single.container.textContent, "Could not list agent runs: disk full");
  single.runCards.dispose();
});

test("dismissing two summaries together keeps both ids", async () => {
  const id = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
  const runs = [
    view({ run_id: id(1), state: "succeeded", answer: ANSWER, finished_at: "2026-10-27T15:59:00Z" }),
    view({ run_id: id(2), state: "failed", error: "x", finished_at: "2026-10-27T15:59:10Z" }),
  ];
  const storage = storageArea(new Map([[DISMISSED_KEY, [id(9)]]]));
  const page = restoreSetup({
    storage,
    reply: (request) => {
      if (request.type === "list_runs") return { type: "runs_listed", runs };
      if (request.type === "get_run") {
        return { type: "run_fetched", run: runs.find((run) => run.run_id === request.run_id), stream_text: "", next_offset: 0, more: false };
      }
      return null;
    },
  });
  await page.restore();
  await settle();
  const [first, second] = cards(page.summaryArea);
  // Both clicks land before either storage read completes.
  button(first, "Dismiss").click();
  button(second, "Dismiss").click();
  await settle();
  assert.equal(cards(page.summaryArea).length, 0);
  assert.deepEqual([...storage.entries.get(DISMISSED_KEY)].sort(), [id(1), id(2), id(9)]);
  // A later restore keeps both out.
  await page.restore();
  await settle();
  assert.equal(cards(page.summaryArea).length, 0);
  page.dispose();
});

const { TaskDetail } = await import("../dist/run-card.js");

// A TaskDetail with a scripted host; `reply` answers get_run.
function detailSetup(initial, { reply = () => null, storage = storageArea() } = {}) {
  globalThis.document = { createElement: (tag) => new FakeElement(tag) };
  const requests = [];
  const copied = [];
  const reused = [];
  const opened = [];
  const dismissed = [];
  const changed = [];
  const host = {
    async call(request, expected) {
      requests.push(structuredClone(request));
      const answer = await reply(request);
      if (answer instanceof Error) throw answer;
      if (!answer) throw new HostError("internal", `unexpected ${request.type}`);
      assert.equal(answer.type, expected);
      return answer;
    },
  };
  const detail = new TaskDetail(initial, {
    host,
    now: () => NOW,
    copy: async (text) => { copied.push(text); },
    visible: () => true,
    storage,
    reuse: (instruction) => reused.push(instruction),
    openPage: (url) => opened.push(url),
    dismissed: (runId) => dismissed.push(runId),
    changed: (run) => changed.push(structuredClone(run)),
  });
  return { detail, requests, copied, reused, opened, dismissed, changed, storage };
}

function output(detail) {
  return byClass(detail.element, "task-detail-output");
}

test("TaskDetail shows Running with live output open, then Done with the report and output closed", async () => {
  const pages = [
    { run: taskView({ thread_id: "thread-1" }), stream_text: "working", next_offset: 7, more: false },
    {
      run: taskView({ thread_id: "thread-1", state: "succeeded", finished_at: "2026-10-27T16:00:10Z", task_report: "Three links." }),
      stream_text: " done", next_offset: 12, more: false,
    },
  ];
  const page = detailSetup(taskView(), { reply: (request) => request.type === "get_run" ? { type: "run_fetched", ...pages.shift() } : null });
  const { detail } = page;
  const started = new Date("2026-10-27T15:59:30.000000000Z").toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  assert.equal(output(detail).tagName, "DETAILS");
  assert.equal(output(detail).open, true);
  detail.start();
  await settle();
  const classes = detail.element.children.map((child) => child.className.split(" ")[0]);
  assert.deepEqual(classes, [
    "task-detail-instruction", "task-detail-meta", "run-card-actions", "run-card-problem", "task-detail-note",
    "task-detail-report-label", "task-detail-report", "task-detail-output",
  ]);
  assert.equal(byClass(detail.element, "task-detail-instruction").tagName, "H2");
  assert.equal(byClass(detail.element, "task-detail-meta").textContent,
    `on A readable article · Codex · started ${started} · Running · 30s`);
  assert.equal(byClass(detail.element, "task-detail-report").hidden, true);
  assert.equal(byClass(detail.element, "task-detail-report-label").hidden, true);
  assert.equal(output(detail).open, true);
  assert.equal(output(detail).children[0].tagName, "SUMMARY");
  assert.equal(output(detail).children[0].textContent, "Agent output");
  assert.equal(byClass(detail.element, "run-card-stream").textContent, "working");
  assert.deepEqual(buttonTexts(detail.element), ["Cancel", "Open in Codex", "Open page"]);
  await tick();
  assert.equal(byClass(detail.element, "task-detail-meta").textContent,
    `on A readable article · Codex · started ${started} · Done · 40s`);
  const report = byClass(detail.element, "task-detail-report");
  assert.equal(report.hidden, false);
  assert.equal(report.textContent, "Three links.");
  assert.equal(byClass(detail.element, "task-detail-report-label").textContent, "Report");
  assert.equal(output(detail).open, false);
  assert.equal(byClass(detail.element, "run-card-stream").textContent, "working done");
  assert.equal(byClass(detail.element, "task-detail-note").hidden, true);
  assert.deepEqual(buttonTexts(detail.element), ["Copy report", "Reuse", "Open in Codex", "Open page", "Dismiss"]);
  button(detail.element, "Copy report").click();
  await settle();
  assert.deepEqual(page.copied, ["Three links."]);
  assert.equal(detail.run.state, "succeeded");
  assert.deepEqual(page.changed.map((run) => run.state), ["running", "succeeded"]);
  detail.dispose();

  // A queued task has no start time yet; a canceled one after it started notes that changes stay.
  const queued = detailSetup(taskView({ state: "queued", started_at: null, harness: "claude", title: "" }));
  assert.equal(byClass(queued.detail.element, "task-detail-meta").textContent,
    "on https://read.example/a · Claude Code · Queued");
  queued.detail.dispose();
  const canceled = taskView({ state: "canceled", finished_at: "2026-10-27T16:00:10Z", error: "Canceled.", error_kind: "canceled" });
  const stopped = detailSetup(canceled, {
    reply: () => ({ type: "run_fetched", run: canceled, stream_text: "partial", next_offset: 7, more: false }),
  });
  assert.equal(output(stopped.detail).open, false);
  stopped.detail.start();
  await settle();
  const note = byClass(stopped.detail.element, "task-detail-note");
  assert.equal(note.hidden, false);
  assert.equal(note.textContent, "Changes the agent already made stay; check before running it again.");
  // Copy is for failed tasks only (DESIGN.md §5.7), even with an error and a stream.
  assert.deepEqual(buttonTexts(stopped.detail.element), ["Reuse", "Open page", "Dismiss"]);
  stopped.detail.dispose();

  // A failed task copies its error, else its stream.
  const failed = taskView({ state: "failed", finished_at: "2026-10-27T16:00:10Z", error: "Timed out.", error_kind: "timed_out" });
  const broke = detailSetup(failed, {
    reply: () => ({ type: "run_fetched", run: failed, stream_text: "partial", next_offset: 7, more: false }),
  });
  broke.detail.start();
  await settle();
  assert.deepEqual(buttonTexts(broke.detail.element), ["Copy", "Reuse", "Open page", "Dismiss"]);
  button(broke.detail.element, "Copy").click();
  await settle();
  assert.deepEqual(broke.copied, ["Timed out."]);
  broke.detail.dispose();
});

test("TaskDetail renders the instruction, title, report and stream as text only", async () => {
  const hostile = "<img src=x onerror=alert(1)> [x](javascript:alert(1))";
  const done = taskView({
    instruction: hostile, title: hostile, state: "succeeded", finished_at: "2026-10-27T16:00:10Z", task_report: hostile,
  });
  const page = detailSetup(done, {
    reply: () => ({ type: "run_fetched", run: done, stream_text: hostile, next_offset: hostile.length, more: false }),
  });
  page.detail.start();
  await settle();
  for (const name of ["task-detail-instruction", "task-detail-report", "run-card-stream"]) {
    const element = byClass(page.detail.element, name);
    assert.equal(element.textContent, hostile, name);
    assert.deepEqual(element.children, [], name);
  }
  const meta = byClass(page.detail.element, "task-detail-meta");
  assert.ok(meta.textContent.startsWith(`on ${hostile} · Codex`));
  assert.deepEqual(meta.children, []);
  button(page.detail.element, "Open page").click();
  assert.deepEqual(page.opened, ["https://read.example/a"]);
  page.detail.dispose();
});

test("TaskDetail Reuse passes the full get_run instruction, not the list cut", async () => {
  const full = `${"a".repeat(300)} and then the end`;
  const cut = `${"a".repeat(280)}…`;
  const finished = taskView({ instruction: full, state: "succeeded", finished_at: "2026-10-27T15:59:50Z", task_report: "Report." });
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const page = detailSetup({ ...finished, instruction: cut, task_report: null }, {
    reply: async () => {
      await gate;
      return { type: "run_fetched", run: finished, stream_text: "", next_offset: 0, more: false };
    },
  });
  page.detail.start();
  await settle();
  // Until get_run answers there is no full instruction, so no Reuse.
  assert.ok(!buttonTexts(page.detail.element).includes("Reuse"));
  assert.equal(byClass(page.detail.element, "task-detail-instruction").textContent, cut);
  release();
  await settle();
  assert.equal(byClass(page.detail.element, "task-detail-instruction").textContent, full);
  button(page.detail.element, "Reuse").click();
  assert.deepEqual(page.reused, [full]);
  page.detail.dispose();
});

test("TaskDetail Open page opens the run url", async () => {
  const running = taskView({ url: "https://read.example/b#section" });
  const page = detailSetup(running, {
    reply: () => ({ type: "run_fetched", run: running, stream_text: "see https://evil.example", next_offset: 24, more: false }),
  });
  page.detail.start();
  await settle();
  button(page.detail.element, "Open page").click();
  assert.deepEqual(page.opened, ["https://read.example/b#section"]);
  page.detail.dispose();
});

test("TaskDetail Dismiss remembers the run and reports it", async () => {
  const done = taskView({ state: "succeeded", finished_at: "2026-10-27T16:00:10Z", task_report: "ok" });
  const storage = storageArea(new Map([[DISMISSED_KEY, ["00000000-0000-4000-8000-000000000009"]]]));
  const page = detailSetup(done, {
    storage, reply: () => ({ type: "run_fetched", run: done, stream_text: "", next_offset: 0, more: false }),
  });
  page.detail.start();
  await settle();
  button(page.detail.element, "Dismiss").click();
  await settle();
  assert.deepEqual(storage.entries.get(DISMISSED_KEY), ["00000000-0000-4000-8000-000000000009", done.run_id]);
  assert.deepEqual(page.dismissed, [done.run_id]);
  page.detail.dispose();
});

test("TaskDetail keeps the output toggle the user chose after finishing", async () => {
  let current = taskView();
  const page = detailSetup(current, {
    reply: () => ({ type: "run_fetched", run: current, stream_text: "", next_offset: 0, more: false }),
  });
  page.detail.start();
  await settle();
  // The user closes it while it runs; it stays closed.
  output(page.detail).open = false;
  await tick();
  assert.equal(output(page.detail).open, false);
  // The user opens it again; the finish closes it once.
  output(page.detail).open = true;
  current = taskView({ state: "succeeded", finished_at: "2026-10-27T16:00:10Z", task_report: "ok" });
  await tick();
  assert.equal(output(page.detail).open, false);
  // The user opens it after the finish; later renders keep it open.
  output(page.detail).open = true;
  await tick(POLL_MS * 3);
  button(page.detail.element, "Copy report").click();
  await settle();
  assert.equal(output(page.detail).open, true);
  page.detail.dispose();
});

test("TaskDetail never shows Retry save or Open summary, even when can_retry_save is true", async () => {
  const failed = taskView({
    state: "failed", finished_at: "2026-10-27T16:00:10Z", error: "The agent finished, but its report could not be kept.",
    error_kind: "save_failed", can_retry_save: true, summary_path: "summaries/x.summary.md",
  });
  const page = detailSetup(failed, {
    reply: (request) => request.type === "get_run"
      ? { type: "run_fetched", run: failed, stream_text: "", next_offset: 0, more: false }
      : null,
  });
  page.detail.start();
  await settle();
  assert.ok(!buttonTexts(page.detail.element).includes("Retry save"));
  assert.ok(!buttonTexts(page.detail.element).includes("Open summary"));
  assert.deepEqual(page.requests.filter((request) => request.type === "get_artifact"), []);
  page.detail.dispose();
});
