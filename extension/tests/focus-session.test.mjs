import assert from "node:assert/strict";
import test from "node:test";
const { focusName, isFocusSession, nextFocus } = await import("../dist/focus-session.js");
const { activateTab, changeFocus, emptyActivityState, focusWindow, freezeCompleted, recordNavigation } = await import("../dist/activity.js");
let n = 0;
const uuid = () => `00000000-0000-4000-8000-${String(++n).padStart(12, "0")}`;
const start = new Date(2026, 10, 3, 10, 10).getTime();
const nav = (title = "Item") => ({ tab_id: 1, window_id: 1, url: "https://example.com/1", title, transition: "link", qualifiers: [], reload: false });
const match = { match: true, type: "test.item", artifact_id: "test:example/1", mode: null, refs: [] };
function measured() {
  const state = emptyActivityState(start);
  recordNavigation(state, nav(), match, start, uuid);
  activateTab(state, 1, 1, start, uuid);
  focusWindow(state, 1, start, uuid);
  return state;
}

test("session names count Unicode scalars and reject invisible or multiline labels", () => {
  assert.equal(focusName("  Ship release 😀  "), "Ship release 😀");
  assert.equal([...focusName("😀".repeat(120))].length, 120);
  for (const name of ["", "  ", "a\nb", "a\tb", "a\u2028b", "a\u202eb", "\ud800", "x".repeat(121)]) {
    assert.throws(() => focusName(name));
  }
  const session = nextFocus(null, null, "Release", uuid);
  assert.ok(isFocusSession(session));
  assert.equal(isFocusSession({ ...session, revision: 0 }), false);
  assert.equal(isFocusSession({ ...session, id: "not-a-uuid" }), false);
  assert.equal(isFocusSession({ ...session, surprise: true }), false);
});

test("stale panels cannot rename/end another session, and rename cannot overflow", () => {
  const a = nextFocus(null, null, "A", uuid);
  const b = nextFocus(a, a, "B", uuid);
  assert.equal(a.id, b.id);
  assert.equal(b.revision, 2);
  assert.throws(() => nextFocus(b, a, "C", uuid), /changed in another panel/);
  assert.throws(() => nextFocus(b, a, null, uuid), /changed in another panel/);
  assert.throws(() => nextFocus(a, null, "C", uuid), /changed in another panel/);
  const max = { ...a, revision: 0xffff_ffff };
  assert.throws(() => nextFocus(max, max, "new", uuid), /End this session/);
  assert.equal(nextFocus(max, max, null, uuid), null);
});

test("one artifact spans ungrouped time and two sessions in the same hour without extra opens", () => {
  const state = measured();
  changeFocus(state, null, "Release", start + 10_000, uuid);
  const first = { ...state.focus_session };
  const ungrouped = freezeCompleted(state, start + 10_000, uuid);
  assert.equal(ungrouped[0].focused_seconds, 10);
  assert.equal(ungrouped[0].opens, 1);
  assert.equal("focus_session" in ungrouped[0], false, "old payloads omit the field entirely");
  changeFocus(state, first, null, start + 25_000, uuid);
  const a = freezeCompleted(state, start + 25_000, uuid);
  changeFocus(state, null, "Incident", start + 25_000, uuid);
  const second = { ...state.focus_session };
  assert.notEqual(first.id, second.id);
  changeFocus(state, second, null, start + 45_000, uuid);
  const b = freezeCompleted(state, start + 45_000, uuid);
  assert.equal(a[0].focused_seconds, 15);
  assert.equal(b[0].focused_seconds, 20);
  assert.equal(a[0].focus_session.id, first.id);
  assert.equal(b[0].focus_session.id, second.id);
  assert.equal(a[0].opens + b[0].opens, 0);
  assert.equal(a[0].artifact_id, b[0].artifact_id);
});

test("rename snapshots and immediate navigation stay in the new bucket", () => {
  const state = measured();
  changeFocus(state, null, "Old", start + 1_000, uuid);
  freezeCompleted(state, start + 1_000, uuid);
  const old = { ...state.focus_session };
  changeFocus(state, old, "New", start + 8_000, uuid);
  recordNavigation(state, nav("Latest"), { ...match, mode: "edit" }, start + 8_000, uuid);
  const frozen = freezeCompleted(state, start + 10_000, uuid);
  assert.equal(frozen.length, 1);
  assert.equal(frozen[0].focus_session.name, "Old");
  assert.equal(frozen[0].focused_seconds, 7);
  assert.equal(frozen[0].modes.length, 0);
  assert.equal(state.totals[0].focus_session.name, "New");
  assert.equal(state.totals[0].focus_session.id, old.id);
  assert.deepEqual(state.totals[0].modes, [{ mode: "edit", url: "https://example.com/1" }]);
});

test("hour boundaries and restored state retain the same session identity", () => {
  const state = measured();
  changeFocus(state, null, "Release", start, uuid);
  freezeCompleted(state, start, uuid);
  const restored = structuredClone(state);
  const hour = new Date(start); hour.setMinutes(60, 0, 0);
  const frozen = freezeCompleted(restored, hour.getTime() + 5_000, uuid);
  assert.equal(frozen.length, 1);
  assert.equal(frozen[0].focused_seconds, 50 * 60);
  assert.equal(restored.totals[0].focused_ms, 5_000);
  assert.deepEqual(frozen[0].focus_session, restored.focus_session);
});

test("empty sessions add no event and subsecond bucket remainders never invent measured seconds", () => {
  const state = measured();
  changeFocus(state, null, "Empty", start, uuid);
  freezeCompleted(state, start, uuid); // The pre-session cold open remains ungrouped.
  changeFocus(state, state.focus_session, null, start, uuid);
  assert.deepEqual(freezeCompleted(state, start, uuid), []);
  changeFocus(state, null, "First", start, uuid);
  changeFocus(state, state.focus_session, null, start + 1_500, uuid);
  const first = freezeCompleted(state, start + 1_500, uuid);
  changeFocus(state, null, "Second", start + 1_500, uuid);
  changeFocus(state, state.focus_session, null, start + 3_000, uuid);
  const second = freezeCompleted(state, start + 3_000, uuid);
  assert.equal(first[0].focused_seconds + second[0].focused_seconds, 2);
  assert.equal(first[0].opens + second[0].opens, 0);
});
