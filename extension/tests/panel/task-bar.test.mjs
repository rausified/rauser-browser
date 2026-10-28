// Built side-panel tests: the task bar. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  ARTICLE_URL, CONFIG, PANEL_HTML, READY, RUN_VIEW, SETUP_WARNING, SET_UP, WARNING_SEEN_KEY,
  buttonIn, descendants, install, readablePage, settle, startsRun, storageKey, teardown, until,
} from "./harness.mjs";

const DISMISSED_KEY = storageKey("dismissed_runs_v1");
const OTHER_URL = "https://other.example/b";
// Finished a minute ago, so a new panel restores it.
const RECENT = new Date(Date.now() - 60_000).toISOString();

const TASK_VIEW = {
  ...RUN_VIEW, run_id: "00000000-0000-4000-8000-000000000050", kind: "task",
  instruction: "List every link on this page", task_report: null,
};

// The page each tab holds, so a draft sent after a tab switch reads the
// new tab's page rather than failing as page_changed.
const PAGES = new Map([[1, ARTICLE_URL], [2, OTHER_URL]]);

// Reads the tab the injection targets.
function pageReader() {
  return async (injection) => {
    const [answer] = await readablePage(injection);
    if (injection.files) return [answer];
    return [{ ...answer, result: { ...answer.result, href: PAGES.get(injection.target.tabId) ?? ARTICLE_URL } }];
  };
}

// start_task answers with a queued task carrying the sent instruction;
// get_run answers with the latest view for that run.
function startsTask(views = new Map()) {
  return (request) => {
    if (request.type === "start_task") {
      const view = { ...TASK_VIEW, url: request.url, instruction: request.instruction };
      if (!views.has(view.run_id)) views.set(view.run_id, view);
      return { type: "run_started", run: view, existing: false };
    }
    if (request.type === "get_run") {
      const run = views.get(request.run_id);
      if (run) return { type: "run_fetched", run, stream_text: "", next_offset: 0, more: false };
    }
    return startsRun(request);
  };
}

async function taskPanel(options = {}) {
  const { intercept = startsTask(), ...rest } = options;
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Tab title" }, agentStatus: READY, config: SET_UP,
    executeScript: pageReader(), local: new Map([[WARNING_SEEN_KEY, true]]), intercept, ...rest,
  });
  await until(() => page.el("note-editor").hidden === false, "the page to show");
  await until(() => page.el("task-run").disabled === false, "the task bar to enable");
  return page;
}

function type(page, text) {
  page.el("task-input").value = text;
  page.el("task-input").dispatch("input");
}

function enter(page, init = {}) {
  let prevented = false;
  page.el("task-input").dispatch("keydown", {
    key: "Enter", shiftKey: false, isComposing: false, keyCode: 13,
    preventDefault() { prevented = true; }, ...init,
  });
  return prevented;
}

const runLines = (area) => area.children.filter((child) => child.className.split(" ").includes("run-line"));
const cards = (area) => area.children.filter((child) => child.className.split(" ").includes("run-card"));
const taskCards = (page) => cards(page.el("task-run-area"));

function held() {
  let release;
  const promise = new Promise((resolve) => { release = resolve; });
  return { promise, release: (reply) => release(reply) };
}

// The hint as the user reads it, and whether it offers Open settings.
function hint(page) {
  return {
    hidden: page.el("task-hint").hidden,
    text: page.el("task-hint-text").textContent,
    settings: page.el("task-hint-settings").hidden === false,
  };
}

test("the task bar sits between the page area and the tabs", async () => {
  const pageArea = PANEL_HTML.indexOf('<section id="page-area"');
  const bar = PANEL_HTML.indexOf('<section id="task-bar"');
  const tabs = PANEL_HTML.indexOf('<nav class="panel-tabs"');
  assert.ok(pageArea !== -1 && bar !== -1 && tabs !== -1);
  assert.ok(PANEL_HTML.indexOf("</section>", pageArea) < bar, "the bar follows the page area");
  assert.ok(bar < tabs, "the bar comes before the tabs");
  const section = PANEL_HTML.slice(bar, tabs);
  for (const id of ["task-form", "task-input", "task-run", "task-context", "task-hint", "task-announce", "task-run-area"]) {
    assert.match(section, new RegExp(`id="${id}"`), id);
  }
  assert.match(section, /<textarea id="task-input"[^>]*maxlength="4000"/);
  // Not inside the This page view, so it shows on Recent too.
  assert.ok(bar < PANEL_HTML.indexOf('<div id="page-view">'));

  const page = await taskPanel();
  assert.equal(page.el("task-bar").hidden, false);
  page.el("tab-recent").click();
  assert.equal(page.el("page-view").hidden, true);
  assert.equal(page.el("task-bar").hidden, false);
});

test("only task-announce is a live region for tasks", () => {
  const section = PANEL_HTML.slice(PANEL_HTML.indexOf('<section id="task-bar"'), PANEL_HTML.indexOf('<nav class="panel-tabs"'));
  const live = [...section.matchAll(/<[a-z]+\b[^>]*\b(aria-live|role="status")[^>]*>/g)].map((match) => match[0]);
  assert.equal(live.length, 1, live.join("\n"));
  assert.match(live[0], /id="task-announce"/);
  assert.match(live[0], /aria-live="polite"/);
  assert.doesNotMatch(section.match(/<div id="task-run-area"[^>]*>/)[0], /aria-live/);
});

test("the hint explains each disabled state in order", async () => {
  const cases = [
    {
      what: "not connected", text: "Waiting for the native host…", settings: false,
      options: { intercept: (request) => request.type === "hello" ? { type: "error", code: "internal", message: "no host" } : null },
    },
    {
      what: "no home folder", text: "Choose a home folder in settings to run tasks.", settings: true,
      options: { config: { ...SET_UP, storage: null } },
    },
    {
      what: "config needs repair", text: "Brauser's settings need repair before a task can run.", settings: true,
      options: { configIssue: "config.toml: bad value" },
    },
    {
      what: "Windows", text: "Tasks are not supported on Windows yet.", settings: false, options: {}, windows: true,
    },
    {
      what: "no harness", text: "Set up an agent (Codex or Claude Code) in settings to run tasks.", settings: true,
      options: { agentStatus: "none" },
    },
    {
      what: "not a web page", text: "Tasks work on http(s) pages.", settings: false,
      options: { tab: { id: 1, url: "chrome://extensions/", title: "Extensions" } },
    },
    {
      what: "site access", text: "Re-enable site access in Chrome so the agent can read this page.", settings: false,
      options: { tab: { id: 1, title: "Article" }, contains: false },
    },
  ];
  for (const { what, text, settings, options, windows } of cases) {
    const { agentStatus = READY, ...rest } = options;
    const page = await install({
      tab: { id: 1, url: ARTICLE_URL, title: "Tab title" }, config: SET_UP, executeScript: pageReader(),
      local: new Map([[WARNING_SEEN_KEY, true]]), ...(agentStatus === "none" ? {} : { agentStatus }), ...rest,
    });
    await until(() => page.calls("tabs.query") > 0 && page.el("task-bar").hidden === false, `${what}: the bar`);
    if (windows) {
      globalThis.navigator.platform = "Win32";
      page.titleChanged("Tab title again");
    }
    await until(() => hint(page).text === text, `${what}: the hint (now "${hint(page).text}")`);
    assert.equal(hint(page).hidden, false, what);
    assert.equal(hint(page).settings, settings, what);
    assert.equal(page.el("task-run").disabled, true, what);
    // The draft stays: the bar still takes typing, but Enter sends nothing.
    type(page, "Draft for later");
    enter(page);
    await settle(20);
    assert.equal(page.sent("start_task").length, 0, what);
    assert.equal(page.el("task-input").value, "Draft for later", what);
    if (settings) {
      page.el("task-hint-settings").click();
      assert.equal(page.calls("openOptionsPage"), 1, what);
    }
    teardown.splice(0).forEach((listener) => listener());
  }

  // Starting: the button reads Starting… and the textarea is read-only.
  const start = held();
  const page = await taskPanel({ intercept: (request) => request.type === "start_task" ? start.promise : startsTask()(request) });
  assert.equal(hint(page).hidden, true);
  type(page, "Summarize the comments");
  enter(page);
  await until(() => page.sent("start_task").length === 1, "start_task");
  assert.equal(page.el("task-run").textContent, "Starting…");
  assert.equal(page.el("task-run").disabled, true);
  assert.equal(page.el("task-input").readOnly, true);
  start.release({ type: "run_started", run: TASK_VIEW, existing: false });
  await until(() => page.el("task-input").readOnly === false, "the bar to reopen");
  assert.equal(page.el("task-run").textContent, "Run");
});

test("with no agent configured the panel never lists runs", async () => {
  const page = await install({ tab: { id: 1, url: ARTICLE_URL, title: "A" }, config: CONFIG, executeScript: pageReader() });
  await until(() => page.el("note-body").disabled === false, "the note to load");
  type(page, "Do something");
  enter(page);
  await settle(20);
  assert.equal(page.sent("list_runs").length, 0);
  assert.equal(page.sent("start_task").length, 0);
  // Nor does it read the page for the context line.
  page.el("task-input").dispatch("focus");
  await settle(20);
  assert.equal(page.state.selectionProbes.length, 0);
});

test("Enter sends start_task, Shift+Enter does not, IME composition does not", async () => {
  const page = await taskPanel();
  type(page, "Find the owner\nof this page");
  assert.equal(enter(page, { shiftKey: true }), false, "Shift+Enter inserts a newline");
  assert.equal(enter(page, { isComposing: true }), false);
  assert.equal(enter(page, { keyCode: 229 }), false);
  assert.equal(enter(page, { key: "a" }), false);
  await settle(20);
  assert.equal(page.sent("start_task").length, 0);

  assert.equal(enter(page), true, "Enter does not insert a newline");
  await until(() => page.sent("start_task").length === 1, "start_task");
  const [request] = page.sent("start_task");
  assert.equal(request.instruction, "Find the owner\nof this page");
  assert.equal(request.url, ARTICLE_URL);
  assert.equal(request.harness, null);
  assert.equal("kind" in request, false);
  assert.equal("record_key" in request, false);

  // The Run button submits the form too.
  await until(() => page.el("task-run").disabled === false, "the bar to reopen");
  type(page, "Second task");
  let prevented = false;
  page.el("task-form").dispatch("submit", { preventDefault() { prevented = true; } });
  assert.equal(prevented, true);
  await until(() => page.sent("start_task").length === 2, "the second start_task");
});

test("an empty instruction sends nothing", async () => {
  const page = await taskPanel();
  for (const text of ["", "   ", "\n\t \n", "\u0007\u001b"]) {
    type(page, text);
    enter(page);
  }
  await settle(20);
  assert.equal(page.sent("start_task").length, 0);
  assert.equal(runLines(page.el("task-run-area")).length, 0);
});

test("a double Enter sends one request", async () => {
  const start = held();
  const page = await taskPanel({ intercept: (request) => request.type === "start_task" ? start.promise : startsTask()(request) });
  type(page, "Once only");
  enter(page);
  enter(page);
  page.el("task-form").dispatch("submit", { preventDefault() {} });
  await until(() => page.sent("start_task").length === 1, "start_task");
  enter(page);
  await settle(20);
  assert.equal(page.sent("start_task").length, 1);
  start.release({ type: "run_started", run: TASK_VIEW, existing: false });
  await until(() => taskCards(page).length === 1, "the card");
  assert.equal(page.sent("start_task").length, 1);
});

test("the bar clears on success and keeps the text on failure", async () => {
  const page = await taskPanel();
  type(page, "List every link on this page");
  page.el("task-input").focused = false;
  enter(page);
  await until(() => taskCards(page).length === 1, "the task card");
  assert.equal(page.el("task-input").value, "");
  assert.equal(page.el("task-input").focused, true, "focus stays in the bar");
  assert.match(taskCards(page)[0].textContent, /List every link on this page/);
  teardown.splice(0).forEach((listener) => listener());

  const conflict = "Four tasks are already waiting or running. Wait for one to finish.";
  const refused = await taskPanel({
    intercept: (request) => request.type === "start_task" ? { type: "error", code: "conflict", message: conflict } : startsTask()(request),
  });
  type(refused, "A fifth task");
  enter(refused);
  await until(() => runLines(refused.el("task-run-area")).length === 1 &&
    refused.el("task-run-area").textContent.includes(conflict), "the failure line");
  assert.equal(refused.el("task-input").value, "A fifth task");
  assert.equal(taskCards(refused).length, 0);
  teardown.splice(0).forEach((listener) => listener());

  // A cause settings fixes offers Open settings in the line.
  const unset = await taskPanel({
    intercept: (request) => request.type === "start_task"
      ? { type: "error", code: "not_configured", message: "No AI harness is set up for summaries." }
      : startsTask()(request),
  });
  type(unset, "Do it");
  enter(unset);
  await until(() => unset.el("task-run-area").textContent.includes("Finish setup in Brauser settings to run tasks"), "the setup line");
  assert.doesNotMatch(unset.el("task-run-area").textContent, /summaries/);
  buttonIn(unset.el("task-run-area"), "Open settings").click();
  assert.equal(unset.calls("openOptionsPage"), 1);
  assert.equal(unset.el("task-input").value, "Do it");
});

test("a failure line belongs to the tab it was started for", async () => {
  const start = held();
  const page = await taskPanel({ intercept: (request) => request.type === "start_task" ? start.promise : startsTask()(request) });
  type(page, "Slow task");
  enter(page);
  await until(() => page.sent("start_task").length === 1, "start_task");
  assert.match(page.el("task-run-area").textContent, /Reading this page/);
  page.navigate(OTHER_URL, 2);
  await until(() => page.el("page-url").textContent === OTHER_URL, "tab 2 to show");
  assert.doesNotMatch(page.el("task-run-area").textContent, /Reading this page/);
  start.release({ type: "error", code: "conflict", message: "Tab one's refusal." });
  await settle(20);
  assert.doesNotMatch(page.el("task-run-area").textContent, /Tab one's refusal/);
  assert.equal(page.el("task-input").value, "Slow task");
});

test("summary and task lines live in their own areas and both clear on a tab switch", async () => {
  for (const order of [["summary", "task"], ["task", "summary"]]) {
    const summary = held();
    const task = held();
    const page = await taskPanel({
      intercept: (request) => {
        if (request.type === "start_run") return summary.promise;
        if (request.type === "start_task") return task.promise;
        return startsTask()(request);
      },
    });
    await until(() => page.el("summarize").disabled === false, "Summarize to enable");
    page.el("summarize").click();
    await until(() => page.sent("start_run").length === 1, "start_run");
    // A pending summary gates only Summarize.
    assert.equal(page.el("summarize").disabled, true);
    assert.equal(page.el("task-run").disabled, false);
    type(page, "Pending task");
    enter(page);
    await until(() => page.sent("start_task").length === 1, "start_task");
    assert.equal(page.el("task-run").disabled, true);
    assert.equal(runLines(page.el("run-area")).length, 1);
    assert.equal(runLines(page.el("task-run-area")).length, 1);

    for (const which of order) {
      if (which === "summary") {
        summary.release({ type: "error", code: "internal", message: "summary broke" });
        await until(() => page.el("run-area").textContent.includes("Could not start the summary: summary broke"), "the summary line");
        // Summarize is free again while the task still starts.
        await until(() => page.el("summarize").disabled === false, "Summarize to reopen");
        if (order[0] === "summary") assert.equal(page.el("task-run").disabled, true);
      } else {
        task.release({ type: "error", code: "internal", message: "task broke" });
        await until(() => page.el("task-run-area").textContent.includes("Could not start the task: task broke"), "the task line");
        await until(() => page.el("task-run").disabled === false, "the bar to reopen");
        if (order[0] === "task") assert.equal(page.el("summarize").disabled, true);
      }
    }
    assert.equal(runLines(page.el("run-area")).length, 1, order.join(","));
    assert.equal(runLines(page.el("task-run-area")).length, 1, order.join(","));
    assert.match(runLines(page.el("run-area"))[0].textContent, /summary broke/);
    assert.match(runLines(page.el("task-run-area"))[0].textContent, /task broke/);
    assert.doesNotMatch(page.el("run-area").textContent, /task broke/);
    assert.doesNotMatch(page.el("task-run-area").textContent, /summary broke/);

    page.navigate(OTHER_URL, 2);
    await until(() => page.el("page-url").textContent === OTHER_URL, "tab 2 to show");
    assert.equal(runLines(page.el("run-area")).length, 0);
    assert.equal(runLines(page.el("task-run-area")).length, 0);
    teardown.splice(0).forEach((listener) => listener());
  }
});

test("the setup warning shows once across summaries and tasks", async () => {
  const local = new Map();
  const page = await taskPanel({ local });
  type(page, "First task");
  enter(page);
  await until(() => page.el("task-run-area").textContent.includes(SETUP_WARNING), "the setup warning");
  await until(() => page.sent("start_task").length === 1, "start_task");
  assert.equal(local.get(WARNING_SEEN_KEY), true);
  assert.ok(!page.el("run-area").textContent.includes(SETUP_WARNING));
  page.el("summarize").click();
  await until(() => page.sent("start_run").length === 1, "start_run");
  await settle(20);
  assert.ok(!page.el("run-area").textContent.includes(SETUP_WARNING));
  buttonIn(page.el("task-run-area"), "Got it").click();
  assert.ok(!page.el("task-run-area").textContent.includes(SETUP_WARNING));
});

test("a first task started from Recent shows the setup warning in the task area", async () => {
  const page = await taskPanel({ local: new Map() });
  page.el("tab-recent").click();
  assert.equal(page.el("page-view").hidden, true);
  type(page, "From Recent");
  enter(page);
  await until(() => page.el("task-run-area").textContent.includes(SETUP_WARNING), "the setup warning");
  assert.ok(!page.el("run-area").textContent.includes(SETUP_WARNING));
});

test("a list_runs failure shows one notice, in the task area", async () => {
  const failing = await taskPanel({
    intercept: (request) => request.type === "list_runs" ? { type: "error", code: "internal", message: "list broke" } : startsTask()(request),
  });
  await until(() => failing.el("task-run-area").textContent.includes("Could not list agent runs: list broke"), "the notice");
  assert.ok(!failing.el("run-area").textContent.includes("list broke"));
  assert.equal(failing.el("task-run-area").textContent.split("list broke").length, 2, "one notice");
  teardown.splice(0).forEach((listener) => listener());

  const unavailable = await taskPanel({
    intercept: (request) => request.type === "list_runs"
      ? { type: "error", code: "service_unavailable", message: "The Brauser service is not running." }
      : startsTask()(request),
  });
  await until(() => unavailable.el("task-run-area").textContent.includes("The Brauser service is not running."), "the service notice");
  assert.ok(!unavailable.el("run-area").textContent.includes("not running"));
  buttonIn(unavailable.el("task-run-area"), "Open settings").click();
  assert.equal(unavailable.calls("openOptionsPage"), 1);
});

test("restore puts task runs in the task area and never in summary cards", async () => {
  const summary = { ...RUN_VIEW, run_id: "00000000-0000-4000-8000-000000000060", title: "Summary page", state: "running" };
  const task = { ...TASK_VIEW, state: "running", instruction: "Restored task instruction" };
  const views = new Map([[summary.run_id, summary], [task.run_id, task]]);
  const page = await taskPanel({
    intercept: (request) => request.type === "list_runs" ? { type: "runs_listed", runs: [task, summary] } : startsTask(views)(request),
  });
  await until(() => taskCards(page).length === 1 && cards(page.el("run-area")).length === 1, "both cards");
  assert.equal(page.sent("list_runs").length, 1, "the runs are listed once");
  assert.match(taskCards(page)[0].textContent, /Restored task instruction/);
  assert.doesNotMatch(page.el("task-run-area").textContent, /Summary page/);
  assert.match(cards(page.el("run-area"))[0].textContent, /Summary page/);
  assert.doesNotMatch(page.el("run-area").textContent, /Restored task instruction/);
});

test("task cards stay visible after following another tab", async () => {
  const page = await taskPanel();
  type(page, "Keep me visible");
  enter(page);
  await until(() => taskCards(page).length === 1, "the task card");
  page.navigate(OTHER_URL, 2);
  await until(() => page.el("page-url").textContent === OTHER_URL, "tab 2 to show");
  assert.equal(taskCards(page)[0].hidden, false);
  page.navigate("chrome://extensions/", 3);
  await until(() => page.el("page-url").textContent === "chrome://extensions/", "tab 3 to show");
  assert.equal(taskCards(page)[0].hidden, false);
});

test("a draft survives a tab switch, the context line follows, and Enter sends the draft with the new page", async () => {
  const page = await taskPanel();
  assert.equal(page.el("task-context").textContent, "Uses this page: Tab title");
  type(page, "Compare with my notes");
  page.state.tab = { ...page.state.tab, title: "Other title" };
  page.navigate(OTHER_URL, 2);
  await until(() => page.el("task-context").textContent === "Uses this page: Other title", "the context line");
  assert.equal(page.el("task-input").value, "Compare with my notes");
  enter(page);
  await until(() => page.sent("start_task").length === 1, "start_task");
  const [request] = page.sent("start_task");
  assert.equal(request.url, OTHER_URL);
  assert.equal(request.instruction, "Compare with my notes");
});

test("the context line says when the selection is used", async () => {
  let selected = true;
  const page = await taskPanel({ selection: () => selected });
  await until(() => page.el("task-context").textContent === "Uses this page and your selection: Tab title", "the selection line");
  assert.deepEqual(page.state.selectionProbes[0].target, { tabId: 1, allFrames: true });
  assert.deepEqual(page.state.selectionProbes[0].args, []);
  selected = false;
  page.el("task-input").dispatch("focus");
  await until(() => page.el("task-context").textContent === "Uses this page: Tab title", "the plain line");
  selected = true;
  page.el("task-input").dispatch("focus");
  await until(() => page.el("task-context").textContent === "Uses this page and your selection: Tab title", "the selection line again");
  // A page that is not a web page names nothing.
  page.navigate("chrome://extensions/", 3);
  await until(() => page.el("task-context").textContent === "", "no context line");
});

test("the bar is never pre-filled from the page, the selection or agent text", async () => {
  const finished = {
    ...TASK_VIEW, state: "succeeded", started_at: "2026-10-27T09:00:01-07:00", finished_at: RECENT,
    instruction: "Restored instruction", task_report: "Report text from the agent",
  };
  const views = new Map([[finished.run_id, finished]]);
  const page = await taskPanel({
    selection: () => true,
    intercept: (request) => {
      if (request.type === "list_runs") return { type: "runs_listed", runs: [{ ...finished, task_report: null }] };
      if (request.type === "get_run" && request.run_id === finished.run_id) {
        return { type: "run_fetched", run: finished, stream_text: "Streamed agent text", next_offset: 19, more: false };
      }
      return startsTask(views)(request);
    },
  });
  await until(() => page.el("task-run-area").textContent.includes("Report text from the agent"), "the restored report");
  assert.equal(page.el("task-input").value, "");
  page.navigate(OTHER_URL, 2);
  await until(() => page.el("page-url").textContent === OTHER_URL, "tab 2 to show");
  page.followLink(`${OTHER_URL}/next`, "Next page");
  await until(() => page.el("page-url").textContent === `${OTHER_URL}/next`, "the followed link");
  await settle(20);
  assert.equal(page.el("task-input").value, "");
  type(page, "Typed by the user");
  assert.equal(page.el("task-input").value, "Typed by the user");
});

test("a finished task announces itself", async () => {
  const instruction = "Summarize every comment in this thread and list who asked open questions";
  const done = {
    ...TASK_VIEW, state: "succeeded", started_at: "2026-10-27T09:00:01-07:00", finished_at: RECENT,
    instruction, task_report: "Three open questions.",
  };
  const views = new Map([[done.run_id, done]]);
  const page = await taskPanel({ intercept: startsTask(views) });
  assert.equal(page.el("task-announce").textContent, "");
  type(page, instruction);
  enter(page);
  await until(() => page.el("task-announce").textContent !== "", "the announcement");
  assert.equal(page.el("task-announce").textContent, `Task done: ${instruction.slice(0, 60)}…`);
  assert.match(page.el("task-run-area").textContent, /Three open questions\./);
});

test("Reuse puts the instruction back in the bar", async () => {
  const full = `Check every claim on this page ${"and its sources ".repeat(30)}`.trim();
  const finished = {
    ...TASK_VIEW, state: "failed", error: "The agent stopped.", error_kind: "harness_failed",
    started_at: "2026-10-27T09:00:01-07:00", finished_at: RECENT, instruction: full,
  };
  const page = await taskPanel({
    intercept: (request) => {
      if (request.type === "list_runs") return { type: "runs_listed", runs: [{ ...finished, instruction: `${full.slice(0, 280)}…` }] };
      if (request.type === "get_run") return { type: "run_fetched", run: finished, stream_text: "", next_offset: 0, more: false };
      return startsTask()(request);
    },
  });
  await until(() => buttonIn(page.el("task-run-area"), "Reuse"), "Reuse");
  page.el("task-input").focused = false;
  buttonIn(page.el("task-run-area"), "Reuse").click();
  assert.equal(page.el("task-input").value, full);
  assert.equal(page.el("task-input").focused, true);
});

test("Dismiss is remembered", async () => {
  const finished = {
    ...TASK_VIEW, state: "canceled", started_at: null, finished_at: RECENT,
  };
  const intercept = (request) => {
    if (request.type === "list_runs") return { type: "runs_listed", runs: [finished] };
    if (request.type === "get_run") return { type: "run_fetched", run: finished, stream_text: "", next_offset: 0, more: false };
    return startsTask()(request);
  };
  const local = new Map([[WARNING_SEEN_KEY, true]]);
  const page = await taskPanel({ local, intercept });
  await until(() => buttonIn(page.el("task-run-area"), "Dismiss"), "Dismiss");
  buttonIn(page.el("task-run-area"), "Dismiss").click();
  await until(() => (local.get(DISMISSED_KEY) ?? []).includes(finished.run_id), "the dismissal");
  assert.equal(taskCards(page).length, 0);
  teardown.splice(0).forEach((listener) => listener());

  const again = await taskPanel({ local, intercept });
  await until(() => again.sent("list_runs").length === 1, "list_runs");
  await settle(20);
  assert.equal(taskCards(again).length, 0);
  assert.equal(descendants(again.el("task-run-area")).length, 0);
});

const MAY_HAVE_STARTED = "The task may have started — check below before running it again.";

test("a lost start_task answer says the task may have started and restores its card", async () => {
  const lost = [
    ["the host's lost answer", (respond) => respond({
      type: "error", code: "service_unavailable", message: "The Brauser service stopped before answering. Try again in a moment.",
    })],
    ["a closed port", (_respond, page) => page.hostExits()],
  ];
  for (const [what, lose] of lost) {
    const start = held();
    let started = null;
    const queued = { ...TASK_VIEW, instruction: "Possibly started" };
    const views = new Map([[queued.run_id, queued]]);
    const page = await taskPanel({
      intercept: (request) => {
        if (request.type === "start_task") {
          started = request;
          return start.promise;
        }
        // The service queued the task before its answer was lost.
        if (request.type === "list_runs") return { type: "runs_listed", runs: started ? [queued] : [] };
        return startsTask(views)(request);
      },
    });
    await until(() => page.sent("list_runs").length === 1, `${what}: the first list`);
    type(page, "Possibly started");
    enter(page);
    await until(() => page.sent("start_task").length === 1, `${what}: start_task`);
    lose(start.release, page);
    await until(() => page.el("task-run-area").textContent.includes(MAY_HAVE_STARTED), `${what}: the softer line`);
    assert.doesNotMatch(page.el("task-run-area").textContent, /Could not start the task/, what);
    assert.equal(page.el("task-input").value, "Possibly started", what);
    await until(() => taskCards(page).length === 1, `${what}: the restored card`);
    assert.equal(page.sent("list_runs").length, 2, what);
    assert.match(taskCards(page)[0].textContent, /Possibly started/, what);
    teardown.splice(0).forEach((listener) => listener());
  }

  // A definite refusal keeps its text and lists nothing again.
  const refused = await taskPanel({
    intercept: (request) => request.type === "start_task"
      ? { type: "error", code: "conflict", message: "Four tasks are already waiting or running." }
      : startsTask()(request),
  });
  await until(() => refused.sent("list_runs").length === 1, "the first list");
  type(refused, "A fifth task");
  enter(refused);
  await until(() => refused.el("task-run-area").textContent.includes("Four tasks are already waiting or running."), "the refusal");
  await settle(20);
  assert.doesNotMatch(refused.el("task-run-area").textContent, /may have started/);
  assert.equal(refused.sent("list_runs").length, 1);
});

test("a task view without an instruction (a corrupt record) still lists, without Reuse", async () => {
  const { instruction: _instruction, ...corrupt } = {
    ...TASK_VIEW, run_id: "00000000-0000-4000-8000-000000000051", state: "failed",
    error: "The task's request is missing.", error_kind: "harness_failed",
    started_at: null, finished_at: RECENT,
  };
  const running = { ...TASK_VIEW, state: "running", instruction: "Still running" };
  const views = new Map([[corrupt.run_id, corrupt], [running.run_id, running]]);
  const page = await taskPanel({
    intercept: (request) => request.type === "list_runs"
      ? { type: "runs_listed", runs: [corrupt, running] }
      : startsTask(views)(request),
  });
  await until(() => taskCards(page).length === 2, "both task cards");
  const shown = taskCards(page).find((card) => card.textContent.includes("The task's request is missing."));
  assert.ok(shown, "the corrupt record's card");
  assert.equal(buttonIn(shown, "Reuse"), null);
  assert.doesNotMatch(page.el("task-run-area").textContent, /Could not list agent runs/);
});

test("a started task clears an earlier restore notice", async () => {
  let listFails = true;
  const page = await taskPanel({
    intercept: (request) => request.type === "list_runs" && listFails
      ? { type: "error", code: "internal", message: "list broke" }
      : startsTask()(request),
  });
  await until(() => page.el("task-run-area").textContent.includes("Could not list agent runs: list broke"), "the notice");
  listFails = false;
  type(page, "Start anyway");
  enter(page);
  await until(() => taskCards(page).length === 1, "the task card");
  assert.doesNotMatch(page.el("task-run-area").textContent, /list broke/);
});

test("the reworded setup warning shows once more to users who saw the old one", async () => {
  const local = new Map([[storageKey("summarize_warning_seen_v1"), true]]);
  const page = await taskPanel({ local });
  type(page, "First task since the rewording");
  enter(page);
  await until(() => page.el("task-run-area").textContent.includes(SETUP_WARNING), "the setup warning");
  assert.equal(local.get(storageKey("summarize_warning_seen_v2")), true);
  assert.equal(WARNING_SEEN_KEY, storageKey("summarize_warning_seen_v2"));
});

test("the selection is checked again when Run is pressed and when the panel gets focus", async () => {
  let selected = false;
  const page = await taskPanel({ selection: () => selected });
  await until(() => page.state.selectionProbes.length >= 1, "the first probe");
  await settle(20);
  assert.equal(page.el("task-context").textContent, "Uses this page: Tab title");
  selected = true;
  page.el("task-run").dispatch("pointerdown");
  await until(() => page.el("task-context").textContent === "Uses this page and your selection: Tab title", "the line after pointerdown");
  selected = false;
  page.windowEvent("focus");
  await until(() => page.el("task-context").textContent === "Uses this page: Tab title", "the line after focus");
});

test("a repeated identical announcement is announced again", async () => {
  const instruction = "Count the links";
  let next = 0;
  const views = new Map();
  const page = await taskPanel({
    intercept: (request) => {
      if (request.type === "start_task") {
        next += 1;
        const run_id = `00000000-0000-4000-8000-00000000007${next}`;
        const queued = { ...TASK_VIEW, run_id, instruction: request.instruction };
        views.set(run_id, {
          ...queued, state: "succeeded", started_at: "2026-10-27T09:00:01-07:00", finished_at: RECENT, task_report: "Seven.",
        });
        return { type: "run_started", run: queued, existing: false };
      }
      if (request.type === "get_run" && views.has(request.run_id)) {
        return { type: "run_fetched", run: views.get(request.run_id), stream_text: "", next_offset: 0, more: false };
      }
      return startsTask()(request);
    },
  });
  const announce = page.el("task-announce");
  const writes = [];
  Object.defineProperty(announce, "textContent", {
    get() { return this.ownText; },
    set(value) {
      writes.push(String(value));
      this.ownText = String(value);
      this.children = [];
    },
  });
  for (const round of [1, 2]) {
    type(page, instruction);
    enter(page);
    await until(() => writes.filter((text) => text === `Task done: ${instruction}`).length === round, `announcement ${round}`);
  }
  // Each announcement first empties the region, so the same text is new.
  assert.deepEqual(writes, ["", `Task done: ${instruction}`, "", `Task done: ${instruction}`]);
});

// The card's parts in order, and its activity disclosure if it has one.
const DETAILS = (card) => card.children.find((child) => child.tagName === "DETAILS") ?? null;
const ofClass = (card, name) => descendants(card).find((child) => (child.className ?? "").split(" ").includes(name));

test("a finished task folds its activity behind Show agent activity, after the report", async () => {
  const running = { ...TASK_VIEW, state: "running", started_at: "2026-10-27T09:00:01-07:00", instruction: "Fold me" };
  const done = { ...running, state: "succeeded", finished_at: RECENT, task_report: "The report." };
  const last = held();
  let fetches = 0;
  const page = await taskPanel({
    intercept: (request) => {
      if (request.type === "get_run") {
        fetches += 1;
        if (fetches === 1) return { type: "run_fetched", run: running, stream_text: "<b>Reading</b> the page", next_offset: 23, more: false };
        return last.promise;
      }
      if (request.type === "start_task") return { type: "run_started", run: { ...running, url: request.url }, existing: false };
      return startsTask()(request);
    },
  });
  type(page, "Fold me");
  enter(page);
  await until(() => taskCards(page).length === 1 && page.el("task-run-area").textContent.includes("Reading"), "the running stream");
  const [card] = taskCards(page);
  // While the task runs its activity shows in the card, with no disclosure.
  const stream = ofClass(card, "run-card-stream");
  assert.ok(card.children.includes(stream), "the stream sits in the card while running");
  assert.equal(stream.hidden, false);
  assert.equal(stream.textContent, "<b>Reading</b> the page");
  assert.deepEqual(stream.children, []);
  assert.notEqual(stream.tagName, "PRE", "the task stream is body text, not a monospace block");
  const runningDetails = DETAILS(card);
  assert.ok(runningDetails === null || runningDetails.hidden, "no disclosure while running");

  last.release({ type: "run_fetched", run: done, stream_text: " and done", next_offset: 32, more: false });
  await until(() => card.textContent.includes("The report."), "the report");
  const details = DETAILS(card);
  assert.ok(details, "a disclosure once finished");
  assert.equal(details.hidden, false);
  assert.equal(details.open, false, "collapsed by default");
  const summary = details.children.find((child) => child.tagName === "SUMMARY");
  assert.equal(summary.textContent, "Show agent activity");
  assert.ok(details.children.includes(stream), "the stream moved behind the disclosure");
  assert.equal(card.children.includes(stream), false);
  assert.equal(stream.textContent, "<b>Reading</b> the page and done");
  const report = ofClass(card, "run-card-report");
  assert.ok(card.children.indexOf(report) < card.children.indexOf(details), "the report comes first");
});

test("a restored finished task starts collapsed and stays expanded once opened", async () => {
  const failed = {
    ...TASK_VIEW, state: "failed", error: "The agent stopped.", error_kind: "harness_failed",
    started_at: "2026-10-27T09:00:01-07:00", finished_at: RECENT, instruction: "Restored and failed",
  };
  const second = held();
  let fetches = 0;
  const page = await taskPanel({
    intercept: (request) => {
      if (request.type === "list_runs") return { type: "runs_listed", runs: [failed] };
      if (request.type === "get_run") {
        fetches += 1;
        if (fetches === 1) return { type: "run_fetched", run: failed, stream_text: "First part.", next_offset: 11, more: true };
        // The next page fails, so the card renders and polls once more.
        if (fetches === 2) return { type: "error", code: "internal", message: "page broke" };
        return second.promise;
      }
      return startsTask()(request);
    },
  });
  await until(() => taskCards(page).length === 1 && page.el("task-run-area").textContent.includes("First part."), "the restored stream");
  const [card] = taskCards(page);
  const details = DETAILS(card);
  assert.ok(details, "the restored card has a disclosure");
  assert.equal(details.open, false, "a restored finished card starts collapsed");
  // The user expands it; the next stream page re-renders the card.
  await until(() => fetches === 3, "the retried page");
  details.open = true;
  details.dispatch("toggle");
  second.release({ type: "run_fetched", run: failed, stream_text: " Second part.", next_offset: 24, more: false });
  await until(() => card.textContent.includes("Second part."), "the next page");
  await settle(20);
  assert.equal(DETAILS(card), details);
  assert.equal(details.open, true, "the expansion survives re-renders");
});

test("a task card's stream uses the card's body type, not monospace", () => {
  const css = readFileSync(new URL("../../dist/panel.css", import.meta.url), "utf8");
  const rule = css.match(/\.task-card \.run-card-stream\s*\{([^}]*)\}/);
  assert.ok(rule, "a task-card stream rule");
  assert.doesNotMatch(rule[1], /monospace/);
  assert.match(rule[1], /font-family:\s*inherit/);
  assert.match(rule[1], /max-height/);
});
