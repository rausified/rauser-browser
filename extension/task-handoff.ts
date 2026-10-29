// Reuse hand-off between the Tasks page and the side panel's task bar
// (spec §5.8): the Tasks page asks Chrome to open (or focus) the panel for
// its own window, then leaves the instruction in `chrome.storage.session`
// for whichever panel reads it next. The panel takes it once, for its own
// window or for any window, and fills the task bar unless a start is
// already in flight. `openTasksPage` is the panel's half of the hand-off in
// the other direction: it reuses an already-open Tasks tab rather than
// opening a second one, found with `runtime.getContexts` so the `tabs`
// permission is never needed (DESIGN.md §5.8).
import { storageKey } from "./brand.js";
import { tasksPageUrl } from "./task-history.js";

/** The one key both pages share in `chrome.storage.session`. */
export const REUSE_KEY = storageKey("task_reuse_v1");
/** The key `openTasksPage` signals an already-open Tasks tab on, naming the
 * run it should select (DESIGN.md §5.8: `#task-open` on a second task while
 * the Tasks tab is already open on an earlier one). */
export const SELECT_KEY = storageKey("tasks_select_v1");

interface ReuseValue {
  instruction: string;
  window_id: number | null;
  at: number;
}

interface SelectValue {
  run_id: string;
  at: number;
}

function isReuseValue(value: unknown): value is ReuseValue {
  return typeof value === "object" && value !== null &&
    typeof (value as ReuseValue).instruction === "string" &&
    ((value as ReuseValue).window_id === null || typeof (value as ReuseValue).window_id === "number") &&
    typeof (value as ReuseValue).at === "number";
}

function isSelectValue(value: unknown): value is SelectValue {
  return typeof value === "object" && value !== null &&
    typeof (value as SelectValue).run_id === "string" &&
    typeof (value as SelectValue).at === "number";
}

/** The run id a `storage.onChanged` value for `SELECT_KEY` names, or null
 * for anything else (missing, malformed, or the key's removal). */
export function parseSelectSignal(value: unknown): string | null {
  return isSelectValue(value) ? value.run_id : null;
}

/** Leaves `instruction` for the task bar to pick up, named to `windowId`
 * when the side panel opened for a known window, or to any window
 * (null) when it did not. */
export async function sendReuse(
  storage: Pick<ChromeStorageArea, "set">, instruction: string, windowId: number | null,
): Promise<void> {
  await storage.set({ [REUSE_KEY]: { instruction, window_id: windowId, at: Date.now() } satisfies ReuseValue });
}

/** Takes the stored instruction for `windowId`: a value named to no window
 * (null) or to this one. The key is always removed once read, so a value
 * for another window is left in place for that window's panel. Two panels
 * opening in the same window at the same moment may both read a
 * null-window value before either removes it; both would then fill their
 * bar with the same instruction, which is accepted. */
export async function takeReuse(storage: ChromeStorageArea, windowId: number): Promise<string | null> {
  const stored = (await storage.get(REUSE_KEY))[REUSE_KEY];
  if (!isReuseValue(stored)) return null;
  if (stored.window_id !== null && stored.window_id !== windowId) return null;
  await storage.remove?.(REUSE_KEY);
  return stored.instruction;
}

/** Opens the Tasks page on `runId` (or the newest task, for null): focuses
 * an already-open Tasks tab and navigates it there, or opens a new one.
 * `chromeApi` is narrowed so a test's fake Chrome need not implement the
 * rest of the API. A freshly opened tab reads `runId` straight off its
 * starting hash; an already-open tab may not notice a hash-only navigation
 * (`tasks.ts`'s `hashchange` listener does not reliably fire for one here),
 * so it is also sent `SELECT_KEY` to select directly, when `runId` is
 * known. */
export async function openTasksPage(
  chromeApi: Pick<ChromeApi, "runtime" | "tabs" | "windows" | "storage">, runId: string | null,
): Promise<void> {
  const url = tasksPageUrl(runId, chromeApi.runtime.getURL);
  const base = chromeApi.runtime.getURL("tasks.html");
  const contexts = await chromeApi.runtime.getContexts({ contextTypes: ["TAB"] });
  const [open] = contexts.filter((context) => context.documentUrl?.startsWith(base) === true);
  if (open) {
    if (runId !== null) {
      await chromeApi.storage.session.set({ [SELECT_KEY]: { run_id: runId, at: Date.now() } satisfies SelectValue });
    }
    await chromeApi.tabs.update(open.tabId, { active: true, url });
    await chromeApi.windows.update(open.windowId, { focused: true });
  } else {
    await chromeApi.tabs.create({ url });
  }
}
