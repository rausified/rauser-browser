import assert from "node:assert/strict";
import test from "node:test";
import { FakeElement } from "./panel/harness.mjs";
import { TaskFolderPicker } from "../dist/task-folders.js";
import { isResponseShape } from "../dist/protocol-shape.js";

function picker() {
  globalThis.document = { createElement: (tag) => new FakeElement(tag) };
  const select = new FakeElement("select");
  return { select, picker: new TaskFolderPicker(select, () => {}) };
}

const result = (folders) => ({ type: "task_folders_listed", protocol_version: 8, request_id: "r", revision: "rev", folders });

test("late folder lists cannot replace a newer refresh or silently drop a Reuse choice", async () => {
  const { picker: view, select } = picker();
  let resolveOld;
  const old = view.refresh({ call: () => new Promise((resolve) => { resolveOld = resolve; }) });
  await view.refresh({ call: async () => result([{ id: "new", path: "/work/new" }]) });
  view.choose("new");
  resolveOld(result([{ id: "old", path: "/work/old" }]));
  await old;
  assert.equal(view.selectedId, "new");
  assert.equal(view.missing, false);
  assert.equal(select.children[1].textContent, "/work/new");
  await view.refresh({ call: async () => result([]) });
  assert.equal(view.selectedId, "new");
  assert.equal(view.missing, true);
  assert.equal(select.value, "new");
  assert.match(select.children.at(-1).textContent, /Unavailable/);
});

test("failed or unsupported folder listing keeps the default usable and a reused folder explicit", async () => {
  const { picker: view } = picker();
  const host = { call: async () => { throw new Error("unsupported"); } };
  await view.refresh(host);
  assert.equal(view.selectedId, null);
  assert.equal(view.missing, false);
  view.choose("removed");
  await view.refresh(host);
  assert.equal(view.selectedId, "removed");
  assert.equal(view.missing, true);
});

test("folder replies have a bounded exact public shape", () => {
  assert.equal(isResponseShape(result([{ id: "id", path: "/project" }])), true);
  for (const folders of [[{ id: "id", path: "/project", identity: "private" }], [{ id: "", path: "/project" }], [{ id: "id", path: "" }], Array(17).fill({ id: "id", path: "/project" })]) {
    assert.equal(isResponseShape(result(folders)), false);
  }
});
