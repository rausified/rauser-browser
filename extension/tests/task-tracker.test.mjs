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

test("a queued refresh still runs after the in-flight list fails", async (t) => {
  let fail;
  const held = new Promise((_, reject) => { fail = reject; });
  let lists = 0;
  const { tracker, requests, changed } = setup(t, () => ++lists === 1 ? held : { runs: [task()] });
  const first = tracker.refresh();
  assert.equal(tracker.refresh(), first);
  assert.equal(tracker.refresh(), first);
  fail(new Error("first list failed"));
  await first;
  assert.equal(requests.length, 2);
  assert.deepEqual(changed, [{ running: 1, newestId: task().run_id }]);
  await tick();
  assert.equal(requests.length, 3, "the follow-up leaves only one polling timer");
});

test("a failed queued refresh rejects with the latest error", async (t) => {
  let fail;
  const held = new Promise((_, reject) => { fail = reject; });
  let lists = 0;
  const latest = new Error("follow-up failed");
  const { tracker, requests } = setup(t, () => {
    if (++lists === 1) return held;
    throw latest;
  });
  const first = tracker.refresh();
  tracker.refresh();
  const rejected = assert.rejects(first, (error) => error === latest);
  fail(new Error("first failed"));
  await rejected;
  assert.equal(requests.length, 2);
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

test("stale track and listed views cannot announce the same run twice", async (t) => {
  let runs = [task({ state: "succeeded" })];
  const { tracker, finished, requests } = setup(t, () => ({ runs }));
  tracker.track(task());
  await tracker.refresh();
  tracker.track(task());
  await tracker.refresh();
  runs = [task()];
  await tracker.refresh();
  runs = [task({ state: "succeeded" })];
  await tracker.refresh();
  await tick(TRACK_MS * 10);
  assert.equal(requests.length, 4, "stale views do not restart watching a finished task");
  assert.deepEqual(finished, runs);

  const next = task({ run_id: "00000000-0000-4000-8000-000000000002" });
  tracker.track(next);
  runs = [{ ...next, state: "succeeded" }];
  await tracker.refresh();
  assert.deepEqual(finished.map((run) => run.run_id), [task().run_id, next.run_id]);
});

for (const failing of ["list_runs", "get_run"]) {
  test(`consecutive ${failing} failures back off, stop, and resume on explicit refresh`, async (t) => {
    let broken = true;
    const { tracker, requests, finished } = setup(t, (request) => {
      if (broken && request.type === failing) throw new Error("host unavailable");
      return request.type === "list_runs" ? { runs: [] } : { run: task({ state: "succeeded" }) };
    });
    tracker.track(task());
    const refresh = tracker.refresh();
    if (failing === "list_runs") await assert.rejects(refresh, /host unavailable/);
    else await refresh;
    const count = () => requests.filter((request) => request.type === failing).length;
    for (let failure = 1; failure < 5; failure += 1) {
      await tick(TRACK_MS * 2 ** (failure - 1) - 1);
      assert.equal(count(), failure, "no early retry");
      await tick(1);
      assert.equal(count(), failure + 1);
    }
    await tick(TRACK_MS * 100);
    assert.equal(count(), 5, "automatic polling stops after five failures");
    broken = false;
    await tracker.refresh();
    assert.equal(count(), 6);
    assert.equal(finished.length, 1, "a resumed refresh can still announce the watched task");
  });
}

test("a successful pass resets the consecutive failure delay", async (t) => {
  let lists = 0;
  const { tracker, requests } = setup(t, () => {
    lists += 1;
    if (lists !== 3) throw new Error("offline");
    return { runs: [task()] };
  });
  tracker.track(task());
  await assert.rejects(tracker.refresh(), /offline/);
  await tick(TRACK_MS);
  assert.equal(requests.length, 2);
  await tick(TRACK_MS * 2);
  assert.equal(requests.length, 3);
  await tick(TRACK_MS);
  assert.equal(requests.length, 4);
  await tick(TRACK_MS);
  assert.equal(requests.length, 5, "first failure after success uses the base delay");
});

test("an explicit refresh grants a fresh retry budget even if the host is still failing", async (t) => {
  const { tracker, requests } = setup(t, () => { throw new Error("offline"); });
  tracker.track(task());
  await assert.rejects(tracker.refresh(), /offline/);
  for (const delay of [1, 2, 4, 8]) await tick(TRACK_MS * delay);
  await tick(TRACK_MS * 100);
  assert.equal(requests.length, 5);
  await assert.rejects(tracker.refresh(), /offline/);
  await tick(TRACK_MS - 1);
  assert.equal(requests.length, 6);
  await tick(1);
  assert.equal(requests.length, 7, "explicit refresh restarts at the base retry delay");
});

test("disposal after a failed held list suppresses the queued refresh and retry", async (t) => {
  let fail;
  const held = new Promise((_, reject) => { fail = reject; });
  const { tracker, requests } = setup(t, () => held);
  const pending = tracker.refresh();
  tracker.refresh();
  tracker.dispose();
  const rejected = assert.rejects(pending, /offline/);
  fail(new Error("offline"));
  await rejected;
  await tick(TRACK_MS * 100);
  assert.equal(requests.length, 1);
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
