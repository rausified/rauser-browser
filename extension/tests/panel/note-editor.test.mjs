// Built side-panel tests: note editor. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  JIRA_URL, NoteRecovery, buttonIn, editNote, install, memoryStorage,
  recoveryBody, recoveryRows, settle, starterOffersFor, until,
} from "./harness.mjs";

for (const exists of [true, false]) {
  test(`older notes offer View${exists ? " only" : " and Use"} when the artifact note ${exists ? "exists" : "is absent"}`, async () => {
    const sourceUrl = "https://example.com/board?selected=7";
    const page = await install({
      tab: { id: 1, url: "https://example.com/items/7", title: "Issue 7" },
      intercept(request) {
        if (request.type === "load_note") return {
          type: "note_loaded", exists, revision: exists ? "shared" : "missing",
          title: "Issue 7", body: exists ? "shared text" : "",
          key: { kind: "artifact", value: "work:example.com/7" },
          older_notes: [{ canonical_url: sourceUrl, revision: "old", title: "Older note",
            key: { kind: "url", value: "older-url-key" } }],
        };
        if (request.type === "view_older_note") return {
          type: "note_loaded", exists: true, revision: "old", title: "Older note", body: "older text",
          key: { kind: "url", value: "older-url-key" }, older_notes: [],
        };
        return null;
      },
    });
    await until(() => page.el("older-note-list").children.length > 0 && !page.el("note-body").disabled, "artifact note to load");
    const buttons = page.el("older-note-list").children[0].children.filter((child) => child.tagName === "BUTTON");
    assert.deepEqual(buttons.map((button) => button.dataset.olderAction), exists ? ["view"] : ["view", "use"]);
    for (const listener of page.el("older-note-list").listeners.get("click")) listener({ target: buttons[0] });
    await until(() => !page.el("older-note-view").hidden, "older note to open read-only");
    assert.equal(page.el("older-note-view-body").value, "older text");
    assert.equal(page.el("note-body").value, exists ? "shared text" : "");
    assert.equal(page.sent("view_older_note")[0].source_url, sourceUrl);
    assert.equal(page.sent("use_artifact_note").length, 0);
    assert.equal(page.sent("save_note").length, 0);
  });
}

const RECOVERY_TAB = { id: 1, url: "https://docs.example/a", title: "A" };
async function notePanel(options = {}) {
  const page = await install({ tab: RECOVERY_TAB, ...options });
  await until(() => page.sent("load_note").length > 0 && !page.el("note-body").disabled,
    "the note editor to load");
  return page;
}

// A policy revision written by settings or another window's panel. The
// host owns note identity; the fake host models its URL-to-artifact change.
function installExternalAdapter(page) {
  page.state.config.adapters = { format: 1, artifacts: [starterOffersFor(new URL(JIRA_URL).origin, [])[0].adapter] };
  page.state.revision = "2";
  page.policyChanged({
    revision: "2", expires_at: Date.now() + 60_000, capture_enabled: false,
    adapters: page.state.config.adapters,
  });
}

const EXTERNAL_NOTE_KEY = { kind: "artifact", value: "jira:acme.atlassian.net/PROJ-42" };
function externalNoteReply(request, state) {
  if (request.type === "load_note" && state.revision === "2") return {
    type: "note_loaded", exists: true, revision: "artifact-r1", title: "Issue", body: "Shared note",
    key: EXTERNAL_NOTE_KEY, older_notes: [],
  };
  if (request.type === "save_note" && state.revision === "2" && request.expected_key.kind === "url") return {
    type: "note_conflict", exists: true, revision: "artifact-r1", title: "Issue", body: "Shared note",
    key: EXTERNAL_NOTE_KEY, reason: "key_changed",
  };
  return null;
}

test("an external adapter revision reloads the note before the next edit uses its new key", async () => {
  const page = await notePanel({ tab: { id: 1, url: JIRA_URL, title: "Issue" }, intercept: externalNoteReply });
  assert.equal(page.sent("load_note").length, 1);
  installExternalAdapter(page);
  await until(() => page.sent("load_note").length === 2 && !page.el("note-body").disabled,
    "the external revision to reload note identity");
  assert.equal(page.sent("load_note")[1].url, JIRA_URL);
  assert.equal(page.el("note-body").value, "Shared note");
  editNote(page, "Shared note with my next edit");
  await until(() => page.el("note-status").textContent === "Saved", "the artifact autosave");
  const [save] = page.sent("save_note");
  assert.deepEqual(save.expected_key, EXTERNAL_NOTE_KEY);
  assert.equal(save.expected_revision, "artifact-r1");
  assert.equal(save.body, "Shared note with my next edit");
  assert.equal(page.el("note-conflict").hidden, true);
});

for (const inFlight of [false, true]) {
  test(`an external revision preserves a ${inFlight ? "saving" : "pending"} note draft in copy-out`, async () => {
    let release;
    const held = new Promise((resolve) => { release = resolve; });
    const page = await notePanel({ tab: { id: 1, url: JIRA_URL, title: "Issue" }, intercept(request, state) {
      if (request.type === "save_note" && request.expected_key.kind === "url") return held;
      return externalNoteReply(request, state);
    } });
    const draft = inFlight ? "Draft already saving plus newer typing" : "Draft still pending";
    if (inFlight) {
      editNote(page, "Draft already saving");
      await until(() => page.sent("save_note").length === 1, "the old-key autosave");
    }
    editNote(page, draft);
    installExternalAdapter(page);
    const reads = page.sent("get_config").length;
    await until(() => page.sent("get_config").length > reads, "the config refresh during save");
    await settle();
    assert.equal(page.sent("save_note").length, 1);
    assert.equal(page.sent("save_note")[0].expected_key.kind, "url");
    assert.equal(page.sent("load_note").length, 1, "note reload must wait for the old-key save");
    assert.equal(page.el("note-body").disabled, true, "stop edits while rebinding the note");
    release(externalNoteReply({ type: "save_note", expected_key: { kind: "url" } }, page.state));
    await until(() => page.el("note-body").value === "Shared note" && !page.el("note-body").disabled,
      "the current artifact note");
    assert.equal(page.el("note-conflict").hidden, false);
    assert.equal(recoveryBody(recoveryRows(page)[0]).value, draft);
    assert.equal(page.sent("save_note").length, 1, "no automatic replay of the refused draft");
    assert.ok(page.state.localStorage.length > 0, "keep the unacknowledged draft in durable recovery");
    editNote(page, "A fresh artifact edit");
    await until(() => page.el("note-status").textContent === "Saved", "the next artifact edit to save");
    assert.deepEqual(page.sent("save_note").at(-1).expected_key, EXTERNAL_NOTE_KEY);
    assert.equal(recoveryBody(recoveryRows(page)[0]).value, draft);
    assert.ok(page.state.localStorage.length > 0, "acknowledging a new edit must keep the conflict copy");
  });
}

test("a policy refresh with the same host revision keeps the editor and pending draft", async () => {
  const page = await notePanel();
  editNote(page, "Still typing");
  const lookups = page.sent("get_artifact").length;
  page.policyChanged(); // The worker invalidated its lease; the host revision is unchanged.
  await until(() => page.sent("get_artifact").length > lookups, "the refresh to finish");
  await settle();
  assert.equal(page.sent("load_note").length, 1);
  assert.equal(page.el("note-body").value, "Still typing");
  assert.equal(page.sent("save_note").length, 0, "an unchanged refresh must not flush the debounce early");
});
function recoveryStorage(...entries) {
  const localStorage = memoryStorage();
  const recovery = new NoteRecovery(() => localStorage);
  for (const entry of entries) recovery.write({ reason: "Not saved", ...entry });
  return { localStorage, recovery };
}

test("individual recovery cards identify each page and copy or discard only the chosen text", async () => {
  const urlA = "https://docs.example/a?title=<img>";
  const urlB = "https://docs.example/b";
  const { localStorage, recovery } = recoveryStorage(
    { url: urlA, text: "First <script> copy" },
    { url: urlB, text: "  Second copy\nwith whitespace  " },
    { url: urlA, text: "" },
  );
  const page = await notePanel({ localStorage });
  const [a, b, empty] = recoveryRows(page);
  assert.equal(recoveryRows(page).length, 3);
  assert.ok(a.textContent.includes(urlA));
  assert.ok(b.textContent.includes(urlB));
  assert.ok(a.textContent.includes("Not saved"));
  assert.equal(recoveryBody(a).value, "First <script> copy");
  assert.equal(recoveryBody(a).readOnly, true);
  assert.ok(recoveryBody(a).getAttribute("aria-label").includes(urlA));
  assert.match(empty.textContent, /empty edit.*clear the note/i);
  buttonIn(b, "Copy").click();
  buttonIn(empty, "Copy").click();
  await settle();
  assert.deepEqual(page.state.copied, ["  Second copy\nwith whitespace  ", ""]);
  buttonIn(b, "Discard").click();
  assert.deepEqual(recovery.list().map((entry) => entry.text), ["First <script> copy", ""]);
  assert.equal(recoveryRows(page).length, 2);
  buttonIn(b, "Discard").click(); // A stale control must not discard the next row.
  assert.equal(recoveryRows(page).length, 2);
  for (const row of [...recoveryRows(page)]) buttonIn(row, "Discard").click();
  assert.equal(page.el("note-conflict").hidden, true);
  assert.equal(localStorage.length, 0);
  assert.equal(page.el("note-body").value, "");
  assert.equal(page.sent("save_note").length, 0, "review never replays saves");
});

test("individual recovery discard preserves live typing and a newer snapshot from another panel", async () => {
  const { localStorage, recovery } = recoveryStorage(
    { url: RECOVERY_TAB.url, text: "old A" },
    { url: "https://docs.example/b", text: "B" },
  );
  const page = await notePanel({ localStorage });
  const [stale, b] = recoveryRows(page);
  editNote(page, "live typing");
  recovery.write({ url: RECOVERY_TAB.url, text: "new A from another panel", reason: "Not saved" });
  buttonIn(stale, "Discard").click();
  buttonIn(b, "Copy").click();
  await settle();
  assert.equal(page.el("note-body").value, "live typing");
  assert.equal(page.el("note-body").disabled, false);
  assert.deepEqual(recovery.list().map((entry) => entry.text), ["B", "live typing", "new A from another panel"]);
  assert.equal(recoveryRows(page).length, 1);
  assert.deepEqual(page.state.copied, ["B"]);
  assert.equal(page.sent("save_note").length, 0, "review does not flush live typing");
});

test("individual recovery discard failure keeps the chosen copy readable and retryable", async () => {
  const { localStorage, recovery } = recoveryStorage(
    { url: RECOVERY_TAB.url, text: "A" },
    { url: "https://docs.example/b", text: "B" },
  );
  const page = await notePanel({ localStorage });
  const [a, b] = recoveryRows(page);
  const remove = localStorage.removeItem;
  localStorage.removeItem = () => { throw new Error("removal blocked"); };
  buttonIn(a, "Discard").click();
  assert.match(page.el("note-recovery-warning").textContent, /removal blocked/);
  assert.equal(recoveryRows(page).length, 2);
  assert.equal(recoveryBody(a).value, "A");
  buttonIn(a, "Copy").click();
  await settle();
  assert.deepEqual(page.state.copied, ["A"]);
  localStorage.removeItem = remove;
  buttonIn(a, "Discard").click();
  assert.deepEqual(recovery.list().map((entry) => entry.text), ["B"]);
  assert.equal(recoveryBody(recoveryRows(page)[0]).value, recoveryBody(b).value);
  assert.equal(page.sent("save_note").length, 0);
});

test("individual recovery copy reports clipboard failure beside the copy and can retry", async () => {
  const { localStorage } = recoveryStorage({ url: RECOVERY_TAB.url, text: "keep me" });
  const page = await notePanel({ localStorage });
  const row = recoveryRows(page)[0];
  navigator.clipboard.writeText = async () => { throw new Error("clipboard blocked"); };
  buttonIn(row, "Copy").click();
  await settle();
  assert.match(row.textContent, /Could not copy.*clipboard blocked/);
  assert.equal(localStorage.length, 1);
  navigator.clipboard.writeText = async (text) => { page.state.copied.push(text); };
  buttonIn(row, "Copy").click();
  await settle();
  assert.deepEqual(page.state.copied, ["keep me"]);
  assert.match(row.textContent, /Copied/);
  assert.doesNotMatch(row.textContent, /clipboard blocked/);
  assert.equal(page.el("note-body").value, "");
  assert.equal(page.sent("save_note").length, 0);
});

test("closing the built panel before debounce restores copy-only text and supports explicit Copy and Discard", async () => {
  const first = await notePanel();
  editNote(first, "pending <img src=x> text");
  assert.equal(first.state.localStorage.length, 1, "recovery is synchronous before pagehide");
  first.close();
  const reopened = await notePanel({ localStorage: first.state.localStorage });
  assert.equal(reopened.el("note-conflict").hidden, false);
  assert.equal(recoveryBody(recoveryRows(reopened)[0]).value, "pending <img src=x> text");
  assert.equal(reopened.el("note-body").value, "");
  assert.equal(reopened.sent("save_note").length, 0, "no automatic replay");
  buttonIn(recoveryRows(reopened)[0], "Copy").click();
  assert.deepEqual(reopened.state.copied, ["pending <img src=x> text"]);
  buttonIn(recoveryRows(reopened)[0], "Discard").click();
  assert.equal(reopened.el("note-conflict").hidden, true);
  assert.equal(reopened.state.localStorage.length, 0);
});

test("closing during a built-panel save preserves newer typing after the old host save completes", async () => {
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const first = await notePanel({ intercept: (request) => request.type === "save_note" ? held : undefined });
  editNote(first, "first save");
  await until(() => first.sent("save_note").length === 1, "the first autosave to start");
  editNote(first, "first save plus newer typing");
  first.close();
  release({
    type: "note_saved", revision: "r2", key: { kind: "url", value: RECOVERY_TAB.url },
    outcome: "created", relative_path: "notes/a.md",
  });
  const reopened = await notePanel({
    localStorage: first.state.localStorage,
    intercept: (request) => request.type === "load_note" ? {
      type: "note_loaded", exists: true, revision: "r2", title: "A", body: "first save",
      key: { kind: "url", value: RECOVERY_TAB.url }, older_notes: [],
    } : undefined,
  });
  assert.equal(reopened.el("note-body").value, "first save");
  assert.equal(recoveryBody(recoveryRows(reopened)[0]).value, "first save plus newer typing");
  assert.equal(reopened.sent("save_note").length, 0);
});

test("a built-panel conflict including an empty deletion survives reopen and later confirmed edits", async () => {
  const first = await notePanel({ intercept: (request) => request.type === "save_note" ? {
    type: "note_conflict", revision: "other-r2", body: "another window's note", exists: true, title: "A",
    key: { kind: "url", value: RECOVERY_TAB.url }, reason: "stale",
  } : undefined });
  editNote(first, "");
  await until(() => !first.el("note-conflict").hidden, "the conflict copy to show");
  assert.equal(recoveryBody(recoveryRows(first)[0]).value, "");
  first.close();
  const reopened = await notePanel({ localStorage: first.state.localStorage });
  assert.equal(reopened.el("note-conflict").hidden, false);
  assert.equal(recoveryBody(recoveryRows(reopened)[0]).value, "");
  editNote(reopened, "a fresh edit");
  await until(() => reopened.el("note-status").textContent === "Saved", "a confirmed new save");
  assert.equal(reopened.state.localStorage.length, 1, "the conflict stays until explicit discard");
  buttonIn(recoveryRows(reopened)[0], "Discard").click();
  assert.equal(reopened.state.localStorage.length, 0);
});

test("the built panel warns about recovery quota without preventing native autosave", async () => {
  const localStorage = memoryStorage();
  localStorage.setItem = () => { throw new Error("quota full"); };
  const page = await notePanel({ localStorage });
  editNote(page, "save through host");
  assert.equal(page.el("note-recovery-warning").hidden, false);
  assert.match(page.el("note-recovery-warning").textContent, /quota full/);
  await until(() => page.el("note-status").textContent === "Saved", "the native save");
  assert.equal(page.sent("save_note")[0].body, "save through host");
  assert.equal(page.el("note-recovery-warning").hidden, false);
});
