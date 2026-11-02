import assert from "node:assert/strict";
import test from "node:test";

const { TaskRecall, MAX_RECALL_PAGES, MAX_RECALL_TASKS } = await import("../dist/task-recall.js");
const { HostError } = await import("../dist/native.js");

function run(n, extra = {}) {
  return {
    kind: "task", state: "succeeded", run_id: `task-${n}`,
    created_at: new Date(Date.UTC(2026, 10, 2) - n * 1000).toISOString(),
    instruction: `Preview ${n}…`, stream_bytes: 123, ...extra,
  };
}
function world(pages, full = (request) => ({ ...run(Number(request.run_id.split("-")[1])), instruction: `Full ${request.run_id}` })) {
  const requests = [];
  let index = 0;
  const host = { async call(request) {
    requests.push(request);
    if (request.type === "list_runs") return pages[Math.min(index++, pages.length - 1)];
    return { run: await full(request) };
  } };
  return { recall: new TaskRecall(host), requests };
}
function held() {
  let release;
  return { promise: new Promise((resolve) => { release = resolve; }), release: (value) => release(value) };
}

test("recall reads the full stored instruction lazily and preserves draft and per-entry edits", async () => {
  const full = "  Long original\n" + "x".repeat(500) + "\tend  ";
  const { recall, requests } = world([{ runs: [run(2), run(1)], more: false }],
    (request) => ({ ...run(1), instruction: request.run_id === "task-1" ? full : "Older original" }));
  assert.equal(requests.length, 0);
  assert.equal(await recall.move(-1, "My draft"), null);
  assert.equal(await recall.move(1, "My draft"), full);
  assert.equal(requests[1].since_offset, 123, "do not download the stream again");
  recall.edited();
  assert.equal(await recall.move(1, "Edited newest"), "Older original");
  recall.edited();
  assert.equal(await recall.move(-1, "Edited older"), "Edited newest");
  assert.equal(await recall.move(-1, "Edited newest"), "My draft");
  assert.equal(await recall.move(1, "My changed draft"), "Edited newest");
  assert.equal(await recall.move(1, "Edited newest"), "Edited older");
  assert.equal(await recall.move(1, "Edited older"), null);
  assert.equal(requests.filter((r) => r.type === "get_run").length, 2, "edited slots need no re-fetch");
});

test("recall pages only on demand, deduplicates active/finished IDs and cursors unfiltered terminal rows", async () => {
  const active = run(1, { state: "running" });
  const summary = run(2, { kind: "summary" });
  const { recall, requests } = world([
    { runs: [active, summary], more: true },
    { runs: [run(1), run(3, { kind: "summary" })], more: true },
    { runs: [run(4)], more: false },
  ]);
  assert.equal(await recall.move(1, "draft"), "Full task-1");
  assert.equal(requests.filter((r) => r.type === "list_runs").length, 1);
  assert.equal(await recall.move(1, "Full task-1"), "Full task-4");
  const lists = requests.filter((r) => r.type === "list_runs");
  assert.deepEqual(lists[1].before, { created_at: summary.created_at, run_id: summary.run_id });
  assert.equal(lists[2].before.run_id, "task-3");
  assert.equal(requests.filter((r) => r.type === "get_run").length, 2);
});

test("an old active task stays behind newer finished tasks across page boundaries", async () => {
  const active = run(4, { state: "queued" });
  const { recall } = world([
    { runs: [active, run(1)], more: true },
    { runs: [active, run(2), run(3)], more: true },
    { runs: [active, run(5)], more: false },
  ]);
  for (let n = 1; n <= 5; n++) assert.equal(await recall.move(1, "draft"), `Full task-${n}`);
  assert.equal(await recall.move(1, "draft"), null);
});

test("pruned or non-task records are skipped without using their list preview", async () => {
  const { recall } = world([{ runs: [run(1), run(2), run(3)], more: false }], (request) => {
    if (request.run_id === "task-1") throw new HostError("invalid_request", "Run was pruned");
    return { ...run(3), kind: request.run_id === "task-2" ? "summary" : "task", instruction: "Only real task" };
  });
  assert.equal(await recall.move(1, "draft"), "Only real task");
  assert.equal(await recall.move(-1, "Only real task"), "draft");
});

test("list and full-record failures leave navigation retryable", async () => {
  for (const failure of ["list_runs", "get_run"]) {
    let fail = true;
    const recall = new TaskRecall({ async call(request) {
      if (fail && request.type === failure) { fail = false; throw new Error("Offline"); }
      return request.type === "list_runs" ? { runs: [run(1)], more: false } : { run: run(1, { instruction: "Complete" }) };
    } });
    await assert.rejects(recall.move(1, "draft"), /Offline/);
    assert.equal(await recall.move(1, "draft"), "Complete");
    assert.equal(await recall.move(-1, "Complete"), "draft");
  }
});

test("typing or resetting during either request invalidates replacement; repeats do not queue", async () => {
  for (const delayed of ["list_runs", "get_run"]) {
    for (const cancel of ["edited", "reset"]) {
      const gate = held();
      let entered = false;
      const recall = new TaskRecall({ async call(request) {
        if (request.type === delayed && !entered) { entered = true; await gate.promise; }
        return request.type === "list_runs" ? { runs: [run(1)], more: false } : { run: run(1, { instruction: "Complete" }) };
      } });
      const pending = recall.move(1, "draft");
      while (!entered) await Promise.resolve();
      assert.equal(await recall.move(1, "repeat"), null);
      recall[cancel]();
      gate.release();
      assert.equal(await pending, null);
      assert.equal(await recall.move(1, "new draft"), "Complete");
      assert.equal(await recall.move(-1, "Complete"), "new draft");
    }
  }
});

test("a failure after cancellation does not surface a stale error", async () => {
  const gate = held();
  const recall = new TaskRecall({ async call() { await gate.promise; throw new Error("old error"); } });
  const pending = recall.move(1, "draft");
  recall.reset();
  gate.release();
  assert.equal(await pending, null);
});

test("an empty, stuck or active-only page cannot loop", async () => {
  for (const runs of [[], [run(1)], [run(1, { state: "running" })]]) {
    const { recall, requests } = world([{ runs, more: true }]);
    let text = "draft";
    for (let n = 0; n < 5; n++) text = await recall.move(1, text) ?? text;
    assert.ok(requests.filter((r) => r.type === "list_runs").length <= 2);
  }
});

test("summary-only histories and huge task histories respect browsing bounds", async () => {
  let listed = 0;
  const summaries = new TaskRecall({ async call() {
    return { runs: [run(++listed, { kind: "summary" })], more: true };
  } });
  assert.equal(await summaries.move(1, "draft"), null);
  assert.equal(listed, MAX_RECALL_PAGES);
  const rows = Array.from({ length: MAX_RECALL_TASKS + 20 }, (_, i) => run(i + 1));
  const { recall, requests } = world([{ runs: rows, more: true }]);
  for (let i = 0; i < MAX_RECALL_TASKS; i++) assert.equal(await recall.move(1, "draft"), `Full task-${i + 1}`);
  assert.equal(await recall.move(1, "draft"), null);
  assert.equal(requests.filter((r) => r.type === "list_runs").length, 1);
});
