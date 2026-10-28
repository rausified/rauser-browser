// The Tasks page's own view of every task run (spec §5.8), independent of
// the live task cards a panel shows. It walks list_runs with kind "task"
// one page at a time, merges what the page already has with what a page
// returns (a run can arrive both active and, later, finished), then lets
// the page filter and group what it shows. Agent text (instruction, title)
// is untrusted and only ever matched or shown as text, never rendered as
// HTML or Markdown.
import type { RunCursor, RunView } from "../protocol/ts/generated.js";
import { PROTOCOL_VERSION, newRequestId, type HostClient } from "./native.js";
import { isTerminal } from "./run-card.js";

/** Which tasks a filter keeps. "running" covers both queued and running. */
export type StatusFilter = "all" | "running" | "done" | "failed" | "canceled";

/** Walking task history stops after this many list_runs calls, so a host
 * that never says it is done cannot hang the page. */
export const MAX_HISTORY_PAGES = 100;

/** One page of task history. A missing `more` (an older host) counts as
 * the last page. `before` is omitted from the request when there is none,
 * so the first page asks for the service's default first page of tasks. */
export async function listTaskPage(
  host: Pick<HostClient, "call">, before: RunCursor | null,
): Promise<{ runs: RunView[]; more: boolean }> {
  const response = await host.call({
    type: "list_runs",
    protocol_version: PROTOCOL_VERSION,
    request_id: newRequestId(),
    kind: "task",
    ...(before === null ? {} : { before }),
  }, "runs_listed");
  return { runs: response.runs, more: response.more ?? false };
}

/** Every task run the service holds, walked one page at a time with the
 * last run of each page as the next page's cursor. Stops when a page says
 * there is no more, after {@link MAX_HISTORY_PAGES} pages, or when a page's
 * last run does not move the cursor past the one it was asked for (a host
 * stuck on the same page cannot be walked forever). */
export async function listAllTasks(host: Pick<HostClient, "call">): Promise<RunView[]> {
  const all: RunView[] = [];
  let cursor: RunCursor | null = null;
  for (let page = 0; page < MAX_HISTORY_PAGES; page += 1) {
    const { runs, more } = await listTaskPage(host, cursor);
    all.push(...runs);
    if (!more) break;
    const last = runs.at(-1);
    if (!last) break;
    const next: RunCursor = { created_at: last.created_at, run_id: last.run_id };
    if (cursor !== null && next.created_at === cursor.created_at && next.run_id === cursor.run_id) break;
    cursor = next;
  }
  return all;
}

/** `known` and `fresh` merged into one view per `run_id`: a run that
 * finished since it was last seen keeps its finished view, whichever list
 * it came from; otherwise the `fresh` view wins. The result is sorted
 * newest first, by `created_at` then `run_id` (both descending), so a tied
 * timestamp still orders the same way every time. */
export function mergeRuns(known: readonly RunView[], fresh: readonly RunView[]): RunView[] {
  const merged = new Map<string, RunView>();
  for (const run of known) merged.set(run.run_id, run);
  for (const run of fresh) {
    const existing = merged.get(run.run_id);
    if (existing && isTerminal(existing) && !isTerminal(run)) continue;
    merged.set(run.run_id, run);
  }
  return [...merged.values()].sort((a, b) => {
    if (a.created_at !== b.created_at) return a.created_at < b.created_at ? 1 : -1;
    return a.run_id < b.run_id ? 1 : -1;
  });
}

/** Whether `run` is still queued or running. */
function isActive(run: RunView): boolean {
  return !isTerminal(run);
}

function statusMatches(run: RunView, status: StatusFilter): boolean {
  switch (status) {
    case "all":
      return true;
    case "running":
      return isActive(run);
    case "done":
      return run.state === "succeeded";
    case "failed":
      return run.state === "failed";
    case "canceled":
      return run.state === "canceled";
  }
}

/** What the page's search box, status tabs and "show dismissed" toggle keep
 * from a list of task runs. A query matches the instruction or the title,
 * case-insensitively. A dismissed run is hidden unless `showDismissed`, but
 * only once it is finished: dismissing only ever applies to a finished run
 * (run-card.ts), and an active run is never hidden. */
export function filterTasks(
  runs: readonly RunView[],
  opts: { query: string; status: StatusFilter; dismissed: ReadonlySet<string>; showDismissed: boolean },
): RunView[] {
  const query = opts.query.trim().toLowerCase();
  return runs.filter((run) => {
    if (!statusMatches(run, opts.status)) return false;
    if (!opts.showDismissed && !isActive(run) && opts.dismissed.has(run.run_id)) return false;
    if (query === "") return true;
    const instruction = (run.instruction ?? "").toLowerCase();
    return instruction.includes(query) || run.title.toLowerCase().includes(query);
  });
}

/** One labelled section of the task list. */
export interface TaskGroup {
  label: string;
  runs: RunView[];
}

function localDayKey(date: Date): string {
  return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
}

function startOfDay(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

/** "Today", "Yesterday", or the weekday-and-date Chrome's locale gives any
 * older day. */
function dayLabel(date: Date, now: Date): string {
  const msPerDay = 24 * 60 * 60 * 1000;
  const daysAgo = Math.round((startOfDay(now).getTime() - startOfDay(date).getTime()) / msPerDay);
  if (daysAgo === 0) return "Today";
  if (daysAgo === 1) return "Yesterday";
  return date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
}

/** `runs` (in whatever order the caller holds them) split into the page's
 * sections: every active run under "Running" first, then one group per
 * local calendar day of `created_at`, in the order each day's first run
 * appears in `runs` (the caller sorts newest first). A section with
 * nothing in it is left out. */
export function groupTasks(runs: readonly RunView[], now: Date): TaskGroup[] {
  const groups: TaskGroup[] = [];
  const running = runs.filter(isActive);
  if (running.length > 0) groups.push({ label: "Running", runs: running });

  const byDay = new Map<string, RunView[]>();
  const dayOf = new Map<string, Date>();
  const order: string[] = [];
  for (const run of runs) {
    if (isActive(run)) continue;
    const date = new Date(run.created_at);
    const key = localDayKey(date);
    if (!byDay.has(key)) {
      byDay.set(key, []);
      dayOf.set(key, date);
      order.push(key);
    }
    byDay.get(key)!.push(run);
  }
  for (const key of order) groups.push({ label: dayLabel(dayOf.get(key)!, now), runs: byDay.get(key)! });
  return groups;
}

/** How many tasks are queued or running, for the panel's "Tasks ↗" button
 * (DESIGN.md §5.7). */
export function runningCount(runs: readonly RunView[]): number {
  return runs.filter(isActive).length;
}

const RUN_HASH = /^#run=([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/;

/** The run id a Tasks page URL's `#run=` fragment names, or null for any
 * other fragment (none, the wrong key, or not a lowercase hyphenated UUID).
 * A hash from outside the page is never trusted further than this. */
export function parseRunHash(hash: string): string | null {
  const match = RUN_HASH.exec(hash);
  return match ? match[1]! : null;
}

/** The Tasks page's own URL, optionally naming one run to open on arrival.
 * `getURL` is `chrome.runtime.getURL`, passed in so tests can fake it. */
export function tasksPageUrl(runId: string | null, getURL: (path: string) => string): string {
  const base = getURL("tasks.html");
  return runId === null ? base : `${base}#run=${runId}`;
}
