// Built side-panel tests: summarize. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  ARTICLE_URL, EXTRACT_LIMITS, PANEL_NOTICE_KEY, READY, RUN_VIEW,
  SETUP_WARNING, WARNING_SEEN_KEY, buttonIn, install, readablePage, startsRun,
  storageKey, summarizePanel, teardown, until,
} from "./harness.mjs";

// --- Summarize & log (plan Task 39) -----------------------------------------
const RUNS_MAY_EXIST_KEY = storageKey("runs_may_exist_v1");

// The extractor read a document at `href`, not the page the run is for.
function pageAt(href) {
  return async (injection) => {
    const [answer] = await readablePage(injection);
    return [injection.files ? answer : { ...answer, result: { ...answer.result, href } }];
  };
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

test("clicking Summarize extracts in every frame then sends start_run with no record key and a local requested_at", async () => {
  const page = await summarizePanel({ local: new Map([[WARNING_SEEN_KEY, true]]) });
  await until(() => page.el("summarize").disabled === false, "the button to enable");
  page.el("summarize").click();
  await until(() => page.sent("start_run").length === 1, "start_run");

  const [library, extract] = page.state.injections;
  assert.deepEqual(library, { target: { tabId: 1, allFrames: true }, files: ["vendor/readability.js"] });
  assert.deepEqual(extract.target, { tabId: 1, allFrames: true });
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

test("a worker notice for a tab the panel has not followed yet shows once that tab is followed", async () => {
  const session = new Map();
  const page = await summarizePanel({ session, local: new Map([[WARNING_SEEN_KEY, true]]) });
  // Startup reads (and clears) whatever was stored before the panel loaded.
  await until(() => page.sent("list_runs").length === 1, "startup's list_runs");
  await new Promise((resolve) => setTimeout(resolve, 20));
  // The shortcut fired on tab 2 just after it was activated, while the panel
  // still followed tab 1: the live notice is not for the followed tab.
  const notice = { kind: "summarize_failed", message: "Tab two failed early.", tab_id: 2, at: Date.now() };
  session.set(PANEL_NOTICE_KEY, notice);
  page.notify(notice);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.doesNotMatch(page.el("run-area").textContent, /Tab two failed early/);
  assert.ok(session.has(PANEL_NOTICE_KEY), "a notice for another tab stays stored");

  page.navigate("https://other.example/b", 2);
  await until(() => page.el("run-area").textContent.includes("Tab two failed early."), "tab 2's stored notice");
  await until(() => !session.has(PANEL_NOTICE_KEY), "the notice to be consumed");
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

test("the setup warning shows once for a summary the shortcut or menu started", async () => {
  const listRuns = (request) => request.type === "list_runs" ? { type: "runs_listed", runs: [RUN_VIEW] } : startsRun(request);
  // The worker opened the panel and stored its notice before the panel loaded.
  const local = new Map();
  const session = new Map([[PANEL_NOTICE_KEY, { kind: "run_started", run_id: RUN_VIEW.run_id, tab_id: 1, at: Date.now() }]]);
  const opened = await summarizePanel({ local, session, intercept: listRuns });
  await until(() => opened.el("run-area").textContent.includes(SETUP_WARNING), "the setup warning");
  await until(() => /Queued/.test(opened.el("run-area").textContent), "the run card");
  assert.equal(local.get(WARNING_SEEN_KEY), true);
  teardown.splice(0).forEach((listener) => listener());

  // A panel already open hears the worker's notice; the warning is not shown twice.
  const fresh = new Map();
  const open = await summarizePanel({ local: fresh, intercept: listRuns });
  open.notify({ kind: "run_started", run_id: RUN_VIEW.run_id, tab_id: 1, at: Date.now() });
  await until(() => open.el("run-area").textContent.includes(SETUP_WARNING), "the live setup warning");
  assert.equal(fresh.get(WARNING_SEEN_KEY), true);
  teardown.splice(0).forEach((listener) => listener());

  const again = await summarizePanel({ local: fresh, intercept: listRuns });
  again.notify({ kind: "run_started", run_id: RUN_VIEW.run_id, tab_id: 1, at: Date.now() + 1 });
  await until(() => /Queued/.test(again.el("run-area").textContent), "the run card");
  assert.ok(!again.el("run-area").textContent.includes(SETUP_WARNING));
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

test("with no summary harness set up, opening the panel does not wake the service", async () => {
  // list_runs makes the host start the registered service (launchctl), which
  // is on demand: only a set-up harness can have runs to restore.
  const page = await install({ tab: { id: 1, url: ARTICLE_URL, title: "A" } });
  await until(() => page.el("note-body").disabled === false, "the note to load");
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(page.sent("list_runs").length, 0);
  teardown.splice(0).forEach((listener) => listener());

  const ready = await summarizePanel();
  await until(() => ready.sent("list_runs").some((request) => request.kind === "summary"), "list_runs");
});

test("a run still in the service restores with Cancel after the harness is removed", async () => {
  // The harness was set up, a run started, then the harness was removed:
  // the service still runs it, so the panel must show it and let it stop.
  const running = { ...RUN_VIEW, state: "running", started_at: "2026-10-27T09:00:01-07:00" };
  const local = new Map([[RUNS_MAY_EXIST_KEY, Date.now() - 60_000]]);
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "A" }, local,
    intercept: (request) => {
      if (request.type === "list_runs") return { type: "runs_listed", runs: request.kind === "summary" ? [running] : [] };
      if (request.type === "get_run") return { type: "run_fetched", run: running, stream_text: "", next_offset: 0, more: false };
      return null;
    },
  });
  await until(() => /Running/.test(page.el("run-area").textContent), "the restored card");
  assert.ok(buttonIn(page.el("run-area"), "Cancel"), "the restored card offers Cancel");
  // Once for the summary cards, once for the Tasks button.
  assert.deepEqual(page.sent("list_runs").map((request) => request.kind), ["task", "summary"]);
  teardown.splice(0).forEach((listener) => listener());

  // Once list_runs finds nothing left to show, later opens stop asking.
  const empty = await install({ tab: { id: 1, url: ARTICLE_URL, title: "A" }, local });
  await until(() => empty.sent("list_runs").length === 2, "list_runs");
  await until(() => !local.has(RUNS_MAY_EXIST_KEY), "the record to clear");
  teardown.splice(0).forEach((listener) => listener());
  const later = await install({ tab: { id: 1, url: ARTICLE_URL, title: "A" }, local });
  await until(() => later.el("note-body").disabled === false, "the note to load");
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(later.sent("list_runs").length, 0);
});

test("a started summary records that runs may exist", async () => {
  const local = new Map([[WARNING_SEEN_KEY, true]]);
  const page = await summarizePanel({ local });
  await until(() => page.el("summarize").disabled === false, "the button to enable");
  page.el("summarize").click();
  await until(() => page.sent("start_run").length === 1, "start_run");
  await until(() => typeof local.get(RUNS_MAY_EXIST_KEY) === "number", "the record");
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
