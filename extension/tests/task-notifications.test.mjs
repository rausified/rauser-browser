import assert from "node:assert/strict";
import test from "node:test";
import {
  TASK_NOTICE_KEY, TaskNotifications, listenForTaskNoticeClicks, parseTaskNoticeId, taskBadge, taskNoticeId, unreadTaskCount,
} from "../dist/task-notifications.js";
import { taskNotificationSettings } from "../dist/task-notification-settings.js";
import { listAllTasks } from "../dist/task-history.js";

const NOW = Date.parse("2026-11-05T10:00:00Z");
const id = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
const run = (n, state = "running", at = NOW + n) => ({
  run_id: id(n), start_request_id: id(1000), kind: "task", state, created_at: new Date(at).toISOString(),
  instruction: "PRIVATE instruction", task_report: "PRIVATE report",
});

test("overlapping starts in different windows use exact request IDs and exclude other profiles", async () => {
  const f = fixture();
  await f.monitor.arm(2, id(1000));
  await f.monitor.arm(3, id(1001));
  f.runs = [run(1, "succeeded"), { ...run(2, "succeeded"), start_request_id: id(1001) },
    { ...run(3, "succeeded"), start_request_id: id(9999) }, { ...run(4, "succeeded"), start_request_id: undefined }];
  await f.monitor.poll();
  assert.deepEqual(f.notices.map((notice) => notice.id), [taskNoticeId(id(1), 2), taskNoticeId(id(2), 3)]);
});

function fixture() {
  const values = new Map();
  const f = { now: NOW, enabled: true, panel: false, runs: [], complete: true,
    notices: [], cleared: [], schedules: [], calls: [], failCreate: false, failWrite: false, failList: false };
  const deps = {
    storage: {
      async get(key) { return values.has(key) ? { [key]: structuredClone(values.get(key)) } : {}; },
      async set(items) {
        if (f.failWrite) throw new Error("storage full");
        for (const [key, value] of Object.entries(items)) values.set(key, structuredClone(value));
      },
    },
    now: () => f.now,
    enabled: async () => f.enabled,
    list: async () => {
      f.calls.push("list");
      if (f.failList) throw new Error("host unavailable");
      return { runs: structuredClone(f.runs), complete: f.complete };
    },
    get: async (runId) => { f.calls.push(runId); return f.offPage ?? null; },
    panelOpen: async () => f.panel,
    schedule: async (needed) => f.schedules.push(needed),
    badge: async () => { f.unread = await unreadTaskCount(deps.storage); },
    create: async (noticeId, text) => {
      f.notices.push({ id: noticeId, text });
      if (f.failCreate) throw new Error("Chrome refused notification");
    },
    clear: async (noticeId) => f.cleared.push(noticeId),
  };
  return Object.assign(f, { values, deps, monitor: new TaskNotifications(deps),
    state: () => values.get(TASK_NOTICE_KEY) });
}

test("opt-out and a cold worker never start discovery or create monitoring state", async () => {
  const f = fixture();
  await f.monitor.resume();
  assert.equal(f.values.size, 0);
  f.enabled = false;
  await f.monitor.arm(3, id(1000));
  assert.equal(f.values.size, 0);
  assert.deepEqual(f.calls, []);
});

test("pre-start intent discovers a fast finish after panel closes with its start answer lost", async () => {
  const f = fixture();
  await f.monitor.arm(3, id(1000));
  assert.equal(f.state().intents[0].since, NOW);
  assert.equal(f.schedules.at(-1), true, "alarm armed before native start");
  f.runs = [run(1, "succeeded"), { ...run(2, "succeeded", NOW - 1000), start_request_id: id(9999) }, { ...run(3, "succeeded"), kind: "summary" }];
  f.monitor = new TaskNotifications(f.deps); // worker suspended during native start
  await f.monitor.resume();
  await f.monitor.poll();
  assert.deepEqual(f.notices, [{ id: taskNoticeId(id(1), 3), text: "Task finished" }]);
  assert.equal(f.unread, 1);
  assert.doesNotMatch(JSON.stringify(f.state()), /PRIVATE|instruction|report/);
  await new TaskNotifications(f.deps).poll();
  assert.equal(f.notices.length, 1, "a cold worker cannot repeat the finish");
});

test("running tasks stay watched beyond discovery expiry, and missing listed tasks get one direct read", async () => {
  const f = fixture();
  await f.monitor.arm(2, id(1000));
  f.runs = [run(1)];
  await f.monitor.poll();
  f.now += 180_000;
  f.runs = [];
  f.offPage = run(1, "failed");
  await new TaskNotifications(f.deps).poll();
  assert.deepEqual(f.calls, ["list", "list", id(1)]);
  assert.equal(f.notices[0].text, "Task failed");
  assert.equal(f.schedules.at(-1), false, "unread badge alone never polls the service");
});

test("pruned runs do not manufacture a completion", async () => {
  const f = fixture();
  await f.monitor.arm(2, id(1000));
  f.runs = [run(1)];
  await f.monitor.poll();
  f.runs = [];
  f.now += 180_000;
  await f.monitor.poll();
  assert.deepEqual(f.notices, []);
  assert.equal(f.schedules.at(-1), false);
});

test("an open panel defers a finish until it acknowledges it or closes, without further host polling", async () => {
  const f = fixture();
  await f.monitor.arm(2, id(1000));
  f.panel = true;
  f.runs = [run(1, "canceled"), run(2, "succeeded")];
  f.now += 180_000;
  await f.monitor.poll();
  assert.equal(f.notices.length, 0);
  await f.monitor.observed(id(1));
  f.panel = false;
  await f.monitor.poll();
  assert.deepEqual(f.calls, ["list"]);
  assert.deepEqual(f.notices.map((n) => n.id), [taskNoticeId(id(2), 2)]);
});

test("a panel-observed finish before discovery suppresses its later historical copy", async () => {
  const f = fixture();
  await f.monitor.arm(2, id(1000));
  await f.monitor.observed(id(1));
  f.runs = [run(1, "succeeded")];
  await f.monitor.poll();
  assert.equal(f.notices.length, 0);
});

test("an observed-finish acknowledgement suppresses a poll already waiting on the host", async () => {
  const f = fixture();
  let release;
  f.deps.list = () => new Promise((resolve) => { release = resolve; });
  f.monitor = new TaskNotifications(f.deps);
  await f.monitor.arm(2, id(1000));
  const polling = f.monitor.poll();
  while (!release) await new Promise((resolve) => setImmediate(resolve));
  const acknowledged = f.monitor.observed(id(1));
  release({ runs: [run(1, "succeeded")], complete: true });
  await Promise.all([polling, acknowledged]);
  assert.deepEqual(f.notices, []);
  assert.equal(f.unread, 0);
});

test("discovery walks finished history beyond the first page", async () => {
  const f = fixture();
  const calls = [];
  f.deps.list = () => listAllTasks({ call: async (request) => {
    calls.push(request);
    return request.before ? { runs: [run(1, "succeeded")], more: false }
      : { runs: Array.from({ length: 20 }, (_, n) => run(n + 2, "succeeded")), more: true };
  } });
  f.monitor = new TaskNotifications(f.deps);
  await f.monitor.arm(2, id(1000));
  await f.monitor.poll();
  assert.equal(calls.length, 2);
  assert.equal(calls[1].before.run_id, id(21));
  assert.equal(f.notices.length, 21);
  assert.ok(f.notices.some((notice) => notice.id.endsWith(id(1))));
});

test("unavailable or incomplete history retains discovery intents for a later successful retry", async () => {
  const f = fixture();
  await f.monitor.arm(2, id(1000));
  f.runs = [run(1, "succeeded")];
  f.now += 180_000;
  f.failList = true;
  await assert.rejects(f.monitor.poll(), /unavailable/);
  f.failList = false;
  f.complete = false;
  await assert.rejects(f.monitor.poll(), /incomplete/);
  assert.equal(f.state().intents.length, 1);
  f.complete = true;
  await f.monitor.poll();
  assert.equal(f.notices.length, 1);
});

test("storage failure cannot show a toast before its handling is durable", async () => {
  const f = fixture();
  await f.monitor.arm(2, id(1000));
  f.runs = [run(1, "succeeded")];
  f.failWrite = true;
  await assert.rejects(f.monitor.poll(), /storage full/);
  assert.deepEqual(f.notices, []);
  f.failWrite = false;
  await f.monitor.poll();
  assert.equal(f.notices.length, 1);
});

test("a refused notification keeps the badge without repeatedly issuing it on restart", async () => {
  const f = fixture();
  await f.monitor.arm(2, id(1000));
  f.runs = [run(1, "succeeded"), run(2, "failed")];
  f.failCreate = true;
  await assert.rejects(f.monitor.poll(), /Chrome refused/);
  assert.equal(f.notices.length, 2, "one failure does not prevent other completions being attempted");
  assert.equal(f.unread, 2);
  await new TaskNotifications(f.deps).poll();
  assert.equal(f.notices.length, 2);
});

test("opening the panel clears unread UI; permission revocation also stops monitoring", async () => {
  const f = fixture();
  await f.monitor.arm(2, id(1000));
  f.runs = [run(1, "succeeded")];
  await f.monitor.poll();
  await f.monitor.opened();
  assert.equal(f.unread, 0);
  assert.deepEqual(f.cleared, [taskNoticeId(id(1), 2)]);
  f.enabled = false;
  await f.monitor.poll();
  assert.equal(f.schedules.at(-1), false);
  assert.equal(f.state().intents.length, 0);
});

test("corrupt monitoring state refuses arming and explicit reset repairs it", async () => {
  const f = fixture();
  f.values.set(TASK_NOTICE_KEY, { version: 9 });
  await assert.rejects(f.monitor.arm(2, id(1000)), /reset/);
  await f.monitor.reset();
  await f.monitor.arm(2, id(1000));
  assert.equal(f.state().intents.length, 1);
  await assert.rejects(f.monitor.arm(-1, id(1000)), /window/);
  await assert.rejects(f.monitor.observed("bad"), /run/);
});

test("capture pressure takes precedence over unread completions", () => {
  assert.equal(taskBadge(true, 4).text, "!");
  assert.equal(taskBadge(false, 4).text, "4");
  assert.equal(taskBadge(false, 100).text, "99+");
  assert.equal(taskBadge(false, 0).text, "");
});

test("cold-click routing decodes only our numeric window and run UUID", () => {
  assert.deepEqual(parseTaskNoticeId(taskNoticeId(id(1), 42)), { run_id: id(1), window_id: 42 });
  for (const value of ["foreign", taskNoticeId("bad", 1), taskNoticeId(id(1), -1), `${taskNoticeId(id(1), 1)}:extra`]) {
    assert.equal(parseTaskNoticeId(value), null);
  }
});

test("click listeners register after the optional API becomes available, exactly once", () => {
  let added;
  const listeners = [];
  const api = { permissions: { onAdded: { addListener(fn) { added = fn; } } } };
  const clicked = () => {};
  listenForTaskNoticeClicks(api, clicked);
  api.notifications = { onClicked: { addListener(fn) { listeners.push(fn); } } };
  added({ permissions: ["storage"] });
  assert.equal(listeners.length, 0);
  added({ permissions: ["notifications"] });
  added({ permissions: ["notifications"] });
  assert.deepEqual(listeners, [clicked]);
  const cold = [];
  listenForTaskNoticeClicks({ permissions: {}, notifications: { onClicked: { addListener(fn) { cold.push(fn); } } } }, clicked);
  assert.deepEqual(cold, [clicked]);
  let retried;
  const unavailable = { permissions: { onAdded: { addListener(fn) { retried = fn; } } },
    notifications: { onClicked: { addListener() { throw new Error("permission required"); } } } };
  assert.doesNotThrow(() => listenForTaskNoticeClicks(unavailable, clicked));
  unavailable.notifications.onClicked.addListener = (fn) => cold.push(fn);
  retried({ permissions: ["notifications"] });
  assert.equal(cold.length, 2);
});

function settingsFixture() {
  const calls = [];
  const f = { enabled: false, grant: true, fail: false, calls };
  const button = { disabled: false, addEventListener(_type, click) { this.click = click; } };
  const status = {};
  const api = {
    permissions: {
      contains: async () => f.enabled,
      request: async (permission) => { calls.push(["request", permission]); if (f.fail) throw new Error(); f.enabled = f.grant; return f.grant; },
      remove: async (permission) => { calls.push(["remove", permission]); f.enabled = false; return true; },
    },
    runtime: { sendMessage: async (message) => { calls.push(message); return { ok: true }; } },
  };
  taskNotificationSettings(api, button, status);
  return Object.assign(f, { button, status });
}
const tick = () => new Promise((resolve) => setImmediate(resolve));

test("Settings requests optional permission synchronously on click and can turn it off", async () => {
  const f = settingsFixture();
  await tick();
  assert.match(f.status.textContent, /^Off/);
  f.button.click();
  assert.deepEqual(f.calls[0], ["request", { permissions: ["notifications"] }]);
  assert.equal(f.button.disabled, true);
  await tick();
  assert.match(f.status.textContent, /^On/);
  f.button.click();
  assert.equal(f.calls.at(-1)[0], "remove");
  await tick();
  assert.match(f.status.textContent, /^Off/);
  assert.equal(f.calls.filter((call) => call.kind === "task_notice_reset").length, 2);
});

test("refusing permission leaves notifications off without resetting history or starting a service", async () => {
  const f = settingsFixture();
  await tick();
  f.grant = false;
  f.button.click();
  await tick();
  assert.match(f.status.textContent, /not granted/);
  assert.equal(f.calls.length, 1);
});

test("permission API failure leaves the Settings button usable", async () => {
  const f = settingsFixture();
  await tick();
  f.fail = true;
  f.button.click();
  await tick();
  assert.equal(f.button.disabled, false);
  assert.match(f.status.textContent, /Could not change/);
});
