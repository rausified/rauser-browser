import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

const {
  activateTab, createdNavigationTarget, emptyActivityState, focusWindow, freezeCompleted,
  recordNavigation, removeTab, replaceTab, setIdle,
} = await import("../dist/activity.js");

let ids = 0;
const uuid = () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`;
const match = (id, mode = null, title) => ({
  match: true, adapter: 0, rule: 0, type: "test.item", captures: {},
  artifact_id: `test:example/${id}`, mode,
  ...(title === undefined ? {} : { title, title_entry: 0, alias_id: null, refs: [] }),
});
const nav = (tab_id, url, extra = {}) => ({
  tab_id, window_id: 1, url, title: null, transition: "link", qualifiers: [], reload: false,
  ...extra,
});

test("the built extension requests idle measurement without broad tab access", async () => {
  const manifest = JSON.parse(await readFile(new URL("../dist/manifest.json", import.meta.url), "utf8"));
  assert.ok(manifest.permissions.includes("idle"));
  assert.ok(!manifest.permissions.includes("tabs"));
});

test("hourly activity freezes with a stable ID and survives state restoration", () => {
  // Ten seconds before a *local* hour boundary, so half-hour offsets work too.
  const boundary = new Date("2026-10-16T10:30:00Z");
  boundary.setMinutes(60, 0, 0);
  const start = boundary.getTime() - 10_000;
  const state = emptyActivityState(start);
  recordNavigation(state, nav(1, "https://example.com/items/a"), match("a", "view"), start, uuid);
  activateTab(state, 1, 1, start, uuid);
  focusWindow(state, 1, start, uuid);
  const restored = structuredClone(state);
  const frozen = freezeCompleted(restored, start + 20_000, uuid);
  assert.equal(frozen.length, 1);
  assert.equal(frozen[0].opens, 1);
  assert.equal(frozen[0].focused_seconds, 10);
  assert.deepEqual(frozen[0].modes, [{ mode: "view", url: "https://example.com/items/a" }]);
  assert.equal(frozen[0].event_id, state.totals[0].event_id);
  assert.equal(restored.totals.length, 1, "the new hour remains live");
  assert.equal(restored.totals[0].focused_ms, 10_000);
});

test("the offset-qualified hour key separates a daylight-saving offset change", () => {
  const previous = process.env.TZ;
  process.env.TZ = "America/New_York";
  try {
    const start = new Date("2026-11-01T05:59:50Z").getTime();
    const state = emptyActivityState(start);
    recordNavigation(state, nav(1, "https://example.com/items/a"), match("a"), start, uuid);
    activateTab(state, 1, 1, start, uuid);
    focusWindow(state, 1, start, uuid);
    freezeCompleted(state, start + 20_000, uuid);
    assert.equal(state.totals[0].hour_start, "2026-11-01T01:00:00-05:00");
  } finally {
    process.env.TZ = previous;
  }
});

test("focus, background, idle, lock, and tab removal classify all measured time", () => {
  const start = Date.now();
  const state = emptyActivityState(start);
  recordNavigation(state, nav(1, "https://example.com/items/a"), match("a"), start, uuid);
  activateTab(state, 1, 1, start, uuid);
  focusWindow(state, 1, start, uuid);
  focusWindow(state, null, start + 4_000, uuid);
  setIdle(state, true, start + 10_000, uuid);
  setIdle(state, false, start + 20_000, uuid);
  focusWindow(state, 1, start + 20_000, uuid);
  setIdle(state, true, start + 23_000, uuid); // locked is the same stopped state.
  setIdle(state, false, start + 30_000, uuid);
  removeTab(state, 1, start + 32_000, uuid);
  const total = state.totals[0];
  assert.equal(total.focused_ms, 9_000);
  assert.equal(total.background_ms, 6_000);
});

test("cold opens, routes, reloads, duplicate tabs, back, trails, and replacement follow tab identity", () => {
  const start = Date.now();
  const state = emptyActivityState(start);
  recordNavigation(state, nav(1, "https://example.com/items/a"), match("a", "view"), start, uuid);
  recordNavigation(state, nav(1, "https://example.com/items/a?mode=files"),
    match("a", "review"), start + 1, uuid);
  assert.equal(state.totals.find((total) => total.artifact_id.endsWith("/a")).opens, 1);
  assert.deepEqual(state.totals.find((total) => total.artifact_id.endsWith("/a")).modes,
    [{ mode: "view", url: "https://example.com/items/a" },
      { mode: "review", url: "https://example.com/items/a?mode=files" }]);

  recordNavigation(state, nav(1, "https://example.com/items/b"), match("b"), start + 2, uuid);
  const b = state.totals.find((total) => total.artifact_id.endsWith("/b"));
  assert.equal(b.opens, 1);
  assert.equal(b.trails[0].from_artifact_id, "test:example/a");

  recordNavigation(state, nav(2, "https://example.com/items/b"), match("b"), start + 3, uuid);
  assert.equal(b.opens, 2, "the same artifact in another tab is another cold open");
  activateTab(state, 1, 1, start + 3, uuid);
  activateTab(state, 2, 1, start + 3, uuid);
  activateTab(state, 1, 1, start + 3, uuid);
  assert.equal(b.opens, 2, "tab switches never add opens");
  recordNavigation(state, nav(3, "https://example.com/items/c", { reload: true, transition: "reload" }),
    match("c"), start + 4, uuid);
  assert.equal(state.totals.find((total) => total.artifact_id.endsWith("/c")), undefined,
    "an inactive restored or reloaded tab emits no empty activity event");

  createdNavigationTarget(state, 1, 4);
  recordNavigation(state, nav(4, "https://example.com/items/d"), match("d"), start + 5, uuid);
  const d = state.totals.find((total) => total.artifact_id.endsWith("/d"));
  assert.equal(d.trails[0].from_artifact_id, "test:example/b");
  assert.equal(d.trails[0].from_url, "https://example.com/items/b");

  recordNavigation(state, nav(1, "https://example.com/items/a", {
    qualifiers: ["forward_back"], transition: "link",
  }), match("a"), start + 6, uuid);
  const back = state.totals.find((total) => total.artifact_id.endsWith("/a"));
  assert.equal(back.opens, 2);
  assert.ok(back.trails.some((trail) =>
    trail.transition === "forward_back" && trail.from_artifact_id === null));

  replaceTab(state, 20, 2);
  assert.equal(state.tabs[20].artifact_id, "test:example/b");
  assert.equal(state.tabs[2], undefined);
});

test("the latest title replaces refs that the host can no longer verify", () => {
  const start = Date.now();
  const state = emptyActivityState(start);
  const withTitle = (title, value) => ({
    ...match("a", null, title), refs: [{ type: "jira.issue", value }],
  });
  recordNavigation(state, nav(1, "https://example.com/items/a", { title: "First" }),
    withTitle("First", "OLD-1"), start, uuid);
  recordNavigation(state, nav(1, "https://example.com/items/a", { title: "Second" }),
    withTitle("Second", "NEW-2"), start + 1, uuid);

  assert.equal(state.totals[0].title, "Second");
  assert.deepEqual(state.totals[0].refs,
    [{ artifact_type: "jira.issue", value: "NEW-2" }]);
});
