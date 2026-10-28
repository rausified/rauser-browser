// Built side-panel tests: preserved notes. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  PANEL_HTML, PROTOCOL_VERSION, install, isResponseShape, until,
} from "./harness.mjs";

const PRESERVED_URL = "https://work.example/items/7";
const PRESERVED_ENTRIES = ["first", "second"].map((prefix) => ({
  key: { kind: "artifact", value: `${prefix}:work.example/7` },
  canonical_url: PRESERVED_URL, title: `<b>${prefix}</b>`, revision: prefix,
}));

function openPreserved(page) {
  page.el("preserved-notes").open = true;
  page.el("preserved-notes").dispatch("toggle");
}

function preservedButton(page, index = 0) {
  return page.el("preserved-note-list").children[index].children.find((child) => child.tagName === "BUTTON");
}

test("preserved notes show distinct identities and copy read-only text without rebinding autosave", async () => {
  const page = await install({
    tab: { id: 1, url: PRESERVED_URL, title: "Board" },
    intercept(request) {
      if (request.type === "load_note") return {
        type: "note_loaded", exists: true, revision: "url-revision", title: "Board", body: "URL note",
        key: { kind: "url", value: "primary-url-key" }, older_notes: [],
      };
      if (request.type === "list_preserved_notes") return { type: "preserved_notes_listed", notes: PRESERVED_ENTRIES };
      if (request.type === "view_preserved_note") return {
        type: "note_loaded", exists: true, revision: "first", title: "<script>first</script>",
        body: "<script>preserved text</script>", key: request.key, older_notes: [],
      };
      return null;
    },
  });
  await until(() => page.sent("load_note").length > 0 && !page.el("note-body").disabled, "primary note");
  assert.equal(page.sent("list_preserved_notes").length, 0, "discovery is explicit");
  openPreserved(page);
  await until(() => page.el("preserved-note-list").children.length === 2, "candidate list");
  assert.match(page.el("preserved-note-list").textContent, /first:work.example\/7/);
  assert.match(page.el("preserved-note-list").textContent, /second:work.example\/7/);
  for (const item of page.el("preserved-note-list").children) {
    assert.deepEqual(item.children.filter((child) => child.tagName === "BUTTON").map((child) => child.textContent), ["View read only"]);
  }
  preservedButton(page).click();
  await until(() => !page.el("preserved-note-view").hidden, "read-only preserved note");
  assert.equal(page.el("preserved-note-body").value, "<script>preserved text</script>");
  assert.match(PANEL_HTML, /id="preserved-note-body"[^>]*readonly/);
  assert.equal(page.el("note-body").value, "URL note");
  page.el("copy-preserved-note").click();
  assert.deepEqual(page.state.copied, ["<script>preserved text</script>"]);
  page.el("note-body").value = "edited URL note";
  page.el("note-body").dispatch("input");
  await page.close();
  const save = page.sent("save_note")[0];
  assert.equal(save.body, "edited URL note");
  assert.equal(save.expected_revision, "url-revision");
  assert.deepEqual(save.expected_key, { kind: "url", value: "primary-url-key" });
  assert.equal(page.sent("use_artifact_note").length, 0);
  assert.equal(page.sent("update_config").length, 0);
});

for (const action of ["navigate", "collapse"]) {
  test(`a preserved-note list reply after ${action} cannot restore old results`, async () => {
    let release;
    const page = await install({
      tab: { id: 1, url: PRESERVED_URL, title: "Board" },
      intercept(request) {
        if (request.type === "list_preserved_notes") return new Promise((resolve) => { release = resolve; });
        return null;
      },
    });
    await until(() => page.sent("load_note").length > 0 && !page.el("note-body").disabled, "primary note");
    openPreserved(page);
    await until(() => release, "list request");
    if (action === "navigate") {
      page.navigate("https://other.example/page", 2);
      await until(() => page.el("page-url").textContent === "https://other.example/page", "navigation");
    } else {
      page.el("preserved-notes").open = false;
      page.el("preserved-notes").dispatch("toggle");
    }
    release({ type: "preserved_notes_listed", notes: PRESERVED_ENTRIES });
    await new Promise((resolve) => setTimeout(resolve, 20));
    assert.equal(page.el("preserved-note-list").children.length, 0);
    assert.equal(page.el("preserved-note-view").hidden, true);
    assert.equal(page.el("preserved-notes").open, false);
  });
}

for (const action of ["navigate", "close", "another selection"]) {
  test(`a late preserved-note view after ${action} cannot show stale text`, async () => {
    let release;
    const reply = (key) => ({ type: "note_loaded", exists: true, revision: "v", title: key.value,
      body: key.value, key, older_notes: [] });
    const page = await install({
      tab: { id: 1, url: PRESERVED_URL, title: "Board" },
      intercept(request) {
        if (request.type === "list_preserved_notes") return { type: "preserved_notes_listed", notes: PRESERVED_ENTRIES };
        if (request.type === "view_preserved_note") {
          if (request.key.value.startsWith("first:")) return new Promise((resolve) => { release = resolve; });
          return reply(request.key);
        }
        return null;
      },
    });
    await until(() => page.sent("load_note").length > 0 && !page.el("note-body").disabled, "primary note");
    openPreserved(page);
    await until(() => page.el("preserved-note-list").children.length === 2, "candidates");
    preservedButton(page).click();
    await until(() => release, "view request");
    if (action === "navigate") {
      page.navigate("https://other.example/page", 2);
      await until(() => page.el("page-url").textContent === "https://other.example/page", "navigation");
    } else if (action === "close") {
      // The close control also cancels an in-flight read.
      page.el("close-preserved-note").click();
    } else {
      preservedButton(page, 1).click();
      await until(() => page.el("preserved-note-body").value.startsWith("second:"), "second note");
    }
    release(reply(PRESERVED_ENTRIES[0].key));
    await new Promise((resolve) => setTimeout(resolve, 20));
    assert.equal(page.el("preserved-note-body").value, action === "another selection" ? PRESERVED_ENTRIES[1].key.value : "");
    assert.equal(page.el("note-body").value, "");
  });
}

test("preserved-note failures and empty results stay separate from the editor", async () => {
  let fail = true;
  const page = await install({
    tab: { id: 1, url: PRESERVED_URL, title: "Board" },
    intercept(request) {
      if (request.type === "list_preserved_notes") return fail
        ? { type: "error", code: "internal", message: "could not read notes" }
        : { type: "preserved_notes_listed", notes: [] };
      return null;
    },
  });
  await until(() => page.sent("load_note").length > 0 && !page.el("note-body").disabled, "primary note");
  openPreserved(page);
  await until(() => page.el("preserved-notes-status").textContent.includes("could not read notes"), "local error");
  assert.equal(page.el("note-body").disabled, false);
  assert.equal(page.el("note-status").textContent, "");
  page.el("preserved-notes").open = false;
  page.el("preserved-notes").dispatch("toggle");
  fail = false;
  openPreserved(page);
  await until(() => page.el("preserved-notes-status").textContent.includes("No matching"), "empty result");
});

test("preserved-note response validation rejects non-artifacts, extra fields and oversized lists", () => {
  const valid = { type: "preserved_notes_listed", protocol_version: PROTOCOL_VERSION, request_id: "r", notes: PRESERVED_ENTRIES };
  assert.equal(isResponseShape(valid), true);
  for (const bad of [
    { ...valid, notes: Array(25).fill(PRESERVED_ENTRIES[0]) },
    { ...valid, notes: [{ ...PRESERVED_ENTRIES[0], key: { kind: "url", value: "x" } }] },
    { ...valid, notes: [{ ...PRESERVED_ENTRIES[0], title: "x".repeat(2049) }] },
    { ...valid, notes: [{ ...PRESERVED_ENTRIES[0], body: "must arrive only on explicit view" }] },
    { ...valid, extra: "no" },
  ]) assert.equal(isResponseShape(bad), false);
});
