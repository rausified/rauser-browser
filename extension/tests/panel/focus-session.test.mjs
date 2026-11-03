import assert from "node:assert/strict";
import test from "node:test";
import { install, until } from "./harness.mjs";
const A = { id: "00000000-0000-4000-8000-000000000001", name: "Release", revision: 1 };
function type(page, value) { page.el("focus-name").value = value; page.el("focus-name").dispatch("input"); }
function submit(page) { page.el("focus-form").dispatch("submit", { preventDefault() {} }); }

test("panel starts, renames and ends sessions only from explicit gestures", async () => {
  let active = null;
  let page;
  const changes = [];
  page = await install({ tab: { id: 1, url: "https://example.com/1", title: "Page" }, workerReply: (request) => {
    if (request.kind !== "change_focus") return;
    changes.push(request);
    active = request.name === null ? null : { ...A, name: request.name, revision: (active?.revision ?? 0) + 1 };
    page.state.workerStatus.focus_session = active;
    return { ok: true, value: { ...page.state.workerStatus } };
  } });
  await until(() => page.state.workerRequests.some((r) => r.kind === "sync_now"), "panel ready");
  assert.equal(changes.length, 0);
  type(page, "Release"); submit(page);
  await until(() => page.el("focus-current").textContent === "Release", "started session");
  assert.equal(changes[0].expected, null);
  assert.equal(page.el("focus-save").textContent, "Rename session");
  assert.equal(page.el("focus-end").hidden, false);
  type(page, "Next milestone"); submit(page);
  await until(() => page.el("focus-current").textContent === "Next milestone", "renamed session");
  assert.deepEqual(changes[1].expected, A);
  page.el("focus-end").click();
  await until(() => page.el("focus-current").textContent === "None", "ended session");
  assert.equal(changes[2].name, null);
  assert.equal(page.sent("start_task").length + page.sent("start_run").length, 0);
});

test("session names render as text and status refresh preserves an unsent name", async () => {
  const active = { ...A, name: "<img src=x onerror=attack()>" };
  const page = await install({ tab: { id: 1, url: "https://example.com/1" }, workerStatus: { focus_session: active } });
  await until(() => page.el("focus-current").textContent === active.name, "saved label");
  type(page, "My unsent draft");
  page.state.workerStatus.focus_session = { ...active, revision: 2, name: "Other panel" };
  page.el("tab-recent").click();
  // The harness's periodic refresh is intentionally not needed: the
  // capture-strip button requests a fresh status too.
  page.el("replay").click();
  await until(() => page.el("focus-current").textContent === "Other panel", "changed label");
  assert.equal(page.el("focus-name").value, "My unsent draft");
});

test("session failure preserves the draft and announces the worker's stale-state refusal", async () => {
  const page = await install({ tab: { id: 1, url: "chrome://extensions/" }, workerReply: (request) =>
    request.kind === "change_focus" ? { ok: false, value: null, error: "The focus session changed in another panel." } : undefined,
  });
  await until(() => page.state.workerRequests.some((r) => r.kind === "sync_now"), "ready");
  type(page, "My next session"); submit(page);
  await until(() => page.el("focus-message").textContent.includes("changed in another panel"), "refusal");
  assert.equal(page.el("focus-name").value, "My next session");
});
