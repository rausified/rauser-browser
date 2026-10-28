// Built side-panel tests: recent worklog. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  CONFIG, FakeElement, PROTOCOL_VERSION, hostReply, install, isResponseShape,
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
  page.el("worklog-period").value = "yesterday";
  page.el("worklog-period").dispatch("change");
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
  page.el("worklog-period").value = "yesterday";
  page.el("worklog-period").dispatch("change");
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
  page.el("worklog-period").value = "yesterday";
  page.el("worklog-period").dispatch("change");
  await until(() => page.el("worklog-text").textContent.includes("Could not generate"), "mismatched report");
  assert.equal(page.el("copy-worklog").disabled, true);
  assert.equal(page.el("worklog-gaps").hidden, true);
  page.el("worklog-include-gaps").dispatch("change");
  assert.match(page.el("worklog-text").textContent, /different worklog period/);
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
