import assert from "node:assert/strict";
import test from "node:test";

// Run after build:extension.
const { MAX_NOTE_TITLE_BYTES, NoteEditor, clampUtf8 } = await import("../dist/note-editor.js");
const { NoteRecovery } = await import("../dist/note-recovery.js");

// Pending autosave timers must not keep the test process alive.
const realSetTimeout = globalThis.setTimeout;
globalThis.setTimeout = (callback, ms, ...args) => {
  const timer = realSetTimeout(callback, ms, ...args);
  timer.unref();
  return timer;
};

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

// Lets queued promise callbacks run.
async function settle() {
  for (let index = 0; index < 20; index += 1) await Promise.resolve();
}

function harness(debounceMs = 60_000, recovery) {
  const view = {
    body: "",
    editable: false,
    status: "",
    warning: false,
    unsaved: [],
    older: [],
    readBody() { return this.body; },
    writeBody(body) { this.body = body; },
    setEditable(editable) { this.editable = editable; },
    setStatus(message, warning = false) {
      this.status = message;
      this.warning = warning;
    },
    showUnsaved(entries) { this.unsaved = entries.map((entry) => ({ ...entry })); },
    setRecoveryWarning(message) { this.recoveryWarning = message; },
    showOlderNotes(entries, canUse) {
      this.older = entries.map((entry) => ({ ...entry }));
      this.canUseOlderNote = canUse;
    },
  };
  const calls = [];
  const host = {
    load(url) {
      const pending = deferred();
      calls.push({ kind: "load", url, ...pending });
      return pending.promise;
    },
    save(request) {
      const pending = deferred();
      calls.push({ kind: "save", ...request, ...pending });
      return pending.promise;
    },
  };
  const editor = new NoteEditor(view, host, (error) => error.message, debounceMs, recovery);
  // Answers the oldest unanswered host call of `kind`.
  async function next(kind) {
    await settle();
    const call = calls.find((item) => item.kind === kind && !item.answered);
    assert.ok(call, `expected a ${kind} call`);
    call.answered = true;
    return call;
  }
  function type(text) {
    view.body = text;
    editor.edited();
  }
  return { view, calls, editor, next, type };
}

const page = (url, title = url) => ({ url, title });
const loaded = (body, revision = `rev-${body}`) => ({
  exists: body !== "",
  revision,
  body,
  key: { kind: "url", value: "test-key" },
  older_notes: [],
});

async function open(h, url, body) {
  const done = h.editor.showPage(page(url));
  (await h.next("load")).resolve(loaded(body));
  await done;
}

test("a stale tab switch cannot bind the editor to a page that is no longer active", async () => {
  const h = harness();
  await open(h, "https://p.test/", "p");
  h.type("p edited");
  void h.editor.showPage(page("https://a.test/"));
  const pSave = await h.next("save");
  // While P's save is in flight the user moves on to A and then B.
  const final = h.editor.showPage(page("https://b.test/"));
  pSave.resolve({ type: "note_saved", revision: "p2" });
  (await h.next("load")).resolve(loaded("b"));
  await final;
  assert.deepEqual(h.calls.filter((call) => call.kind === "load").map((call) => call.url),
    ["https://p.test/", "https://b.test/"]);
  assert.equal(h.view.body, "b");
  h.type("b edited");
  const save = h.editor.flush();
  const bSave = await h.next("save");
  assert.equal(bSave.url, "https://b.test/");
  assert.equal(bSave.expected_revision, "rev-b");
  bSave.resolve({ type: "note_saved", revision: "b2" });
  await save;
});

test("a late load result for an abandoned page is ignored", async () => {
  const h = harness();
  void h.editor.showPage(page("https://a.test/"));
  const aLoad = await h.next("load");
  const final = h.editor.showPage(page("https://b.test/"));
  aLoad.resolve(loaded("a"));
  await settle();
  // Under B's header, A's note must never become editable, even briefly.
  assert.equal(h.view.editable, false);
  assert.equal(h.view.body, "");
  (await h.next("load")).resolve(loaded("b"));
  await final;
  assert.equal(h.view.body, "b");
  assert.equal(h.view.editable, true);
});

test("switching to an unusable page drops an in-flight load", async () => {
  const h = harness();
  void h.editor.showPage(page("https://a.test/"));
  const aLoad = await h.next("load");
  const final = h.editor.showPage(null);
  assert.equal(h.view.editable, false);
  aLoad.resolve(loaded("a"));
  await final;
  assert.equal(h.view.body, "");
  assert.equal(h.view.editable, false);
});

test("text that fails to save before a tab switch is kept for the user", async () => {
  const h = harness();
  await open(h, "https://p.test/", "p");
  h.type("p unsaved");
  const failed = h.editor.flush();
  (await h.next("save")).reject(new Error("Native host is not connected"));
  await failed;
  assert.equal(h.view.warning, true);
  const switched = h.editor.showPage(page("https://a.test/"));
  (await h.next("save")).reject(new Error("Native host is not connected"));
  (await h.next("load")).resolve(loaded("a"));
  await switched;
  assert.equal(h.view.body, "a");
  assert.deepEqual(h.view.unsaved, [{
    url: "https://p.test/",
    text: "p unsaved",
    reason: "Could not save: Native host is not connected",
  }]);
});

test("edits typed during a save are saved once it finishes", async () => {
  const h = harness();
  await open(h, "https://p.test/", "p");
  h.type("one");
  void h.editor.flush();
  const first = await h.next("save");
  h.type("one two");
  const second = h.editor.flush();
  await settle();
  assert.equal(h.calls.filter((call) => call.kind === "save").length, 1, "saves never overlap");
  first.resolve({ type: "note_saved", revision: "r1" });
  const followUp = await h.next("save");
  assert.equal(followUp.body, "one two");
  assert.equal(followUp.expected_revision, "r1");
  followUp.resolve({ type: "note_saved", revision: "r2" });
  await second;
  assert.equal(h.view.status, "Saved");
});

test("a save that outlasts the debounce does not claim Saved while edits are pending", async () => {
  const h = harness();
  await open(h, "https://p.test/", "p");
  h.type("one");
  void h.editor.flush();
  const first = await h.next("save");
  h.type("one two");
  first.resolve({ type: "note_saved", revision: "r1" });
  await settle();
  assert.notEqual(h.view.status, "Saved");
});

test("a tab switch waits for the in-flight save and saves later edits", async () => {
  const h = harness();
  await open(h, "https://p.test/", "p");
  h.type("one");
  void h.editor.flush();
  const first = await h.next("save");
  h.type("one two");
  const switched = h.editor.showPage(page("https://a.test/"));
  first.resolve({ type: "note_saved", revision: "r1" });
  const tail = await h.next("save");
  assert.equal(tail.url, "https://p.test/");
  assert.equal(tail.body, "one two");
  tail.resolve({ type: "note_saved", revision: "r2" });
  (await h.next("load")).resolve(loaded("a"));
  await switched;
  assert.deepEqual(h.view.unsaved, []);
});

test("a conflict keeps edits typed during the save and does not save the server body back", async () => {
  const h = harness(0);
  await open(h, "https://p.test/", "p");
  h.type("mine");
  void h.editor.flush();
  const save = await h.next("save");
  h.type("mine and more");
  save.resolve({ type: "note_conflict", exists: true, revision: "theirs-rev", body: "theirs" });
  await h.editor.idle();
  await new Promise((resolve) => setTimeout(resolve, 5));
  await settle();
  assert.equal(h.view.body, "theirs");
  assert.deepEqual(h.view.unsaved.map((entry) => entry.text), ["mine and more"]);
  assert.equal(h.calls.filter((call) => call.kind === "save").length, 1);
});

const keyChanged = {
  type: "note_conflict",
  exists: false,
  revision: "missing",
  body: "",
  key: { kind: "artifact", value: "work:p.test/7" },
  reason: "key_changed",
};
const olderUrlNote = {
  canonical_url: "https://p.test/7",
  revision: "rev-old key note",
  title: "old key note",
  key: { kind: "url", value: "test-key" },
};
const artifactLoad = {
  exists: false,
  revision: "missing",
  body: "",
  key: { kind: "artifact", value: "work:p.test/7" },
  older_notes: [olderUrlNote],
};

for (const outcome of ["saved", "conflict", "deleted"]) {
  test(`older-note migration follows artifact existence after save outcome: ${outcome}`, async () => {
    const h = harness();
    const opening = h.editor.showPage(page("https://p.test/7"));
    (await h.next("load")).resolve(outcome === "deleted"
      ? { ...artifactLoad, exists: true, revision: "existing", body: "shared text" }
      : artifactLoad);
    await opening;
    assert.equal(h.view.canUseOlderNote, outcome !== "deleted");
    h.type("new shared text");
    const pending = h.editor.flush();
    const save = await h.next("save");
    save.resolve(outcome === "saved"
      ? { type: "note_saved", revision: "saved", key: artifactLoad.key }
      : { type: "note_conflict", exists: outcome !== "deleted", revision: outcome === "deleted" ? "missing" : "newer",
        key: artifactLoad.key, body: outcome === "deleted" ? "" : "other writer", reason: "stale_revision" });
    await pending;
    assert.deepEqual(h.view.older, [olderUrlNote], "View keeps the older note available");
    assert.equal(h.view.canUseOlderNote, outcome === "deleted");
    if (outcome !== "saved") assert.equal(h.view.unsaved[0].text, "new shared text");
  });
}

test("a key change refuses the save and keeps the text for copy-out", async () => {
  const h = harness();
  await open(h, "https://p.test/", "old key note");
  h.type("mine under the old key");
  const pending = h.editor.flush();
  const save = await h.next("save");
  assert.deepEqual(save.expected_key, { kind: "url", value: "test-key" });
  save.resolve(keyChanged);
  (await h.next("load")).resolve(artifactLoad);
  await pending;
  assert.equal(h.view.body, "");
  assert.equal(h.view.warning, true);
  assert.match(h.view.status, /artifact identity changed/);
  assert.equal(h.view.unsaved[0].text, "mine under the old key");
});

test("a key change reloads the note so its new key, revision, and older notes are shown", async () => {
  const h = harness();
  await open(h, "https://p.test/", "old key note");
  h.type("mine under the old key");
  const pending = h.editor.flush();
  (await h.next("save")).resolve(keyChanged);
  const reload = await h.next("load");
  assert.equal(reload.url, "https://p.test/");
  reload.resolve(artifactLoad);
  await pending;
  assert.deepEqual(h.view.older, [olderUrlNote]);
  assert.equal(h.view.editable, true);
  assert.deepEqual(h.view.unsaved.map((entry) => entry.text), ["mine under the old key"]);
  // Pasting the kept text back saves under the reloaded artifact key.
  h.type("mine under the old key");
  const resaved = h.editor.flush();
  const save = await h.next("save");
  assert.deepEqual(save.expected_key, { kind: "artifact", value: "work:p.test/7" });
  assert.equal(save.expected_revision, "missing");
  save.resolve({ type: "note_saved", revision: "r-artifact", key: save.expected_key });
  await resaved;
  assert.equal(h.view.status, "Saved");
});

test("a key change never autosaves under the stale key while the note reloads", async () => {
  const h = harness(0);
  await open(h, "https://p.test/", "old key note");
  h.type("mine");
  void h.editor.flush();
  const save = await h.next("save");
  h.type("mine and more");
  save.resolve(keyChanged);
  const reload = await h.next("load");
  // The stale note is closed to edits until the reload lands.
  assert.equal(h.view.editable, false);
  h.type("typed into a closed editor");
  await new Promise((resolve) => setTimeout(resolve, 5));
  await settle();
  assert.equal(h.calls.filter((call) => call.kind === "save").length, 1);
  reload.resolve(artifactLoad);
  await h.editor.idle();
  await new Promise((resolve) => setTimeout(resolve, 5));
  await settle();
  assert.equal(h.calls.filter((call) => call.kind === "save").length, 1);
  // The reload shows the new note without clearing the copy-out box.
  assert.equal(h.view.body, "");
  assert.deepEqual(h.view.unsaved.map((entry) => entry.text), ["mine and more"]);
  assert.match(h.view.status, /artifact identity changed/);
  assert.equal(h.view.warning, true);
});

test("a key change during a tab switch loads the new tab without a second reload", async () => {
  const h = harness();
  await open(h, "https://p.test/", "old key note");
  h.type("mine under the old key");
  const switched = h.editor.showPage(page("https://a.test/"));
  (await h.next("save")).resolve(keyChanged);
  (await h.next("load")).resolve(loaded("a"));
  await switched;
  await settle();
  assert.deepEqual(h.calls.filter((call) => call.kind === "load").map((call) => call.url),
    ["https://p.test/", "https://a.test/"]);
  assert.equal(h.view.body, "a");
  assert.deepEqual(h.view.unsaved.map((entry) => entry.text), ["mine under the old key"]);
});

test("a key change in a flush queued before a tab switch leaves the new tab to that switch", async () => {
  const h = harness();
  await open(h, "https://p.test/", "old key note");
  h.type("one");
  void h.editor.flush();
  const first = await h.next("save");
  h.type("one two");
  void h.editor.flush();
  const switched = h.editor.showPage(page("https://a.test/"));
  first.resolve({ type: "note_saved", revision: "r1" });
  (await h.next("save")).resolve(keyChanged);
  (await h.next("load")).resolve(loaded("a"));
  await switched;
  await settle();
  assert.deepEqual(h.calls.filter((call) => call.kind === "load").map((call) => call.url),
    ["https://p.test/", "https://a.test/"]);
  assert.equal(h.view.body, "a");
  assert.equal(h.view.status, "Saved");
  assert.equal(h.view.warning, false);
  assert.deepEqual(h.view.unsaved.map((entry) => entry.text), ["one two"]);
});

test("a flush with nothing running sends its save before returning", async () => {
  const h = harness();
  await open(h, "https://p.test/", "p");
  h.type("closing");
  // The pagehide handler disconnects from the host right after this call.
  void h.editor.flush();
  assert.equal(h.calls.filter((call) => call.kind === "save").length, 1);
});

test("a failed load is retried for the same page", async () => {
  const h = harness();
  void h.editor.showPage(page("https://p.test/"));
  (await h.next("load")).reject(new Error("No home folder is chosen."));
  await h.editor.idle();
  assert.equal(h.view.editable, false);
  // A title update for the same page reloads the failed note.
  const again = h.editor.showPage(page("https://p.test/", "New title"));
  (await h.next("load")).resolve(loaded("p"));
  await again;
  assert.equal(h.view.body, "p");
  assert.equal(h.view.editable, true);
  // Once loaded, a repeat of the same page does not reload it.
  await h.editor.showPage(page("https://p.test/"));
  await h.editor.retry();
  await settle();
  assert.equal(h.calls.filter((call) => call.kind === "load").length, 2);
});

test("retry reloads the note after a failed load", async () => {
  const h = harness();
  void h.editor.showPage(page("https://p.test/"));
  (await h.next("load")).reject(new Error("not configured"));
  await h.editor.idle();
  const retried = h.editor.retry();
  (await h.next("load")).resolve(loaded("p"));
  await retried;
  assert.equal(h.view.editable, true);
});

test("long page titles are clamped to the host's byte limit", async () => {
  const h = harness();
  await open(h, "https://p.test/", "");
  void h.editor.showPage(page("https://p.test/", "界".repeat(1_000)));
  h.type("x");
  void h.editor.flush();
  const save = await h.next("save");
  assert.ok(new TextEncoder().encode(save.title).length <= MAX_NOTE_TITLE_BYTES);
  assert.equal(save.title, "界".repeat(682));
  save.resolve({ type: "note_saved", revision: "r" });
});

test("clampUtf8 never splits a character", () => {
  assert.equal(clampUtf8("abc", 2), "ab");
  assert.equal(clampUtf8("aé", 2), "a");
  assert.equal(clampUtf8("a😀", 4), "a");
  assert.equal(clampUtf8("a😀", 5), "a😀");
  assert.equal(clampUtf8("short", 2_048), "short");
});

function recoveryFixture() {
  const data = new Map();
  const storage = {
    get length() { return data.size; },
    key(index) { return [...data.keys()][index] ?? null; },
    getItem(key) { return data.get(key) ?? null; },
    setItem(key, value) { data.set(key, value); },
    removeItem(key) { data.delete(key); },
  };
  return { data, storage, recovery: new NoteRecovery(() => storage) };
}

test("closing before debounce leaves a synchronous recovery copy without autosaving it on reopen", async () => {
  const { recovery } = recoveryFixture();
  const h = harness(60_000, recovery);
  await open(h, "https://p.test/", "saved");
  h.type("pending before close");
  assert.equal(recovery.list()[0].text, "pending before close", "persisted in the input stack");
  assert.equal(h.calls.filter((call) => call.kind === "save").length, 0);
  const reopened = harness(60_000, recovery);
  await open(reopened, "https://p.test/", "saved");
  assert.equal(reopened.view.body, "saved");
  assert.equal(reopened.view.unsaved[0].text, "pending before close");
  assert.equal(reopened.calls.filter((call) => call.kind === "save").length, 0);
});

test("a late save acknowledgement cannot delete edits made during that save", async () => {
  const { recovery } = recoveryFixture();
  const h = harness(60_000, recovery);
  await open(h, "https://p.test/", "saved");
  h.type("first");
  const saving = h.editor.flush();
  const first = await h.next("save");
  h.type("first and more, just before closing");
  first.resolve({ type: "note_saved", revision: "first-rev" });
  await saving;
  assert.equal(recovery.list()[0].text, "first and more, just before closing");
  const reopened = harness(60_000, recovery);
  await open(reopened, "https://p.test/", "first");
  assert.equal(reopened.view.unsaved[0].text, "first and more, just before closing");
  // Completing the next save removes only this panel's pending copy.
  const tail = h.editor.flush();
  (await h.next("save")).resolve({ type: "note_saved", revision: "tail-rev" });
  await tail;
  assert.deepEqual(recovery.list(), []);
});

test("a failed save and an empty deletion both remain recoverable", async () => {
  const { recovery } = recoveryFixture();
  const h = harness(60_000, recovery);
  await open(h, "https://p.test/", "saved");
  h.type("");
  const saving = h.editor.flush();
  (await h.next("save")).reject(new Error("disconnected"));
  await saving;
  const reopened = harness(60_000, recovery);
  assert.equal(reopened.view.unsaved.length, 1);
  assert.equal(reopened.view.unsaved[0].text, "");
  assert.equal(recovery.list()[0].text, "");
});

test("a conflicted copy survives reopening and later successful edits until explicit discard", async () => {
  const { recovery } = recoveryFixture();
  const h = harness(60_000, recovery);
  await open(h, "https://p.test/", "saved");
  h.type("mine");
  const saving = h.editor.flush();
  (await h.next("save")).resolve({ type: "note_conflict", exists: true, revision: "other-rev", body: "theirs" });
  await saving;
  h.type("theirs plus a fresh edit");
  const next = h.editor.flush();
  (await h.next("save")).resolve({ type: "note_saved", revision: "next-rev" });
  await next;
  assert.deepEqual(recovery.list().map((entry) => entry.text), ["mine"]);
  const reopened = harness(60_000, recovery);
  assert.equal(reopened.view.unsaved[0].text, "mine");
  reopened.editor.discardUnsaved();
  assert.deepEqual(recovery.list(), []);
  assert.deepEqual(reopened.view.unsaved, []);
});

test("independent panels and stale discard never erase a newer recovery snapshot", async () => {
  const { recovery } = recoveryFixture();
  const a = harness(60_000, recovery);
  const b = harness(60_000, recovery);
  await open(a, "https://p.test/", "");
  await open(b, "https://p.test/", "");
  a.type("A");
  b.type("B");
  const viewer = harness(60_000, recovery); // Holds A and B's earlier versions.
  a.type("A newer");
  viewer.editor.discardUnsaved();
  assert.deepEqual(recovery.list().map((entry) => entry.text), ["A newer"]);
  const saving = b.editor.flush();
  (await b.next("save")).resolve({ type: "note_saved", revision: "b-rev" });
  await saving;
  assert.deepEqual(recovery.list().map((entry) => entry.text), ["A newer"]);
});

test("quota failure warns without replacing newer text with an older recovery copy", async () => {
  const { recovery, storage } = recoveryFixture();
  const h = harness(60_000, recovery);
  await open(h, "https://p.test/", "saved");
  h.type("old draft");
  storage.setItem = () => { throw new Error("quota full"); };
  h.type("newest draft");
  assert.match(h.view.recoveryWarning, /quota full/);
  const switched = h.editor.showPage(page("https://a.test/"));
  (await h.next("save")).reject(new Error("offline"));
  (await h.next("load")).resolve(loaded("a"));
  await switched;
  assert.equal(h.view.unsaved[0].text, "newest draft");
  assert.equal(recovery.list()[0].text, "old draft", "old bytes stay intact on quota failure");
});

test("blocked storage cannot prevent loading and saving a note", async () => {
  const recovery = new NoteRecovery(() => { throw new Error("storage blocked"); });
  const h = harness(60_000, recovery);
  assert.match(h.view.recoveryWarning, /storage blocked/);
  await open(h, "https://p.test/", "saved");
  h.type("new text");
  const saving = h.editor.flush();
  (await h.next("save")).resolve({ type: "note_saved", revision: "r2" });
  await saving;
  assert.equal(h.view.status, "Saved");
  assert.match(h.view.recoveryWarning, /storage blocked/);
});

test("failed recovery discard leaves the displayed text available for copy", async () => {
  const { recovery, storage } = recoveryFixture();
  recovery.write({ url: "https://p.test/", text: "still needed", reason: "pending" });
  const h = harness(60_000, recovery);
  storage.removeItem = () => { throw new Error("storage blocked"); };
  h.editor.discardUnsaved();
  assert.equal(h.view.unsaved[0].text, "still needed");
  assert.match(h.view.recoveryWarning, /storage blocked/);
  assert.equal(recovery.list().length, 1);
});

test("recovery listing ignores malformed and unrelated keys without deleting them", () => {
  const { recovery, data } = recoveryFixture();
  recovery.write({ url: "https://p.test/", text: "<img src=x>", reason: "pending" });
  const validKey = [...data.keys()][0];
  data.set("unrelated", "keep me");
  data.set(`${validKey}-bad`, "{bad JSON");
  data.set(`${validKey}-wrong-id`, data.get(validKey));
  assert.equal(recovery.list().length, 1);
  recovery.remove(recovery.list()[0]);
  assert.equal(data.size, 3);
  assert.equal(data.get("unrelated"), "keep me");
});

test("a partial recovery update keeps the newest conflict copy after retiring an old snapshot fails", async () => {
  const { recovery, storage } = recoveryFixture();
  const h = harness(60_000, recovery);
  await open(h, "https://p.test/", "saved");
  h.type("old draft");
  storage.removeItem = () => { throw new Error("remove blocked"); };
  h.type("newest draft");
  assert.match(h.view.recoveryWarning, /remove blocked/);
  const saving = h.editor.flush();
  (await h.next("save")).resolve({ type: "note_conflict", exists: true, revision: "other-r2", body: "theirs" });
  await saving;
  assert.equal(h.view.unsaved[0].text, "newest draft");
  const reopened = harness(60_000, recovery);
  assert.ok(reopened.view.unsaved.some((entry) => entry.text === "newest draft"));
  assert.equal(reopened.view.body, "", "a recovered conflict is never placed in the editor");
});

test("a recovered identity-conflict draft cannot autosave into the newly derived artifact key", async () => {
  const { recovery } = recoveryFixture();
  const h = harness(60_000, recovery);
  await open(h, "https://p.test/", "saved");
  h.type("edit under the old key");
  const saving = h.editor.flush();
  (await h.next("save")).resolve({
    type: "note_conflict", exists: false, revision: "other-r2", body: "", reason: "key_changed",
    key: { kind: "artifact", value: "doc:new" },
  });
  const load = await h.next("load");
  load.resolve({ ...loaded("new artifact note"), key: { kind: "artifact", value: "doc:new" } });
  await saving;
  assert.equal(h.view.body, "new artifact note");
  const reopened = harness(60_000, recovery);
  await open(reopened, "https://p.test/", "new artifact note");
  await reopened.editor.flush();
  assert.equal(reopened.view.unsaved[0].text, "edit under the old key");
  assert.equal(reopened.calls.filter((call) => call.kind === "save").length, 0);
});
