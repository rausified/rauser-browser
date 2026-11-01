import assert from "node:assert/strict";
import test, { mock } from "node:test";

// Run after build:extension. Drive polls with mocked timers and defer host
// replies explicitly so overlapping refreshes need no wall-clock waits.
const { TaskTracker, TRACK_MS } = await import("../dist/task-tracker.js");

function task(overrides = {}) {
  return {
    run_id: "00000000-0000-4000-8000-000000000001", kind: "task", state: "queued",
    created_at: "2026-11-01T00:00:00Z", stream_bytes: 42,
    ...overrides,
  };
}

function setup(t, reply = () => ({ runs: [task()] })) {
  const requests = [];
  const changed = [];
  const finished = [];
  const state = { visible: true };
  const tracker = new TaskTracker({
    host: { async call(request, expected) {
      requests.push(request);
      assert.equal(expected, request.type === "list_runs" ? "runs_listed" : "run_fetched");
      return reply(request);
    } },
    now: () => Date.parse("2026-11-01T00:00:00Z"),
    visible: () => state.visible,
    changed: (value) => changed.push(value),
    finished: (run) => finished.push(run),
  });
  t.after(() => tracker.dispose());
  return { tracker, requests, changed, finished, state };
}

async function tick(ms = TRACK_MS) {
  mock.timers.tick(ms);
  // Drain the host/list/refresh promise chain without advancing timers.
  await new Promise((resolve) => setImmediate(resolve));
}

test.beforeEach(() => mock.timers.enable({ apis: ["setTimeout"] }));
test.afterEach(() => mock.timers.reset());

test("overlapping refresh requests merge into exactly one follow-up list", async (t) => {
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  let lists = 0;
  const { tracker, requests, changed } = setup(t, () => {
    lists += 1;
    return lists === 1 ? held : { runs: [task()] };
  });
  const first = tracker.refresh();
  assert.equal(tracker.refresh(), first);
  assert.equal(tracker.refresh(), first);
  assert.equal(requests.length, 1);
  release({ runs: [task()] });
  await first;
  assert.equal(requests.length, 2);
  assert.deepEqual(requests.map((request) => request.type), ["list_runs", "list_runs"]);
  assert.ok(requests.every((request) => request.kind === "task" && request.before === undefined));
  assert.deepEqual(changed, [
    { running: 1, newestId: task().run_id },
    { running: 1, newestId: task().run_id },
  ]);
});

test("repeated refreshes keep one poll timer and disposal clears it", async (t) => {
  const { tracker, requests } = setup(t);
  for (let i = 0; i < 4; i += 1) await tracker.refresh();
  assert.equal(requests.length, 4);
  await tick(TRACK_MS - 1);
  assert.equal(requests.length, 4);
  await tick(1);
  assert.equal(requests.length, 5);
  await tick();
  assert.equal(requests.length, 6);
  tracker.dispose();
  await tick(TRACK_MS * 3);
  await tracker.refresh();
  assert.equal(requests.length, 6);
});

test("track followed by a terminal list announces completion once and stops polling", async (t) => {
  for (const state of ["succeeded", "failed", "canceled"]) {
    const done = task({ state });
    const { tracker, requests, changed, finished } = setup(t, () => ({ runs: [done] }));
    tracker.track(task());
    await tracker.refresh();
    await tracker.refresh();
    assert.deepEqual(finished, [done]);
    assert.deepEqual(changed.at(-1), { running: 0, newestId: done.run_id });
    await tick(TRACK_MS * 3);
    assert.equal(requests.length, 2);
  }
});

test("previously finished tasks are not announced just because they are listed", async (t) => {
  const done = task({ state: "succeeded" });
  const { tracker, finished } = setup(t, () => ({ runs: [done] }));
  tracker.track(done);
  await tracker.refresh();
  assert.deepEqual(finished, []);
});

test("a tracked task missing from the list is fetched from its stream offset", async (t) => {
  const done = task({ state: "succeeded" });
  const { tracker, requests, finished } = setup(t, (request) => request.type === "list_runs"
    ? { runs: [] }
    : { run: done });
  tracker.track(task());
  await tracker.refresh();
  await tracker.refresh();
  assert.deepEqual(requests.map((request) => request.type), ["list_runs", "get_run", "list_runs"]);
  assert.equal(requests[1].run_id, done.run_id);
  assert.equal(requests[1].since_offset, 42);
  assert.deepEqual(finished, [done]);
});

test("a hidden panel stops polling until an explicit visible refresh", async (t) => {
  const { tracker, requests, state } = setup(t);
  await tracker.refresh();
  state.visible = false;
  await tick();
  assert.equal(requests.length, 1);
  state.visible = true;
  await tick(TRACK_MS * 3);
  assert.equal(requests.length, 1);
  await tracker.refresh();
  await tick();
  assert.equal(requests.length, 3);
});

test("disposal during a held list suppresses callbacks and follow-up refreshes", async (t) => {
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const { tracker, requests, changed, finished } = setup(t, () => held);
  tracker.track(task());
  const pending = tracker.refresh();
  tracker.refresh();
  tracker.dispose();
  release({ runs: [task({ state: "succeeded" })] });
  await pending;
  await tick(TRACK_MS * 3);
  assert.equal(requests.length, 1);
  assert.deepEqual(changed, []);
  assert.deepEqual(finished, []);
});
