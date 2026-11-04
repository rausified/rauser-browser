// Panel-memory instruction browsing. The service owns history; list_runs is
// only an index because its instruction previews are cut to 280 characters.
import type { RunCursor, RunView } from "../protocol/ts/generated.js";
import type { HostClient } from "./native.js";
import { fetchOffPageRun, lastFinishedRun, listTaskPage, mergeRuns, ofKind } from "./task-history.js";

import { literalTaskInstruction } from "./task-commands.js";

export const MAX_RECALL_PAGES = 20;
export const MAX_RECALL_TASKS = 200;

export class TaskRecall {
  private runs: RunView[] = [];
  private deferred: RunView[] = [];
  private edits = new Map<number, string>();
  private missing = new Set<number>();
  private cursor = -1;
  private before: RunCursor | null = null;
  private more = true;
  private pages = 0;
  private revision = 0;
  private pending = false;

  constructor(private readonly host: Pick<HostClient, "call">) {}

  /** Typing invalidates a delayed replacement without losing visited edits. */
  edited(): void { this.revision += 1; }

  /** Reuse, submission and teardown start a new browsing session. */
  reset(): void {
    this.edited();
    this.runs = [];
    this.deferred = [];
    this.edits.clear();
    this.missing.clear();
    this.cursor = -1;
    this.before = null;
    this.more = true;
    this.pages = 0;
  }

  /** +1 is older (Up); -1 is newer (Down). null leaves the field alone.
   * One request chain at a time: key repeats never queue delayed replacements. */
  async move(direction: 1 | -1, text: string): Promise<string | null> {
    if (this.pending || (direction === -1 && this.cursor === -1)) return null;
    this.edits.set(this.cursor, text);
    const revision = this.revision;
    this.pending = true;
    try {
      for (let next = this.cursor + direction; next >= -1; next += direction) {
        while (next >= this.runs.length && this.more) {
          if (!await this.appendPage(revision)) return null;
        }
        if (revision !== this.revision || next >= this.runs.length) return null;
        if (this.missing.has(next)) continue;
        let value = this.edits.get(next);
        if (value === undefined) {
          const run = await fetchOffPageRun(this.host, this.runs[next]!);
          if (revision !== this.revision) return null;
          if (run?.kind !== "task" || typeof run.instruction !== "string") {
            this.missing.add(next); // Pruned after listing: try the next entry.
            continue;
          }
          value = literalTaskInstruction(run.instruction);
          this.edits.set(next, value);
        }
        this.cursor = next;
        return value;
      }
      return null;
    } catch (error) {
      // A failure after typing/Reuse/disposal no longer belongs to this field.
      if (revision !== this.revision) return null;
      throw error;
    } finally {
      this.pending = false;
    }
  }

  private async appendPage(revision: number): Promise<boolean> {
    const page = await listTaskPage(this.host, this.before);
    if (revision !== this.revision) return false;
    this.pages += 1;
    // Older hosts can ignore kind. Cursor from the unfiltered page, so a
    // summary-only page can still lead to older tasks; never cursor an active run.
    const last = lastFinishedRun(page.runs);
    const next = last ? { created_at: last.created_at, run_id: last.run_id } : null;
    const advances = next !== null && (this.before === null ||
      next.created_at !== this.before.created_at || next.run_id !== this.before.run_id);
    const canPage = page.more && advances && this.pages < MAX_RECALL_PAGES;
    const known = new Set(this.runs.map((run) => run.run_id));
    const deferred: RunView[] = [];
    for (const run of mergeRuns(this.deferred, ofKind(page.runs, "task"))) {
      if (known.has(run.run_id)) continue;
      // Active runs repeat on every page, even when older than the terminal
      // page window. Hold those until the cursor reaches their chronological
      // place, so an old queued task cannot jump ahead of newer finished work.
      const older = last !== undefined && (run.created_at < last.created_at ||
        (run.created_at === last.created_at && run.run_id < last.run_id));
      if (canPage && older) {
        deferred.push(run);
        continue;
      }
      known.add(run.run_id);
      this.runs.push(run);
      if (this.runs.length === MAX_RECALL_TASKS) break;
    }
    this.more = canPage && this.runs.length < MAX_RECALL_TASKS;
    this.deferred = this.more ? deferred : [];
    this.before = next;
    return true;
  }
}
