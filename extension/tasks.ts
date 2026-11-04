// The Tasks page (spec 2026-10-28, layout A): every task and summary run
// the service keeps (a page's sidebar shows only its newest summary run, so
// this is where an older one stays reachable), listed on the left by day with search and a status filter, and
// the chosen task on the right as a live TaskDetail. The page lists runs
// only when a harness is ready or a run may exist, so opening it never
// wakes the on-demand service needlessly, and it does not poll while
// hidden. Agent text (instruction, title, report, stream) is shown with
// textContent only, and none of it reaches diag (DESIGN.md §15).
import type { RunView } from "../protocol/ts/generated.js";
import { serviceSetupText } from "./agent.js";
import { APP_NAME } from "./brand.js";
import { initDiag } from "./diag.js";
import { HostError, PROTOCOL_VERSION, newRequestId } from "./native.js";
import {
  RUNS_REBUILD_HOST_MESSAGE, TaskDetail, dismissedIds, elapsedText, isTerminal, runsMayExist,
  type TaskDetailDeps,
} from "./run-card.js";
import { ConfigSession, describe, element } from "./settings.js";
import { harnessReady } from "./summarize.js";
import { SELECT_KEY, parseSelectSignal, sendReuse } from "./task-handoff.js";
import { TASK_NEEDS_HARNESS_MESSAGE, UNSUPPORTED_REQUEST_TEXT } from "./task.js";
import {
  HISTORY_KINDS, fetchOffPageRun, filterTasks, groupTasks, listAllTasks, listTaskPage, mergeRuns, ofKind,
  parseRunHash, runKind, supersededSummaries, type StatusFilter,
} from "./task-history.js";

initDiag("tasks");

/** How often the list is re-read while any listed task is unfinished. */
export const LIST_REFRESH_MS = 2000;
const WINDOWS_TEXT = "Tasks are not supported on Windows yet.";
const REUSE_DONE_TEXT = "Instruction sent to the task bar.";

// This page's own window, read once at load so Reuse's click handler can
// call `chrome.sidePanel.open` synchronously, before any await (Chrome
// requires the call to start within the user gesture). A page that cannot
// read its window (an old Chrome) sends Reuse to any window instead.
// `pageWindowId` can briefly still be null here; the `{}` reject path below already covers that.
let pageWindowId: number | null = null;
void chrome.windows.getCurrent().then((current) => { pageWindowId = current.id ?? null; }, () => undefined);

const statusBox = element<HTMLDivElement>("tasks-status-box");
const statusLine = element<HTMLParagraphElement>("tasks-status");
const statusSettings = element<HTMLButtonElement>("tasks-status-settings");
const controls = element<HTMLDivElement>("tasks-controls");
const searchInput = element<HTMLInputElement>("tasks-search");
const statusFilter = element<HTMLSelectElement>("tasks-filter");
const showDismissed = element<HTMLInputElement>("tasks-show-dismissed");
const layout = element<HTMLDivElement>("tasks-layout");
const list = element<HTMLDivElement>("tasks-list");
const detailArea = element<HTMLElement>("tasks-detail");
const empty = element<HTMLParagraphElement>("tasks-empty");
const noMatches = element<HTMLParagraphElement>("tasks-none");

const session = new ConfigSession(() => undefined);
const storage = chrome.storage.local;

// Every task and summary run seen, newest first (mergeRuns' order).
let runs: RunView[] = [];
// The summary runs a newer run of their page replaced, from `runs`.
let superseded = new Set<string>();
let dismissed = new Set<string>();
// The run ids in the order the list shows them, for the arrow keys.
let order: string[] = [];
interface RowParts { row: HTMLLIElement; instruction: HTMLParagraphElement; pill: HTMLSpanElement; time: HTMLSpanElement }
const rows = new Map<string, RowParts>();
// The groups and rows the list last drew, so a refresh that changes neither
// updates the rows in place instead of rebuilding them (and losing focus).
let layoutKey = "";
let selectedId: string | null = null;
let detail: TaskDetail | null = null;
// Whether the first full listing has finished, so refreshes may run.
let listed = false;
// The open (first full listing) in flight, so a show or a select signal
// that retries it after a failure or the setup text runs it only once.
let opening: Promise<void> | null = null;
let refreshing: Promise<void> | null = null;
let refreshAgain = false;
// Whether the status line shows a list error, the only line a refresh that
// works may clear (a Reuse or Open page line stays).
let listErrorShown = false;
let pollTimer: ReturnType<typeof setTimeout> | null = null;
let closed = false;
// The home/config this history page opened against; a later settings change
// makes a note write refuse until the user reloads this page.
let noteConfigRevision: string | null = null;

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
  listErrorShown = false;
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
    showStatus(RUNS_REBUILD_HOST_MESSAGE, { problem: true });
  } else {
    showStatus(`Could not list tasks: ${describe(error)}`, { problem: true });
  }
  listErrorShown = true;
}

/** The first page of each kind the page lists (active runs and the newest
 * finished), keeping only that kind's runs from each answer. */
async function listFirstPages(): Promise<RunView[]> {
  const pages = await Promise.all(HISTORY_KINDS.map((kind) => listTaskPage(session.host, null, kind)));
  return pages.flatMap((page, index) => ofKind(page.runs, HISTORY_KINDS[index]!));
}

/** A row's headline: a task's instruction, or the page a summary is of
 * (marked once a newer run of the page replaced its file). */
function rowText(run: RunView): string {
  if (runKind(run) === "task") return run.instruction ?? "";
  const text = `Summary · ${run.title || run.url}`;
  return superseded.has(run.run_id) ? `${text} · superseded` : text;
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
  if (pill.className !== pillClass(run)) pill.className = pillClass(run);
  if (pill.textContent !== pillText(run)) pill.textContent = pillText(run);
}

function setText(node: HTMLElement, text: string): void {
  if (node.textContent !== text) node.textContent = text;
}

/** A row's text, from the latest view of its run. */
function fillRow(parts: RowParts, run: RunView): void {
  setText(parts.instruction, rowText(run));
  setPill(parts.pill, run);
  setText(parts.time, startTime(run));
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
  const parts: RowParts = {
    row,
    instruction: make("p", "task-row-instruction"),
    pill: make("span", "task-pill"),
    time: make("span", "task-row-time"),
  };
  fillRow(parts, run);
  const meta = make("p", "task-row-meta");
  meta.append(parts.pill, parts.time);
  row.append(parts.instruction, meta);
  row.addEventListener("click", () => select(run.run_id, true));
  rows.set(run.run_id, parts);
  return row;
}

/** Draws the list. When the groups and rows are the ones already drawn,
 * only their text changes, so a row keeps its element and its focus; a
 * rebuild puts focus back on the selected row if a row held it. */
function renderList(): void {
  superseded = supersededSummaries(runs);
  detail?.setSuperseded(selectedId !== null && superseded.has(selectedId));
  const groups = runs.length === 0 ? [] : groupTasks(shownRuns(), new Date());
  const key = runs.length === 0
    ? "empty"
    : JSON.stringify(groups.map((group) => [group.label, ...group.runs.map((run) => run.run_id)]));
  if (key === layoutKey) {
    for (const group of groups) {
      for (const run of group.runs) fillRow(rows.get(run.run_id)!, run);
    }
    markSelected();
    return;
  }
  layoutKey = key;
  const active = document.activeElement;
  const hadFocus = [...rows.values()].some((parts) => parts.row === active);
  rows.clear();
  if (runs.length === 0) {
    order = [];
    empty.hidden = false;
    layout.hidden = true;
    list.replaceChildren();
    list.hidden = true;
    noMatches.hidden = true;
    return;
  }
  empty.hidden = true;
  layout.hidden = false;
  order = groups.flatMap((group) => group.runs.map((run) => run.run_id));
  const blocks = groups.map((group, index) => {
    const block = make("div", "task-group-block");
    block.setAttribute("role", "group");
    const label = make("h3", "task-group");
    // A listbox owns groups/options, not headings. The visible label names
    // its group through aria-labelledby without adding a heading child.
    label.setAttribute("role", "presentation");
    label.id = `task-group-${index}`;
    label.textContent = group.label;
    block.setAttribute("aria-labelledby", label.id);
    const items = make("ul", "task-rows");
    items.setAttribute("role", "none");
    items.append(...group.runs.map(rowFor));
    block.append(label, items);
    return block;
  });
  // An empty result is prose beside the widget, not a listbox with no
  // options. Keep the selected detail available while the user filters.
  noMatches.hidden = blocks.length !== 0;
  list.hidden = blocks.length === 0;
  list.replaceChildren(...blocks);
  markSelected();
  if (hadFocus) {
    const target = (selectedId !== null ? rows.get(selectedId) : undefined) ?? rows.get(order[0] ?? "");
    if (target) target.row.focus();
    else searchInput.focus();
  }
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

/** Drops the detail but keeps the selection, so `select` builds it again. */
function clearDetailOnly(): void {
  detail?.dispose();
  detail = null;
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
  saveToNote: async (run) => {
    if (!noteConfigRevision) throw new Error("Settings have not loaded. Reload Tasks and try again.");
    try {
      const saved = await session.host.call({
        type: "save_task_note", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
        run_id: run.run_id, expected_revision: noteConfigRevision,
      }, "task_note_saved");
      if (saved.run_id !== run.run_id) throw new Error("The host answered for a different task. Reload Tasks and try again.");
      return saved.already_saved;
    } catch (error) {
      if (error instanceof HostError && error.code === "invalid_request" && error.message === UNSUPPORTED_REQUEST_TEXT) {
        throw new Error(`Rebuild the ${APP_NAME} host and extension together to save task reports to notes.`, { cause: error });
      }
      throw error;
    }
  },
  // Opens the side panel for this page's window before any await, as the
  // user gesture requires, then hands the instruction off to it: to its
  // window once the panel is known to be there, or to any window once
  // opening it failed (an older Chrome, or no window to open it in). It
  // also reopens the run's own page (bounded to http(s) by the host, never
  // agent text), active in this window, so the panel follows that tab and
  // the reused instruction runs against the page it was first given on. A
  // page that fails to open still leaves the instruction in the bar.
  reuse: (instruction, url, noPage = false) => {
    const windowId = pageWindowId;
    const opened = chrome.sidePanel.open(windowId === null ? {} : { windowId });
    const tab: Promise<string | null> = !noPage && /^https?:\/\//i.test(url)
      ? chrome.tabs.create(windowId === null ? { url, active: true } : { url, windowId, active: true })
        .then(() => null, (error: unknown) => describe(error))
      : Promise.resolve(null);
    void opened.then(
      () => sendReuse(chrome.storage.session, instruction, windowId, noPage),
      () => sendReuse(chrome.storage.session, instruction, null, noPage),
    ).then(
      async () => {
        const tabError = await tab;
        if (tabError === null) showStatus(REUSE_DONE_TEXT);
        else showStatus(`${REUSE_DONE_TEXT} Could not open the page: ${tabError}`, { problem: true });
      },
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
    detail.setSuperseded(superseded.has(runId));
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

/** Keeps the selection in step with the hash after a list refresh: picks
 * one when nothing is selected yet, and also follows the hash on to a
 * run it now shows that it did not before (`#task-open` sending the page
 * to a run started after its last list, while it was already open on an
 * earlier one — the `hashchange` listener below tries this once too, but
 * a same-document navigation does not reliably fire that event). */
function syncSelectionToHash(): void {
  if (selectedId === null) {
    selectFromHash();
    return;
  }
  const wanted = parseRunHash(location.hash);
  if (wanted !== null && wanted !== selectedId && runs.some((run) => run.run_id === wanted)) select(wanted);
}

/** A select signal from `openTasksPage` (task-handoff.ts): another Tasks
 * tab reused by Tasks ↗ named a run for this page to show. Selects it
 * directly when it is already known; otherwise this page may simply not
 * have it yet (started after this page's last list, while no task was
 * active to keep it polling) — one `list_runs` refresh is tried first, then
 * the run is selected if it showed up, falling back to the newest, the way
 * an unknown `#run=` hash does. A read failure is not surfaced here; the
 * signal just falls back to whatever the page already knows. */
async function selectFromSignal(runId: string): Promise<void> {
  if (closed) return;
  if (!listed) {
    // The first list failed or was never tried: open again, on this run
    // when it is listed (the hash sent with the signal names it too).
    await reopen();
    if (!closed && listed && runs.some((run) => run.run_id === runId)) select(runId);
    return;
  }
  if (runs.some((run) => run.run_id === runId)) {
    select(runId);
    return;
  }
  try {
    const fresh = await listFirstPages();
    if (closed) return;
    runs = mergeRuns(runs, fresh);
    renderList();
  } catch {
    // Select from whatever is already known instead.
  }
  if (closed) return;
  const shown = shownRuns();
  const target = shown.find((run) => run.run_id === runId) ?? shown[0];
  if (target) select(target.run_id);
}

function taskChanged(run: RunView): void {
  runs = mergeRuns(runs, [run]);
  // Finishing moves a row out of Running (and maybe out of the filter);
  // otherwise only its pill changes.
  renderList();
  schedulePoll();
}

function taskDismissed(runId: string): void {
  const hadDetailFocus = detailArea.contains(document.activeElement);
  dismissed.add(runId);
  const at = order.indexOf(runId);
  renderList();
  if (selectedId !== runId || order.includes(runId)) return;
  const next = order[at] ?? order[at - 1];
  if (next !== undefined) select(next, hadDetailFocus);
  else {
    clearDetail();
    if (hadDetailFocus) searchInput.focus();
  }
}

function schedulePoll(): void {
  if (closed || !listed || refreshing !== null || pollTimer !== null || !visible() || !runs.some((run) => !isTerminal(run))) return;
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
 * merges it into the list. An unfinished run the list has that the page
 * left out gets one get_run, as the panel's TaskTracker does, so it still
 * shows its finish (and polling stops); one the service no longer holds
 * leaves the list. */
function refresh(): Promise<void> {
  if (closed || !listed) return Promise.resolve();
  if (refreshing !== null) {
    refreshAgain = true;
    return refreshing;
  }
  stopPolling();
  const run = (async () => {
    do {
      refreshAgain = false;
      await refreshOnce();
    } while (refreshAgain && !closed);
  })();
  refreshing = run.finally(() => {
    refreshing = null;
    schedulePoll();
  });
  return refreshing;
}

async function refreshOnce(): Promise<void> {
  try {
    const fresh = await listFirstPages();
    if (closed) return;
    const onPage = new Set(fresh.map((run) => run.run_id));
    const gone = new Set<string>();
    const missing = runs.filter((candidate) => !isTerminal(candidate) && !onPage.has(candidate.run_id));
    const results = await Promise.allSettled(missing.map((run) => fetchOffPageRun(session.host, run)));
    if (closed) return;
    for (const [index, result] of results.entries()) {
      // A failed fetch leaves its row to be tried again on the next refresh.
      if (result.status === "rejected") continue;
      if (result.value === null) gone.add(missing[index]!.run_id);
      else fresh.push(result.value);
    }
    runs = mergeRuns(runs.filter((run) => !gone.has(run.run_id)), fresh);
    if (listErrorShown) showStatus(null);
    renderList();
    // A finished run's detail stops polling, so one that changed since (a
    // summary whose save the panel retried) is shown again from the list.
    const shown = detail?.run;
    const current = runs.find((run) => run.run_id === selectedId);
    if (shown && current && isTerminal(shown)
      && (current.state !== shown.state || current.summary_path !== shown.summary_path)) {
      clearDetailOnly();
      select(current.run_id);
    }
    // An intermediate list may lack the requested run. With no selection
    // yet, falling back would rewrite its hash before the queued read.
    if (!refreshAgain) syncSelectionToHash();
  } catch (error) {
    if (!closed) showListError(error);
  }
}

/** Runs `open` unless one is in flight (that one is returned) or the list
 * is already up: the first open, and a retry once the setup text or a
 * failed first list was shown. */
function reopen(): Promise<void> {
  if (closed || listed) return Promise.resolve();
  opening ??= open().finally(() => {
    opening = null;
  });
  return opening;
}

async function open(): Promise<void> {
  // The Brauser service runs on macOS (and Unix) only.
  if (/^win/i.test(navigator.platform ?? "")) {
    controls.hidden = true;
    showStatus(WINDOWS_TEXT);
    return;
  }
  showStatus("Waiting for the native host…");
  let ready: boolean;
  try {
    const config = await session.readHostConfig();
    noteConfigRevision = config.revision;
    ready = harnessReady({ agentStatus: config.agent_status });
  } catch (error) {
    showListError(error);
    return;
  }
  if (!ready && !(await runsMayExist(storage))) {
    showStatus(TASK_NEEDS_HARNESS_MESSAGE, { problem: true, settings: true });
    return;
  }
  showStatus("Loading tasks…");
  let complete: boolean;
  try {
    const walks = await Promise.all(HISTORY_KINDS.map((kind) => listAllTasks(session.host, kind)));
    runs = mergeRuns([], walks.flatMap((walk, index) => ofKind(walk.runs, HISTORY_KINDS[index]!)));
    complete = walks.every((walk) => walk.complete);
  } catch (error) {
    showListError(error);
    return;
  }
  // An unreadable dismissed list hides nothing.
  dismissed = new Set(await dismissedIds(storage).catch((): string[] => []));
  if (closed) return;
  listed = true;
  // An incomplete walk (the page cap reached with more still true) shows a
  // plain note, not an error: showStatus resets listErrorShown, so a later
  // refresh's "clear only a list error" rule leaves this note alone.
  showStatus(complete ? null : `Showing the newest ${runs.length} tasks. Older history could not be loaded.`);
  renderList();
  selectFromHash();
  schedulePoll();
}

statusSettings.addEventListener("click", openSettings);
searchInput.addEventListener("input", () => renderList());
statusFilter.addEventListener("change", () => renderList());
showDismissed.addEventListener("change", () => renderList());

list.addEventListener("keydown", (event: KeyboardEvent) => {
  if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
    event.preventDefault();
    if (order.length === 0) return;
    const at = selectedId === null ? -1 : order.indexOf(selectedId);
    const step = event.key === "ArrowDown" ? 1 : -1;
    const next = event.key === "Home" ? order[0]! : event.key === "End" ? order.at(-1)!
      : at === -1 ? order[0]! : order[Math.min(order.length - 1, Math.max(0, at + step))]!;
    select(next, true);
  } else if (event.key === "Enter" || event.key === " ") {
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
  // Shown again before a list ever worked (the setup text, or a failed
  // first list): try opening again.
  if (!listed) void reopen();
  else void refresh();
});

window.addEventListener("hashchange", () => {
  const wanted = parseRunHash(location.hash);
  if (wanted === null || wanted === selectedId) return;
  // Not listed yet: opening again selects the hash's run.
  if (!listed) {
    void reopen();
    return;
  }
  if (runs.some((run) => run.run_id === wanted)) {
    select(wanted);
    return;
  }
  // A run started since the list was read: read the first page, then look.
  // refresh coalesces this request even if an older list is in flight;
  // syncSelectionToHash follows the latest hash when the lists finish.
  void refresh();
});

// `openTasksPage` (task-handoff.ts) signals an already-open Tasks tab this
// way rather than relying only on the hash: a `chrome.tabs.update` that
// changes only the fragment does not reliably fire `hashchange` here, so
// without this, `syncSelectionToHash` (reached only from a `refresh`) is
// this page's only other way to notice — fine once a poll is already
// running, not fine when nothing is active to trigger one.
chrome.storage.onChanged.addListener((changes, areaName) => {
  if (areaName !== "session") return;
  const change = changes[SELECT_KEY];
  if (!change) return;
  const runId = parseSelectSignal(change.newValue);
  if (runId !== null) void selectFromSignal(runId);
});

window.addEventListener("pagehide", () => {
  closed = true;
  stopPolling();
  detail?.dispose();
  detail = null;
});

void reopen();
