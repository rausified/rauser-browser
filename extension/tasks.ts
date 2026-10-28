// The Tasks page (spec 2026-10-28, layout A): every task run the service
// keeps, listed on the left by day with search and a status filter, and
// the chosen task on the right as a live TaskDetail. The page lists runs
// only when a harness is ready or a run may exist, so opening it never
// wakes the on-demand service needlessly, and it does not poll while
// hidden. Agent text (instruction, title, report, stream) is shown with
// textContent only, and none of it reaches diag (DESIGN.md §15).
import type { RunView } from "../protocol/ts/generated.js";
import { serviceSetupText } from "./agent.js";
import { APP_NAME, storageKey } from "./brand.js";
import { initDiag } from "./diag.js";
import { HostError } from "./native.js";
import {
  TaskDetail, dismissedIds, elapsedText, isTerminal, runsMayExist, type TaskDetailDeps,
} from "./run-card.js";
import { ConfigSession, describe, element } from "./settings.js";
import { harnessReady } from "./summarize.js";
import { TASK_NEEDS_HARNESS_MESSAGE, UNSUPPORTED_REQUEST_TEXT } from "./task.js";
import {
  filterTasks, groupTasks, listAllTasks, listTaskPage, mergeRuns, parseRunHash, type StatusFilter,
} from "./task-history.js";

initDiag("tasks");

/** How often the list is re-read while any listed task is unfinished. */
export const LIST_REFRESH_MS = 2000;
const WINDOWS_TEXT = "Tasks are not supported on Windows yet.";
const OLDER_HOST_TEXT = `Rebuild the ${APP_NAME} host to see task history.`;
const REUSE_DONE_TEXT = "Instruction sent to the task bar.";
const TASK_REUSE_KEY = storageKey("task_reuse_v1");

const statusBox = element<HTMLDivElement>("tasks-status-box");
const statusLine = element<HTMLParagraphElement>("tasks-status");
const statusSettings = element<HTMLButtonElement>("tasks-status-settings");
const searchInput = element<HTMLInputElement>("tasks-search");
const statusFilter = element<HTMLSelectElement>("tasks-filter");
const showDismissed = element<HTMLInputElement>("tasks-show-dismissed");
const layout = element<HTMLDivElement>("tasks-layout");
const list = element<HTMLDivElement>("tasks-list");
const detailArea = element<HTMLElement>("tasks-detail");
const empty = element<HTMLParagraphElement>("tasks-empty");

const session = new ConfigSession(() => undefined);
const storage = chrome.storage.local;

// Every task run seen, newest first (mergeRuns' order).
let runs: RunView[] = [];
let dismissed = new Set<string>();
// The run ids in the order the list shows them, for the arrow keys.
let order: string[] = [];
const rows = new Map<string, { row: HTMLLIElement; pill: HTMLSpanElement }>();
let selectedId: string | null = null;
let detail: TaskDetail | null = null;
// Whether the first full listing has finished, so refreshes may run.
let listed = false;
let refreshing = false;
let pollTimer: ReturnType<typeof setTimeout> | null = null;
let closed = false;

function make<K extends keyof HTMLElementTagNameMap>(tag: K, className: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  return node;
}

function visible(): boolean {
  return document.visibilityState !== "hidden";
}

/** One line above the list: a problem (styled as a warning, optionally
 * with Open settings) or plain progress; null hides it. */
function showStatus(text: string | null, options: { problem?: boolean; settings?: boolean } = {}): void {
  statusBox.hidden = text === null;
  statusBox.className = options.problem ? "setup-warning" : "status";
  statusLine.textContent = text ?? "";
  statusSettings.hidden = options.settings !== true;
}

function openSettings(): void {
  void chrome.runtime.openOptionsPage().catch((error: unknown) => {
    showStatus(`Could not open settings: ${describe(error)}`, { problem: true });
  });
}

function showListError(error: unknown): void {
  if (error instanceof HostError && error.code === "service_unavailable") {
    showStatus(serviceSetupText(error.message), { problem: true, settings: true });
  } else if (error instanceof HostError && error.code === "invalid_request" && error.message === UNSUPPORTED_REQUEST_TEXT) {
    showStatus(OLDER_HOST_TEXT, { problem: true });
  } else {
    showStatus(`Could not list tasks: ${describe(error)}`, { problem: true });
  }
}

/** Reuse: hands the full instruction to the side panel's task bar. Task 7
 * replaces the hand-off; the page only needs it to resolve once sent. */
export async function reuseInstruction(instruction: string): Promise<void> {
  await chrome.storage.session.set({ [TASK_REUSE_KEY]: { instruction, window_id: null, at: Date.now() } });
}

/** An older service ignores `kind` and lists summaries too. */
function tasksOnly(views: readonly RunView[]): RunView[] {
  return views.filter((run) => run.kind === "task");
}

function shownRuns(): RunView[] {
  return filterTasks(runs, {
    query: searchInput.value,
    status: statusFilter.value as StatusFilter,
    dismissed,
    showDismissed: showDismissed.checked,
  });
}

function pillText(run: RunView): string {
  const elapsed = elapsedText(run, Date.now());
  const withElapsed = (label: string): string => elapsed ? `${label} · ${elapsed}` : label;
  switch (run.state) {
    case "queued":
      return "Queued";
    case "running":
      return withElapsed("Running");
    case "succeeded":
      return withElapsed("Done");
    case "failed":
      return "Failed";
    case "canceled":
      return "Canceled";
  }
}

function pillClass(run: RunView): string {
  switch (run.state) {
    case "queued":
    case "running":
      return "task-pill running";
    case "succeeded":
      return "task-pill done";
    case "failed":
      return "task-pill failed";
    case "canceled":
      return "task-pill canceled";
  }
}

function setPill(pill: HTMLSpanElement, run: RunView): void {
  pill.className = pillClass(run);
  pill.textContent = pillText(run);
}

/** The local time a run started, or was queued if it has not started. */
function startTime(run: RunView): string {
  const at = Date.parse(run.started_at ?? run.created_at);
  return Number.isFinite(at) ? new Date(at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" }) : "";
}

function rowFor(run: RunView): HTMLLIElement {
  const row = make("li", "task-row");
  row.id = `task-row-${run.run_id}`;
  row.dataset.runId = run.run_id;
  row.setAttribute("role", "option");
  row.setAttribute("aria-selected", "false");
  row.tabIndex = -1;
  const instruction = make("p", "task-row-instruction");
  instruction.textContent = run.instruction ?? "";
  const meta = make("p", "task-row-meta");
  const pill = make("span", "task-pill");
  setPill(pill, run);
  const time = make("span", "task-row-time");
  time.textContent = startTime(run);
  meta.append(pill, time);
  row.append(instruction, meta);
  row.addEventListener("click", () => select(run.run_id, true));
  rows.set(run.run_id, { row, pill });
  return row;
}

function renderList(): void {
  rows.clear();
  if (runs.length === 0) {
    order = [];
    empty.hidden = false;
    layout.hidden = true;
    list.replaceChildren();
    return;
  }
  empty.hidden = true;
  layout.hidden = false;
  const groups = groupTasks(shownRuns(), new Date());
  order = groups.flatMap((group) => group.runs.map((run) => run.run_id));
  const blocks = groups.map((group, index) => {
    const block = make("div", "task-group-block");
    block.setAttribute("role", "group");
    const label = make("h3", "task-group");
    label.id = `task-group-${index}`;
    label.textContent = group.label;
    block.setAttribute("aria-labelledby", label.id);
    const items = make("ul", "task-rows");
    items.setAttribute("role", "none");
    items.append(...group.runs.map(rowFor));
    block.append(label, items);
    return block;
  });
  if (blocks.length === 0) {
    const none = make("p", "tasks-none");
    none.textContent = "No tasks match.";
    list.replaceChildren(none);
  } else {
    list.replaceChildren(...blocks);
  }
  markSelected();
}

/** Marks the selected row, and keeps one row in the tab order. */
function markSelected(): void {
  for (const [runId, { row }] of rows) {
    const chosen = runId === selectedId;
    row.setAttribute("aria-selected", chosen ? "true" : "false");
    row.tabIndex = chosen ? 0 : -1;
  }
  if (selectedId === null || !rows.has(selectedId)) {
    const first = order[0];
    if (first !== undefined) rows.get(first)!.row.tabIndex = 0;
  }
}

function clearDetail(): void {
  detail?.dispose();
  detail = null;
  selectedId = null;
  detailArea.replaceChildren();
}

const detailDeps: TaskDetailDeps = {
  host: session.host,
  now: () => Date.now(),
  copy: (text) => navigator.clipboard.writeText(text),
  visible,
  storage,
  reuse: (instruction) => {
    void reuseInstruction(instruction).then(
      () => showStatus(REUSE_DONE_TEXT),
      (error: unknown) => showStatus(`Could not send the instruction: ${describe(error)}`, { problem: true }),
    );
  },
  // The run's own url, which the host bounds to http(s); never agent text.
  openPage: (url) => {
    if (!/^https?:\/\//i.test(url)) return;
    void chrome.tabs.create({ url }).catch((error: unknown) => {
      showStatus(`Could not open the page: ${describe(error)}`, { problem: true });
    });
  },
  dismissed: (runId) => taskDismissed(runId),
  changed: (run) => taskChanged(run),
};

/** Shows `runId` on the right, with a fresh TaskDetail, and names it in
 * the URL. */
function select(runId: string, focus = false): void {
  if (closed || !runs.some((run) => run.run_id === runId)) return;
  if (selectedId !== runId || detail === null) {
    const run = runs.find((candidate) => candidate.run_id === runId)!;
    detail?.dispose();
    detail = new TaskDetail(run, detailDeps);
    selectedId = runId;
    detailArea.replaceChildren(detail.element);
    detail.start();
  }
  history.replaceState(null, "", `#run=${runId}`);
  markSelected();
  if (focus) rows.get(runId)?.row.focus();
}

/** Selects the hash's run when the list shows it, otherwise the newest. */
function selectFromHash(): void {
  const wanted = parseRunHash(location.hash);
  const shown = shownRuns();
  const target = shown.find((run) => run.run_id === wanted) ?? shown[0];
  if (target) select(target.run_id);
  else clearDetail();
}

function taskChanged(run: RunView): void {
  const before = runs.find((candidate) => candidate.run_id === run.run_id);
  runs = mergeRuns(runs, [run]);
  const after = runs.find((candidate) => candidate.run_id === run.run_id)!;
  const parts = rows.get(run.run_id);
  // Finishing moves a row out of Running (and maybe out of the filter).
  if (!before || !parts || isTerminal(before) !== isTerminal(after)) renderList();
  else setPill(parts.pill, after);
  schedulePoll();
}

function taskDismissed(runId: string): void {
  dismissed.add(runId);
  const at = order.indexOf(runId);
  renderList();
  if (selectedId !== runId || order.includes(runId)) return;
  const next = order[at] ?? order[at - 1];
  if (next !== undefined) select(next);
  else clearDetail();
}

function schedulePoll(): void {
  if (closed || !listed || pollTimer !== null || !visible() || !runs.some((run) => !isTerminal(run))) return;
  pollTimer = setTimeout(() => {
    pollTimer = null;
    void refresh();
  }, LIST_REFRESH_MS);
}

function stopPolling(): void {
  if (pollTimer !== null) clearTimeout(pollTimer);
  pollTimer = null;
}

/** Re-reads the first page (active runs and the newest finished) and
 * merges it into the list. */
async function refresh(): Promise<void> {
  if (closed || !listed || refreshing) return;
  refreshing = true;
  try {
    const page = await listTaskPage(session.host, null);
    if (closed) return;
    runs = mergeRuns(runs, tasksOnly(page.runs));
    if (!statusBox.hidden && statusBox.className === "setup-warning") showStatus(null);
    renderList();
    if (selectedId === null) selectFromHash();
  } catch (error) {
    if (!closed) showListError(error);
  } finally {
    refreshing = false;
    schedulePoll();
  }
}

async function open(): Promise<void> {
  // The Brauser service runs on macOS (and Unix) only.
  if (/^win/i.test(navigator.platform ?? "")) {
    showStatus(WINDOWS_TEXT);
    return;
  }
  showStatus("Waiting for the native host…");
  let ready: boolean;
  try {
    ready = harnessReady({ agentStatus: (await session.readHostConfig()).agent_status });
  } catch (error) {
    showListError(error);
    return;
  }
  if (!ready && !(await runsMayExist(storage))) {
    showStatus(TASK_NEEDS_HARNESS_MESSAGE, { problem: true, settings: true });
    return;
  }
  showStatus("Loading tasks…");
  try {
    runs = mergeRuns([], tasksOnly(await listAllTasks(session.host)));
  } catch (error) {
    showListError(error);
    return;
  }
  dismissed = new Set(await dismissedIds(storage));
  if (closed) return;
  listed = true;
  showStatus(null);
  renderList();
  selectFromHash();
  schedulePoll();
}

statusSettings.addEventListener("click", openSettings);
searchInput.addEventListener("input", () => renderList());
statusFilter.addEventListener("change", () => renderList());
showDismissed.addEventListener("change", () => renderList());

list.addEventListener("keydown", (event: KeyboardEvent) => {
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    if (order.length === 0) return;
    const at = selectedId === null ? -1 : order.indexOf(selectedId);
    const step = event.key === "ArrowDown" ? 1 : -1;
    const next = at === -1 ? order[0]! : order[Math.min(order.length - 1, Math.max(0, at + step))]!;
    select(next, true);
  } else if (event.key === "Enter") {
    const runId = (event.target as HTMLElement | null)?.dataset?.runId;
    if (runId === undefined) return;
    event.preventDefault();
    select(runId, true);
  }
});

document.addEventListener("visibilitychange", () => {
  if (!visible()) {
    stopPolling();
    return;
  }
  stopPolling();
  void refresh();
});

window.addEventListener("hashchange", () => {
  const wanted = parseRunHash(location.hash);
  if (wanted === null || wanted === selectedId || !listed) return;
  if (runs.some((run) => run.run_id === wanted)) {
    select(wanted);
    return;
  }
  // A run started since the list was read: read the first page, then look.
  void refresh().then(() => {
    if (runs.some((run) => run.run_id === wanted)) select(wanted);
  });
});

window.addEventListener("pagehide", () => {
  closed = true;
  stopPolling();
  detail?.dispose();
  detail = null;
});

void open();
