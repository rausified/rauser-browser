import assert from "node:assert/strict";
import test, { mock } from "node:test";

// Run after build:extension. The run card gets a fake DOM, a scripted host,
// and mocked timers, so each poll happens when a test ticks the clock.
const {
  MAX_RESTORED_FINISHED, POLL_MS, RESTORE_FINISHED_WITHIN_MS, RunCards, answerLine, elapsedText,
  markRunsMayExist, restorable, resumeCommand, runsMayExist, stateText,
} = await import("../dist/run-card.js");
const { HostError } = await import("../dist/native.js");
const { SERVICE_SETUP_INSTRUCTIONS } = await import("../dist/agent.js");
const { storageKey } = await import("../dist/brand.js");

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
function setup({ reply = () => null, visible = true, storage = storageArea() } = {}) {
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

  // Dismiss removes a finished card and remembers it, keeping the newest 200.
  storage.entries.set(DISMISSED_KEY, Array.from({ length: 200 }, (_, n) => id(1000 + n)));
  const card = cards(page.container).find((candidate) => candidate.textContent.includes("Summarized"));
  assert.ok(!buttonTexts(cards(page.container)[0]).includes("Dismiss"), "an in-flight card has no Dismiss");
  button(card, "Dismiss").click();
  await settle();
  assert.equal(cards(page.container).length, 6);
  const stored = storage.entries.get(DISMISSED_KEY);
  assert.equal(stored.length, 200);
  assert.equal(stored.at(-1), id(3));
  assert.ok(!stored.includes(id(1000)));
  page.runCards.dispose();
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
  assert.deepEqual(page.sent("open_summary").map((r) => r.url), [saved.url]);
  assert.ok(!card.textContent.includes("Could not open"));

  // Once per saved file: later updates and polls ask nothing more.
  page.runCards.show(saved);
  await tick(POLL_MS * 4);
  assert.equal(page.sent("get_artifact").length, 1);
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
  assert.ok(!card.textContent.includes("importance"));
  assert.ok(buttonTexts(card).includes("Open summary"));
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
