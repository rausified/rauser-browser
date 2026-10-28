// Built side-panel tests: host refresh. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  CAPTURING, JIRA_URL, artifact, editNote, install, noteLoaded,
  outageWithDraft, recognized, settle, starterOffersFor, typeWhy, until,
  withStoredWhy,
} from "./harness.mjs";

// Assigned panel refresh regressions (brwr-09c, brwr-b50, brwr-mm0).
for (const hasAdapters of [false, true]) {
  test(`enabled-capture wording describes adapter sites (${hasAdapters ? "with" : "without"} adapters)`, async () => {
    const adapters = hasAdapters
      ? [starterOffersFor("https://acme.atlassian.net")[0].adapter]
      : [];
    const page = await install({
      tab: { id: 1, url: JIRA_URL, title: "Issue" },
      config: { ...CAPTURING, adapters: { format: 1, artifacts: adapters } },
    });
    await noteLoaded(page);
    await settle();
    assert.doesNotMatch(page.el("status").textContent, /listed sites/i);
    assert.match(page.el("status").textContent, /sites with adapters/i);
    assert.equal(page.el("artifact-status").textContent, "○ Not an artifact",
      "a readable unrecognized page still has an artifact verdict");
  });
}

for (const [name, tab, contains, warning] of [
  ["site-access", { id: 1, title: "Hidden page" }, false, "site-access-warning"],
  ["chrome", { id: 1, url: "chrome://settings/", title: "Settings" }, true, "page-unusable"],
]) {
  test(`an unreadable ${name} tab has no artifact verdict`, async () => {
    const page = await install({
      tab: { id: 1, url: "https://docs.example/a", title: "A" },
      intercept: withStoredWhy,
    });
    await noteLoaded(page);
    await until(() => !page.el("artifact-card").hidden, "the initial artifact card");
    await settle();
    const before = page.sent("get_artifact").length;
    page.state.tab = tab;
    page.state.contains = contains;
    page.tabUpdated({ status: "complete" });
    await until(() => !page.el(warning).hidden, "the unreadable page warning");
    assert.equal(page.el("artifact-status").textContent, "");
    assert.equal(page.el("capture-explanation").hidden, true);
    assert.equal(page.el("artifact-card").hidden, true);
    assert.equal(page.sent("get_artifact").length, before);
  });
}

test("note autosave after a host reconnect refreshes the artifact once and preserves context", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" }, config: CAPTURING,
    intercept: (request, state) => request.type === "get_artifact"
      ? recognized({ ...artifact("doc:a", "Doc A", "old reason"), focused_seconds: state.focused ?? 60 })
      : undefined,
  });
  await outageWithDraft(page);
  await until(() => /not connected/.test(page.el("artifact-status").textContent), "the disconnected status");
  const before = page.sent("get_artifact").length;
  page.state.focused = 120;
  page.state.hostDead = false;
  editNote(page, "reconnect by saving my note");
  await until(() => page.el("note-status").textContent === "Saved", "the autosave");
  await until(() => page.el("artifact-status").textContent === "● Artifact recognized", "the recovered artifact status");
  assert.match(page.el("artifact-measures").textContent, /Focused 2 min/);
  assert.equal(page.el("artifact-why").value, "my unsaved draft");
  await settle();
  assert.equal(page.sent("get_artifact").length, before + 1);
  // Successful host calls on an already-connected session are not reconnects.
  editNote(page, "another ordinary autosave");
  await until(() => page.sent("save_note").length === 2 && page.el("note-status").textContent === "Saved", "the second autosave");
  await settle();
  assert.equal(page.sent("get_artifact").length, before + 1);
});

for (const action of [null, "dismiss-notices", "pause-capture", "discard-pending"]) {
  test(`delivery refresh survives ${action ?? "the normal poll"} and unchanged polls send no lookup`, async (t) => {
    const originalInterval = globalThis.setInterval;
    let poll;
    t.mock.method(globalThis, "setInterval", (callback, delay, ...args) => {
      if (delay === 30_000) poll = callback;
      return originalInterval(callback, delay, ...args);
    });
    const page = await install({
      tab: { id: 1, url: "https://docs.example/a", title: "A" }, config: CAPTURING,
      workerStatus: { delivered_batches: 1, rejected_count: 1, queued: 1 },
      intercept: (request, state) => request.type === "get_artifact"
        ? recognized({ ...artifact("doc:a", "Doc A"), focused_seconds: state.focused ?? 60 })
        : undefined,
      workerReply: (request) => request.kind === "get_pending_ids"
        ? { ok: true, value: ["pending-visit"] }
        : undefined,
    });
    await noteLoaded(page);
    await until(() => !page.el("pause-capture").disabled, "startup replay to finish");
    await settle();
    const before = page.sent("get_artifact").length;
    page.state.focused = 180;
    page.state.workerStatus.delivered_batches = 2;
    if (action) {
      window.confirm = () => true;
      assert.equal(page.el(action).disabled, false);
      page.el(action).click();
      await until(() => !page.el("pause-capture").disabled || !page.state.config.capture_enabled,
        "the action to finish");
      await settle();
    }
    assert.equal(typeof poll, "function");
    poll();
    await until(() => /Focused 3 min/.test(page.el("artifact-measures").textContent), "the delivered totals");
    await settle();
    assert.equal(page.sent("get_artifact").length, before + 1);
    assert.equal(page.sent("get_artifact").at(-1).url, "https://docs.example/a");
    poll();
    await settle();
    assert.equal(page.sent("get_artifact").length, before + 1, "unchanged delivery generation does not refresh");
  });
}

test("pending-pause hints never point at Pause capture while the host is unavailable", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" }, config: CAPTURING,
    workerStatus: { pause_pending: true }, intercept: withStoredWhy,
  });
  await noteLoaded(page);
  await settle();
  assert.equal(page.el("pause-capture").disabled, false);
  assert.match(page.el("why-paused-hint").textContent, /Use Pause capture/);
  page.hostExits();
  page.state.hostDead = true;
  page.titleChanged("A (1)");
  await settle();
  page.titleChanged("A (2)");
  await until(() => page.el("pause-capture").disabled, "the host outage");
  for (const id of ["why-paused-hint", "status"]) {
    assert.doesNotMatch(page.el(id).textContent, /Use Pause capture/);
  }
  assert.match(page.el("why-paused-hint").textContent, /host.*unavailable/i);
  typeWhy(page, "keep this draft");
  page.el("save-why").dispatch("click");
  await settle();
  assert.equal(page.sent("record_why").length, 0);
  assert.doesNotMatch(page.el("status").textContent, /Use Pause capture/);
  page.state.hostDead = false;
  page.followLink("https://docs.example/a?view=1", "A");
  await until(() => page.el("pause-capture").disabled === false, "the host to answer again");
  assert.match(page.el("why-paused-hint").textContent, /Use Pause capture/);
});

for (const pendingOrigin of ["https://docs.example", "https://other.example"]) {
  test(`Why saves respect this page's pending removal: ${pendingOrigin}`, async () => {
    const page = await install({
      tab: { id: 1, url: "https://docs.example/a", title: "A" }, config: CAPTURING,
      workerStatus: { pending_removals: [pendingOrigin] }, intercept: withStoredWhy,
    });
    await noteLoaded(page);
    await settle();
    typeWhy(page, "my context draft");
    const blocked = pendingOrigin === "https://docs.example";
    assert.equal(page.el("save-why").disabled, blocked);
    if (blocked) {
      assert.equal(page.el("why-paused-hint").hidden, false);
      assert.match(page.el("why-paused-hint").textContent, /settings/i);
      assert.match(page.el("why-paused-hint").textContent, /remov/i);
    }
    // Exercise the handler guard as well as disabled-button presentation.
    page.el("save-why").dispatch("click");
    await settle();
    assert.equal(page.sent("record_why").length, blocked ? 0 : 1);
    if (blocked) assert.equal(page.el("artifact-why").value, "my context draft");
  });
}
