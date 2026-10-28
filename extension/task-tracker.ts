// The side panel's watch over task runs (DESIGN.md §5.7), now that task
// cards live on the Tasks page: it reads the first page of task history to
// count what is queued or running, and tells the panel when a task it saw
// unfinished has finished, so `#task-announce` can say so. It never draws
// agent text, and nothing here reaches diag (DESIGN.md §15).
import type { RunView } from "../protocol/ts/generated.js";
import { HostError, PROTOCOL_VERSION, newRequestId, type HostClient } from "./native.js";
import { isTerminal } from "./run-card.js";
import { listTaskPage, mergeRuns, runningCount } from "./task-history.js";

/** How often the tracker re-reads the list while a task is active. */
export const TRACK_MS = 2000;

/** What the tracker reports after each list. */
export interface TrackedState {
  /** Queued plus running tasks in the first page. */
  running: number;
  /** The newest task in the first page, or null when there is none. */
  newestId: string | null;
}

export interface TaskTrackerDeps {
  host: Pick<HostClient, "call">;
  now(): number;
  /** Whether the panel is visible; the tracker polls only then. */
  visible(): boolean;
  changed(state: TrackedState): void;
  /** A task the tracker saw unfinished has finished. Called once per run. */
  finished(run: RunView): void;
}

export class TaskTracker {
  // Every task seen queued or running and not yet reported finished.
  private readonly active = new Map<string, RunView>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private inFlight: Promise<void> | null = null;
  private again = false;
  private disposed = false;

  constructor(private readonly deps: TaskTrackerDeps) {}

  /** Watches `run`, one the panel just started, before any list has it, so
   * a task that finishes before the next list is still announced. */
  track(run: RunView): void {
    if (this.disposed || isTerminal(run)) return;
    this.active.set(run.run_id, run);
  }

  /** Reads the first page of tasks once (a refresh asked for while one is in
   * flight runs once more after it) and reports what changed. Rejects with
   * the list's error; the tracker keeps polling while it watches a task. */
  refresh(): Promise<void> {
    if (this.disposed) return Promise.resolve();
    if (this.inFlight) {
      this.again = true;
      return this.inFlight;
    }
    const run = (async () => {
      do {
        this.again = false;
        await this.once();
      } while (this.again && !this.disposed);
    })();
    this.inFlight = run.finally(() => {
      this.inFlight = null;
    });
    return this.inFlight;
  }

  dispose(): void {
    this.disposed = true;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  }

  private async once(): Promise<void> {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    let running = 0;
    try {
      const { runs } = await listTaskPage(this.deps.host, null);
      if (this.disposed) return;
      const listed = new Set(runs.map((run) => run.run_id));
      for (const run of runs) this.saw(run);
      for (const run of [...this.active.values()]) {
        if (!listed.has(run.run_id)) await this.fetchMissing(run);
        if (this.disposed) return;
      }
      running = runningCount(runs);
      this.deps.changed({ running, newestId: mergeRuns([], runs)[0]?.run_id ?? null });
    } finally {
      if (!this.disposed) this.schedule(running);
    }
  }

  /** A run seen in a list or a get_run: watched while unfinished, reported
   * once when a watched run has finished. */
  private saw(run: RunView): void {
    if (!isTerminal(run)) {
      this.active.set(run.run_id, run);
      return;
    }
    if (!this.active.delete(run.run_id)) return;
    this.deps.finished(run);
  }

  /** A watched run the first page left out: one get_run, from the stream
   * offset it had, says how it stands. A run the service no longer holds is
   * dropped; any other failure is tried again on the next refresh. */
  private async fetchMissing(run: RunView): Promise<void> {
    try {
      const fetched = await this.deps.host.call({
        type: "get_run", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
        run_id: run.run_id, since_offset: run.stream_bytes,
      }, "run_fetched");
      if (!this.disposed) this.saw(fetched.run);
    } catch (error) {
      if (error instanceof HostError && error.code === "invalid_request") this.active.delete(run.run_id);
    }
  }

  private schedule(running: number): void {
    if (this.timer !== null || (running === 0 && this.active.size === 0) || !this.deps.visible()) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      if (this.deps.visible()) void this.refresh().catch(() => undefined);
    }, TRACK_MS);
  }
}
