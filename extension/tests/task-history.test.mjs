import assert from "node:assert/strict";
import test from "node:test";

// Run after build:extension.
const {
  filterTasks, groupTasks, listAllTasks, mergeRuns, parseRunHash, runningCount, tasksPageUrl,
} = await import("../dist/task-history.js");

// A fake host that answers a script of list_runs replies in order, and
// records every request it was sent.
function scriptedHost(replies) {
  const requests = [];
  let call = 0;
  const host = {
    async call(request, expected) {
      assert.equal(expected, "runs_listed");
      requests.push(request);
      const reply = replies[Math.min(call, replies.length - 1)];
      call += 1;
      return { type: "runs_listed", runs: reply.runs, ...(reply.hasMore ? { more: reply.more } : {}) };
    },
  };
  return { host, requests, callCount: () => call };
}

const BASE = Date.parse("2026-10-28T00:00:00Z");

// n = 1 is the newest; created_at strictly decreases as n grows.
function taskRun(n, overrides = {}) {
  return {
    run_id: `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`,
    state: "succeeded", harness: "codex",
    record_key: { kind: "url", value: "https://read.example/a" }, written_record_key: null,
    url: "https://read.example/a", title: `Task ${n}`, cwd: null,
    created_at: new Date(BASE - n * 1000).toISOString(),
    started_at: null, finished_at: null, error: null, error_kind: null, thread_id: null, session_id: null,
    summary_path: null, stream_bytes: 0, can_retry_save: false, answer: null,
    kind: "task", instruction: `Do task ${n}`, task_report: null,
    ...overrides,
  };
}

test("listAllTasks walks pages with the last view as cursor", async () => {
  const page1 = Array.from({ length: 20 }, (_, index) => taskRun(index + 1));
  const page2 = Array.from({ length: 20 }, (_, index) => taskRun(index + 21));
  const page3 = Array.from({ length: 5 }, (_, index) => taskRun(index + 41));
  const { host, requests } = scriptedHost([
    { runs: page1, hasMore: true, more: true },
    { runs: page2, hasMore: true, more: true },
    { runs: page3, hasMore: true, more: false },
  ]);
  const all = await listAllTasks(host);
  assert.equal(all.length, 45);
  assert.deepEqual(all.map((run) => run.run_id), [...page1, ...page2, ...page3].map((run) => run.run_id));

  assert.equal(requests.length, 3);
  assert.equal(requests[0].kind, "task");
  assert.equal("before" in requests[0], false);
  assert.deepEqual(requests[1].before, { created_at: page1.at(-1).created_at, run_id: page1.at(-1).run_id });
  assert.deepEqual(requests[2].before, { created_at: page2.at(-1).created_at, run_id: page2.at(-1).run_id });
});

test("listAllTasks stops when the cursor does not advance", async () => {
  // A host stuck on the same page: every answer repeats the same one run
  // and keeps claiming there is more.
  const stuck = [taskRun(1)];
  const { host, requests, callCount } = scriptedHost([{ runs: stuck, hasMore: true, more: true }]);
  const all = await listAllTasks(host);
  // The first call (before: none) always advances past a null cursor; the
  // second call's cursor matches what it was asked for, so it stops there.
  assert.equal(callCount(), 2);
  assert.equal(requests.length, 2);
  assert.equal(all.length, 2);
});

test("listAllTasks treats a missing more as the last page", async () => {
  const runs = Array.from({ length: 5 }, (_, index) => taskRun(index + 1));
  const { host, requests } = scriptedHost([{ runs, hasMore: false }]);
  const all = await listAllTasks(host);
  assert.equal(requests.length, 1);
  assert.equal(all.length, 5);
});

test("listAllTasks takes the cursor from the last finished view", async () => {
  // The first page: a newer finished run, an older finished run, then an
  // active run older than both (one old queued task sitting among the
  // page's active runs). The active run's cursor must never be used: a
  // `before` that old would skip every finished run strictly between it
  // and B on a real service.
  const runA = taskRun(1, { state: "succeeded" });
  const runB = taskRun(2, { state: "succeeded" });
  const oldActive = taskRun(50, { state: "queued", started_at: null });
  const page1 = [runA, runB, oldActive];
  const page2 = [taskRun(3, { state: "succeeded" })];
  const { host, requests } = scriptedHost([
    { runs: page1, hasMore: true, more: true },
    { runs: page2, hasMore: true, more: false },
  ]);
  const all = await listAllTasks(host);
  assert.equal(requests.length, 2);
  assert.deepEqual(requests[1].before, { created_at: runB.created_at, run_id: runB.run_id });
  assert.equal(all.length, 4);
});

test("listAllTasks stops when a page has no finished view", async () => {
  // A page full of active runs: there is no finished view to cursor from,
  // so paging stops rather than looping on an undefined cursor.
  const activeOnly = [
    taskRun(1, { state: "queued", started_at: null }),
    taskRun(2, { state: "running" }),
  ];
  const { host, requests } = scriptedHost([{ runs: activeOnly, hasMore: true, more: true }]);
  const all = await listAllTasks(host);
  assert.equal(requests.length, 1);
  assert.equal(all.length, 2);
});

test("mergeRuns keeps one view per run, the fresher state", () => {
  const running = taskRun(1, { state: "running" });
  const succeeded = taskRun(1, { state: "succeeded", finished_at: new Date(BASE).toISOString() });

  const knownRunningFreshDone = mergeRuns([running], [succeeded]);
  assert.equal(knownRunningFreshDone.length, 1);
  assert.equal(knownRunningFreshDone[0].state, "succeeded");

  const knownDoneFreshRunning = mergeRuns([succeeded], [running]);
  assert.equal(knownDoneFreshRunning.length, 1);
  assert.equal(knownDoneFreshRunning[0].state, "succeeded");
});

test("mergeRuns dedupes and sorts newest first by created_at then run_id", () => {
  const a = taskRun(1);
  const b = taskRun(2);
  const c = taskRun(3);
  const merged = mergeRuns([b, c], [a, b]);
  assert.deepEqual(merged.map((run) => run.run_id), [a, b, c].map((run) => run.run_id));
});

test("filterTasks matches instruction and title case-insensitively", () => {
  const byInstruction = taskRun(1, { instruction: "Summarize the Quarterly Report", title: "Unrelated page" });
  const byTitle = taskRun(2, { instruction: "something else", title: "Quarterly Numbers" });
  const neither = taskRun(3, { instruction: "nope", title: "also nope" });
  const runs = [byInstruction, byTitle, neither];
  const opts = { status: "all", dismissed: new Set(), showDismissed: false };
  assert.deepEqual(
    filterTasks(runs, { ...opts, query: "quarterly" }).map((run) => run.run_id),
    [byInstruction.run_id, byTitle.run_id],
  );
  assert.deepEqual(
    filterTasks(runs, { ...opts, query: "QUARTERLY" }).map((run) => run.run_id),
    [byInstruction.run_id, byTitle.run_id],
  );
  assert.deepEqual(filterTasks(runs, { ...opts, query: "" }).map((run) => run.run_id),
    runs.map((run) => run.run_id));
});

test("filterTasks matches the title when a run has no instruction", () => {
  const noInstruction = taskRun(1, { title: "Findable title" });
  delete noInstruction.instruction;
  const opts = { status: "all", dismissed: new Set(), showDismissed: false, query: "findable" };
  assert.deepEqual(filterTasks([noInstruction], opts).map((run) => run.run_id), [noInstruction.run_id]);
});

test("filterTasks running includes queued", () => {
  const queued = taskRun(1, { state: "queued" });
  const running = taskRun(2, { state: "running" });
  const done = taskRun(3, { state: "succeeded" });
  const opts = { query: "", dismissed: new Set(), showDismissed: false };
  assert.deepEqual(
    filterTasks([queued, running, done], { ...opts, status: "running" }).map((run) => run.run_id),
    [queued.run_id, running.run_id],
  );
  assert.deepEqual(
    filterTasks([queued, running, done], { ...opts, status: "done" }).map((run) => run.run_id),
    [done.run_id],
  );
});

test("filterTasks hides dismissed finished runs but never active ones", () => {
  const dismissedFinished = taskRun(1, { state: "succeeded" });
  const dismissedButActive = taskRun(2, { state: "running" });
  const keptFinished = taskRun(3, { state: "failed" });
  const runs = [dismissedFinished, dismissedButActive, keptFinished];
  const dismissed = new Set([dismissedFinished.run_id, dismissedButActive.run_id]);

  const hidden = filterTasks(runs, { query: "", status: "all", dismissed, showDismissed: false });
  assert.deepEqual(hidden.map((run) => run.run_id), [dismissedButActive.run_id, keptFinished.run_id]);

  const shown = filterTasks(runs, { query: "", status: "all", dismissed, showDismissed: true });
  assert.deepEqual(shown.map((run) => run.run_id), runs.map((run) => run.run_id));
});

test("groupTasks puts active runs under Running, then Today and Yesterday", () => {
  const now = new Date("2026-10-28T12:00:00Z");
  const running = taskRun(1, { state: "running", created_at: "2026-10-28T11:00:00.000000000Z" });
  const queued = taskRun(2, { state: "queued", created_at: "2026-10-28T10:00:00.000000000Z" });
  const today = taskRun(3, { state: "succeeded", created_at: "2026-10-28T08:00:00.000000000Z" });
  const yesterday = taskRun(4, { state: "failed", created_at: "2026-10-27T08:00:00.000000000Z" });

  const groups = groupTasks([running, queued, today, yesterday], now);
  assert.deepEqual(groups.map((group) => group.label), ["Running", "Today", "Yesterday"]);
  assert.deepEqual(groups[0].runs.map((run) => run.run_id), [running.run_id, queued.run_id]);
  assert.deepEqual(groups[1].runs.map((run) => run.run_id), [today.run_id]);
  assert.deepEqual(groups[2].runs.map((run) => run.run_id), [yesterday.run_id]);
});

test("groupTasks labels an older day by weekday, month and day, and omits empty groups", () => {
  const now = new Date("2026-10-28T12:00:00Z");
  const older = taskRun(1, { state: "succeeded", created_at: "2026-10-20T12:00:00.000000000Z" });
  const groups = groupTasks([older], now);
  assert.equal(groups.length, 1);
  assert.equal(groups[0].label, new Date(older.created_at).toLocaleDateString(undefined, {
    weekday: "short", month: "short", day: "numeric",
  }));
  assert.deepEqual(groupTasks([], now), []);
});

test("runningCount counts queued and running runs only", () => {
  const runs = [
    taskRun(1, { state: "queued" }), taskRun(2, { state: "running" }),
    taskRun(3, { state: "succeeded" }), taskRun(4, { state: "failed" }), taskRun(5, { state: "canceled" }),
  ];
  assert.equal(runningCount(runs), 2);
  assert.equal(runningCount([]), 0);
});

test("parseRunHash accepts only #run=<uuid>", () => {
  const uuid = "0000000a-0000-4000-8000-00000000000b";
  assert.equal(parseRunHash(`#run=${uuid}`), uuid);
  assert.equal(parseRunHash("#run=ABC"), null);
  assert.equal(parseRunHash("#run="), null);
  assert.equal(parseRunHash(`#x=${uuid}`), null);
  assert.equal(parseRunHash(""), null);
  assert.equal(parseRunHash(`#run=${uuid.toUpperCase()}`), null);
});

test("tasksPageUrl adds #run= only when a run id is given", () => {
  const getURL = (path) => `chrome-extension://abc/${path}`;
  assert.equal(tasksPageUrl(null, getURL), "chrome-extension://abc/tasks.html");
  const uuid = "00000000-0000-4000-8000-000000000002";
  assert.equal(tasksPageUrl(uuid, getURL), `chrome-extension://abc/tasks.html#run=${uuid}`);
});
