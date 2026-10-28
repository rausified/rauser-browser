// Built side-panel tests: site access. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  PANEL_HTML, install, settle, shownTitle, until,
} from "./harness.mjs";

test("an HTTP(S) tab shows the note editor and no grant or address warning", async () => {
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, contains: true });
  await until(() => page.el("note-editor").hidden === false, "the note editor to show");
  assert.equal(page.el("page-url").textContent, "https://new.example/a");
  assert.equal(shownTitle(page), "A");
  assert.equal(page.el("note-editor").hidden, false);
  assert.equal(page.el("site-access-warning").hidden, true);
  assert.equal(page.el("page-unusable").hidden, true);
  assert.equal(page.calls("permissions.request"), 0);
  await until(() => page.el("note-body").disabled === false, "the note to load");
});

test("a note refusal from damaged adopted-contract state shows the host's named repair", async () => {
  // The host names the state file and its repair; the panel must show it
  // whole and keep the editor read-only, so nothing autosaves under a guess.
  const refusal = "Notes on this page are paused to protect note identity: Brauser's record of the " +
    "adapters it has used (/config/brauser/adopted-contracts.json) cannot be read. Move " +
    "adopted-contracts.json aside (rename it, keeping a backup) or fix it to restore notes on pages " +
    "the current adapters do not recognize.";
  const page = await install({
    tab: { id: 1, url: "https://work.example/boards/1", title: "Board" },
    intercept: (request) => request.type === "load_note"
      ? { type: "error", code: "internal", message: refusal }
      : undefined,
  });
  await until(() => page.el("note-status").textContent.includes("adopted-contracts.json"),
    "the named refusal");
  assert.equal(page.el("note-status").textContent, `Could not load this page's note: ${refusal}`);
  assert.equal(page.el("note-body").disabled, true);
  await settle();
  assert.equal(page.sent("save_note").length, 0);
});

test("an HTTP(S) tab shows the note editor even when Chrome reports access withdrawn", async () => {
  // A URL means Chrome granted this site, whatever the all-sites check says.
  const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, contains: false });
  await until(() => page.el("note-editor").hidden === false, "the note editor to show");
  assert.equal(page.el("site-access-warning").hidden, true);
});

test("a tab whose address Chrome withholds while access is withdrawn shows the single banner", async () => {
  const page = await install({ tab: { id: 1, url: undefined }, contains: false });
  await until(() => page.el("site-access-warning").hidden === false, "the site-access banner");
  assert.match(page.el("site-access-warning").textContent, /Brauser needs site access — re-enable in Chrome/);
  assert.equal(page.el("note-editor").hidden, true);
  assert.equal(page.el("page-unusable").hidden, true);
  assert.equal(page.calls("permissions.request"), 0);
});

test("a chrome:// tab with access granted shows no site-access banner", async () => {
  const page = await install({ tab: { id: 1, url: undefined, title: "Extensions" }, contains: true });
  // The "not a web page" state is the settled answer for this tab.
  await until(() => page.el("page-unusable").hidden === false, "the not-a-web-page state");
  assert.equal(page.el("site-access-warning").hidden, true);
  assert.equal(page.el("note-editor").hidden, true);
  assert.equal(shownTitle(page), "Extensions");
});

test("withdrawing then restoring access hides the banner without a reload", async () => {
  const page = await install({ tab: { id: 1, url: undefined }, contains: false });
  await until(() => page.el("site-access-warning").hidden === false, "the site-access banner");

  // Re-enabling in Chrome makes the tab's address visible again.
  page.state.tab.url = "https://new.example/a";
  page.state.tab.title = "A";
  page.grant();
  await until(() => page.el("site-access-warning").hidden === true, "the banner to hide");
  await until(() => page.el("note-editor").hidden === false, "the note editor to show");
  assert.equal(page.el("page-url").textContent, "https://new.example/a");
  await until(() => page.el("note-body").disabled === false, "the note to load");

  // Withdrawing again brings the one banner back.
  page.state.tab.url = undefined;
  page.withdraw();
  await until(() => page.el("site-access-warning").hidden === false, "the banner to return");
  assert.equal(page.el("note-editor").hidden, true);
});

test("re-enable opens this extension's Chrome page", async () => {
  const page = await install({ tab: { id: 1, url: undefined }, contains: false });
  await until(() => page.el("site-access-warning").hidden === false, "the site-access banner");
  page.el("site-access-reenable").click();
  assert.deepEqual(page.created, [{ url: "chrome://extensions/?id=test-id" }]);
});

test("the panel markup has no grant button or address-unavailable note", () => {
  assert.ok(!PANEL_HTML.includes('id="note-grant-origin"'));
  assert.ok(!PANEL_HTML.includes('id="note-unavailable"'));
  assert.ok(!PANEL_HTML.includes("can't see this tab's address"));
});
