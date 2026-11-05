// Completion monitoring belongs to the worker, independently of panel and
// capture lifetimes. Persist only IDs, outcomes and times, never agent text.
import type { RunView } from "../protocol/ts/generated.js";
import { BINARY_NAME, storageKey } from "./brand.js";
import { isTerminal } from "./run-card.js";

export const TASK_NOTICE_KEY = storageKey("task_notifications_v1");
export const TASK_NOTICE_ALARM = `${BINARY_NAME}-task-notifications`;
const DISCOVERY_MS = 2 * 60_000;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const MAX_HANDLED = 4096;

interface Watched { run_id: string; window_id: number }
interface Completion extends Watched { state: "succeeded" | "failed" | "canceled" }
interface Intent { request_id: string; since: number; until: number; window_id: number }
interface Ledger {
  version: 1;
  intents: Intent[];
  watched: Watched[];
  deferred: Completion[];
  handled: string[];
  unread: Completion[];
}

function empty(): Ledger {
  return { version: 1, intents: [], watched: [], deferred: [], handled: [], unread: [] };
}

export function taskNoticeId(runId: string, windowId: number): string {
  return `${BINARY_NAME}-task:${windowId}:${runId}`;
}

export function parseTaskNoticeId(id: string): Watched | null {
  const prefix = `${BINARY_NAME}-task:`;
  if (!id.startsWith(prefix)) return null;
  const [window, run_id, extra] = id.slice(prefix.length).split(":");
  const window_id = Number(window);
  return extra === undefined && window !== undefined && /^\d+$/.test(window) &&
    Number.isSafeInteger(window_id) && run_id !== undefined && UUID.test(run_id)
    ? { run_id, window_id } : null;
}

function isWatched(value: unknown): value is Watched {
  if (typeof value !== "object" || value === null) return false;
  const row = value as Watched;
  return typeof row.run_id === "string" && UUID.test(row.run_id) &&
    Number.isSafeInteger(row.window_id) && row.window_id >= 0;
}

function isCompletion(value: unknown): value is Completion {
  return isWatched(value) && "state" in value &&
    (value.state === "succeeded" || value.state === "failed" || value.state === "canceled");
}

function isLedger(value: unknown): value is Ledger {
  if (typeof value !== "object" || value === null) return false;
  const row = value as Ledger;
  return row.version === 1 && Array.isArray(row.intents) && row.intents.length <= 32 &&
    row.intents.every((intent) => typeof intent === "object" && intent !== null &&
      typeof intent.request_id === "string" && UUID.test(intent.request_id) &&
      Number.isFinite(intent.since) && Number.isFinite(intent.until) && intent.until >= intent.since &&
      Number.isSafeInteger(intent.window_id) && intent.window_id >= 0) &&
    Array.isArray(row.watched) && row.watched.length <= MAX_HANDLED && row.watched.every(isWatched) &&
    Array.isArray(row.deferred) && row.deferred.length <= MAX_HANDLED && row.deferred.every(isCompletion) &&
    Array.isArray(row.unread) && row.unread.length <= MAX_HANDLED && row.unread.every(isCompletion) &&
    Array.isArray(row.handled) && row.handled.length <= MAX_HANDLED &&
    row.handled.every((id) => typeof id === "string" && UUID.test(id));
}

export async function unreadTaskCount(storage: Pick<ChromeStorageArea, "get">): Promise<number> {
  const value = (await storage.get(TASK_NOTICE_KEY))[TASK_NOTICE_KEY];
  return isLedger(value) ? value.unread.length : 0;
}

export function taskBadge(pressure: boolean, unread: number): { text: string; color: string } {
  return pressure ? { text: "!", color: "#b45309" }
    : { text: unread > 99 ? "99+" : unread > 0 ? String(unread) : "", color: "#2563eb" };
}

export interface TaskNotificationDeps {
  storage: ChromeStorageArea;
  enabled(): Promise<boolean>;
  now(): number;
  list(): Promise<{ runs: RunView[]; complete: boolean }>;
  get(runId: string): Promise<RunView | null>;
  panelOpen(): Promise<boolean>;
  schedule(needed: boolean): Promise<void>;
  badge(): Promise<void>;
  create(id: string, outcome: string): Promise<void>;
  clear(id: string): Promise<unknown>;
}

export class TaskNotifications {
  private sequence: Promise<unknown> = Promise.resolve();
  // A panel can acknowledge while a slow host poll holds the ledger chain.
  // Let that acknowledgement suppress delivery immediately, then persist it.
  private readonly pendingObserved = new Set<string>();

  constructor(private readonly deps: TaskNotificationDeps) {}

  // This chain is separate from capture. Slow host requests cannot delay a
  // navigation or a policy change. Acknowledgements serialize with polling.
  private serial<T>(work: () => Promise<T>): Promise<T> {
    const next = this.sequence.then(work, work);
    this.sequence = next.catch(() => undefined);
    return next;
  }

  private async read(): Promise<Ledger> {
    const value = (await this.deps.storage.get(TASK_NOTICE_KEY))[TASK_NOTICE_KEY];
    if (value === undefined) return empty();
    if (!isLedger(value)) throw new Error("Task completion monitoring needs to be reset in settings.");
    return value;
  }

  private async save(state: Ledger): Promise<void> {
    if (state.intents.length + state.watched.length + state.deferred.length === 0) {
      state.handled = state.unread.map((run) => run.run_id);
    }
    if (!isLedger(state)) throw new Error("Too many task completions to monitor. Open the panel first.");
    await this.deps.storage.set({ [TASK_NOTICE_KEY]: state });
    await this.deps.schedule(state.intents.length + state.watched.length + state.deferred.length > 0);
    await this.deps.badge();
  }

  /** Persist and arm BEFORE dispatching start_task. Discovery includes fast
   * finishes and lost start replies, without waking an idle service forever. */
  arm(windowId: number, requestId: string): Promise<void> {
    return this.serial(async () => {
      if (!Number.isSafeInteger(windowId) || windowId < 0) throw new Error("Invalid task window.");
      if (typeof requestId !== "string" || !UUID.test(requestId)) throw new Error("Invalid task start request.");
      if (!await this.deps.enabled()) return;
      const state = await this.read();
      const now = this.deps.now();
      const existing = state.intents.find((intent) => intent.request_id === requestId);
      if (existing && existing.window_id !== windowId) throw new Error("Task start window changed.");
      if (!existing) state.intents.push({ request_id: requestId, since: now, until: now + DISCOVERY_MS, window_id: windowId });
      await this.save(state);
    });
  }

  resume(): Promise<void> {
    return this.serial(async () => {
      // Permission alone never creates monitoring state or wakes the host.
      if ((await this.deps.storage.get(TASK_NOTICE_KEY))[TASK_NOTICE_KEY] === undefined) return;
      const state = await this.read();
      if (!await this.deps.enabled()) return this.resetState(state);
      await this.deps.schedule(state.intents.length + state.watched.length + state.deferred.length > 0);
      await this.deps.badge();
    });
  }

  observed(runId: string): Promise<void> {
    if (!UUID.test(runId)) return Promise.reject(new Error("Invalid task run."));
    this.pendingObserved.add(runId);
    return this.serial(async () => {
      const state = await this.read();
      if (!await this.deps.enabled()) return;
      this.handled(state, runId);
      const unread = state.unread.filter((run) => run.run_id === runId);
      state.unread = state.unread.filter((run) => run.run_id !== runId);
      await this.save(state);
      await Promise.all(unread.map((run) => this.deps.clear(taskNoticeId(run.run_id, run.window_id))));
    }).finally(() => { this.pendingObserved.delete(runId); });
  }

  opened(): Promise<void> {
    return this.serial(async () => {
      const state = await this.read();
      const unread = state.unread;
      if (unread.length === 0) return;
      state.unread = [];
      await this.save(state);
      await Promise.all(unread.map((run) => this.deps.clear(taskNoticeId(run.run_id, run.window_id))));
    });
  }

  reset(): Promise<void> {
    return this.serial(async () => this.resetState(await this.read().catch(() => empty())));
  }

  private async resetState(state: Ledger): Promise<void> {
    await this.save(empty());
    await Promise.all(state.unread.map((run) => this.deps.clear(taskNoticeId(run.run_id, run.window_id)).catch(() => undefined)));
  }

  private handled(state: Ledger, runId: string): void {
    if (!state.handled.includes(runId)) state.handled.push(runId);
    state.watched = state.watched.filter((run) => run.run_id !== runId);
    state.deferred = state.deferred.filter((run) => run.run_id !== runId);
  }

  poll(): Promise<void> {
    return this.serial(async () => {
      const state = await this.read();
      if (!await this.deps.enabled()) return this.resetState(state);
      // No host connection when only a completion waits for the panel to close.
      if (state.intents.length + state.watched.length > 0) {
        const { runs, complete } = state.intents.length > 0
          ? await this.deps.list() : { runs: [], complete: true };
        if (!complete) throw new Error("Task history is incomplete; completion monitoring will retry.");
        for (const run of runs) {
          if (run.kind !== "task" || state.handled.includes(run.run_id) ||
              state.watched.some((known) => known.run_id === run.run_id) ||
              state.deferred.some((known) => known.run_id === run.run_id)) continue;
          const intent = state.intents.find((item) => item.request_id === run.start_request_id);
          if (intent) state.watched.push({ run_id: run.run_id, window_id: intent.window_id });
        }
        for (const watched of [...state.watched]) {
          const run = runs.find((item) => item.run_id === watched.run_id) ?? await this.deps.get(watched.run_id);
          if (run === null) {
            this.handled(state, watched.run_id); // pruned, never manufacture a finish
          } else if (run.kind === "task" && isTerminal(run)) {
            state.watched = state.watched.filter((item) => item.run_id !== run.run_id);
            const outcome = run.state === "succeeded" ? "succeeded" : run.state === "canceled" ? "canceled" : "failed";
            state.deferred.push({ ...watched, state: outcome });
          }
        }
        // Expire intents only after a complete successful discovery pass.
        state.intents = state.intents.filter((intent) => intent.until > this.deps.now());
      }
      const deliver = await this.deps.panelOpen() ? [] : [...state.deferred];
      for (const run of deliver) {
        this.handled(state, run.run_id);
        if (!this.pendingObserved.has(run.run_id)) state.unread.push(run);
      }
      // Persist before the external UI side effect: a cold worker cannot
      // repeat a toast. A crash in between still leaves the unread badge.
      await this.save(state);
      const results = await Promise.allSettled(deliver.map(async (run) => {
        if (this.pendingObserved.has(run.run_id)) return;
        const outcome = run.state === "succeeded" ? "finished" : run.state === "canceled" ? "canceled" : "failed";
        await this.deps.create(taskNoticeId(run.run_id, run.window_id), `Task ${outcome}`);
      }));
      const failed = results.find((result) => result.status === "rejected");
      if (failed?.status === "rejected") throw failed.reason;
    });
  }
}

/** The only start handshake; permission is optional and checked by worker. */
export async function armTaskNotifications(windowId: number, requestId: string): Promise<void> {
  const reply = await chrome.runtime.sendMessage<{ ok: boolean }>({ kind: "task_notice_arm", window_id: windowId, request_id: requestId });
  if (!reply?.ok) throw new Error("Could not prepare task completion monitoring. Try again.");
}

export function taskNoticeSignal(kind: "task_notice_seen" | "task_notice_opened", runId?: string): void {
  void chrome.runtime.sendMessage({ kind, ...(runId === undefined ? {} : { run_id: runId }) }).catch(() => undefined);
}

/** Optional API bindings can become available only after permission is
 * granted. Register on that event as well as cold start, without duplicates. */
export function listenForTaskNoticeClicks(
  api: Pick<ChromeApi, "permissions" | "notifications">, clicked: (id: string) => void,
): void {
  let installed = false;
  const install = () => {
    if (installed || !api.notifications) return;
    try {
      api.notifications.onClicked.addListener(clicked);
      installed = true;
    } catch {
      // An unavailable optional API must never prevent capture startup.
      // A later permission grant (or worker start) tries registration again.
    }
  };
  api.permissions.onAdded?.addListener((added) => {
    if (added.permissions?.includes("notifications")) install();
  });
  install();
}

