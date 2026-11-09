import assert from "node:assert/strict";
import test from "node:test";
import { CONFIG, FakeElement, install, memoryStorage, until } from "./harness.mjs";
import { SharingDrafts, compareDraft, draftScope, selectionPreview } from "../../dist/sharing-drafts.js";

const scope = draftScope(CONFIG.storage);
const day = "2026-11-09";
const row = (id, text = id, local_date = day) => ({ artifact_id: `work:${id}`, local_date, text, url: `https://work.example/${id}` });
const selection = (choices = [row("A"), row("B")], extra = {}) => ({
  start_date: day, end_date: day, focus_session_id: null, heading: "", footer: "", include_gaps: true, choices, ...extra,
});
function reply(request = selection(), rows = [row("A"), row("B")], extra = {}) {
  return { type: "worklog_choices_result", start_date: request.start_date, end_date: request.end_date,
    choices: rows, gaps: [], gaps_truncated: false, truncated: false,
    page: { offset: request.page?.offset ?? 0, snapshot: "a".repeat(64), next_offset: null, omitted: false }, ...extra };
}
function saved(storage = memoryStorage(), extra = {}) {
  const store = new SharingDrafts(() => storage);
  return { store, storage, draft: store.save(scope, "Weekly update", selection(undefined, extra)) };
}

test("independent panels save immutable versions with exact previews and no shared-list lost updates", () => {
  const { store, storage, draft } = saved();
  const second = new SharingDrafts(() => storage).save(scope, draft.name, selection([row("B", "Later wording")]));
  assert.notEqual(second.id, draft.id);
  assert.equal(store.list(scope).length, 2);
  assert.equal(store.list(scope).find((d) => d.id === draft.id).preview, draft.preview);
  assert.equal(draft.preview, selectionPreview(selection()));
  assert.deepEqual(store.list(draftScope({ ...CONFIG.storage, root: "/another-home" })), []);
  assert.deepEqual(store.list(draftScope({ ...CONFIG.storage, profile: "other-profile" })), []);
  store.remove(second);
  assert.deepEqual(store.list(scope).map((d) => d.id), [draft.id]);
});

test("malformed and mismatched previews are ignored without deleting stored bytes", () => {
  const { store, storage, draft } = saved();
  const key = storage.key(0);
  for (const change of [{ choices: [draft.choices[0], draft.choices[0]] }, { preview: "forged" },
    { start_date: "2026-02-30" }, { include_gaps: "yes" }, { id: "bad" }, { choices: [] }]) {
    const raw = JSON.stringify({ ...draft, ...change });
    storage.setItem(key, raw);
    assert.deepEqual(store.list(scope), []);
    assert.equal(storage.getItem(key), raw);
  }
  storage.setItem(key, "{");
  assert.deepEqual(store.list(scope), []);
});

test("storage quota, catalog limits, duplicate rows and overlarge snapshots refuse without eviction", () => {
  const { store, storage } = saved();
  assert.throws(() => store.save(scope, "bad\nname", selection()), /name/);
  assert.throws(() => store.save(scope, "dup", selection([row("A"), row("A")])), /smaller selection/);
  assert.throws(() => store.save(scope, "huge", selection(Array.from({ length: 20 }, (_, i) => row(String(i), "🦊".repeat(8000))))), /smaller selection/);
  for (let i = 1; i < 20; i++) store.save(scope, `Draft ${i}`, selection());
  assert.throws(() => store.save(scope, "Overflow", selection()), /20 drafts/);
  assert.equal(storage.length, 20);
  const inaccessible = new SharingDrafts(() => ({ ...memoryStorage(), setItem() { throw new Error("quota"); } }));
  assert.throws(() => inaccessible.save(scope, "test", selection()), /quota/);
});

test("delete refuses a changed record rather than removing another panel's replacement", () => {
  const { store, storage, draft } = saved();
  storage.setItem(storage.key(0), JSON.stringify({ ...draft, name: "Elsewhere" }));
  assert.throws(() => store.remove(draft), /changed elsewhere/);
  assert.equal(store.list(scope)[0].name, "Elsewhere");
});

test("comparison crosses pages, excludes new work and proves absence only after complete traversal", async () => {
  const { draft } = saved();
  const calls = [];
  const comparison = await compareDraft(draft, async (offset, snapshot) => {
    calls.push([offset, snapshot]);
    return offset === 0 ? reply({}, [row("NEW")], { truncated: true,
      start_date: day, end_date: day, page: { snapshot: "a".repeat(64), offset: 0, next_offset: 1, omitted: false } })
      : reply(selection(), [row("A", "Changed")], { page: { snapshot: "a".repeat(64), offset: 1, next_offset: null, omitted: false } });
  }, () => true);
  assert.deepEqual(calls, [[0, undefined], [1, "a".repeat(64)]]);
  assert.deepEqual(comparison.rows.map((r) => [r.saved.artifact_id, r.state]), [["work:A", "changed"], ["work:B", "missing"]]);
  assert.equal(comparison.incomplete, false);
});

for (const limit of ["omitted", "legacy-truncated", "page-budget", "focus-truncated", "session-unavailable"]) {
  test(`comparison labels unseen entries unreached under ${limit}`, async () => {
    const session = "00000000-0000-4000-8000-000000000001";
    const { draft } = saved(memoryStorage(), limit.startsWith("focus") || limit === "session-unavailable" ? { focus_session_id: session } : {});
    let calls = 0;
    const comparison = await compareDraft(draft, async (offset) => {
      calls++;
      if (limit.startsWith("focus") || limit === "session-unavailable") return {
        ...reply(), type: "worklog_focus_choices_result", focus_session_id: session,
        selected_session: limit === "session-unavailable" ? null : { id: session, name: "Review" },
        choices: [], truncated: limit !== "session-unavailable",
      };
      if (limit === "legacy-truncated") return { ...reply(), choices: [], page: undefined, truncated: true };
      if (limit === "page-budget") return reply(selection(), [row(`new-${offset}`)], { truncated: true,
        page: { snapshot: "a".repeat(64), offset, next_offset: offset + 1, omitted: false } });
      return reply(selection(), [], { truncated: true, page: { snapshot: "a".repeat(64), offset: 0, next_offset: null, omitted: true } });
    }, () => true);
    assert.ok(comparison.rows.every((r) => r.state === "unreached"));
    assert.equal(comparison.incomplete, true);
    assert.equal(calls, limit === "page-budget" ? 50 : 1);
  });
}

test("mismatched dates, duplicate rows and changing snapshots invalidate comparison", async () => {
  const { draft } = saved();
  await assert.rejects(compareDraft(draft, async () => reply({ start_date: "2026-11-08", end_date: day }), () => true), /dates/);
  await assert.rejects(compareDraft(draft, async () => reply(selection(), [row("A"), row("A")]), () => true), /duplicate/);
  await assert.rejects(compareDraft(draft, async (offset) => reply(selection(), [row(`new-${offset}`)], {
    truncated: true, page: { snapshot: (offset ? "b" : "a").repeat(64), offset, next_offset: offset + 1, omitted: false },
  }), () => true), /changed during/);
});

test("a context change during a host reply refuses the complete result", async () => {
  const { draft } = saved(); let current = true;
  await assert.rejects(compareDraft(draft, async () => { current = false; return reply(); }, () => current), /Settings/);
});

test("snapshot pagination cannot fall back to an unpaged continuation and prove absence", async () => {
  const { draft } = saved();
  const calls = [];
  await assert.rejects(compareDraft(draft, async (offset, snapshot) => {
    calls.push([offset, snapshot]);
    return offset === 0 ? reply(selection(), [row("NEW")], { truncated: true,
      page: { snapshot: "a".repeat(64), offset: 0, next_offset: 1, omitted: false } })
      : { ...reply(selection(), [row("OTHER")]), page: undefined };
  }, () => true), /changed during/);
  assert.deepEqual(calls, [[0, undefined], [1, "a".repeat(64)]]);
  const legacy = await compareDraft(draft, async () => ({ ...reply(), page: undefined }), () => true);
  assert.ok(legacy.rows.every((r) => r.state === "unchanged"));
  assert.equal(legacy.incomplete, false);
});

function options() {
  globalThis.Option = class extends FakeElement { constructor(text, value) { super("option"); this.textContent = text; this.value = value; } };
}
const mainInputs = (page) => page.el("worklog-choices").children.filter((c) => c.tagName === "LABEL").map((c) => c.children[0]);
const reviewInputs = (page) => page.el("sharing-draft-rows").children.map((c) => c.children[0]);
function check(input, value = true) { input.checked = value; input.dispatch("change"); }
async function panel(storage = memoryStorage(), respond = (request) => reply(request, [row("A", "A <script>", request.start_date), row("B", "B", request.start_date)])) {
  options();
  const page = await install({ localStorage: storage, tab: { id: 1, url: "https://work.example/A", title: "A" },
    intercept: (request) => request.type === "worklog_choices" ? respond(request)
      : request.type === "worklog_report" ? { type: "worklog_reported", start_date: request.start_date, end_date: request.end_date,
        text: "Work", artifact_count: 1, gaps: [], gaps_truncated: false } : null });
  await until(() => page.sent("load_note").length > 0, "startup");
  page.el("worklog-curate").checked = true;
  page.el("tab-recent").click();
  await until(() => mainInputs(page).length === 2, "current choices");
  return page;
}
function name(page, value = "Release update") { page.el("sharing-draft-name").value = value; page.el("sharing-draft-name").dispatch("input"); }
function saveCurrent(page) {
  check(mainInputs(page)[0]); name(page);
  const expected = page.el("worklog-text").textContent;
  page.el("sharing-draft-save").click();
  assert.match(page.el("sharing-draft-status").textContent, /Draft saved/);
  assert.equal(page.el("sharing-draft-original").textContent, expected);
  return expected;
}

test("panel saves exactly the checked preview and reopens, copies and deletes it after restart", async () => {
  const storage = memoryStorage();
  const first = await panel(storage); const expected = saveCurrent(first);
  const draft = new SharingDrafts(() => storage).list(scope)[0];
  const second = await panel(storage);
  assert.equal(second.state.copied.length, 0);
  second.el("sharing-draft-list").value = draft.id; second.el("sharing-draft-list").dispatch("change");
  assert.equal(second.el("sharing-draft-original").textContent, expected);
  assert.equal(second.state.copied.length, 0);
  second.el("sharing-draft-copy").click();
  await until(() => second.state.copied.length === 1, "explicit saved Copy");
  assert.deepEqual(second.state.copied, [expected]);
  assert.ok(mainInputs(second).every((input) => !input.checked));
  second.el("sharing-draft-delete").click();
  assert.equal(storage.length, 0);
  assert.equal(second.el("sharing-draft-list").focused, true);
  assert.equal(second.sent("start_task").length + second.sent("save_note").length, 0);
});

test("changed entries and changed gap context require explicit review into a separate new version", async () => {
  const storage = memoryStorage(); let changed = false;
  const page = await panel(storage, (request) => reply(request, [row("A", changed ? "Revised action" : "A", request.start_date), row("NEW", "New private work", request.start_date)], {
    gaps: changed ? [{ count: 1, first_occurred_at: `${request.start_date}T12:00:00Z`, last_occurred_at: `${request.start_date}T13:00:00Z` }] : [],
  }));
  const original = saveCurrent(page);
  changed = true;
  page.el("sharing-draft-refresh").click();
  await until(() => !page.el("sharing-draft-review").hidden, "comparison");
  assert.equal(page.el("sharing-draft-original").textContent, original);
  assert.equal(reviewInputs(page).length, 1, "new activity never joins the saved identity set");
  assert.equal(reviewInputs(page)[0].checked, false);
  check(reviewInputs(page)[0]);
  assert.equal(page.el("sharing-draft-copy-review").disabled, true, "changed gap notices need separate approval");
  check(page.el("sharing-draft-context-approve"));
  const preview = page.el("sharing-draft-preview").textContent;
  assert.match(preview, /Revised action/); assert.match(preview, /Capture gaps/);
  assert.doesNotMatch(preview, /New private work/);
  page.el("sharing-draft-copy-review").click();
  await until(() => page.state.copied.length === 1, "reviewed Copy");
  assert.equal(page.state.copied[0], preview);
  page.el("sharing-draft-save-review").click();
  const versions = new SharingDrafts(() => storage).list(scope);
  assert.equal(versions.length, 2);
  assert.ok(versions.some((d) => d.preview === original));
  assert.ok(versions.some((d) => d.preview === preview));
});

test("saved focus drafts refresh their original absolute dates and session, with renamed headings reviewed separately", async () => {
  options();
  const session = "00000000-0000-4000-8000-000000000001";
  const { storage, draft } = saved(memoryStorage(), { focus_session_id: session, heading: "Focus session: Former name\n" });
  const page = await install({ localStorage: storage, tab: { id: 1, url: "https://work.example/A", title: "A" },
    intercept: (request) => {
      if (request.type !== "worklog_focus_choices") return null;
      const response = reply(request);
      delete response.page;
      return { ...response, type: "worklog_focus_choices_result", focus_session_id: session,
        sessions: [{ id: session, name: "Renamed", revision: 2 }], sessions_truncated: false,
        selected_session: { id: session, name: "Renamed", revision: 2 }, name_changed: true };
    } });
  await until(() => page.el("sharing-draft-list").children.length > 1, "draft catalog");
  page.el("sharing-draft-list").value = draft.id; page.el("sharing-draft-list").dispatch("change");
  page.el("sharing-draft-refresh").click();
  await until(() => !page.el("sharing-draft-review").hidden, "focus comparison");
  const request = page.sent("worklog_focus_choices")[0];
  assert.equal(request.start_date, day); assert.equal(request.end_date, day);
  assert.equal(request.focus_session_id, session); assert.equal(request.expected_revision, "1");
  assert.ok(reviewInputs(page).every((input) => input.checked));
  assert.equal(page.el("sharing-draft-copy-review").disabled, true);
  check(page.el("sharing-draft-context-approve"));
  assert.match(page.el("sharing-draft-preview").textContent, /Focus session: Renamed \(name changed in this period\)/);
  assert.equal(page.el("sharing-draft-original").textContent, draft.preview);
  assert.equal(page.el("sharing-draft-copy-review").disabled, false);
});

for (const incomplete of [false, true]) {
  test(`${incomplete ? "unreached" : "missing"} entries cannot enter a refreshed preview`, async () => {
    let compare = false;
    const page = await panel(memoryStorage(), (request) => reply(request,
      [row(compare ? "NEW" : "A", "A", request.start_date), row("B", "B", request.start_date)],
      { truncated: compare && incomplete, page: { snapshot: "a".repeat(64), offset: 0, next_offset: null, omitted: compare && incomplete } }));
    const original = saveCurrent(page); compare = true;
    page.el("sharing-draft-refresh").click();
    await until(() => !page.el("sharing-draft-review").hidden, "absence review");
    assert.match(page.el("sharing-draft-review-status").textContent, incomplete ? /1 unreached/ : /1 missing/);
    assert.equal(reviewInputs(page)[0].disabled, true);
    assert.equal(page.el("sharing-draft-copy-review").disabled, true);
    assert.equal(page.el("sharing-draft-save-review").disabled, true);
    assert.equal(page.el("sharing-draft-original").textContent, original);
  });
}

for (const boundary of ["selected draft", "home", "revision", "host exit"]) {
  test(`late comparisons cannot publish after ${boundary} changes`, async () => {
    let release, pendingRequest, hold = false;
    const pending = new Promise((resolve) => { release = resolve; });
    const page = await panel(memoryStorage(), (request) => {
      if (hold) { pendingRequest = request; return pending; }
      return reply(request, [row("A", "A", request.start_date), row("B", "B", request.start_date)]);
    });
    saveCurrent(page); hold = true;
    page.el("sharing-draft-refresh").click();
    await until(() => pendingRequest, "pending comparison");
    if (boundary === "selected draft") { page.el("sharing-draft-list").value = ""; page.el("sharing-draft-list").dispatch("change"); }
    else if (boundary === "host exit") page.hostExits();
    else { page.state.revision = "2"; if (boundary === "home") page.state.config.storage.root = "/another-home"; page.policyChanged(); }
    release(reply(pendingRequest, [row("A", "LATE PRIVATE", pendingRequest.start_date)]));
    await new Promise((resolve) => setTimeout(resolve, 20));
    assert.equal(page.el("sharing-draft-review").hidden, true);
    assert.doesNotMatch(page.el("sharing-draft-preview").textContent, /LATE PRIVATE/);
    if (boundary === "home") assert.equal(page.el("sharing-draft-original").textContent, "");
  });
}

test("failed storage and clipboard actions remain visible and retryable", async () => {
  const storage = memoryStorage(); const set = storage.setItem;
  const page = await panel(storage); check(mainInputs(page)[0]); name(page);
  storage.setItem = () => { throw new Error("Quota exhausted"); };
  page.el("sharing-draft-save").click();
  assert.match(page.el("sharing-draft-status").textContent, /Draft not saved.*Quota/);
  assert.equal(storage.length, 0);
  storage.setItem = set; page.el("sharing-draft-save").click();
  assert.equal(storage.length, 1);
  navigator.clipboard.writeText = async () => { throw new Error("Clipboard denied"); };
  page.el("sharing-draft-copy").click();
  await until(() => /Clipboard denied/.test(page.el("sharing-draft-status").textContent), "clipboard error");
  assert.equal(page.el("sharing-draft-copy").disabled, false);
});

test("a storage read failure clears the catalog and viewer until an explicit retry can read them", async () => {
  const storage = memoryStorage(); const get = storage.getItem;
  const page = await panel(storage); saveCurrent(page);
  storage.getItem = () => { throw new Error("Storage unavailable"); };
  page.el("worklog-include-gaps").dispatch("change");
  assert.match(page.el("sharing-draft-status").textContent, /Could not read.*Storage unavailable/);
  assert.equal(page.el("sharing-draft-list").children.length, 0);
  assert.equal(page.el("sharing-draft-list").disabled, true);
  assert.equal(page.el("sharing-draft-original").textContent, "");
  storage.getItem = get;
  page.el("worklog-include-gaps").dispatch("change");
  assert.equal(page.el("sharing-draft-list").disabled, false);
  assert.equal(page.el("sharing-draft-list").children.length, 2);
  assert.match(page.el("sharing-draft-status").textContent, /available again/);
  page.el("sharing-draft-list").value = page.el("sharing-draft-list").children[1].value;
  page.el("sharing-draft-list").dispatch("change");
  const key = storage.key(0);
  storage.setItem(key, JSON.stringify({ ...JSON.parse(storage.getItem(key)), name: "Changed elsewhere" }));
  page.el("worklog-include-gaps").dispatch("change");
  assert.equal(page.el("sharing-draft-view").hidden, true);
  assert.equal(page.el("sharing-draft-copy").disabled, true);
});
