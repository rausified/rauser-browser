// Built side-panel tests: recent worklog. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  CONFIG, FakeElement, PANEL_HTML, PROTOCOL_VERSION, hostReply, install, isResponseShape,
  statusFor, until,
} from "./harness.mjs";

for (const phase of ["before hello", "during policy install", "after startup", "before policy failure"]) {
  test(`Recent loads once when opened ${phase}`, async () => {
    const waitForHello = phase.startsWith("before");
    let releaseHello;
    const hello = new Promise((resolve) => { releaseHello = resolve; });
    let releasePolicy;
    const policy = new Promise((resolve) => { releasePolicy = resolve; });
    globalThis.Option = class extends FakeElement {
      constructor(text, value) {
        super("option");
        this.textContent = text;
        this.value = value;
      }
    };
    const page = await install({
      tab: { id: 1, url: "https://new.example/a", title: "A" },
      intercept(request) {
        if (request.type === "hello" && waitForHello) return hello;
        if (request.type === "list_artifacts") return {
          type: "artifact_list_result", artifact_types: ["work.item"], truncated: false,
          items: [{ artifact_id: "work:example/a", artifact_type: "work.item", title: "Brief review",
            url: "https://new.example/a", local_date: "2026-10-27", focused_seconds: 30,
            background_seconds: 20, opens: 1, why: null, summary: null }],
        };
        if (request.type === "worklog_report") return {
          type: "worklog_reported", text: "Reviewed the release", artifact_count: 1, gaps: [], gaps_truncated: false,
          start_date: request.start_date, end_date: request.end_date,
        };
        return null;
      },
      workerReply(request) {
        if (request.kind === "install_policy" && phase === "during policy install") return policy;
        if (request.kind === "install_policy" && phase === "before policy failure") {
          return { ok: false, error: "Worker policy install failed" };
        }
      },
    });
    if (phase === "during policy install") {
      await until(() => page.state.workerRequests.some((request) => request.kind === "install_policy"), "policy installation");
    } else if (phase === "after startup") {
      await until(() => page.sent("load_note").length > 0 && page.el("note-body").disabled === false, "connected note");
    }
    page.el("tab-recent").click();
    try {
      if (waitForHello) {
        assert.equal(page.el("recent-status").textContent, "Waiting for the host…");
        assert.equal(page.el("worklog-text").textContent, "Waiting for the host…");
        assert.equal(page.el("copy-worklog").disabled, true);
        assert.equal(page.sent("list_artifacts").length, 0);
        assert.equal(page.sent("worklog_report").length, 0);
      }
    } finally {
      releaseHello(hostReply({ type: "hello" }));
      releasePolicy({ ok: true, value: statusFor() });
      // Drain startup even if the waiting-state assertions fail.
      if (phase === "before policy failure") {
        await until(() => page.el("status").textContent.includes("Worker policy install failed"), "policy error");
      } else {
        await until(() => page.state.workerRequests.some((request) => request.kind === "sync_now"), "startup replay");
      }
    }
    await until(() => page.el("worklog-text").textContent === "Reviewed the release", "worklog result");
    await until(() => page.el("recent-list").textContent.includes("Brief review"), "Recent result");
    assert.equal(page.sent("list_artifacts").length, 1);
    assert.equal(page.sent("worklog_report").length, 1);
    assert.equal(page.el("copy-worklog").disabled, false);
  });
}

test("Recent opened before hello loads once configuration arrives, while policy installation is still pending", async () => {
  let releaseHello;
  const hello = new Promise((resolve) => { releaseHello = resolve; });
  let releasePolicy;
  const policy = new Promise((resolve) => { releasePolicy = resolve; });
  globalThis.Option = class extends FakeElement {
    constructor(text, value) {
      super("option");
      this.textContent = text;
      this.value = value;
    }
  };
  const page = await install({
    tab: { id: 1, url: "https://new.example/a", title: "A" },
    intercept(request) {
      if (request.type === "hello") return hello;
      if (request.type === "list_artifacts") return {
        type: "artifact_list_result", artifact_types: ["work.item"], truncated: false,
        items: [{ artifact_id: "work:example/a", artifact_type: "work.item", title: "Brief review",
          url: "https://new.example/a", local_date: "2026-10-27", focused_seconds: 30,
          background_seconds: 20, opens: 1, why: null, summary: null }],
      };
      if (request.type === "worklog_report") return {
        type: "worklog_reported", text: "Reviewed the release", artifact_count: 1, gaps: [], gaps_truncated: false,
        start_date: request.start_date, end_date: request.end_date,
      };
      return null;
    },
    workerReply(request) {
      if (request.kind === "install_policy") return policy;
    },
  });
  page.el("tab-recent").click();
  assert.equal(page.el("recent-status").textContent, "Waiting for the host…");
  releaseHello(hostReply({ type: "hello" }));
  try {
    await until(() => page.state.workerRequests.some((request) => request.kind === "install_policy"), "policy installation");
    await until(() => page.el("worklog-text").textContent === "Reviewed the release", "worklog before policy install finishes");
    await until(() => page.el("recent-list").textContent.includes("Brief review"), "Recent before policy install finishes");
  } finally {
    releasePolicy({ ok: true, value: statusFor() });
  }
  await until(() => page.state.workerRequests.some((request) => request.kind === "sync_now"), "startup replay");
  assert.equal(page.sent("list_artifacts").length, 1);
  assert.equal(page.sent("worklog_report").length, 1);
});

test("Recent replaces its startup wait with setup guidance when no home folder is chosen", async () => {
  let releaseHello;
  const hello = new Promise((resolve) => { releaseHello = resolve; });
  const page = await install({
    tab: { id: 1, url: "https://new.example/a", title: "A" },
    config: { ...CONFIG, storage: null },
    intercept: (request) => request.type === "hello" ? hello : null,
  });
  page.el("tab-recent").click();
  assert.equal(page.el("recent-status").textContent, "Waiting for the host…");
  releaseHello(hostReply({ type: "hello" }));
  await until(() => page.state.workerRequests.some((request) => request.kind === "sync_now"), "startup replay");
  for (const clickAgain of [false, true]) {
    if (clickAgain) page.el("tab-recent").click();
    assert.match(page.el("recent-status").textContent, /home folder.*settings/i);
    assert.match(page.el("worklog-text").textContent, /home folder.*settings/i);
    assert.equal(page.el("copy-worklog").disabled, true);
  }
  assert.equal(page.sent("list_artifacts").length, 0);
  assert.equal(page.sent("worklog_report").length, 0);
});

test("Recent sends reading filters with title and type, clears old rows and ignores late results", async () => {
  worklogOptions();
  let releaseLimited;
  const limited = new Promise((resolve) => { releaseLimited = resolve; });
  const reply = (title, scope) => ({
    type: "artifact_list_result", artifact_types: ["work.item"], truncated: false,
    items: [{ artifact_id: "work:example/a", artifact_type: "work.item", title,
      url: "https://new.example/a", local_date: "2026-10-27", focused_seconds: 30,
      background_seconds: 20, opens: 1, why: null, summary: {
        summarized_at: "2026-10-27T12:00:00Z", document_type: "report", category: "work", importance: "high",
        summary_path: "summaries/report.md", harness: "codex", ...(scope ? { read_scope: scope } : {}),
      } }],
  });
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, intercept(request) {
    if (request.type !== "list_artifacts") return null;
    if (request.summary_reading === "limited") return limited;
    return reply(request.summary_reading === "whole" ? "Whole report" : "All reports", "whole");
  } });
  await until(() => page.sent("load_note").length > 0, "startup");
  page.el("tab-recent").click();
  await until(() => page.el("recent-list").textContent.includes("All reports"), "unfiltered results");
  assert.equal(Object.hasOwn(page.sent("list_artifacts")[0], "summary_reading"), false);
  page.el("recent-type").value = "work.item";
  page.el("recent-title").value = "report";
  page.el("recent-reading").value = "limited";
  page.el("recent-reading").dispatch("change");
  assert.equal(page.el("recent-list").textContent, "");
  await until(() => page.sent("list_artifacts").length === 2, "limited request");
  assert.equal(page.sent("list_artifacts").at(-1).artifact_type, "work.item");
  assert.equal(page.sent("list_artifacts").at(-1).title_query, "report");
  assert.equal(page.sent("list_artifacts").at(-1).summary_reading, "limited");
  page.el("recent-reading").value = "whole";
  page.el("recent-reading").dispatch("change");
  await until(() => page.el("recent-list").textContent.includes("Whole report"), "whole results");
  releaseLimited(reply("Late partial report", "partial"));
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(page.el("recent-list").textContent.includes("Late partial report"), false);
  assert.equal(page.el("recent-type").value, "work.item");
  assert.equal(page.sent("start_run").length + page.sent("start_task").length, 0);
});

test("Recent reading has a named selector, honest empty results and older-host guidance", async () => {
  worklogOptions();
  assert.match(PANEL_HTML, /<label for="recent-reading">Saved summary reading<\/label>/);
  for (const value of ["limited", "whole", "unknown"]) assert.ok(PANEL_HTML.includes(`value="${value}"`));
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, intercept(request) {
    if (request.type !== "list_artifacts") return null;
    if (request.summary_reading === "limited") return {
      type: "error", code: "invalid_request", message: "unsupported request",
    };
    return { type: "artifact_list_result", artifact_types: [], items: [], truncated: false };
  } });
  await until(() => page.sent("load_note").length > 0, "startup");
  page.el("tab-recent").click();
  await until(() => page.el("recent-status").textContent.startsWith("0 artifact-day"), "all results");
  page.el("recent-reading").value = "unknown";
  page.el("recent-reading").dispatch("change");
  await until(() => page.sent("list_artifacts").at(-1)?.summary_reading === "unknown" && page.el("recent-status").textContent.startsWith("0 artifact-day"), "unknown empty results");
  assert.match(page.el("recent-list").textContent, /No matching saved summaries/);
  page.el("recent-reading").value = "limited";
  page.el("recent-reading").dispatch("change");
  await until(() => page.el("recent-status").textContent.includes("Rebuild"), "old host guidance");
  assert.match(page.el("recent-status").textContent, /choose All activity/);
  assert.equal(page.el("recent-list").textContent, "");
  page.el("recent-reading").value = "";
  page.el("recent-reading").dispatch("change");
  await until(() => page.el("recent-status").textContent.startsWith("0 artifact-day"), "all restored");
  assert.equal(Object.hasOwn(page.sent("list_artifacts").at(-1), "summary_reading"), false);
});

/** Picks a worklog period the way a click on its segment does: the radio
 * becomes the checked one of its group and fires change. */
function chooseWorklogPeriod(page, period) {
  for (const name of ["today", "yesterday", "week"]) page.el(`worklog-${name}`).checked = name === period;
  page.el(`worklog-${period}`).dispatch("change");
}

test("the worklog period is a Today / Yesterday / Week radio group, Today first", () => {
  const group = PANEL_HTML.match(/<fieldset id="worklog-period"[^>]*>[\s\S]*?<\/fieldset>/)?.[0];
  assert.ok(group, "a fieldset groups the period segments");
  assert.match(group, /<legend class="visually-hidden">Worklog period<\/legend>/);
  assert.equal(PANEL_HTML.includes('<select id="worklog-period"'), false, "the select is gone");
  const radios = [...group.matchAll(/<input id="worklog-(\w+)" type="radio" name="worklog-period"[^>]*>/g)];
  assert.deepEqual(radios.map((match) => match[1]), ["today", "yesterday", "week"]);
  assert.match(radios[0][0], /\bchecked\b/, "Today starts selected");
  for (const [id, text] of [["today", "Today"], ["yesterday", "Yesterday"], ["week", "Week"]]) {
    assert.match(group, new RegExp(`<label for="worklog-${id}"[^>]*>${text}</label>`));
  }
});

function worklogReply(request, gaps = [], gaps_truncated = false) {
  return {
    type: "worklog_reported", start_date: request.start_date, end_date: request.end_date,
    text: "Opened 1 artifact.\nReviewed the release", artifact_count: 1, gaps, gaps_truncated,
  };
}

function worklogOptions() {
  globalThis.Option = class extends FakeElement {
    constructor(text, value) { super("option"); this.textContent = text; this.value = value; }
  };
}

const WORKLOG_GAP = { count: 3, first_occurred_at: "2026-10-27T23:00:00-07:00", last_occurred_at: "2026-10-28T02:00:00-07:00" };

test("worklog preview shows gaps and Copy follows the explicit include choice", async () => {
  worklogOptions();
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, intercept: (request) => request.type === "worklog_report" ? worklogReply(request, [WORKLOG_GAP]) : null });
  await until(() => page.sent("load_note").length > 0, "startup");
  page.el("tab-recent").click();
  await until(() => !page.el("copy-worklog").disabled, "worklog report");
  assert.equal(page.el("worklog-gaps").hidden, false);
  assert.match(page.el("worklog-gap-list").textContent, /3 completed activity-hour records missing/);
  assert.match(page.el("worklog-gap-list").textContent, /2026-10-27T23:00:00-07:00/);
  assert.match(page.el("worklog-gap-status").textContent, /full reported range.*overlap/);
  assert.equal(page.el("worklog-include-gaps").checked, true);
  page.el("copy-worklog").click();
  await until(() => page.state.copied.length === 1, "Copy with gaps");
  assert.equal(page.state.copied[0], page.el("worklog-text").textContent);
  assert.match(page.state.copied[0], /Capture gaps: this worklog is incomplete/);
  page.el("worklog-include-gaps").checked = false;
  page.el("worklog-include-gaps").dispatch("change");
  assert.equal(page.el("worklog-text").textContent, "Opened 1 artifact.\nReviewed the release");
  assert.equal(page.el("worklog-gaps").hidden, false, "notices stay visible when excluded from Copy");
  page.el("copy-worklog").click();
  await until(() => page.state.copied.length === 2, "Copy without gaps");
  assert.equal(page.state.copied[1], page.el("worklog-text").textContent);
  // Keep the person's choice on a new period; do not silently include notices again.
  chooseWorklogPeriod(page, "yesterday");
  await until(() => page.sent("worklog_report").length === 2 && !page.el("copy-worklog").disabled, "new period");
  assert.equal(page.el("worklog-include-gaps").checked, false);
  assert.doesNotMatch(page.el("worklog-text").textContent, /Capture gaps/);
});

test("new worklog periods discard old gaps and ignore late reports", async () => {
  worklogOptions();
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  let first;
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, intercept(request) {
    if (request.type !== "worklog_report") return null;
    if (!first) { first = request; return held; }
    return worklogReply(request);
  } });
  await until(() => page.sent("load_note").length > 0, "startup");
  page.el("tab-recent").click();
  await until(() => first, "held report");
  chooseWorklogPeriod(page, "yesterday");
  await until(() => !page.el("copy-worklog").disabled, "second report");
  release(worklogReply(first, [WORKLOG_GAP], true));
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(page.el("worklog-gaps").hidden, true);
  assert.doesNotMatch(page.el("worklog-text").textContent, /Capture gaps/);
});

test("truncated gap notices are explicit in the preview and copied text", async () => {
  worklogOptions();
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, intercept: (request) => request.type === "worklog_report" ? worklogReply(request, [WORKLOG_GAP], true) : null });
  await until(() => page.sent("load_note").length > 0, "startup");
  page.el("tab-recent").click();
  await until(() => !page.el("copy-worklog").disabled, "worklog report");
  assert.match(page.el("worklog-gap-list").textContent, /only the first 32/);
  page.el("copy-worklog").click();
  await until(() => page.state.copied.length === 1, "Copy");
  assert.match(page.state.copied[0], /only the first 32/);
});

test("a failed or mismatched worklog report cannot copy a stale preview", async () => {
  worklogOptions();
  let fail = false;
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, intercept(request) {
    if (request.type !== "worklog_report") return null;
    return fail ? { ...worklogReply(request), start_date: "1900-01-01" } : worklogReply(request, [WORKLOG_GAP]);
  } });
  await until(() => page.sent("load_note").length > 0, "startup");
  page.el("tab-recent").click();
  await until(() => !page.el("copy-worklog").disabled, "first report");
  fail = true;
  chooseWorklogPeriod(page, "yesterday");
  await until(() => page.el("worklog-text").textContent.includes("Could not generate"), "mismatched report");
  assert.equal(page.el("copy-worklog").disabled, true);
  assert.equal(page.el("worklog-gaps").hidden, true);
  page.el("worklog-include-gaps").dispatch("change");
  assert.match(page.el("worklog-text").textContent, /different worklog period/);
});

function choiceRows(page) {
  return page.el("worklog-choices").children.filter((row) => row.tagName === "LABEL").map((row) => row.children[0]);
}
function checkChoice(input, checked = true) { input.checked = checked; input.dispatch("change"); }
function choicesReply(request, ids = ["A", "PRIVATE"], extra = {}) {
  return {
    type: "worklog_choices_result", start_date: request.start_date, end_date: request.end_date,
    choices: ids.map((id) => ({ artifact_id: `doc:${id}`, local_date: request.start_date,
      text: `${id} <script> — focused 2 min · opened 1×`, url: `https://work.example/${id}?view=full` })),
    truncated: false, gaps: [], gaps_truncated: false, ...extra,
  };
}
async function curation(intercept = (request) => choicesReply(request)) {
  worklogOptions();
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, workerStatus: { delivered_batches: 0 },
    intercept: (request, state) => request.type === "worklog_choices" ? intercept(request, state)
      : request.type === "worklog_report" ? worklogReply(request) : null });
  await until(() => page.sent("load_note").length > 0, "startup");
  page.el("worklog-curate").checked = true;
  page.el("tab-recent").click();
  await until(() => choiceRows(page).length > 0, "choices");
  return page;
}

test("selected work copies exactly checked day rows, measured text, links and the period gap choice", async () => {
  const page = await curation((request) => choicesReply(request, ["A", "PRIVATE"], { gaps: [WORKLOG_GAP], truncated: true }));
  assert.equal(page.el("copy-worklog").disabled, true);
  assert.match(page.el("worklog-selection-status").textContent, /More work omitted/);
  checkChoice(choiceRows(page)[0]);
  const preview = page.el("worklog-text").textContent;
  assert.match(preview, /Selected work: \d{4}-\d{2}-\d{2} through/);
  assert.match(preview, /A <script> — focused 2 min · opened 1×/);
  assert.match(preview, /https:\/\/work.example\/A\?view=full/);
  assert.doesNotMatch(preview, /PRIVATE/);
  assert.match(preview, /Capture gaps: this period has missing activity/);
  page.el("copy-worklog").click();
  await until(() => page.state.copied.length === 1, "selected Copy");
  assert.equal(page.state.copied[0], preview);
  page.el("worklog-include-gaps").checked = false;
  page.el("worklog-include-gaps").dispatch("change");
  assert.doesNotMatch(page.el("worklog-text").textContent, /Capture gaps/);
  page.el("worklog-select-all").click();
  assert.match(page.el("worklog-text").textContent, /2 artifact-day entries/);
  assert.ok(choiceRows(page).every((row) => row.checked));
  page.el("worklog-clear").click();
  assert.equal(page.el("copy-worklog").disabled, true);
  assert.ok(choiceRows(page).every((row) => !row.checked));
  assert.equal(page.sent("save_note").length + page.sent("update_config").length, 0);
});

test("same-period refresh keeps stable selections, drops removed ones and leaves arriving work unchecked", async () => {
  let ids = ["A", "PRIVATE"];
  const page = await curation((request) => choicesReply(request, ids));
  checkChoice(choiceRows(page)[0]);
  ids = ["NEW", "A", "PRIVATE"];
  page.el("tab-recent").click();
  assert.equal(page.el("copy-worklog").disabled, true, "refresh cannot copy stale text");
  await until(() => choiceRows(page).length === 3, "refreshed choices");
  assert.deepEqual(choiceRows(page).map((row) => row.checked), [false, true, false]);
  ids = ["PRIVATE"];
  page.el("tab-recent").click();
  await until(() => choiceRows(page).length === 1, "removed choice");
  assert.equal(page.el("copy-worklog").disabled, true);
  ids = ["A", "PRIVATE"];
  page.el("tab-recent").click();
  await until(() => choiceRows(page).length === 2, "returning choice");
  assert.ok(choiceRows(page).every((row) => !row.checked), "a returning row is new work");
});

test("status polling refreshes selected work without selecting newly captured entries", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  let ids = ["A", "PRIVATE"];
  const page = await curation((request) => choicesReply(request, ids));
  checkChoice(choiceRows(page)[0]);
  ids = ["NEW", "A", "PRIVATE"];
  const before = page.sent("worklog_choices").length;
  page.state.workerStatus.delivered_batches = 1;
  t.mock.timers.tick(30_000);
  await until(() => page.sent("worklog_choices").length > before && choiceRows(page).length === 3, "status-driven choices");
  assert.deepEqual(choiceRows(page).map((row) => row.checked), [false, true, false]);
  assert.doesNotMatch(page.el("worklog-text").textContent, /NEW|PRIVATE/);
});

for (const movedAway of [false, true]) {
  test(`delivery refresh ${movedAway ? "respects moved" : "restores checkbox"} keyboard focus`, async () => {
    let release, waiting;
    const pending = new Promise((resolve) => { release = resolve; });
    let delay = false;
    const page = await curation((request) => {
      if (delay) { waiting = request; return pending; }
      return choicesReply(request);
    });
    // Model the browser's active element. FakeElement.focus marks the target;
    // the real keyboard fixture below checks Chromium's actual focus behavior.
    document.body = new FakeElement("body");
    document.activeElement = choiceRows(page)[1];
    delay = true;
    page.el("tab-recent").click();
    await until(() => waiting, "focus refresh");
    document.activeElement = movedAway ? page.el("recent-title") : document.body;
    release(choicesReply(waiting, ["PRIVATE", "A"]));
    await until(() => choiceRows(page).length === 2, "reordered focus choices");
    assert.equal(choiceRows(page)[0].focused ?? false, !movedAway);
    assert.equal(choiceRows(page)[1].focused ?? false, false);
  });
}

test("changing period clears selection even when Today and This week cover the same Monday", async (t) => {
  t.mock.timers.enable({ apis: ["Date"], now: new Date(2026, 10, 2, 12).getTime() });
  const page = await curation();
  checkChoice(choiceRows(page)[0]);
  const first = page.sent("worklog_choices").at(-1);
  const before = page.sent("worklog_choices").length;
  chooseWorklogPeriod(page, "week");
  await until(() => page.sent("worklog_choices").length > before && choiceRows(page).length === 2, "Monday week choices");
  const latest = page.sent("worklog_choices").at(-1);
  assert.equal(latest.start_date, first.start_date);
  assert.equal(latest.end_date, first.end_date);
  assert.ok(choiceRows(page).every((row) => !row.checked));
  assert.equal(page.el("copy-worklog").disabled, true);
});

test("changing the worklog period clears selection and late replies cannot restore it", async () => {
  let release, first;
  let delay = false;
  const pending = new Promise((resolve) => { release = resolve; });
  const page = await curation((request) => {
    if (delay) { first = request; return pending; }
    return choicesReply(request);
  });
  checkChoice(choiceRows(page)[0]);
  delay = true;
  page.el("tab-recent").click();
  await until(() => first, "delayed refresh");
  delay = false;
  chooseWorklogPeriod(page, "yesterday");
  await until(() => choiceRows(page).length === 2, "new period choices");
  assert.ok(choiceRows(page).every((row) => !row.checked));
  release(choicesReply(first, ["OLD"]));
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.doesNotMatch(page.el("worklog-choices").textContent, /OLD/);
  assert.equal(page.el("copy-worklog").disabled, true);
});

test("a host disconnect clears sharing selection and invalidates an in-flight reply", async () => {
  let release, first;
  let delay = false;
  const pending = new Promise((resolve) => { release = resolve; });
  const page = await curation((request) => {
    if (delay) { first = request; return pending; }
    return choicesReply(request);
  });
  checkChoice(choiceRows(page)[0]);
  delay = true;
  page.el("tab-recent").click();
  await until(() => first, "in-flight choices");
  page.state.hostDead = true;
  page.hostExits();
  release(choicesReply(first));
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(page.el("copy-worklog").disabled, true);
  assert.doesNotMatch(page.el("worklog-text").textContent, /Selected work:/);
  delay = false;
  page.state.hostDead = false;
  page.el("tab-recent").click();
  await until(() => choiceRows(page).length === 2, "reconnected choices");
  assert.ok(choiceRows(page).every((row) => !row.checked));
});

test("changing home folders clears selected identities even when artifact IDs repeat", async () => {
  const page = await curation();
  checkChoice(choiceRows(page)[0]);
  const before = page.sent("worklog_choices").length;
  page.state.config.storage.root = "/different-home";
  page.state.revision = "2";
  page.policyChanged();
  await until(() => page.sent("worklog_choices").length > before && choiceRows(page).length === 2, "new home choices");
  assert.ok(choiceRows(page).every((row) => !row.checked));
  assert.equal(page.sent("worklog_choices").at(-1).expected_revision, "2");
  assert.equal(page.el("copy-worklog").disabled, true);
});

for (const failure of ["unsupported", "mismatched", "duplicate", "outside period"]) {
  test(`selected work refuses ${failure} replies and keeps Copy disabled`, async () => {
    let fail = false;
    const page = await curation((request) => {
      const reply = choicesReply(request);
      if (!fail) return reply;
      if (failure === "unsupported") return { type: "error", code: "invalid_request", message: "invalid or unsupported request JSON" };
      if (failure === "mismatched") return { ...reply, start_date: "1900-01-01" };
      if (failure === "duplicate") return { ...reply, choices: [reply.choices[0], reply.choices[0]] };
      reply.choices[0].local_date = "1900-01-01";
      return reply;
    });
    checkChoice(choiceRows(page)[0]);
    fail = true;
    page.el("tab-recent").click();
    await until(() => /Could not generate|Rebuild/.test(page.el("worklog-text").textContent), "failed choices");
    assert.equal(page.el("copy-worklog").disabled, true);
    assert.equal(choiceRows(page).length, 0);
    page.el("worklog-select-all").click();
    assert.equal(page.el("copy-worklog").disabled, true);
    page.el("worklog-curate").checked = false;
    page.el("worklog-curate").dispatch("change");
    await until(() => !page.el("copy-worklog").disabled, "ordinary worklog remains usable");
    assert.match(page.el("worklog-text").textContent, /Opened 1 artifact/);
  });
}


const FOCUS_ONE = { id: "10000000-0000-4000-8000-000000000001", name: "Release <script>", revision: 2 };
const FOCUS_TWO = { id: "10000000-0000-4000-8000-000000000002", name: "Private work", revision: 1 };
function focusChoicesReply(request, extra = {}) {
  const selected = [FOCUS_ONE, FOCUS_TWO].find((session) => session.id === request.focus_session_id) ?? null;
  return { ...choicesReply(request, selected ? [selected.id === FOCUS_ONE.id ? "RELEASE" : "PRIVATE"] : []),
    type: "worklog_focus_choices_result", focus_session_id: request.focus_session_id,
    sessions: [FOCUS_ONE, FOCUS_TWO], sessions_truncated: false, selected_session: selected,
    name_changed: selected?.id === FOCUS_ONE.id, ...extra };
}
async function focusCuration(intercept = (request) => focusChoicesReply(request)) {
  worklogOptions();
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, workerStatus: { delivered_batches: 0 },
    intercept: (request, state) => request.type === "worklog_focus_choices" ? intercept(request, state)
      : request.type === "worklog_choices" ? choicesReply(request)
        : request.type === "worklog_report" ? worklogReply(request) : null });
  await until(() => page.sent("load_note").length > 0, "startup");
  page.el("worklog-curate").checked = true;
  page.el("worklog-share-scope").value = "focus";
  page.el("tab-recent").click();
  await until(() => page.el("worklog-focus-status").textContent !== "Loading recorded sessions…", "session catalog");
  return page;
}
function chooseFocus(page, id) {
  page.el("worklog-focus-session").value = id;
  page.el("worklog-focus-session").dispatch("change");
}

test("a recorded focus session narrows sharing before explicit selection and copies the exact preview", async () => {
  const page = await focusCuration((request) => focusChoicesReply(request, { gaps: [WORKLOG_GAP], sessions_truncated: true }));
  assert.equal(page.sent("worklog_choices").length, 0);
  assert.equal(page.el("worklog-focus-area").hidden, false);
  assert.match(page.el("worklog-focus-status").textContent, /Choose a session.*More sessions omitted/);
  assert.equal(page.el("copy-worklog").disabled, true);
  assert.equal(choiceRows(page).length, 0);
  assert.ok(page.el("worklog-focus-session").textContent.includes("Release <script>"), "session names remain text");
  chooseFocus(page, FOCUS_ONE.id);
  await until(() => choiceRows(page).length === 1, "release session entries");
  assert.equal(choiceRows(page)[0].checked, false);
  assert.match(page.el("worklog-focus-status").textContent, /name changed/);
  page.el("worklog-select-all").click();
  const preview = page.el("worklog-text").textContent;
  assert.match(preview, /Focus session: Release <script> \(name changed in this period\)/);
  assert.match(preview, /RELEASE <script> — focused 2 min/);
  assert.doesNotMatch(preview, /PRIVATE/);
  assert.match(preview, /Capture gaps: this period has missing activity/);
  page.el("copy-worklog").click();
  await until(() => page.state.copied.length === 1, "focus Copy");
  assert.equal(page.state.copied[0], preview);
  page.el("worklog-include-gaps").checked = false;
  page.el("worklog-include-gaps").dispatch("change");
  assert.doesNotMatch(page.el("worklog-text").textContent, /Capture gaps/);
  page.el("worklog-clear").click();
  assert.equal(page.el("copy-worklog").disabled, true);
  assert.equal(page.sent("start_task").length + page.sent("start_run").length + page.sent("save_note").length, 0);
});

for (const unavailable of ["empty", "unknown"]) {
  test(`session sharing explains ${unavailable} without falling back to whole-day work`, async () => {
    const page = await focusCuration((request) => focusChoicesReply(request, unavailable === "empty" ? { sessions: [] } : {}));
    if (unavailable === "empty") {
      assert.match(page.el("worklog-text").textContent, /No recorded focus sessions/);
      assert.equal(page.el("worklog-focus-session").disabled, true);
    } else {
      chooseFocus(page, "10000000-0000-4000-8000-999999999999");
      await until(() => page.el("worklog-text").textContent.includes("no recorded activity in this period"), "unavailable session");
    }
    assert.equal(choiceRows(page).length, 0);
    assert.equal(page.el("copy-worklog").disabled, true);
    assert.equal(page.sent("worklog_choices").length, 0);
  });
}

test("a policy reload with an unchanged revision restores the invalidated session catalog", async () => {
  const page = await focusCuration();
  chooseFocus(page, FOCUS_ONE.id);
  await until(() => choiceRows(page).length === 1, "session entries");
  checkChoice(choiceRows(page)[0]);
  const before = page.sent("worklog_focus_choices").length;
  page.policyChanged();
  assert.equal(page.el("copy-worklog").disabled, true);
  await until(() => page.sent("worklog_focus_choices").length > before &&
    page.el("worklog-focus-session").disabled === false, "reloaded unchanged-revision catalog");
  assert.equal(page.el("worklog-focus-session").value, "");
  assert.equal(choiceRows(page).length, 0);
  assert.match(page.el("worklog-text").textContent, /Choose a session/);
});

test("session refresh preserves checked entries and leaves newly arriving entries unchecked", async () => {
  let extra = {};
  const page = await focusCuration((request) => focusChoicesReply(request, extra));
  chooseFocus(page, FOCUS_ONE.id);
  await until(() => choiceRows(page).length === 1, "session entries");
  checkChoice(choiceRows(page)[0]);
  extra = { choices: choicesReply({ start_date: page.sent("worklog_focus_choices").at(-1).start_date }, ["NEW", "RELEASE"]).choices, truncated: true };
  page.el("tab-recent").click();
  assert.equal(page.el("copy-worklog").disabled, true);
  await until(() => choiceRows(page).length === 2, "updated session entries");
  assert.deepEqual(choiceRows(page).map((row) => row.checked), [false, true]);
  assert.match(page.el("worklog-selection-status").textContent, /More work omitted/);
  assert.doesNotMatch(page.el("worklog-text").textContent, /NEW/);
});

test("the session selector stays usable while an earlier selection is loading", async () => {
  let waiting, release;
  const pending = new Promise((resolve) => { release = resolve; });
  const page = await focusCuration((request) => {
    if (request.focus_session_id === FOCUS_ONE.id) { waiting = request; return pending; }
    return focusChoicesReply(request);
  });
  chooseFocus(page, FOCUS_ONE.id);
  await until(() => waiting, "first session request");
  assert.equal(page.el("worklog-focus-session").disabled, false,
    "disabling a focused native select drops keyboard focus");
  chooseFocus(page, FOCUS_TWO.id);
  await until(() => choiceRows(page).length === 1, "second session entries");
  release(focusChoicesReply(waiting));
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(page.el("worklog-focus-session").value, FOCUS_TWO.id);
  assert.match(page.el("worklog-choices").textContent, /PRIVATE/);
  assert.doesNotMatch(page.el("worklog-choices").textContent, /RELEASE/);
});

for (const changed of ["session", "period", "scope", "config", "home", "host exit"]) {
  test(`a late session response cannot restore sharing after changing ${changed}`, async () => {
    let pendingRequest, release;
    let delay = false;
    const pending = new Promise((resolve) => { release = resolve; });
    const page = await focusCuration((request) => {
      if (delay) { pendingRequest = request; return pending; }
      return focusChoicesReply(request);
    });
    chooseFocus(page, FOCUS_ONE.id);
    await until(() => choiceRows(page).length === 1, "original session");
    checkChoice(choiceRows(page)[0]);
    delay = true;
    page.el("tab-recent").click();
    await until(() => pendingRequest, "held session reply");
    delay = false;
    if (changed === "session") chooseFocus(page, FOCUS_TWO.id);
    if (changed === "period") chooseWorklogPeriod(page, "yesterday");
    if (changed === "scope") {
      page.el("worklog-share-scope").value = "entries";
      page.el("worklog-share-scope").dispatch("change");
    }
    if (changed === "config" || changed === "home") {
      if (changed === "home") page.state.config.storage.root = "/other-home";
      page.state.revision = "2";
      page.policyChanged();
      assert.equal(page.el("copy-worklog").disabled, true);
    }
    if (changed === "host exit") { page.state.hostDead = true; page.hostExits(); }
    release(focusChoicesReply(pendingRequest, { choices: choicesReply(pendingRequest, ["LATE"]).choices }));
    await new Promise((resolve) => setTimeout(resolve, 30));
    assert.doesNotMatch(page.el("worklog-text").textContent, /LATE/);
    assert.doesNotMatch(page.el("worklog-choices").textContent, /LATE/);
    assert.equal(page.el("copy-worklog").disabled, true);
    assert.ok(choiceRows(page).every((row) => !row.checked));
    if (changed === "session") assert.match(page.el("worklog-choices").textContent, /PRIVATE/);
    if (changed === "config" || changed === "home") {
      await until(() => page.sent("worklog_focus_choices").at(-1).expected_revision === "2", "refreshed sharing config");
      assert.equal(page.sent("worklog_focus_choices").at(-1).expected_revision, "2");
      assert.equal(page.el("worklog-focus-session").value, "");
    }
  });
}

for (const invalid of ["unsupported", "wrong session", "wrong dates", "duplicate entries", "outside date", "duplicate sessions", "unavailable with entries"]) {
  test(`focus sharing rejects ${invalid} and leaves Copy disabled`, async () => {
    const page = await focusCuration((request) => {
      if (request.focus_session_id === null) return focusChoicesReply(request);
      const reply = focusChoicesReply(request);
      if (invalid === "unsupported") return { type: "error", code: "invalid_request", message: "invalid or unsupported request JSON" };
      if (invalid === "wrong session") { reply.focus_session_id = FOCUS_TWO.id; reply.selected_session = FOCUS_TWO; }
      if (invalid === "wrong dates") reply.start_date = "1900-01-01";
      if (invalid === "duplicate entries") reply.choices.push(reply.choices[0]);
      if (invalid === "outside date") reply.choices[0].local_date = "1900-01-01";
      if (invalid === "duplicate sessions") reply.sessions.push(FOCUS_ONE);
      if (invalid === "unavailable with entries") reply.selected_session = null;
      return reply;
    });
    chooseFocus(page, FOCUS_ONE.id);
    await until(() => page.el("worklog-text").textContent.includes(invalid === "unsupported" ? "Rebuild" : "Could not generate"), "refused session scope");
    assert.equal(page.el("copy-worklog").disabled, true);
    assert.equal(choiceRows(page).length, 0);
  });
}

test("worklog choice response validation rejects malformed rows and oversized arrays", () => {
  const request = { start_date: "2026-11-01", end_date: "2026-11-02" };
  const reply = { ...choicesReply(request), protocol_version: PROTOCOL_VERSION, request_id: "choices" };
  assert.equal(isResponseShape(reply), true);
  const row = reply.choices[0];
  for (const invalid of [{ ...row, artifact_id: "" }, { ...row, local_date: "yesterday" },
    { ...row, text: 12 }, { ...row, url: "x".repeat(8193) }, { ...row, selected: true }]) {
    assert.equal(isResponseShape({ ...reply, choices: [invalid] }), false);
  }
  assert.equal(isResponseShape({ ...reply, choices: Array(201).fill(row) }), false);
});

test("worklog gap response validation rejects malformed notices", () => {
  const request = { start_date: "2026-10-28", end_date: "2026-10-28" };
  const reply = { ...worklogReply(request, [WORKLOG_GAP]), protocol_version: PROTOCOL_VERSION, request_id: "gap" };
  assert.equal(isResponseShape(reply), true);
  for (const gaps of [[null], [{ ...WORKLOG_GAP, count: 0 }], [{ ...WORKLOG_GAP, count: -1 }], [{ ...WORKLOG_GAP, first_occurred_at: 2 }], Array(33).fill(WORKLOG_GAP)]) {
    assert.equal(isResponseShape({ ...reply, gaps }), false);
  }
});

test("an older host gives worklog rebuild guidance with Copy disabled", async () => {
  worklogOptions();
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, intercept: (request) => request.type === "worklog_report"
    ? { type: "error", code: "invalid_request", message: "unsupported request" } : null });
  await until(() => page.sent("load_note").length > 0, "startup");
  page.el("tab-recent").click();
  await until(() => page.el("worklog-text").textContent.includes("Rebuild"), "rebuild guidance");
  assert.equal(page.el("copy-worklog").disabled, true);
  assert.equal(page.el("worklog-gaps").hidden, true);
});
