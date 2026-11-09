import {
  PANEL_NOTICE_KEY,
  PANEL_NOTICE_MAX_AGE_MS,
  SITE_ACCESS_ORIGINS,
  PolicyLeaseCache,
  captureUrl,
  isHttpUrl,
  isPanelNotice,
  localTimestamp,
  type PanelNotice,
  type WorkerStatus,
} from "./model.js";
import { ConfigSession, describe, element, leaseFor, setupProblem, worker } from "./settings.js";
import { focusName, sameFocus } from "./focus-session.js";
import { evaluate } from "./adapter-templates.js";
import { HostError, PROTOCOL_VERSION, newRequestId } from "./native.js";
import { CaptureSitePanel } from "./capture-site-panel.js";
import { coverageSentence, nearMiss, siteCoverage } from "./site-coverage.js";
import type { AdapterRule } from "./starter-adapters.js";
import { APP_NAME, storageKey } from "./brand.js";
import { hostBuildText } from "./build-info.js";
import { initDiag } from "./diag.js";
import { SETUP_WARNING } from "./agent.js";
import { NEEDS_HARNESS_MESSAGE, harnessReady, summarizeFailureText, summarizeTab } from "./summarize.js";
import { RunCards, clearNotice, restoreRunCards, runsMayExist, showListFailure } from "./run-card.js";
import {
  TASK_NEEDS_HARNESS_MESSAGE, TASK_REPAIR_MESSAGE, cleanInstruction, startTask, taskFailureText,
} from "./task.js";
import { REUSE_KEY, openTasksPage, takeReuse } from "./task-handoff.js";
import { TaskTracker } from "./task-tracker.js";
import { armTaskNotifications, taskNoticeSignal } from "./task-notifications.js";
import { TaskRecall } from "./task-recall.js";
import { literalTaskInstruction } from "./task-commands.js";
import { readScopeText, recentSummaryText, summaryLine } from "./summary-text.js";
import { SharingDraftPanel } from "./sharing-draft-panel.js";
import { choiceKey, draftScope, focusHeading, gapFooter, gapLines, selectionPreview, type DraftSelection } from "./sharing-drafts.js";
import {
  artifactLook, capturePill, compactDuration, importanceLabel, importanceTone, pageLook, summaryTimeLabel,
} from "./panel-look.js";
import { NoteEditor } from "./note-editor.js";
import { NoteRecovery, type RecoveryText } from "./note-recovery.js";
import { chromeManifests, newerBuildOnDisk } from "./build-update.js";
import {
  ArtifactRefresher, deliveredSince, followsTabReplacement, followsTabUpdate,
} from "./artifact-refresh.js";
import type {
  ArtifactListItem,
  ArtifactListResult,
  ArtifactRefView,
  ArtifactView,
  ContainedArtifactView,
  OlderNote,
  RunView,
  SummaryView,
  WorklogReported,
  WorklogChoice,
  WorklogChoicesResult,
  WorklogFocusChoicesResult,
  FocusSession,
} from "../protocol/ts/generated.js";

initDiag("panel");

const status = element<HTMLDivElement>("status");
const focusCurrent = element<HTMLElement>("focus-current");
const focusForm = element<HTMLFormElement>("focus-form");
const focusInput = element<HTMLInputElement>("focus-name");
const focusSave = element<HTMLButtonElement>("focus-save");
const focusEnd = element<HTMLButtonElement>("focus-end");
const focusMessage = element<HTMLElement>("focus-message");
let focusBusy = false;
let focusDirty = false;
let focusDisplayed: FocusSession | null = null;
let focusExpected: FocusSession | null = null;

function renderFocus(): void {
  const active = session.status?.focus_session ?? null;
  if (!sameFocus(active, focusDisplayed)) {
    focusDisplayed = active;
    if (!focusDirty) {
      focusExpected = active;
      focusInput.value = active?.name ?? "";
    }
  }
  focusCurrent.textContent = active?.name ?? "None";
  focusSave.textContent = active ? "Rename session" : "Start session";
  focusEnd.hidden = active === null;
  focusInput.disabled = focusBusy;
  focusSave.disabled = focusBusy || !session.status || !focusInput.value.trim();
  focusEnd.disabled = focusBusy || !session.status;
}

async function saveFocus(end: boolean): Promise<void> {
  if (focusBusy || !session.status) return;
  let name: string | null;
  try { name = end ? null : focusName(focusInput.value); }
  catch (error) { focusMessage.textContent = describe(error); return; }
  focusBusy = true;
  renderFocus();
  try {
    await worker<WorkerStatus>({ kind: "change_focus", expected: end ? focusDisplayed : focusExpected, name });
    focusDirty = false;
    focusMessage.textContent = end ? "Session ended. New activity is ungrouped." : "New activity uses this session name.";
    await session.refreshStatus();
    focusDisplayed = session.status?.focus_session ?? null;
    focusExpected = focusDisplayed;
    focusInput.value = focusDisplayed?.name ?? "";
  } catch (error) {
    focusMessage.textContent = describe(error);
    await session.refreshStatus().catch(() => undefined);
    // A retry follows the current session shown above, preserving the draft.
    focusExpected = session.status?.focus_session ?? null;
  } finally {
    focusBusy = false;
    renderFocus();
  }
}

focusInput.addEventListener("input", () => {
  if (!focusDirty) focusExpected = focusDisplayed;
  focusDirty = true;
  renderFocus();
});
focusForm.addEventListener("submit", (event) => { event.preventDefault(); void saveFocus(false); });
focusEnd.addEventListener("click", () => { void saveFocus(true); });
const buildId = element<HTMLSpanElement>("build-id");
const hostBuild = element<HTMLSpanElement>("host-build");
const updateWarning = element<HTMLDivElement>("update-warning");
const updateMessage = element<HTMLParagraphElement>("update-message");
const updateReloadButton = element<HTMLButtonElement>("update-reload");

function showHostBuild(hostVersion: string): void {
  hostBuild.textContent = hostBuildText(buildId.textContent ?? "", hostVersion);
}
const settingsButton = element<HTMLButtonElement>("open-settings");
const setupWarning = element<HTMLDivElement>("setup-warning");
const setupReason = element<HTMLSpanElement>("setup-reason");
const setupSettingsButton = element<HTMLButtonElement>("setup-open-settings");
const siteAccessWarning = element<HTMLDivElement>("site-access-warning");
const siteAccessReenableButton = element<HTMLButtonElement>("site-access-reenable");
const pauseButton = element<HTMLButtonElement>("pause-capture");
const queueSummary = element<HTMLParagraphElement>("queue-summary");
const queueWarning = element<HTMLParagraphElement>("queue-warning");
const dismissButton = element<HTMLButtonElement>("dismiss-notices");
const replayButton = element<HTMLButtonElement>("replay");
const discardButton = element<HTMLButtonElement>("discard-pending");
const captureStrip = element<HTMLDetailsElement>("capture-strip");
const captureStripSummary = element<HTMLElement>("capture-strip-summary");
const captureStripAction = element<HTMLButtonElement>("capture-strip-action");
const capturePillElement = element<HTMLElement>("capture-pill");
const capturePillLabel = element<HTMLElement>("capture-pill-label");
const pageGlyph = element<HTMLElement>("page-glyph");

const pageTitle = element<HTMLParagraphElement>("page-title");
const pageUrl = element<HTMLParagraphElement>("page-url");
const pageUnusable = element<HTMLParagraphElement>("page-unusable");
const noteStatus = element<HTMLSpanElement>("note-status");
const noteEditor = element<HTMLDivElement>("note-editor");
const noteBody = element<HTMLTextAreaElement>("note-body");
const noteConflict = element<HTMLDivElement>("note-conflict");
const noteRecoveryList = element<HTMLUListElement>("note-recovery-list");
const noteRecoveryWarning = element<HTMLParagraphElement>("note-recovery-warning");
const artifactStatus = element<HTMLParagraphElement>("artifact-status");
const captureExplanation = element<HTMLDivElement>("capture-explanation");
const captureExplanationText = element<HTMLParagraphElement>("capture-explanation-text");
const captureExplanationAction = element<HTMLButtonElement>("capture-explanation-action");
const capturePolicy = new PolicyLeaseCache();
// The review the capture explanation's button opens in settings: an adapter
// identity ([origin, type, id]) and a starter rule, or null. Never a URL.
let pendingAdapterReview: { adapter: [string, string, string]; rule: AdapterRule | null } | null = null;
const artifactCard = element<HTMLElement>("artifact-card");
const artifactHeading = element<HTMLHeadingElement>("artifact-heading");
const artifactIdentity = element<HTMLParagraphElement>("artifact-identity");
const artifactMeasures = element<HTMLParagraphElement>("artifact-measures");
const artifactLinks = element<HTMLParagraphElement>("artifact-links");
const artifactIncomingList = element<HTMLUListElement>("artifact-incoming-list");
const artifactIncomingEmpty = element<HTMLParagraphElement>("artifact-incoming-empty");
const artifactWithin = element<HTMLParagraphElement>("artifact-within");
const artifactContained = element<HTMLDivElement>("artifact-contained");
const artifactContainedList = element<HTMLUListElement>("artifact-contained-list");
const artifactMode = element<HTMLSpanElement>("artifact-mode");
const artifactKindLabel = element<HTMLSpanElement>("artifact-kind-label");
const artifactMeasuresShort = element<HTMLSpanElement>("artifact-measures-short");
const artifactDetails = element<HTMLDetailsElement>("artifact-details");
const artifactIncomingCount = element<HTMLSpanElement>("artifact-incoming-count");
const artifactWhy = element<HTMLInputElement>("artifact-why");
const saveWhyButton = element<HTMLButtonElement>("save-why");
const whyPausedHint = element<HTMLParagraphElement>("why-paused-hint");
const olderNotes = element<HTMLDivElement>("older-notes");
const olderNoteList = element<HTMLUListElement>("older-note-list");
const olderNoteView = element<HTMLDivElement>("older-note-view");
const olderNoteViewTitle = element<HTMLParagraphElement>("older-note-view-title");
const olderNoteViewBody = element<HTMLTextAreaElement>("older-note-view-body");
const copyOlderNoteViewButton = element<HTMLButtonElement>("copy-older-note-view");
const closeOlderNoteViewButton = element<HTMLButtonElement>("close-older-note-view");
const preservedNotes = element<HTMLDetailsElement>("preserved-notes");
const preservedNotesStatus = element<HTMLParagraphElement>("preserved-notes-status");
const preservedNoteList = element<HTMLUListElement>("preserved-note-list");
const preservedNoteView = element<HTMLDivElement>("preserved-note-view");
const preservedNoteTitle = element<HTMLParagraphElement>("preserved-note-title");
const preservedNoteBody = element<HTMLTextAreaElement>("preserved-note-body");
let preservedNotesSeq = 0;
const pageTab = element<HTMLButtonElement>("tab-page");
const recentTab = element<HTMLButtonElement>("tab-recent");
const pageView = element<HTMLDivElement>("page-view");
const recentView = element<HTMLDivElement>("recent-view");
const recentType = element<HTMLSelectElement>("recent-type");
const recentTitle = element<HTMLInputElement>("recent-title");
const recentReading = element<HTMLSelectElement>("recent-reading");
const recentStatus = element<HTMLParagraphElement>("recent-status");
const recentList = element<HTMLDivElement>("recent-list");
const recentMore = element<HTMLButtonElement>("recent-more");
const recentReload = element<HTMLButtonElement>("recent-reload");
/** The worklog period's segments: one native radio group, so arrows move
 * the choice and the group is one tab stop. */
const WORKLOG_PERIODS = ["today", "yesterday", "week"] as const;
type WorklogPeriod = (typeof WORKLOG_PERIODS)[number];
const worklogPeriodRadios = new Map<WorklogPeriod, HTMLInputElement>(
  WORKLOG_PERIODS.map((period) => [period, element<HTMLInputElement>(`worklog-${period}`)]),
);
function worklogPeriod(): WorklogPeriod {
  return WORKLOG_PERIODS.find((period) => worklogPeriodRadios.get(period)!.checked) ?? "today";
}
const worklogText = element<HTMLElement>("worklog-text");
const copyWorklogButton = element<HTMLButtonElement>("copy-worklog");
const curateWorklog = element<HTMLInputElement>("worklog-curate");
const worklogChoicesArea = element<HTMLElement>("worklog-choices-area");
const worklogShareScope = element<HTMLSelectElement>("worklog-share-scope");
const worklogFocusArea = element<HTMLElement>("worklog-focus-area");
const worklogFocusSession = element<HTMLSelectElement>("worklog-focus-session");
const worklogFocusStatus = element<HTMLElement>("worklog-focus-status");
const worklogChoices = element<HTMLFieldSetElement>("worklog-choices");
const worklogMore = element<HTMLButtonElement>("worklog-more");
const worklogSelectAll = element<HTMLButtonElement>("worklog-select-all");
const worklogClear = element<HTMLButtonElement>("worklog-clear");
const worklogSelectionStatus = element<HTMLElement>("worklog-selection-status");
const worklogGaps = element<HTMLElement>("worklog-gaps");
const worklogGapStatus = element<HTMLElement>("worklog-gap-status");
const worklogGapList = element<HTMLElement>("worklog-gap-list");
const includeWorklogGaps = element<HTMLInputElement>("worklog-include-gaps");
const summarizeButton = element<HTMLButtonElement>("summarize");
const summarizeLabel = element<HTMLSpanElement>("summarize-label");
const pageActions = element<HTMLDivElement>("page-actions");
const runArea = element<HTMLDivElement>("run-area");
const pageSummary = element<HTMLDivElement>("page-summary");
const pageSummaryText = element<HTMLParagraphElement>("page-summary-text");
const pageSummaryOpen = element<HTMLButtonElement>("page-summary-open");
const pageSummaryTime = element<HTMLSpanElement>("page-summary-time");
const pageSummaryChips = element<HTMLDivElement>("page-summary-chips");
const pageSummaryHead = element<HTMLDivElement>("page-summary-head");
const taskBar = element<HTMLElement>("task-bar");
const taskForm = element<HTMLFormElement>("task-form");
const taskInput = element<HTMLTextAreaElement>("task-input");
const taskCommands = element<HTMLDetailsElement>("task-commands");
const taskCommand = element<HTMLSelectElement>("task-command");
const taskCommandInsert = element<HTMLButtonElement>("task-command-insert");
const taskCommandRefresh = element<HTMLButtonElement>("task-command-refresh");
const taskCommandStatus = element<HTMLElement>("task-command-status");
let commandRevision: string | null = null;
let commandIssue: string | null = null;
let commandLoading: Promise<void> | null = null;

const taskRunButton = element<HTMLButtonElement>("task-run");
const taskRunLabel = element<HTMLSpanElement>("task-run-label");
const taskUsePage = element<HTMLInputElement>("task-use-page");
const taskContext = element<HTMLParagraphElement>("task-context");
const taskHint = element<HTMLParagraphElement>("task-hint");
const taskHintText = element<HTMLSpanElement>("task-hint-text");
const taskHintSettings = element<HTMLButtonElement>("task-hint-settings");
const taskAnnounce = element<HTMLParagraphElement>("task-announce");
const taskRunArea = element<HTMLDivElement>("task-run-area");
const taskOpenButton = element<HTMLButtonElement>("task-open");

const session = new ConfigSession(render);
const taskRecall = new TaskRecall(session.host);
const runCards = new RunCards(runArea, {
  host: session.host,
  now: () => Date.now(),
  copy: (text) => navigator.clipboard.writeText(text),
  visible: () => document.visibilityState !== "hidden",
  storage: chrome.storage.local,
  openSettings,
  summarySaved: () => void refreshArtifact(true),
});
// Tasks show on the Tasks page; the panel only counts the active ones on
// its Tasks button and announces a finish (§5.7). It starts once the panel
// may list runs (a harness, or a run record), so it never wakes the service
// needlessly. Nothing here passes a task's instruction or report to diag
// (DESIGN.md §15).
let tracker: TaskTracker | null = null;
// What the tracker last reported, and its last list's error, if it failed.
let tasksRunning = 0;
let newestTaskId: string | null = null;
let trackerError: unknown = null;
// The task this panel last started, which Tasks ↗ opens.
let lastStartedRunId: string | null = null;
// This panel's own window, read once so a Reuse value meant for it (and not
// another window's panel) can be told apart. Resolves to null on an old
// Chrome that cannot answer, which then claims any window's Reuse value.
const panelWindowId: Promise<number | null> = chrome.windows.getCurrent()
  .then((current) => current.id ?? null, () => null);
taskNoticeSignal("task_notice_opened");
let captureSite: CaptureSitePanel | null = null;
// Suppress note loads only while capture policy can change their identity.
// Once the post-save reload starts, tab events must reach NoteEditor's token
// handling so a slow response cannot bind the editor to a page we have left.
let changingNoteIdentity = false;

// The header names the host process that answered. Another one, possibly
// from a rebuilt binary, starts on the next call after it exits.
session.host.onHostExit(() => { hostBuild.textContent = ""; });
session.host.onHostRestart(() => {
  void session.hello().then((hello) => showHostBuild(hello.host_version), () => undefined);
});
let busy = false;
let replaying = false;
let tickRunning = false;
let closed = false;
let reloadTimer: ReturnType<typeof setTimeout> | null = null;
// The worker delivers capture records and renews its lease on its own; the panel only
// polls to show the queue.
const STATUS_POLL_MS = 30_000;
const NOTE_SAVE_DEBOUNCE_MS = 1_000;
const WHY_WAIT_FOR_ARTIFACT = "Wait for this page's artifact to load, then save.";
// v2 since the warning was reworded for tasks, so it shows once more to
// everyone who saw the summary-only text.
const SUMMARIZE_WARNING_SEEN_KEY = storageKey("summarize_warning_seen_v2");

// --- Page area (§5.3, §5.7) --------------------------------------------------
// The panel follows the active tab. `tabRefreshSeq` lets a refresh whose tab
// query was overtaken by a newer one be ignored; the note itself is owned by
// `notes` (note-editor.ts), which serializes every load and save.
let tabRefreshSeq = 0;
// The tab the panel last resolved as active; only its updates matter.
let followedTabId: number | null = null;
let currentPage: { url: string; title: string; tabId: number | undefined } | null = null;
let currentArtifact: ArtifactView | null = null;
// Observe session changes here rather than in individual status readers:
// Pause, Discard and Dismiss can see a delivery before the next poll does.
let artifactHostConnected = false;
let artifactDeliveryGeneration: number | null = null;
let olderNoteEntries: OlderNote[] = [];
// Tracks the artifact the "Why I'm here" field was last rendered for, so an
// unrelated tab/title refresh for the SAME artifact never overwrites text the
// user has not saved yet; switching to a different artifact always resets it.
let whyArtifactId: string | null = null;
let whyEdited = false;
// The page URL the card was looked up for. A failed or skipped lookup for
// another page must not leave this card, and its artifact_id, in place.
let artifactPageUrl: string | null = null;
let savingWhy = false;
// The page whose saved summary the summary line shows, or null when hidden.
// Like the artifact card, a line for another page never stays in place.
let summaryPageUrl: string | null = null;
/** That summary's own file, which Open summary names. */
let summaryPagePath: string | null = null;
let summarizing = false;
// A task start is in flight; it gates only the task bar, as `summarizing`
// gates only Summarize & log.
let taskStarting = false;
// Each run area's latest line (`#run-area` for summaries, `#task-run-area`
// for tasks) and the tab it is about; another tab's line goes when the panel
// follows a different tab. One slot per area, so neither replaces the other.
const runLines = new Map<HTMLElement, { tabId: number; element: HTMLElement }>();
// What the page area shows for the followed tab; "site-access" means Chrome
// withdrew Brauser's access, so the task bar says so.
let pageState: "none" | "unusable" | "site-access" | "note" = "none";
// Whether the followed page had a selection when last checked, which the
// task bar's context line names. Only a boolean is read from the page.
let taskSelection: { tabId: number; url: string; selected: boolean } | null = null;
let selectionSeq = 0;
// The last worker notice shown, since one can arrive both live and stored.
let shownNotice: string | null = null;
let pendingTaskFocus: Extract<PanelNotice, { kind: "focus_task" }> | null = null;
let recentTimer: ReturnType<typeof setTimeout> | null = null;
// The shown Recent rows' Open summary buttons, disabled with the host.
let recentOpenButtons: HTMLButtonElement[] = [];
let recentLoadSeq = 0;
let recentResult: { scope: string; items: ArtifactListItem[]; page: ArtifactListResult["page"] } | null = null;
let recentPageLoading = false;
let recentWaitingForHost = false;
let worklogWaitingForHost = false;
let worklogLoadSeq = 0;
let worklogReport: WorklogReported | WorklogChoicesResult | WorklogFocusChoicesResult | null = null;
// A pending refresh disables Copy but must not discard the last validated
// pages: another refresh can supersede it before either response arrives.
let worklogValidated: { scope: string; report: WorklogChoicesResult } | null = null;
let worklogScope = "";
let worklogStorage = "";
let worklogPageLoading = false;
let worklogPageNotice = "";
const selectedWork = new Set<string>();
const worklogChoiceKey = choiceKey;
let sharingDraftPanel: SharingDraftPanel | null = null;
let draftPolicyPending = false;

function resetWorklogSelection(): void {
  worklogScope = "";
  selectedWork.clear();
  worklogFocusSession.value = "";
  worklogFocusSession.replaceChildren();
  worklogFocusSession.disabled = true;
  worklogFocusStatus.textContent = "";
  ++worklogLoadSeq;
  worklogReport = null;
  worklogValidated = null;
  worklogChoices.replaceChildren();
  worklogMore.hidden = true;
  worklogPageLoading = false;
  worklogPageNotice = "";
  worklogGaps.hidden = true;
  copyWorklogButton.disabled = true;
  worklogSelectAll.disabled = true;
  worklogClear.disabled = true;
  sharingDraftPanel?.invalidate();
}

// connected means a host can be reached, even after its port exits. Invalidate
// the sharing snapshot on the actual port lifecycle, including idle exits.
session.host.onHostExit(() => {
  invalidateRecent();
  recentWaitingForHost = false;
  recentStatus.textContent = "The host exited. Reload results to continue.";
  recentReload.hidden = false;
  resetWorklogSelection();
  worklogWaitingForHost = false;
  worklogSelectionStatus.textContent = "Open Recent again to reload choices.";
  worklogText.textContent = "The host exited. Open Recent again to generate a worklog.";
});

function show(message: string, warning = false): void {
  status.textContent = message;
  status.classList.toggle("warning", warning);
}

function showNoteStatus(message: string, warning = false): void {
  noteStatus.textContent = message;
  noteStatus.classList.toggle("error", warning);
}

function duration(seconds: number): string {
  if (seconds < 60) return seconds ? `${seconds} sec` : "0 min";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest ? `${hours} h ${rest} min` : `${hours} h`;
}

/** Shows `summary`, the page's saved summary, whether or not an adapter
 * recognizes the page; null hides the line. Agent text goes through
 * textContent only. */
function renderPageSummary(summary: SummaryView | null, pageUrl: string | null): void {
  summaryPageUrl = summary === null ? null : pageUrl;
  summaryPagePath = summary?.summary_path ?? null;
  pageSummary.hidden = summaryPageUrl === null;
  const shown = summary !== null && summaryPageUrl !== null ? summary : null;
  // A summary with an overview shows it as prose and its labels as chips;
  // an older one keeps its one-line description.
  pageSummaryText.textContent = shown === null ? "" : shown.overview ?? summaryLine(shown);
  pageSummaryTime.textContent = shown === null ? "" : summaryTimeLabel(shown.summarized_at);
  pageSummaryChips.replaceChildren();
  if (shown?.overview) {
    for (const [text, tone] of [
      [shown.document_type, "plain"], [shown.category, "plain"],
      [importanceLabel(shown.importance), importanceTone(shown.importance)],
    ] as const) {
      const chip = document.createElement("span");
      chip.className = "tag";
      chip.dataset.tone = tone;
      chip.textContent = text;
      pageSummaryChips.append(chip);
    }
  }
  const scope = readScopeText(shown?.read_scope);
  if (scope !== null) {
    const chip = document.createElement("span");
    chip.className = "tag";
    chip.textContent = scope;
    pageSummaryChips.append(chip);
  }
  renderSummarizeLabel();
}

/** The one Summarize & log button reads as the design's action for the
 * page: Save Page for a page no adapter recognizes, Summarize & log for an
 * artifact, and once a summary exists it moves into the Summary card as its
 * re-summarize icon (`data-place="card"`, the label kept for assistive
 * technology). It stays one control, so its id, focus and state carry over. */
function renderSummarizeLabel(): void {
  const summarized = summaryPageUrl !== null;
  summarizeLabel.textContent = summarized ? "Summarize again" : artifactPageUrl === null ? "Save Page" : "Summarize & log";
  const place = summarized ? "card" : "actions";
  if (summarizeButton.dataset.place !== place) {
    const hadFocus = document.activeElement === summarizeButton;
    if (place === "card") pageSummaryHead.append(summarizeButton);
    else pageActions.prepend(summarizeButton);
    summarizeButton.dataset.place = place;
    if (hadFocus) summarizeButton.focus();
  }
  if (summarizeButton.title !== NEEDS_HARNESS_MESSAGE) summarizeButton.title = summarizeTooltip();
}

/** The icon-only form names itself on hover too. */
function summarizeTooltip(): string {
  return summarizeButton.dataset.place === "card" ? "Summarize again" : "";
}

/** The page tile: the artifact type's look when recognized, else the host's. */
function renderPageGlyph(): void {
  const look = currentArtifact !== null && artifactPageUrl !== null
    ? artifactLook(currentArtifact.artifact_type)
    : pageLook(currentPage?.url ?? null);
  pageGlyph.textContent = look.glyph;
  pageGlyph.dataset.hue = look.hue;
}

async function openPageSummary(): Promise<void> {
  const url = summaryPageUrl;
  if (url === null || currentPage?.url !== url) return;
  try {
    await session.openSummary(url, summaryPagePath);
  } catch (error) {
    show(`Could not open the summary: ${describe(error)}`, true);
  }
}

async function openRecentSummary(url: string, summaryPath: string): Promise<void> {
  try {
    await session.openSummary(url, summaryPath);
  } catch (error) {
    recentStatus.textContent = `Could not open the summary: ${describe(error)}`;
  }
}

/** Whether an artifact's history holds no measured activity: a summary
 * moves first and last seen but adds no time, opens, or days. */
function noActivityMeasured(artifact: ArtifactView): boolean {
  return artifact.focused_seconds === 0 && artifact.background_seconds === 0 &&
    artifact.opens === 0 && artifact.days_opened === 0;
}

// A null/empty container field hides its element along with its contents, so
// a later artifact that lacks one never shows a stale card from the artifact
// before it.
function renderWithin(within: ArtifactRefView | null): void {
  artifactWithin.replaceChildren();
  artifactWithin.hidden = within === null;
  if (!within) return;
  const label = document.createElement("span");
  label.textContent = "In ";
  const link = document.createElement("a");
  link.href = within.url;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  link.textContent = within.title;
  artifactWithin.append(label, link);
}

function containedRow(item: ContainedArtifactView): HTMLLIElement {
  const row = document.createElement("li");
  const link = document.createElement("a");
  link.href = item.url;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  link.textContent = `${item.title} · ${duration(item.focused_seconds)}`;
  row.append(link);
  return row;
}

function renderContained(contained: readonly ContainedArtifactView[]): void {
  artifactContainedList.replaceChildren();
  artifactContained.hidden = contained.length === 0;
  if (!contained.length) return;
  artifactContainedList.append(...contained.map(containedRow));
}

const NAVIGATION_LABELS: Readonly<Record<string, string>> = {
  link: "Link", typed: "Address", auto_bookmark: "Bookmark",
  auto_subframe: "Automatic frame", manual_subframe: "Manual frame",
  generated: "Generated", start_page: "Start page", form_submit: "Form submission",
  reload: "Reload", keyword: "Keyword", keyword_generated: "Generated keyword",
};

function renderIncoming(trails: ArtifactView["incoming_trails"]): void {
  artifactIncomingCount.textContent = trails.length ? String(trails.length) : "";
  artifactIncomingList.replaceChildren();
  artifactIncomingList.hidden = trails.length === 0;
  artifactIncomingEmpty.hidden = trails.length !== 0;
  for (const trail of trails) {
    const row = document.createElement("li");
    const label = document.createElement("span");
    label.textContent = `${NAVIGATION_LABELS[trail.transition]} navigation from `;
    const link = document.createElement("a");
    link.href = trail.source.url;
    link.target = "_blank";
    link.rel = "noopener noreferrer";
    link.textContent = trail.source.title || trail.source.artifact_id;
    const date = document.createElement("span");
    date.textContent = ` · last recorded ${trail.last_seen}`;
    row.append(label, link, date);
    artifactIncomingList.append(row);
  }
}

function renderArtifact(artifact: ArtifactView | null, recognized: boolean, pageUrl: string | null): void {
  currentArtifact = artifact;
  artifactPageUrl = artifact ? pageUrl : null;
  artifactCard.hidden = artifact === null;
  artifactDetails.hidden = artifact === null;
  renderPageGlyph();
  renderSummarizeLabel();
  artifactStatus.textContent = pageUrl === null ? "" : recognized ? "● Artifact recognized" : "○ Not an artifact";
  if (!artifact) {
    whyArtifactId = null;
    whyEdited = false;
    renderWithin(null);
    renderContained([]);
    renderIncoming([]);
    updateControls();
    return;
  }
  artifactHeading.textContent = artifact.title;
  artifactIdentity.textContent = `${artifact.artifact_type} · ${artifact.artifact_id}`;
  artifactMode.textContent = artifact.last_mode ?? "";
  artifactMode.hidden = artifact.last_mode === null;
  artifactKindLabel.textContent = artifactLook(artifact.artifact_type).label;
  artifactMeasuresShort.textContent = !artifact.first_seen || noActivityMeasured(artifact)
    ? ""
    : `${compactDuration(artifact.focused_seconds)} focused · ${artifact.opens} open${artifact.opens === 1 ? "" : "s"}`;
  // A restored tab can deliver focused/background time with no cold open,
  // so days_opened alone cannot distinguish real delivered activity from
  // the no-record-yet placeholder; first_seen is empty only for that
  // placeholder (native.rs's get_artifact fallback).
  artifactMeasures.textContent = !artifact.first_seen
    ? "Recognized; no delivered activity yet."
    : noActivityMeasured(artifact)
    ? "No activity measured yet"
    : `Focused ${duration(artifact.focused_seconds)} over ${artifact.days_opened} day${artifact.days_opened === 1 ? "" : "s"} · ${duration(artifact.background_seconds)} background · last ${artifact.last_seen}`;
  artifactLinks.textContent = artifact.linked_count
    ? `${artifact.linked_count} linked artifact${artifact.linked_count === 1 ? "" : "s"}`
    : "No linked artifacts yet.";
  renderWithin(artifact.within);
  renderContained(artifact.contained);
  renderIncoming(artifact.incoming_trails);
  if (artifact.artifact_id !== whyArtifactId) {
    whyArtifactId = artifact.artifact_id;
    whyEdited = false;
  }
  // A refresh triggered by an unrelated tab/title event for this same
  // artifact must never clobber context the user is still typing.
  if (!whyEdited) {
    artifactWhy.value = artifact.why ?? "";
  }
  updateControls();
}

/** Reports that `page`'s artifact could not be read. The card and any
 * unsaved "Why I'm here" draft stay when they belong to this page, so a
 * transient failure or host outage loses nothing. Another page's card goes,
 * since its artifact_id is not this page's; the draft stays in the field
 * until a lookup shows a different artifact. */
function showArtifactUnavailable(page: { url: string }, reason: string): void {
  if (artifactPageUrl !== page.url) {
    currentArtifact = null;
    artifactPageUrl = null;
    artifactCard.hidden = true;
    artifactDetails.hidden = true;
    renderPageGlyph();
  }
  if (summaryPageUrl !== page.url) renderPageSummary(null, null);
  artifactStatus.textContent = `Artifact status unavailable: ${reason}`;
  updateControls();
}

const artifactRefresher = new ArtifactRefresher(
  (page) => session.host.call({
    type: "get_artifact",
    protocol_version: PROTOCOL_VERSION,
    request_id: newRequestId(),
    url: page.url,
    title: page.title,
  }, "artifact_result"),
  () => currentPage,
  (page, response) => {
    renderPageSummary(response.summary, page.url);
    renderArtifact(response.artifact, response.recognized, page.url);
  },
  (page, error) => showArtifactUnavailable(page, describe(error)),
);

/** Pass `force` when the page's history may have changed (context saved,
 * capture delivered, settings changed); otherwise a page already shown is
 * not asked about again. */
function refreshArtifact(force = false): Promise<void> {
  const page = currentPage;
  if (page && !session.connected) {
    // Keep this page's card and any unsaved draft through a host outage.
    artifactRefresher.clear();
    showArtifactUnavailable(page, "the native host is not connected");
    return Promise.resolve();
  }
  if (!page || !session.config?.storage) {
    artifactRefresher.clear();
    renderPageSummary(null, null);
    renderArtifact(null, false, null);
    return Promise.resolve();
  }
  return artifactRefresher.refresh(force);
}

// Local wall-clock time with its numeric UTC offset, matching the worker's
// offset-qualified activity timestamps, so explicit context lands in the
// same local day's log as the activity around it rather than always UTC's.
function timestampSeconds(): string {
  return localTimestamp(new Date());
}

async function saveWhy(): Promise<void> {
  const page = currentPage;
  const artifact = currentArtifact;
  const text = artifactWhy.value.trim();
  if (savingWhy) return;
  const blocked = whySaveBlocked();
  if (blocked || !page || !artifact) {
    show(blocked ?? WHY_WAIT_FOR_ARTIFACT, true);
    return;
  }
  savingWhy = true;
  updateControls();
  try {
    const response = await session.host.call({
      type: "record_why",
      protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(),
      event: {
        event_id: crypto.randomUUID(),
        artifact_id: artifact.artifact_id,
        url: page.url,
        occurred_at: timestampSeconds(),
        incognito: false,
        text,
      },
    }, "why_recorded");
    if (response.outcome !== "persisted") throw new Error(response.reason ?? response.outcome);
    // Only clear the dirty flag if nothing changed while this save was in
    // flight; a newer, still-unsaved draft (or a switch to a different
    // artifact) must not be overwritten by the refresh below.
    if (currentArtifact === artifact && artifactWhy.value.trim() === text) {
      whyEdited = false;
    }
    await refreshArtifact(true);
    show("Artifact context saved.");
  } catch (error) {
    show(`Could not save artifact context: ${describe(error)}`, true);
  } finally {
    savingWhy = false;
    updateControls();
  }
}

/** Why capture keeps "Why I'm here" from saving, or null. The host records
 * nothing while paused. A local pause the host has not confirmed counts too:
 * the user turned capture off, though the host would still record, and
 * finishing it takes Pause capture rather than Resume. */
function whyPausedReason(): string | null {
  if (currentPage && session.status?.pending_removals.includes(new URL(currentPage.url).origin)) {
    return "This site's removal is pending, so this can't be saved. Open settings to finish removing the site.";
  }
  if (session.config?.capture_enabled === false) {
    return "Capture is paused, so this can't be saved. Use Resume capture in settings to turn it back on.";
  }
  if (session.status?.pause_pending === true) {
    if (!session.connected) {
      return "Capture is paused locally, so this can't be saved. The native host is unavailable. Reopen the panel to finish saving the pause.";
    }
    if (changing() || session.configIssue || session.adapterIssue) {
      return "Capture is paused locally, so this can't be saved. Wait for the panel to finish, or open settings if the configuration needs repair.";
    }
    return "Capture is paused locally, so this can't be saved. Use Pause capture to finish saving this setting in the host.";
  }
  return null;
}

/** Why "Why I'm here" cannot be saved right now, or null when it can. An
 * unreachable host does not block Save: the call itself restarts the host,
 * so a draft kept through an outage can be saved once it returns, and a
 * failure shows its reason. */
function whySaveBlocked(): string | null {
  if (!currentPage || !currentArtifact || artifactPageUrl !== currentPage.url) return WHY_WAIT_FOR_ARTIFACT;
  const paused = whyPausedReason();
  if (paused) return paused;
  if (!artifactWhy.value.trim()) return "Type why you're here, then save.";
  if (changing()) return "Wait for the panel to finish its current task, then save.";
  return null;
}

function clearPreservedNoteView(): void {
  preservedNoteView.hidden = true;
  preservedNoteTitle.textContent = "";
  preservedNoteBody.value = "";
}

function resetPreservedNotes(): void {
  preservedNotesSeq++;
  preservedNotes.open = false;
  preservedNoteList.replaceChildren();
  preservedNotesStatus.textContent = "";
  clearPreservedNoteView();
}

async function listPreservedNotes(): Promise<void> {
  const page = currentPage;
  const seq = ++preservedNotesSeq;
  preservedNoteList.replaceChildren();
  clearPreservedNoteView();
  if (!page || !preservedNotes.open) {
    preservedNotesStatus.textContent = "";
    return;
  }
  const current = () => seq === preservedNotesSeq && preservedNotes.open && currentPage?.url === page.url;
  preservedNotesStatus.textContent = "Looking for preserved notes…";
  try {
    const response = await session.host.call({
      type: "list_preserved_notes", protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(), url: page.url,
    }, "preserved_notes_listed");
    if (!current()) return;
    preservedNotesStatus.textContent = response.notes.length
      ? "Choose a note to view read only."
      : "No matching preserved artifact notes found in this bounded search.";
    for (const entry of response.notes) {
      const item = document.createElement("li");
      const label = document.createElement("span");
      label.textContent = `${entry.title || entry.canonical_url} — ${entry.key.value} `;
      const button = document.createElement("button");
      button.type = "button";
      button.className = "secondary compact";
      button.textContent = "View read only";
      button.addEventListener("click", () => void viewPreservedNote(entry, page.url));
      item.append(label, button);
      preservedNoteList.append(item);
    }
  } catch (error) {
    if (current()) preservedNotesStatus.textContent = `Could not find preserved notes: ${describe(error)}`;
  }
}

async function viewPreservedNote(entry: OlderNote, url: string): Promise<void> {
  if (!preservedNotes.open || currentPage?.url !== url) return;
  const seq = ++preservedNotesSeq;
  const current = () => seq === preservedNotesSeq && preservedNotes.open && currentPage?.url === url;
  clearPreservedNoteView();
  preservedNotesStatus.textContent = "Opening preserved note…";
  try {
    const response = await session.host.call({
      type: "view_preserved_note", protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(), url, key: entry.key,
    }, "note_loaded");
    if (!current()) return;
    preservedNoteTitle.textContent = `${response.title || entry.canonical_url} — ${entry.key.value}`;
    preservedNoteBody.value = response.body;
    preservedNoteView.hidden = false;
    preservedNotesStatus.textContent = "Read only. This page's editable note is unchanged.";
  } catch (error) {
    if (current()) preservedNotesStatus.textContent = `Could not open preserved note: ${describe(error)}`;
  }
}

function renderOlderNotes(entries: readonly OlderNote[], canUseAsArtifactNote: boolean): void {
  olderNoteEntries = entries.map((entry) => ({ ...entry, key: { ...entry.key } }));
  olderNoteList.replaceChildren();
  olderNotes.hidden = entries.length === 0;
  closeOlderNoteView();
  for (const [index, entry] of entries.entries()) {
    const item = document.createElement("li");
    const label = document.createElement("span");
    label.textContent = `${entry.title || entry.canonical_url} — ${entry.canonical_url} `;
    const viewButton = document.createElement("button");
    viewButton.type = "button";
    viewButton.className = "secondary compact";
    viewButton.dataset.olderIndex = String(index);
    viewButton.dataset.olderAction = "view";
    viewButton.textContent = "View";
    item.append(label, viewButton);
    if (canUseAsArtifactNote) {
      const useButton = document.createElement("button");
      useButton.type = "button";
      useButton.className = "secondary compact";
      useButton.dataset.olderIndex = String(index);
      useButton.dataset.olderAction = "use";
      useButton.textContent = "Use as artifact note";
      item.append(useButton);
    }
    olderNoteList.append(item);
  }
}

async function useOlderNote(index: number): Promise<void> {
  const page = currentPage;
  const entry = olderNoteEntries[index];
  if (!page || !entry) return;
  try {
    await notes.flush();
    await session.host.call({
      type: "use_artifact_note",
      protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(),
      url: page.url,
      source_url: entry.canonical_url,
      expected_revision: entry.revision,
    }, "note_loaded");
    await notes.reload();
    show("Older note is now the shared artifact note.");
  } catch (error) {
    showNoteStatus(`Could not use that note: ${describe(error)}`, true);
  }
}

// Opens an older URL-keyed note read-only, without migrating it, so it stays
// reachable even after a sibling note has already become the shared artifact
// note (at which point its own URL would otherwise resolve to that note).
async function viewOlderNote(index: number): Promise<void> {
  const page = currentPage;
  const entry = olderNoteEntries[index];
  if (!page || !entry) return;
  try {
    const response = await session.host.call({
      type: "view_older_note",
      protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(),
      url: page.url,
      source_url: entry.canonical_url,
    }, "note_loaded");
    olderNoteViewTitle.textContent = response.title || entry.canonical_url;
    olderNoteViewBody.value = response.body;
    olderNoteView.hidden = false;
  } catch (error) {
    showNoteStatus(`Could not open that note: ${describe(error)}`, true);
  }
}

function closeOlderNoteView(): void {
  olderNoteView.hidden = true;
  olderNoteViewTitle.textContent = "";
  olderNoteViewBody.value = "";
}

function copyOlderNoteView(): void {
  void navigator.clipboard.writeText(olderNoteViewBody.value).catch((error: unknown) => {
    showNoteStatus(`Could not copy: ${describe(error)}`, true);
  });
}

function changing(): boolean {
  return busy || replaying || tickRunning || captureSite?.saving === true;
}

/** Explain automatic capture from the host snapshot and local narrowing
 * state. Use the worker's compiler/evaluator; page links never enter this
 * decision. Recognition and history on the artifact card remain host-owned. */
function renderCaptureExplanation(): void {
  pendingAdapterReview = null;
  captureExplanationAction.dataset.action = "settings";
  captureExplanationAction.textContent = "Open settings";
  captureExplanationAction.disabled = changing();
  const config = session.config;
  if (!currentPage || !config || !session.revision) {
    captureExplanation.hidden = true;
    captureExplanationText.textContent = "";
    return;
  }
  const state = session.status;
  let reason: string;
  if (session.configIssue || session.adapterIssue) {
    reason = "Capture is unavailable until the configuration is repaired in settings.";
  } else if (!config.storage) {
    reason = "Capture needs a home folder. Choose one in settings.";
  } else if (state?.pending_removals.includes(new URL(currentPage.url).origin)) {
    reason = "This site's removal is pending. Capture stays off here until settings finishes the removal.";
  } else if (!config.capture_enabled) {
    reason = "Capture is paused. Resume capture in settings to remember work on matching pages.";
  } else if (state?.pause_pending) {
    reason = "Capture is paused in this browser. Open settings to finish saving the pause.";
  } else if (!captureUrl(currentPage.url)) {
    reason = "Pages whose addresses include sign-in credentials are excluded from capture.";
  } else if (!session.connected || !state || state.policy_expires_at === null || state.policy_expires_at <= Date.now()) {
    reason = "Capture settings are unavailable or out of date in this browser. Open settings to refresh them.";
  } else {
    const lease = capturePolicy.restore(leaseFor(config, session.revision));
    const compiled = lease && capturePolicy.compiledFor(lease);
    if (!lease || !compiled) {
      reason = "Capture rules could not be read. Repair them in settings.";
    } else {
      const result = evaluate(compiled, currentPage.url, currentPage.title);
      if (result.match) {
        captureExplanation.hidden = true;
        captureExplanationText.textContent = "";
        return;
      }
      const coverage = siteCoverage(lease.adapters.artifacts, currentPage.url);
      switch (result.reason) {
        case "out_of_scope": {
          const sentence = coverage && coverageSentence(coverage, "out_of_scope");
          reason = sentence
            ? `${sentence} If it should be, review this site's scope in settings.`
            : "This page is outside the workspaces or accounts chosen for capture. Review this site's scope in settings.";
          break;
        }
        case "no_adapter":
          reason = "This site has no capture rules. Review Capture Site to choose which pages to remember.";
          captureExplanationAction.textContent = "Review Capture Site";
          captureExplanationAction.dataset.action = "capture-site";
          captureExplanationAction.disabled ||= element<HTMLButtonElement>("capture-site").disabled;
          break;
        case "no_rule": {
          const miss = nearMiss(lease.adapters.artifacts, currentPage.url);
          const adapter = miss && lease.adapters.artifacts[miss.adapter];
          const identity = adapter && typeof adapter.origin === "string" && typeof adapter.type === "string" &&
            typeof adapter.id === "string"
            ? [adapter.origin, adapter.type, adapter.id] as [string, string, string]
            : null;
          // An offered near-miss is close to a captured type, so its sentence
          // names the missing rule instead of saying the page is not one.
          const sentence = coverage
            ? coverageSentence(coverage, miss && identity ? "near_miss" : "no_rule")
            : "This page does not match this site's capture rules.";
          if (miss?.kind === "starter_rule" && identity) {
            reason = `${sentence} The current ${miss.label} starter has a rule for pages like this one.`;
            pendingAdapterReview = { adapter: identity, rule: miss.rule };
            captureExplanationAction.textContent = "Review rule";
          } else if (miss?.kind === "identifier" && identity) {
            // The ID's rule may cover this route and fail on another
            // condition, so the line does not say which part is missing.
            reason = `${sentence} The address carries an ID Brauser recognizes, so a rule could cover it.`;
            pendingAdapterReview = { adapter: identity, rule: null };
            captureExplanationAction.textContent = "Edit rules";
          } else {
            reason = coverage
              ? `${sentence} Only matching pages are remembered.`
              : "This page does not match this site's capture rules. Only matching pages are remembered. Review the rules in settings.";
          }
          if (pendingAdapterReview) captureExplanationAction.dataset.action = "adapter-review";
          break;
        }
        default:
          reason = "This page's address cannot safely match the capture rules. Review the rules in settings.";
      }
    }
  }
  if (captureExplanationText.textContent !== reason) captureExplanationText.textContent = reason;
  captureExplanation.hidden = false;
}

function updateControls(): void {
  captureSite?.render(currentPage, changing());
  renderCaptureExplanation();
  const connected = session.connected;
  const config = session.config;
  const state = session.status;
  // While the host configuration needs repair, the host accepts no save or
  // visit, so pausing and sending wait for the repair in settings.
  const repairing = session.configIssue !== null || session.adapterIssue !== null;
  pauseButton.disabled = !connected || changing() || repairing || config?.capture_enabled !== true;
  replayButton.disabled = !connected || changing() || repairing || config?.capture_enabled !== true ||
    state?.pause_pending === true || (state?.queued ?? 0) === 0;
  discardButton.disabled = changing() || (state?.queued ?? 0) === 0;
  renderCaptureControls();
  dismissButton.disabled = changing() || !state || (!state.last_error &&
    !state.overflow_count && !state.rejected_count);
  saveWhyButton.disabled = savingWhy || whySaveBlocked() !== null;
  const ready = harnessReady(session);
  summarizeButton.hidden = currentPage === null;
  summarizeButton.disabled = !ready || summarizing || currentPage?.tabId === undefined;
  summarizeButton.title = ready ? summarizeTooltip() : NEEDS_HARNESS_MESSAGE;
  renderTaskBar();
  pageSummaryOpen.disabled = !connected;
  for (const open of recentOpenButtons) open.disabled = !connected;
  const paused = whyPausedReason();
  whyPausedHint.hidden = paused === null;
  if (paused !== null) whyPausedHint.textContent = paused;
}

/** Whether capture is switched on in a healthy host configuration. */
function captureIsOn(): boolean {
  return session.config?.capture_enabled === true && session.configIssue === null &&
    session.adapterIssue === null && session.status?.pause_pending !== true;
}

/** The header pill and the strip's one quick action (DESIGN.md §5.7). The
 * action is never the destructive pause: it sends pending records while
 * capture is on, or opens settings to resume a paused capture. */
function renderCaptureControls(): void {
  const state = session.status;
  const pill = capturePill({
    captureOn: captureIsOn(),
    queued: state?.queued ?? 0,
    deliveryProblem: Boolean(state?.retry_error || state?.host_error || state?.last_error),
  });
  capturePillElement.dataset.tone = pill.tone;
  capturePillElement.title = pill.detail;
  capturePillLabel.textContent = pill.label;
  const paused = session.connected && session.config !== null && session.config?.capture_enabled === false;
  if (captureIsOn() && (state?.queued ?? 0) > 0) {
    captureStripAction.hidden = false;
    captureStripAction.dataset.action = "send";
    captureStripAction.textContent = "Send now";
    captureStripAction.disabled = replayButton.disabled;
  } else if (paused) {
    captureStripAction.hidden = false;
    captureStripAction.dataset.action = "settings";
    captureStripAction.textContent = "Resume in settings";
    captureStripAction.disabled = false;
  } else {
    captureStripAction.hidden = true;
    captureStripAction.dataset.action = "";
  }
}

function renderSetupWarning(): void {
  const problem = setupProblem(session.config, session.configIssue ?? session.adapterIssue);
  setupWarning.hidden = problem === null;
  setupReason.textContent = problem ?? "";
}

function renderQueue(state: WorkerStatus): void {
  queueSummary.textContent =
    `${state.live_activity} live hour total${state.live_activity === 1 ? "" : "s"} · ` +
    `${state.queued} pending capture record${state.queued === 1 ? "" : "s"}.`;
  const captureOn = captureIsOn();
  captureStripSummary.textContent =
    `${captureOn ? "●" : "○"} Capture ${captureOn ? "on" : "off"} · ` +
    `${state.live_activity} live · ${state.queued} pending`;
  const messages: string[] = [];
  if (state.overflow_count) {
    const range = state.overflow_first_at && state.overflow_last_at
      ? ` from ${state.overflow_first_at} through ${state.overflow_last_at}`
      : "";
    messages.push(`${state.overflow_count} completed activity hours${range} could not be buffered; this will be marked in the log on the next delivery.`);
  }
  if (state.rejected_count) messages.push(`${state.rejected_count} capture records were rejected by host policy.`);
  if (state.last_error) messages.push(state.last_error);
  if (state.retry_error) messages.push(`Retrying: ${state.retry_error}`);
  if (state.host_error) messages.push(`The native host could not be reached in the background: ${state.host_error}`);
  if (state.pause_pending && session.config?.capture_enabled) {
    messages.push("A local pause is active until the host confirms capture is off.");
  }
  if (state.pending_removals.length) {
    messages.push("Some sites stay off until the settings page saves their removal in the host. Open settings to finish.");
  }
  queueWarning.textContent = messages.join(" ");
  // Open the strip on its own once there is something worth surfacing.
  if (messages.length && !captureStrip.open) captureStrip.open = true;
}

function render(): void {
  const connected = session.connected;
  const storage = JSON.stringify([session.config?.storage ?? null, session.revision]);
  if (storage !== worklogStorage || (!connected && artifactHostConnected)) {
    worklogStorage = storage;
    resetWorklogSelection();
    worklogText.textContent = connected ? "Open Recent to generate a worklog." : "Waiting for the host…";
    invalidateRecent();
    if (!recentView.hidden) {
      void loadWorklog();
      void loadRecent();
    }
  }
  const reconnected = connected && !artifactHostConnected;
  const generation = session.status?.delivered_batches ?? null;
  const delivered = generation !== null && deliveredSince(artifactDeliveryGeneration, generation);
  // Advance before asking the host: its response can notify us again.
  artifactHostConnected = connected;
  artifactDeliveryGeneration = generation;
  renderSetupWarning();
  renderFocus();
  if (session.status) renderQueue(session.status);
  updateControls();
  sharingDraftPanel?.render();
  retryWaitingHistory();
  if (!closed && (reconnected || delivered)) void refreshArtifact(true);
  if (!closed && delivered && !recentView.hidden && curateWorklog.checked) void loadWorklog();
}

// History needs only the host configuration, not the worker policy, so
// Recent opened before the host answered loads as soon as the session
// connects, even while policy installation is still pending or later fails.
function retryWaitingHistory(): void {
  if (!session.connected || recentView.hidden) return;
  if (recentWaitingForHost) void loadRecent();
  if (worklogWaitingForHost) void loadWorklog();
}

/** Open settings on the review of one adapter's rules. The hash carries only
 * config and catalog data; settings re-reads the adapter and saves nothing
 * until its editor's Save. */
function openAdapterReview(review: { adapter: [string, string, string]; rule: AdapterRule | null }): void {
  const url = chrome.runtime.getURL(`options.html#adapter-review=${encodeURIComponent(JSON.stringify(review))}`);
  void chrome.tabs.create({ url }).catch((error: unknown) => show(`Could not open settings: ${describe(error)}`, true));
}

function openSettings(): void {
  void chrome.runtime.openOptionsPage().catch((error: unknown) => {
    // Chrome reads an unpacked extension's manifest only when it loads the
    // extension, but serves rebuilt pages from disk. A panel from a newer build
    // can therefore run against a manifest with no settings page.
    if (!chrome.runtime.getManifest().options_ui) {
      show(`Chrome is still running an older ${APP_NAME} build. Click Reload on ${APP_NAME} in chrome://extensions, then reopen this panel.`, true);
    } else {
      show(`Could not open settings: ${describe(error)}`, true);
    }
  });
}

async function reloadConfig(): Promise<void> {
  const response = await session.reload();
  draftPolicyPending = false;
  sharingDraftPanel?.render();
  if (response.config_issue || response.adapter_issue) {
    show(response.config_issue ?? response.adapter_issue ?? "Adapter policy is invalid.", true);
  }
  else show(session.config!.capture_enabled ? "" : "Capture is paused. Resume it in settings.");
  if (session.status?.pause_pending && session.config?.capture_enabled &&
      !session.configIssue && !session.adapterIssue) {
    // This status can outlive the connection. The live Why hint names the
    // control only while the host is reachable.
    show("Capture is paused locally; its host setting has not been saved yet.", true);
  }
}

// The settings page saved a different revision. Wait for this panel's own
// work to finish, then read the host again.
function scheduleReload(): void {
  if (reloadTimer !== null || closed) return;
  reloadTimer = setTimeout(() => {
    reloadTimer = null;
    if (changing()) {
      scheduleReload();
      return;
    }
    busy = true;
    updateControls();
    const previousRevision = session.revision;
    void reloadConfig()
      .catch((error: unknown) => show(`Could not refresh settings: ${describe(error)}`, true))
      .finally(() => {
        busy = false;
        updateControls();
        // Another page's adapter edit may have changed this URL's note key.
        // Reload through the editor's save/load queue so pending edits retain
        // the old key and any key_changed refusal remains in copy-out.
        if (session.revision !== previousRevision) void notes.reload();
        else void notes.retry();
        void refreshArtifact(true);
        // A policy signal can require a reload even when the host's actual
        // revision is unchanged. Restore the invalidated history views too.
        if (!closed && !recentView.hidden && worklogReport === null) void loadWorklog();
        if (!closed && !recentView.hidden && recentResult === null) void loadRecent();
      });
  }, 250);
}

/** Ask the worker to send pending capture records now instead of at its next sync. */
async function replayVisits(): Promise<void> {
  if (closed || changing()) return;
  replaying = true;
  updateControls();
  try {
    await worker<WorkerStatus>({ kind: "sync_now" });
    // Delivered activity changes the current artifact's totals.
    void refreshArtifact(true);
  } catch (error) {
    show(`Capture delivery stopped: ${describe(error)}`, true);
  } finally {
    replaying = false;
    await session.refreshStatus().catch((error: unknown) => show(describe(error), true));
    updateControls();
  }
}

async function panelTick(): Promise<void> {
  if (closed || changing() || !session.config) return;
  tickRunning = true;
  updateControls();
  try {
    await session.refreshStatus();
  } catch (error) {
    show(`Panel refresh failed: ${describe(error)}`, true);
  } finally {
    tickRunning = false;
    updateControls();
  }
}

async function pauseCapture(): Promise<void> {
  if (!session.config || session.configIssue || session.adapterIssue) return;
  busy = true;
  updateControls();
  try {
    await session.pauseCapture();
    show("Capture paused. Pending capture records cleared. Use Resume capture in settings to turn it back on.");
  } catch (error) {
    show(`Could not pause capture: ${describe(error)}`, true);
  } finally {
    busy = false;
    await session.refreshStatus().catch((error: unknown) => show(describe(error), true));
    updateControls();
  }
}

async function discardPendingVisits(): Promise<void> {
  if (changing()) return;
  busy = true;
  updateControls();
  try {
    const eventIds = await worker<string[]>({ kind: "get_pending_ids" });
    if (!eventIds.length) return;
    const count = eventIds.length;
    if (!window.confirm(`Discard ${count} pending capture record${count === 1 ? "" : "s"}? This cannot be undone.`)) {
      return;
    }
    await worker<WorkerStatus>({ kind: "discard_pending", event_ids: eventIds });
    show("Selected pending capture records discarded.");
  } catch (error) {
    show(`Could not discard pending capture records: ${describe(error)}`, true);
  } finally {
    busy = false;
    await session.refreshStatus().catch((error: unknown) => show(describe(error), true));
    updateControls();
  }
}

async function dismissNotices(): Promise<void> {
  if (changing()) return;
  busy = true;
  updateControls();
  try {
    await worker<WorkerStatus>({ kind: "clear_notices" });
  } catch (error) {
    show(`Could not dismiss notices: ${describe(error)}`, true);
  } finally {
    busy = false;
    await session.refreshStatus().catch((error: unknown) => show(describe(error), true));
    updateControls();
  }
}

// --- Summarize & log ---------------------------------------------------------

function withoutRunAreaChild(area: HTMLElement, child: Element): Element[] {
  return Array.from(area.children).filter((other) => other !== child);
}

/** Shows `text` as `area`'s line about `tabId`, replacing only that area's
 * previous line. `withSettings` adds Open settings. */
function showRunLine(area: HTMLElement, tabId: number, text: string, warning = false, withSettings = false): void {
  const line = document.createElement("p");
  line.className = warning ? "run-line warning" : "run-line";
  if (withSettings) {
    const words = document.createElement("span");
    words.textContent = text;
    const settings = document.createElement("button");
    settings.type = "button";
    settings.className = "secondary compact";
    settings.textContent = "Open settings";
    settings.addEventListener("click", openSettings);
    line.append(words, settings);
  } else {
    line.textContent = text;
  }
  const previous = runLines.get(area);
  const kept = previous ? withoutRunAreaChild(area, previous.element) : Array.from(area.children);
  area.replaceChildren(...kept, line);
  runLines.set(area, { tabId, element: line });
}

/** Clears every area's line that is about another tab than `tabId`. */
function clearRunLineUnlessFor(tabId: number | undefined): void {
  for (const [area, line] of runLines) {
    if (line.tabId !== tabId) clearRunLine(area);
  }
}

function clearRunLine(area: HTMLElement): void {
  const line = runLines.get(area);
  if (!line) return;
  area.replaceChildren(...withoutRunAreaChild(area, line.element));
  runLines.delete(area);
}

/** Shows SETUP_WARNING (spec §6.1) at the top of `area` with the first agent
 * run ever, of either kind and from any path: Summarize & log, the shortcut
 * or context menu (showNotice), or the task bar. It does not hold the run
 * back. */
async function showSetupWarningOnce(area: HTMLElement): Promise<void> {
  try {
    const stored = await chrome.storage.local.get(SUMMARIZE_WARNING_SEEN_KEY);
    if (stored[SUMMARIZE_WARNING_SEEN_KEY] === true) return;
    await chrome.storage.local.set({ [SUMMARIZE_WARNING_SEEN_KEY]: true });
  } catch {
    return;
  }
  const box = document.createElement("div");
  box.className = "setup-warning";
  const text = document.createElement("p");
  text.textContent = SETUP_WARNING;
  const dismiss = document.createElement("button");
  dismiss.type = "button";
  dismiss.className = "secondary compact";
  dismiss.textContent = "Got it";
  dismiss.addEventListener("click", () => area.replaceChildren(...withoutRunAreaChild(area, box)));
  box.append(text, dismiss);
  area.replaceChildren(box, ...Array.from(area.children));
}

async function summarize(): Promise<void> {
  const page = currentPage;
  if (summarizing || !page || page.tabId === undefined) return;
  const tabId = page.tabId;
  const hadFocus = document.activeElement === summarizeButton;
  summarizing = true;
  updateControls();
  showRunLine(runArea, tabId, "Reading this page…");
  try {
    await showSetupWarningOnce(runArea);
    // A page already being summarized answers with its run, which shows
    // (or keeps) the same card.
    const { run } = await summarizeTab(
      { id: tabId, url: page.url, title: page.title },
      { scripting: chrome.scripting, host: session.host, storage: chrome.storage.local, now: () => new Date() },
    );
    // The card shows only while its page is followed; this tab's line goes.
    if (runLines.get(runArea)?.tabId === tabId) clearRunLine(runArea);
    runCards.show(run);
  } catch (error) {
    // The panel may follow another tab by now (a slow cold start); this
    // line is about the tab that was summarized, so it is dropped.
    if (followedTabId === tabId) showRunLine(runArea, tabId, summarizeFailureText(error), true);
  } finally {
    summarizing = false;
    updateControls();
    if (hadFocus && document.activeElement === document.body) summarizeButton.focus();
  }
}

// --- Task bar (§5.7) ----------------------------------------------------------

const SELECTION_PROBE_MS = 1_000;

/** Why the task bar cannot run a task now, in the order the user should fix
 * it, and whether settings is the fix; null when it can. */
function taskBlocked(): { text: string; settings: boolean } | null {
  if (!session.connected) return { text: "Waiting for the native host…", settings: false };
  if (!session.config?.storage) return { text: "Choose a home folder in settings to run tasks.", settings: true };
  if (session.configIssue !== null || session.adapterIssue !== null) return { text: TASK_REPAIR_MESSAGE, settings: true };
  // The Brauser service runs on macOS (and Unix) only; settings cannot fix it.
  if (/^win/i.test(navigator.platform ?? "")) return { text: "Tasks are not supported on Windows yet.", settings: false };
  if (!harnessReady(session)) return { text: TASK_NEEDS_HARNESS_MESSAGE, settings: true };
  if (taskUsePage.checked && pageState === "site-access") {
    return { text: "Re-enable site access in Chrome so the agent can read this page.", settings: false };
  }
  if (followedTabId === null) return { text: "Open a browser tab to run a task.", settings: false };
  return null;
}

async function loadTaskCommands(): Promise<void> {
  if (commandLoading) return commandLoading;
  taskCommand.disabled = true;
  taskCommandInsert.disabled = true;
  taskCommandRefresh.disabled = true;
  commandLoading = (async () => {
    try {
      const response = await session.host.call({
        type: "list_task_commands", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
      }, "task_commands_listed");
      if (closed) return;
      commandRevision = response.revision;
      commandIssue = response.issue;
      const placeholder = document.createElement("option");
      placeholder.value = "";
      placeholder.textContent = "Choose a command…";
      taskCommand.replaceChildren(placeholder);
      for (const command of response.commands) {
        const option = document.createElement("option");
        option.value = command.name;
        option.textContent = `/${command.name} · ${command.description}`;
        taskCommand.append(option);
      }
      taskCommand.value = "";
      taskCommandStatus.textContent = response.issue ?? (response.commands.length === 0
        ? "No saved commands. Open Settings to add one under Saved task commands."
        : "Choose a command, insert it before your draft, then press Run. Use // for a literal leading slash.");
      taskCommand.disabled = taskStarting || response.commands.length === 0;
    } catch (error) {
      commandRevision = null;
      commandIssue = error instanceof HostError && error.code === "invalid_request"
        ? `Rebuild the ${APP_NAME} host to use saved commands.` : `Could not load saved commands: ${describe(error)}`;
      if (!closed) taskCommandStatus.textContent = commandIssue;
    } finally {
      commandLoading = null;
      if (!closed) taskCommandRefresh.disabled = taskStarting;
    }
  })();
  return commandLoading;
}

function renderTaskBar(): void {
  const blocked = taskBlocked();
  taskBar.hidden = followedTabId === null;
  taskRunButton.disabled = blocked !== null || taskStarting;
  const label = taskStarting ? "Starting…" : "Run";
  if (taskRunLabel.textContent !== label) taskRunLabel.textContent = label;
  taskInput.readOnly = taskStarting;
  taskCommand.disabled = taskStarting || commandLoading !== null || commandRevision === null ||
    commandIssue !== null || taskCommand.children.length <= 1;
  taskCommandInsert.disabled = taskCommand.disabled || !taskCommand.value;
  taskCommandRefresh.disabled = taskStarting || commandLoading !== null;
  taskUsePage.disabled = taskStarting || (currentPage === null && pageState !== "site-access");
  taskHint.hidden = blocked === null;
  if (taskHintText.textContent !== (blocked?.text ?? "")) taskHintText.textContent = blocked?.text ?? "";
  taskHintSettings.hidden = blocked?.settings !== true;
  renderTaskContext();
  if (pendingTaskFocus !== null && showNotice(pendingTaskFocus)) void clearStoredNotice();
}

/** "Uses this page: <title>", naming the selection when the page has one,
 * so the user sees which page a task will get. */
function renderTaskContext(): void {
  const page = currentPage;
  let text = "No page · Runs in your knowledge base";
  if (taskUsePage.checked && pageState === "site-access") text = "Page access required · Uncheck Include this page to run without it";
  if (taskUsePage.checked && page !== null && page.tabId !== undefined) {
    const selected = taskSelection !== null && taskSelection.selected &&
      taskSelection.tabId === page.tabId && taskSelection.url === page.url;
    text = `${selected ? "Uses this page and your selection: " : "Uses this page: "}${page.title}`;
  }
  if (taskContext.textContent !== text) taskContext.textContent = text;
}

/** Runs in the page: whether any text is selected. Only the boolean leaves
 * the page. */
function pageHasSelection(): boolean {
  return (window.getSelection()?.toString() ?? "").trim() !== "";
}

/** Checks whether the followed page has a selection, for the context line.
 * Chrome has no selection event the panel can hear, so this runs when the
 * panel follows a page and when the bar gets focus. Only with a ready agent:
 * without one no task can run, so the page is not touched. */
async function probeSelection(): Promise<void> {
  const seq = ++selectionSeq;
  const page = currentPage;
  if (!taskUsePage.checked || page === null || page.tabId === undefined || !harnessReady(session)) {
    taskSelection = null;
    renderTaskContext();
    return;
  }
  const tabId = page.tabId;
  let selected = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const timedOut = new Promise<"timeout">((resolve) => {
      timer = setTimeout(() => resolve("timeout"), SELECTION_PROBE_MS);
    });
    const results = await Promise.race([
      chrome.scripting.executeScript({ target: { tabId, allFrames: true }, func: pageHasSelection, args: [] }),
      timedOut,
    ]);
    selected = Array.isArray(results) && results.some((frame) => frame?.result === true);
  } catch {
    // A page Brauser cannot script reads as having no selection.
  } finally {
    clearTimeout(timer);
  }
  if (seq !== selectionSeq) return;
  taskSelection = { tabId, url: page.url, selected };
  renderTaskContext();
}

/** Starts the typed task on the followed page. The instruction and the page
 * are taken together now, so the request goes with the page the context
 * line names. Only the user's Enter or Run reaches this. */
async function runTask(): Promise<void> {
  const page = currentPage;
  const instruction = taskInput.value;
  const commandInvocation = cleanInstruction(instruction).startsWith("/") && !cleanInstruction(instruction).startsWith("//");
  if (taskStarting || taskBlocked() !== null || followedTabId === null) return;
  if (cleanInstruction(instruction) === "") return;
  const tabId = followedTabId;
  const tab = taskUsePage.checked && page?.tabId !== undefined
    ? { id: page.tabId, url: page.url, title: page.title } : null;
  const hadFocus = document.activeElement === taskRunButton || document.activeElement === taskInput;
  taskRecall.reset();
  taskStarting = true;
  updateControls();
  showRunLine(taskRunArea, tabId, tab === null ? "Starting task…" : "Reading this page…");
  try {
    await showSetupWarningOnce(taskRunArea);
    if (commandInvocation && commandRevision === null) await loadTaskCommands();
    if (commandInvocation && commandIssue !== null) throw new HostError("invalid_request", commandIssue);
    const run = await startTask(
      tab,
      instruction,
      { scripting: chrome.scripting, host: session.host, storage: chrome.storage.local, now: () => new Date() },
      commandRevision ?? undefined,
      async (requestId) => {
        const windowId = await panelWindowId;
        if (windowId !== null) await armTaskNotifications(windowId, requestId);
      },
    );
    if (runLines.get(taskRunArea)?.tabId === tabId) clearRunLine(taskRunArea);
    if (run !== null) {
      if (taskInput.value === instruction) taskInput.value = "";
      // A start proves the host answers, so an earlier list notice goes.
      clearNotice(taskRunArea);
      lastStartedRunId = run.run_id;
      startTracker().track(run);
      void refreshTasks();
      taskInput.focus();
    }
  } catch (error) {
    const failure = taskFailureText(error);
    // Like Summarize, the line is about the tab the task was started on.
    if (followedTabId === tabId) showRunLine(taskRunArea, tabId, failure.text, true, failure.openSettings);
    // A lost answer may hide a queued task; listing the tasks counts it on
    // the Tasks button, so Run is not pressed again for a duplicate.
    if (failure.uncertain) void refreshTasks();
    if (error instanceof HostError && error.code === "conflict" && commandInvocation) {
      await loadTaskCommands();
    }
  } finally {
    taskStarting = false;
    updateControls();
    if (hadFocus && followedTabId === tabId &&
        (document.activeElement === document.body || document.activeElement === taskRunButton)) taskInput.focus();
    // A Reuse value that arrived while this start was in flight was left in
    // storage (checkReuse); take it now that the bar is free again.
    void checkReuse();
  }
}

// The announcement waiting for its tick after the region was emptied.
let announceTimer: ReturnType<typeof setTimeout> | null = null;

/** Announces a tracked task's finish in `#task-announce`, the bar's only live
 * region. The region is emptied first and written on the next tick, so a
 * screen reader announces a text identical to the last one again. */
function announceTask(run: RunView): void {
  taskNoticeSignal("task_notice_seen", run.run_id);
  const chars = Array.from(run.instruction ?? "");
  const shown = chars.length > 60 ? `${chars.slice(0, 60).join("")}…` : chars.join("");
  const outcome = run.state === "succeeded" ? "done" : run.state === "canceled" ? "canceled" : "failed";
  const text = `Task ${outcome}: ${shown}`;
  if (announceTimer !== null) clearTimeout(announceTimer);
  taskAnnounce.textContent = "";
  announceTimer = setTimeout(() => {
    announceTimer = null;
    if (!closed) taskAnnounce.textContent = text;
  }, 0);
}

/** The tracker, started on first use. */
function startTracker(): TaskTracker {
  tracker ??= new TaskTracker({
    host: session.host,
    visible: () => document.visibilityState !== "hidden",
    changed: ({ running, newestId }) => {
      // Automatic polls recover too; a later summary restore must not
      // bring back an error from a task list that has since succeeded.
      if (trackerError !== null) clearNotice(taskRunArea);
      trackerError = null;
      tasksRunning = running;
      newestTaskId = newestId;
      const label = running > 0 ? `Tasks · ${running} running ↗` : "Tasks ↗";
      if (taskOpenButton.textContent !== label) taskOpenButton.textContent = label;
    },
    finished: announceTask,
  });
  return tracker;
}

/** Lists the tasks again. A failed list shows the run-list notice under
 * the bar (an older host's reads "Rebuild the Brauser host…") and leaves
 * the button as it was. */
async function refreshTasks(): Promise<void> {
  if (closed) return;
  try {
    await startTracker().refresh();
    trackerError = null;
  } catch (error) {
    trackerError = error;
    if (!closed) showListFailure(taskRunArea, error, openSettings);
  }
}

/** Opens the Tasks page on the task this panel last started, else the
 * newest one, focusing an already-open Tasks tab rather than opening a
 * second one. */
function openTasks(): void {
  // Nothing to show for a failure here: the user can click again.
  void openTasksPage(chrome, lastStartedRunId ?? newestTaskId).catch(() => undefined);
}

/** Takes any Reuse instruction left for this panel's window and fills the
 * task bar with it. While a start is in flight, the value is left in
 * storage untouched (`taskStarting` gates the take itself, not just the
 * fill): `runTask`'s `finally` calls this again once the start settles, so
 * the value still arrives, just later, rather than being taken and
 * dropped. */
async function checkReuse(): Promise<void> {
  if (taskStarting) return;
  const windowId = await panelWindowId;
  if (windowId === null) return;
  const reused = await takeReuse(chrome.storage.session, windowId).catch(() => null);
  if (!closed && reused !== null && !taskStarting) {
    taskRecall.reset();
    taskInput.value = literalTaskInstruction(reused.instruction);
    taskUsePage.checked = !reused.no_page;
    renderTaskBar();
    taskInput.focus();
  }
}

/** Restores the summary cards. The run record stays while a task is active,
 * when the tracker's latest list found any task (even only finished ones,
 * so an empty summary restore cannot clear history the Tasks page still
 * has), or when the task list failed and nothing is known. */
async function restoreCards(): Promise<void> {
  await restoreRunCards([runCards], taskRunArea, {
    host: session.host, now: () => Date.now(), storage: chrome.storage.local, openSettings,
  }, { keepRecord: tasksRunning > 0 || newestTaskId !== null || trackerError !== null });
  // Summary restoration clears the shared list notice. The task list's
  // last failure still stands, including restores triggered by the worker.
  if (trackerError !== null && !closed) showListFailure(taskRunArea, trackerError, openSettings);
}

/** Shows a notice from the worker's shortcut or context-menu summary when it
 * is about the followed tab. Returns whether it is now shown. */
function showNotice(notice: PanelNotice): boolean {
  if (closed) return false;
  if (notice.kind === "focus_task" && !freshTaskFocus(notice)) {
    pendingTaskFocus = null;
    return false;
  }
  if (notice.tab_id !== followedTabId) return false;
  const id = `${notice.kind}:${notice.tab_id}:${notice.at}`;
  if (id === shownNotice) return true;
  if (notice.kind === "focus_task") {
    // Startup can discover the tab before rendering its bar; a starting
    // task temporarily makes the field read-only. Keep the focus intent.
    pendingTaskFocus = notice;
    if (taskStarting || taskBar.hidden) return false;
    taskInput.focus();
    pendingTaskFocus = null;
    shownNotice = id;
    return true;
  }
  shownNotice = id;
  // The shortcut and the context menu start a summary without the panel's
  // button, so the one-time warning (spec §6.1) shows here for them.
  void showSetupWarningOnce(runArea);
  if (notice.kind === "summarize_failed") {
    showRunLine(runArea, notice.tab_id, notice.message, true);
  } else {
    // The notice names only the run; list_runs brings its card, as it does
    // for any unfinished run.
    clearRunLine(runArea);
    void restoreCards();
  }
  return true;
}

function freshTaskFocus(notice: PanelNotice): boolean {
  const age = Date.now() - notice.at;
  return age >= 0 && age < PANEL_NOTICE_MAX_AGE_MS;
}

function clearStoredNotice(): Promise<void> {
  return (chrome.storage.session.remove?.(PANEL_NOTICE_KEY) ?? Promise.resolve()).catch(() => undefined);
}

/** A notice the worker stored, such as a service_unavailable from the
 * shortcut that opened this panel, or one for a tab the panel had not
 * followed yet when the worker sent it. A notice for the followed tab is
 * shown and consumed; a stale or malformed one is dropped. At startup
 * (`dropOthers`) a notice for another tab is dropped too; on a later follow
 * it stays stored for the tab it names. */
async function showStoredNotice(dropOthers: boolean): Promise<void> {
  let stored: unknown;
  try {
    stored = (await chrome.storage.session.get(PANEL_NOTICE_KEY))[PANEL_NOTICE_KEY];
  } catch {
    return;
  }
  if (stored === undefined) return;
  const current = isPanelNotice(stored) && Date.now() - stored.at < PANEL_NOTICE_MAX_AGE_MS &&
    (stored.kind !== "focus_task" || freshTaskFocus(stored)) ? stored : null;
  if (current && current.tab_id !== followedTabId && !dropOthers) return;
  // Focus is consumed only after the bar can accept it. Summary notices
  // retain their existing one-shot behavior.
  const shown = current ? showNotice(current) : false;
  if (current?.kind !== "focus_task" || shown || current.tab_id !== followedTabId) await clearStoredNotice();
}

// --- Page area and note (§5.3, §5.7) ---------------------------------------

function showUnsavedText(entries: readonly RecoveryText[]): void {
  noteRecoveryList.replaceChildren();
  for (const entry of entries) {
    const row = document.createElement("li");
    const page = document.createElement("p");
    page.className = "page-url";
    page.textContent = entry.url;
    const reason = document.createElement("p");
    reason.textContent = entry.reason;
    const text = document.createElement("textarea");
    text.rows = 4;
    text.readOnly = true;
    text.setAttribute("aria-label", `Recovered note text for ${entry.url}`);
    text.value = entry.text;
    row.append(page, reason, text);
    if (entry.text === "") {
      const empty = document.createElement("p");
      empty.textContent = "Empty edit: this copy would clear the note.";
      row.append(empty);
    }
    const status = document.createElement("p");
    status.setAttribute("role", "status");
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "secondary compact";
    copy.textContent = "Copy";
    copy.setAttribute("aria-label", `Copy recovered note for ${entry.url}`);
    copy.addEventListener("click", () => {
      void navigator.clipboard.writeText(entry.text).then(() => {
        status.textContent = "Copied";
      }).catch((error: unknown) => {
        status.textContent = `Could not copy: ${describe(error)}`;
      });
    });
    const discard = document.createElement("button");
    discard.type = "button";
    discard.className = "secondary compact";
    discard.textContent = "Discard";
    discard.setAttribute("aria-label", `Discard recovered note for ${entry.url}`);
    discard.addEventListener("click", () => notes.discardUnsaved(entry.id));
    row.append(copy, discard, status);
    noteRecoveryList.append(row);
  }
  noteConflict.hidden = entries.length === 0;
}

const notes = new NoteEditor({
  readBody: () => noteBody.value,
  writeBody: (body) => { noteBody.value = body; },
  setEditable: (editable) => { noteBody.disabled = !editable; },
  setStatus: showNoteStatus,
  showUnsaved: showUnsavedText,
  setRecoveryWarning: (message) => {
    noteRecoveryWarning.textContent = message;
    noteRecoveryWarning.hidden = !message;
  },
  showOlderNotes: renderOlderNotes,
}, {
  load: (url) => session.host.call({
    type: "load_note",
    protocol_version: PROTOCOL_VERSION,
    request_id: newRequestId(),
    url,
  }, "note_loaded"),
  save: (request) => session.host.callAny({
    type: "save_note",
    protocol_version: PROTOCOL_VERSION,
    request_id: newRequestId(),
    ...request,
  }, ["note_saved", "note_conflict"]),
}, describe, NOTE_SAVE_DEBOUNCE_MS, new NoteRecovery(() => window.localStorage));

captureSite = new CaptureSitePanel(session, {
  isBusy: changing,
  changed: updateControls,
  show,
  beforeSave: () => {
    changingNoteIdentity = true;
    return notes.showPage(null);
  },
  afterSave: async () => {
    changingNoteIdentity = false;
    await notes.showPage(currentPage);
    // The new adapter changes whether this page is an artifact.
    await refreshArtifact(true);
  },
  openSettings,
  agentReady: () => harnessReady(session),
});

function localDate(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function worklogRange(): { start: string; end: string } {
  const now = new Date();
  now.setHours(12, 0, 0, 0);
  if (worklogPeriod() === "yesterday") {
    now.setDate(now.getDate() - 1);
    const day = localDate(now);
    return { start: day, end: day };
  }
  if (worklogPeriod() === "week") {
    const end = localDate(now);
    const mondayOffset = (now.getDay() + 6) % 7;
    now.setDate(now.getDate() - mondayOffset);
    return { start: localDate(now), end };
  }
  const day = localDate(now);
  return { start: day, end: day };
}

/** A Recent row: a link to the page and, for a page summarized that day,
 * the summary line and Open summary. The button sits beside the link, not
 * inside it, since a link may not hold a button. */
function recentRow(item: ArtifactListItem): HTMLElement {
  const row = document.createElement("div");
  row.className = "recent-row";
  const look = artifactLook(item.artifact_type);
  const glyph = document.createElement("span");
  glyph.className = "glyph glyph-small";
  glyph.dataset.hue = look.hue;
  glyph.setAttribute("aria-hidden", "true");
  glyph.textContent = look.glyph;
  const link = document.createElement("a");
  link.className = "recent-link";
  link.href = item.url;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  const title = document.createElement("strong");
  title.textContent = item.title;
  const details = document.createElement("span");
  details.className = "recent-sub";
  details.textContent = `${look.label}${item.opens ? ` · opened ${item.opens}×` : ""}`;
  // The full measures stay one hover away, as the row used to show them.
  link.title = `${item.artifact_type} · focused ${duration(item.focused_seconds)} · background ${duration(item.background_seconds)}`;
  link.append(title, details);
  const time = document.createElement("span");
  time.className = "recent-time";
  time.setAttribute("aria-hidden", "true");
  time.textContent = compactDuration(item.focused_seconds);
  if (item.why) {
    const why = document.createElement("span");
    why.textContent = `Why: ${item.why}`;
    link.append(why);
  }
  row.append(glyph, link, time);
  if (item.summary) {
    const summarized = document.createElement("span");
    summarized.textContent = recentSummaryText(item.summary);
    link.append(summarized);
    const open = document.createElement("button");
    open.type = "button";
    open.className = "secondary compact";
    open.textContent = "Open summary";
    open.setAttribute("aria-label", `Open summary: ${item.title}`);
    open.disabled = !session.connected;
    recentOpenButtons.push(open);
    // The row's own summary, by its path: the URL alone may now resolve
    // to another record (strip_params, or an adapter added or removed).
    const url = item.url;
    const path = item.summary.summary_path;
    open.addEventListener("click", () => void openRecentSummary(url, path));
    row.append(open);
  }
  return row;
}

function renderRecent(items: readonly ArtifactListItem[], append = false): HTMLElement | null {
  if (!append) {
    recentList.replaceChildren();
    recentOpenButtons = [];
  }
  let first: HTMLElement | null = null;
  let day = append ? recentResult?.items.at(-1)?.local_date ?? "" : "";
  for (const item of items) {
    if (item.local_date !== day) {
      day = item.local_date;
      const heading = document.createElement("h3");
      heading.textContent = day;
      recentList.append(heading);
    }
    const row = recentRow(item);
    first ??= row.querySelector<HTMLAnchorElement>("a");
    recentList.append(row);
  }
  if (!append && !items.length) {
    const empty = document.createElement("p");
    empty.textContent = recentReading.value
      ? "No matching saved summaries. Try another reading filter or All activity."
      : "No matching artifact activity yet.";
    recentList.append(empty);
  }
  return first;
}

function recentScope(): string {
  return JSON.stringify([session.config?.storage, session.revision,
    recentType.value, recentTitle.value.trim(), recentReading.value]);
}

function invalidateRecent(): void {
  ++recentLoadSeq;
  recentResult = null;
  recentPageLoading = false;
  recentMore.hidden = true;
  recentMore.setAttribute("aria-disabled", "false");
  recentReload.hidden = true;
  recentList.replaceChildren();
  recentOpenButtons = [];
}

function validateRecentPage(response: ArtifactListResult, offset: number, snapshot?: string): void {
  const page = response.page;
  if (!page || !/^[a-f0-9]{64}$/.test(page.snapshot) || page.offset !== offset ||
      (snapshot !== undefined && page.snapshot !== snapshot) || response.items.length > 32 ||
      (page.next_offset !== null && (response.items.length === 0 || page.next_offset !== offset + response.items.length)) ||
      response.truncated !== (page.next_offset !== null)) {
    throw new Error("Recent changed or this page is unavailable. Reload results.");
  }
}

function recentCount(count: number, more: boolean): void {
  recentStatus.textContent = `${count} artifact-day entr${count === 1 ? "y" : "ies"}${more ? " · more available" : ""}`;
}

async function loadMoreRecent(): Promise<void> {
  const result = recentResult;
  const page = result?.page;
  if (recentPageLoading || !result || !page || page.next_offset === null || !session.connected) return;
  const scope = recentScope();
  if (scope !== result.scope) return;
  const seq = ++recentLoadSeq;
  recentPageLoading = true;
  // Native disabling blurs Chrome's focused button; the loading guard keeps
  // this control inert while aria-disabled preserves keyboard focus.
  recentMore.setAttribute("aria-disabled", "true");
  recentStatus.textContent = "Loading more…";
  try {
    const response = await session.host.call({
      type: "list_artifacts", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
      artifact_type: recentType.value || null, title_query: recentTitle.value.trim() || null,
      ...(recentReading.value === "limited" || recentReading.value === "whole" || recentReading.value === "unknown"
        ? { summary_reading: recentReading.value } : {}),
      limit: 32, page: { offset: page.next_offset, snapshot: page.snapshot },
    }, "artifact_list_result");
    if (closed || seq !== recentLoadSeq || !session.connected || scope !== recentScope()) return;
    validateRecentPage(response, page.next_offset, page.snapshot);
    const first = renderRecent(response.items, true);
    recentResult = { scope, items: [...result.items, ...response.items], page: response.page };
    const focusMore = document.activeElement === recentMore;
    recentMore.setAttribute("aria-disabled", "false");
    recentMore.hidden = response.page?.next_offset === null;
    recentCount(recentResult.items.length, response.truncated);
    if (focusMore) first?.focus();
  } catch (error) {
    if (closed || seq !== recentLoadSeq || !session.connected || scope !== recentScope()) return;
    const focusMore = document.activeElement === recentMore;
    recentMore.hidden = true;
    recentReload.hidden = false;
    recentStatus.textContent = error instanceof HostError && error.code === "conflict"
      ? "Recent changed. Reload results to continue."
      : `Could not load more Recent: ${describe(error)} Reload results to continue.`;
    if (focusMore) recentReload.focus();
  } finally {
    if (seq === recentLoadSeq) {
      recentPageLoading = false;
      recentMore.setAttribute("aria-disabled", "false");
    }
  }
}

async function loadRecent(): Promise<void> {
  invalidateRecent();
  const seq = recentLoadSeq;
  const scope = recentScope();
  const reading = recentReading.value;
  // The selector now describes a new result; do not leave old rows beneath it.
  recentList.replaceChildren();
  if (!session.connected) {
    recentWaitingForHost = true;
    recentStatus.textContent = "Waiting for the host…";
    return;
  }
  recentWaitingForHost = false;
  if (!session.config?.storage) {
    recentStatus.textContent = "Choose a home folder in settings to view Recent.";
    recentList.replaceChildren();
    return;
  }
  recentStatus.textContent = "Loading…";
  try {
    const response = await session.host.call({
      type: "list_artifacts",
      protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(),
      artifact_type: recentType.value || null,
      title_query: recentTitle.value.trim() || null,
      ...(reading === "limited" || reading === "whole" || reading === "unknown"
        ? { summary_reading: reading } : {}),
      limit: 32,
      page: { offset: 0 },
    }, "artifact_list_result");
    if (closed || seq !== recentLoadSeq || !session.connected || scope !== recentScope()) return;
    // Keep displaying an unpaged response if an older host supplies one.
    if (response.page) validateRecentPage(response, 0);
    const selected = recentType.value;
    recentType.replaceChildren(new Option("All types", ""));
    for (const type of response.artifact_types) recentType.add(new Option(type, type));
    if (selected && !response.artifact_types.includes(selected)) recentType.add(new Option(selected, selected));
    recentType.value = selected;
    renderRecent(response.items);
    recentResult = { scope: recentScope(), items: response.items, page: response.page };
    recentMore.hidden = !response.page || response.page.next_offset === null;
    recentCount(response.items.length, response.truncated);
    if (response.truncated && !response.page) recentStatus.textContent += " · rebuild the host to load more";
  } catch (error) {
    if (closed || seq !== recentLoadSeq || !session.connected || scope !== recentScope()) return;
    recentStatus.textContent = reading && error instanceof HostError && error.code === "invalid_request"
      ? `Rebuild the ${APP_NAME} host to filter saved summary reading, or choose All activity.`
      : error instanceof HostError && error.code === "invalid_request"
        ? `Rebuild the ${APP_NAME} host and extension together to page through Recent.`
        : `Could not load Recent: ${describe(error)}`;
    recentReload.hidden = false;
    recentList.replaceChildren();
  }
}

async function loadWorklog(): Promise<void> {
  const previous = worklogValidated;
  const focused = document.activeElement as HTMLElement | null;
  const focusedKey = focused?.dataset?.worklogKey ?? null;
  const focusedScope = worklogScope;
  const seq = ++worklogLoadSeq;
  worklogReport = null;
  sharingDraftPanel?.render();
  worklogPageLoading = false;
  worklogPageNotice = "";
  worklogChoices.disabled = false;
  worklogMore.hidden = true;
  worklogGaps.hidden = true;
  worklogChoices.replaceChildren();
  worklogSelectAll.disabled = true;
  worklogClear.disabled = true;
  worklogChoicesArea.hidden = !curateWorklog.checked;
  const focusOnly = curateWorklog.checked && worklogShareScope.value === "focus";
  worklogFocusArea.hidden = !focusOnly;
  // Keep an existing catalog usable: disabling a native select drops its
  // keyboard focus. The sequence guard discards superseded selections.
  worklogFocusStatus.textContent = focusOnly ? "Loading recorded sessions…" : "";
  worklogSelectionStatus.textContent = "Loading choices…";
  if (!session.connected) {
    worklogWaitingForHost = true;
    worklogText.textContent = "Waiting for the host…";
    copyWorklogButton.disabled = true;
    return;
  }
  worklogWaitingForHost = false;
  if (!session.config?.storage) {
    worklogText.textContent = "Choose a home folder in settings to generate a worklog.";
    copyWorklogButton.disabled = true;
    return;
  }
  const range = worklogRange();
  const revision = session.revision ?? "";
  const sessionId = focusOnly ? worklogFocusSession.value || null : null;
  const scope = JSON.stringify([session.config.storage, revision, range.start, range.end,
    curateWorklog.checked, focusOnly, sessionId]);
  if (scope !== worklogScope) {
    selectedWork.clear();
    worklogValidated = null;
  }
  worklogScope = scope;
  worklogText.textContent = "Generating…";
  copyWorklogButton.disabled = true;
  try {
    const dates = { protocol_version: PROTOCOL_VERSION, request_id: newRequestId(), start_date: range.start, end_date: range.end };
    const response = focusOnly
      ? await session.host.call({ ...dates, type: "worklog_focus_choices", expected_revision: revision,
        focus_session_id: sessionId }, "worklog_focus_choices_result")
      : curateWorklog.checked
      ? await session.host.call({ ...dates, type: "worklog_choices", expected_revision: revision, page: { offset: 0 } }, "worklog_choices_result")
      : await session.host.call({ ...dates, type: "worklog_report" }, "worklog_reported");
    if (seq !== worklogLoadSeq || closed) return;
    if (response.start_date !== range.start || response.end_date !== range.end) throw new Error("The host returned a different worklog period. Try again.");
    if (response.type !== "worklog_reported") {
      if (session.revision !== revision) throw new Error("Settings changed. Open Recent again to choose work to share.");
      const replyKeys = new Set(response.choices.map(worklogChoiceKey));
      if (replyKeys.size !== response.choices.length || response.choices.some((row) => row.local_date < range.start || row.local_date > range.end)) {
        throw new Error("The host returned invalid worklog choices. Try again.");
      }
      if (response.type === "worklog_choices_result") {
        validateWorklogPage(response, 0);
        if (previous?.scope === scope && previous.report.page) {
          if (previous.report.page.snapshot === response.page?.snapshot) {
            // The digest binds every row, including pages not in this reply.
            response.choices = previous.report.choices;
            response.page = { ...previous.report.page };
            response.truncated = previous.report.truncated;
          } else if (previous.report.choices.length > response.choices.length || previous.report.page.offset > 0) {
            worklogPageNotice = "History changed. Later pages were cleared; review the refreshed choices.";
          }
        }
      }
      const keys = new Set(response.choices.map(worklogChoiceKey));
      for (const key of selectedWork) if (!keys.has(key)) selectedWork.delete(key);
    }
    if (response.type === "worklog_focus_choices_result") {
      if (response.focus_session_id !== sessionId) throw new Error("The host returned a different focus session. Try again.");
      renderWorklogSessions(response);
    }
    worklogReport = response;
    worklogValidated = response.type === "worklog_choices_result" ? { scope, report: response } : null;
    // A delivery refresh removes the old checkbox. Restore its stable identity
    // only if the user has not moved focus elsewhere while the host answered.
    const restoreFocus = focusedScope === scope &&
      (document.activeElement === focused || document.activeElement === document.body);
    renderWorklogChoices(restoreFocus ? focusedKey : null);
    renderWorklog();
    if (restoreFocus && focused === worklogMore && !worklogMore.hidden) worklogMore.focus();
  } catch (error) {
    if (seq !== worklogLoadSeq || closed) return;
    worklogValidated = null;
    worklogSelectionStatus.textContent = "Choices unavailable. Open Recent again to retry.";
    worklogFocusStatus.textContent = focusOnly ? "Recorded sessions unavailable. Open Recent again to retry." : "";
    worklogText.textContent = error instanceof HostError && error.code === "invalid_request" && /unsupported request/.test(error.message)
      ? "Rebuild the Brauser host and extension together, then reopen Recent to generate this worklog."
      : `Could not generate worklog: ${describe(error)}`;
  }
}

function validateWorklogPage(report: WorklogChoicesResult, offset: number, snapshot?: string): void {
  const page = report.page;
  if (!page || !/^[a-f0-9]{64}$/.test(page.snapshot) || page.offset !== offset || (snapshot !== undefined && page.snapshot !== snapshot) ||
      (page.next_offset !== null && (page.next_offset <= offset || report.choices.length === 0)) ||
      report.truncated !== (page.next_offset !== null || page.omitted)) {
    throw new Error("Work to share changed or this page is unavailable. Reload choices.");
  }
}

async function loadMoreWorklog(): Promise<void> {
  const report = worklogReport;
  if (worklogPageLoading) return;
  if (report?.type !== "worklog_choices_result" || !report.page || report.page.next_offset === null) {
    await loadWorklog();
    return;
  }
  const seq = ++worklogLoadSeq;
  const revision = session.revision;
  const offset = report.page.next_offset;
  worklogPageLoading = true;
  worklogChoices.disabled = true;
  // A natively disabled button loses keyboard focus in Chrome. Keep it
  // focusable while the loading guard prevents a second request.
  worklogMore.setAttribute("aria-disabled", "true");
  worklogMore.textContent = "Loading…";
  worklogSelectAll.disabled = true;
  worklogClear.disabled = true;
  copyWorklogButton.disabled = true;
  sharingDraftPanel?.render();
  try {
    const response = await session.host.call({ type: "worklog_choices", protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(), expected_revision: revision ?? "",
      start_date: report.start_date, end_date: report.end_date,
      page: { offset, snapshot: report.page.snapshot } }, "worklog_choices_result");
    if (seq !== worklogLoadSeq || closed) return;
    if (revision !== session.revision || response.start_date !== report.start_date || response.end_date !== report.end_date) {
      throw new Error("The sharing period or settings changed. Reload choices.");
    }
    validateWorklogPage(response, offset, report.page.snapshot);
    const keys = new Set(report.choices.map(worklogChoiceKey));
    for (const row of response.choices) {
      const key = worklogChoiceKey(row);
      if (keys.has(key) || row.local_date < report.start_date || row.local_date > report.end_date) {
        throw new Error("The host returned invalid worklog choices. Reload choices.");
      }
      keys.add(key);
    }
    const omitted = report.page.omitted || response.page!.omitted;
    worklogReport = { ...response, choices: [...report.choices, ...response.choices],
      truncated: response.page!.next_offset !== null || omitted,
      page: { ...response.page!, omitted } };
    worklogValidated = { scope: worklogScope, report: worklogReport };
    worklogPageLoading = false;
    // Descendants of a disabled fieldset cannot receive focus. Re-enable
    // it before appending and focusing the final page's first new choice.
    worklogChoices.disabled = false;
    // Keep old checkboxes. If the final page hides the button holding focus,
    // move into the new choices instead of dropping focus to the document.
    const focusNew = document.activeElement === worklogMore && response.page!.next_offset === null;
    const firstNew = response.choices[0];
    appendWorklogChoices(response.choices, focusNew && firstNew ? worklogChoiceKey(firstNew) : null);
    renderWorklog();
  } catch (error) {
    if (seq !== worklogLoadSeq || closed) return;
    worklogReport = null;
    worklogValidated = null;
    selectedWork.clear();
    worklogChoices.replaceChildren();
    worklogGaps.hidden = true;
    worklogPageLoading = false;
    worklogSelectionStatus.textContent = "Choices changed or are unavailable. Reload before selecting work.";
    worklogText.textContent = `Could not load more work: ${describe(error)}`;
    worklogMore.textContent = "Reload choices";
    worklogMore.hidden = false;
  } finally {
    if (seq === worklogLoadSeq && !closed) {
      worklogChoices.disabled = false;
      worklogMore.setAttribute("aria-disabled", "false");
      sharingDraftPanel?.render();
    }
  }
}

function renderWorklogSessions(report: WorklogFocusChoicesResult): void {
  worklogFocusSession.replaceChildren();
  const add = (value: string, text: string): void => {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = text;
    worklogFocusSession.append(option);
  };
  add("", "Choose a session…");
  for (const recorded of report.sessions) add(recorded.id, `${recorded.name} · ${recorded.id.slice(-8)}`);
  if (report.focus_session_id && !report.sessions.some((recorded) => recorded.id === report.focus_session_id)) {
    add(report.focus_session_id, report.selected_session?.name ?? "Unavailable session");
  }
  worklogFocusSession.value = report.focus_session_id ?? "";
  worklogFocusSession.disabled = report.sessions.length === 0 && report.focus_session_id === null;
  const state = report.focus_session_id !== null && report.selected_session === null
    ? "This session has no recorded activity in this period."
    : report.sessions.length === 0 && report.selected_session === null
      ? "No recorded focus sessions in this period."
      : report.selected_session === null
        ? "Choose a session, then check entries to include."
        : `Only this session’s direct activity is offered.${report.name_changed ? " Its name changed in this period." : ""}`;
  worklogFocusStatus.textContent = state + (report.sessions_truncated
    ? " More sessions omitted; choose a shorter period to find them." : "");
}

function renderWorklogChoices(focusedKey: string | null = null): void {
  worklogChoices.replaceChildren();
  const legend = document.createElement("legend");
  legend.textContent = "Work to include";
  worklogChoices.append(legend);
  const report = worklogReport;
  if (!report || report.type === "worklog_reported") return;
  appendWorklogChoices(report.choices, focusedKey);
}

function appendWorklogChoices(rows: WorklogChoice[], focusedKey: string | null = null): void {
  for (const row of rows) {
    const label = document.createElement("label");
    label.className = "check-label";
    const input = document.createElement("input");
    input.type = "checkbox";
    const key = worklogChoiceKey(row);
    input.dataset.worklogKey = key;
    input.checked = selectedWork.has(key);
    input.addEventListener("change", () => {
      if (worklogPageLoading) return;
      if (input.checked) selectedWork.add(key);
      else selectedWork.delete(key);
      renderWorklog();
    });
    const text = document.createElement("span");
    text.textContent = `${row.local_date} · ${row.text}\n${row.url}`;
    label.append(input, text);
    worklogChoices.append(label);
    if (key === focusedKey) input.focus();
  }
}

function renderWorklog(): void {
  const report = worklogReport;
  if (!report) return;
  const lines = gapLines(report);
  const hasGaps = lines.length > 0 || report.gaps_truncated;
  worklogGaps.hidden = !hasGaps;
  worklogGapStatus.textContent = "Known capture gaps intersect this period. Counts cover each full reported range; ranges may overlap. Missing activity is excluded from totals.";
  worklogGapList.replaceChildren();
  for (const line of lines) {
    const item = document.createElement("li");
    item.textContent = line;
    worklogGapList.append(item);
  }
  let text: string;
  if (report.type !== "worklog_reported") {
    const selected = report.choices.filter((row) => selectedWork.has(worklogChoiceKey(row)));
    const page = report.type === "worklog_choices_result" ? report.page : null;
    const more = page?.next_offset !== null && page?.next_offset !== undefined;
    worklogMore.hidden = !more;
    worklogMore.setAttribute("aria-disabled", String(worklogPageLoading));
    worklogMore.textContent = worklogPageLoading ? "Loading…" : "Load more work";
    const omission = page ? page.omitted ? " More work omitted; some entries exceed the display limits." : ""
      : report.truncated ? " More work omitted; choose a shorter period to review it." : "";
    worklogSelectionStatus.textContent = `${selected.length} of ${report.choices.length} shown entries selected.${more ? " More work available below." : ""}${omission}${worklogPageNotice ? ` ${worklogPageNotice}` : ""}`;
    worklogSelectAll.disabled = report.choices.length === 0 || worklogPageLoading;
    worklogClear.disabled = selected.length === 0 || worklogPageLoading;
    copyWorklogButton.disabled = selected.length === 0 || worklogPageLoading;
    const focus = report.type === "worklog_focus_choices_result";
    const heading = focusHeading(report);
    text = selected.length
      ? selectionPreview({ start_date: report.start_date, end_date: report.end_date, choices: selected,
        heading, footer: "", include_gaps: false, focus_session_id: null })
      : focus && report.selected_session === null ? worklogFocusStatus.textContent ?? "Choose a session."
        : "Select work to include in the preview.";
  } else {
    text = report.text;
    copyWorklogButton.disabled = false;
  }
  worklogText.textContent = text + gapFooter(report, includeWorklogGaps.checked, report.type !== "worklog_reported");
  sharingDraftPanel?.render();
}

function currentDraftSelection(): DraftSelection | null {
  const report = worklogReport;
  if (!session.connected || draftPolicyPending || !report || report.type === "worklog_reported" ||
      worklogPageLoading || copyWorklogButton.disabled) return null;
  const choices = report.choices.filter((row) => selectedWork.has(worklogChoiceKey(row)));
  if (!choices.length) return null;
  return { start_date: report.start_date, end_date: report.end_date, choices,
    focus_session_id: report.type === "worklog_focus_choices_result" ? report.focus_session_id : null,
    heading: focusHeading(report), footer: gapFooter(report, includeWorklogGaps.checked),
    include_gaps: includeWorklogGaps.checked };
}

sharingDraftPanel = new SharingDraftPanel({
  context: () => !closed && !draftPolicyPending && session.connected && session.revision &&
      session.config?.storage && !session.configIssue && !session.adapterIssue
    ? { scope: draftScope(session.config.storage), revision: session.revision } : null,
  selection: currentDraftSelection,
  load: (draft, revision, offset, snapshot) => {
    const dates = { protocol_version: PROTOCOL_VERSION, request_id: newRequestId(), expected_revision: revision,
      start_date: draft.start_date, end_date: draft.end_date };
    return draft.focus_session_id === null
      ? session.host.call({ ...dates, type: "worklog_choices", page: snapshot === undefined ? { offset } : { offset, snapshot } }, "worklog_choices_result")
      : session.host.call({ ...dates, type: "worklog_focus_choices", focus_session_id: draft.focus_session_id }, "worklog_focus_choices_result");
  },
});
window.addEventListener("storage", () => sharingDraftPanel?.render());

function scheduleRecent(): void {
  // Invalidate immediately, before the debounce can let an old page append.
  invalidateRecent();
  recentStatus.textContent = "Loading…";
  if (recentTimer !== null) clearTimeout(recentTimer);
  recentTimer = setTimeout(() => {
    recentTimer = null;
    void loadRecent();
  }, 200);
}

function selectView(view: "page" | "recent"): void {
  const recent = view === "recent";
  pageView.hidden = recent;
  recentView.hidden = !recent;
  pageTab.classList.toggle("active", !recent);
  recentTab.classList.toggle("active", recent);
  pageTab.setAttribute("aria-selected", String(!recent));
  recentTab.setAttribute("aria-selected", String(recent));
  pageTab.tabIndex = recent ? -1 : 0;
  recentTab.tabIndex = recent ? 0 : -1;
  if (recent) {
    void Promise.all([loadRecent(), loadWorklog()]);
  }
}

function copyWorklog(): void {
  if (copyWorklogButton.disabled || !worklogReport || !session.connected) return;
  void navigator.clipboard.writeText(worklogText.textContent ?? "")
    .then(() => show("Worklog copied."))
    .catch((error: unknown) => show(`Could not copy worklog: ${describe(error)}`, true));
}

/** Follows the active tab (§5.7). The page area is pointed at the tab this
 * call found, so the header and the note editor cannot disagree; `notes`
 * saves the page being left before loading the new one. */
async function refreshActiveTab(): Promise<void> {
  const seq = ++tabRefreshSeq;
  let tab: ChromeTab | undefined;
  try {
    [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  } catch {
    tab = undefined;
  }
  if (seq !== tabRefreshSeq) return; // A newer refresh has the current tab.
  followedTabId = tab?.id ?? null;
  // The worker's notice may have come while the panel still followed the
  // previous tab; the live message was not shown, so the stored copy is.
  void showStoredNotice(false);
  await refreshSiteAccess(tab);
}

function showPageArea(state: "none" | "unusable" | "site-access" | "note"): void {
  pageState = state;
  pageUnusable.hidden = state !== "unusable";
  siteAccessWarning.hidden = state !== "site-access";
  noteEditor.hidden = state !== "note";
  preservedNotes.hidden = state !== "note";
}

async function showNoPage(title: string, url: string, state: "none" | "unusable" | "site-access"): Promise<void> {
  resetPreservedNotes();
  currentPage = null;
  pageState = state;
  runCards.follow(null);
  taskSelection = null;
  selectionSeq++;
  updateControls();
  void refreshArtifact();
  pageTitle.textContent = title;
  pageUrl.textContent = url;
  renderPageGlyph();
  showPageArea(state);
  await notes.showPage(null);
}

/** Decides between the note editor, the "not a web page" state, and the one
 * site-access banner for `tab`. Brauser holds HTTP(S) access from install, so
 * Chrome withholds a tab's address only for pages outside it (chrome://, New
 * Tab, file://) or after the user withdrew that access in Chrome. */
async function refreshSiteAccess(tab: ChromeTab | undefined): Promise<void> {
  const seq = tabRefreshSeq;
  clearRunLineUnlessFor(tab?.id);
  if (!tab || tab.incognito) {
    await showNoPage("No page yet", "", "none");
    return;
  }

  if (tab.url === undefined) {
    let granted = true;
    try {
      granted = await chrome.permissions.contains({ origins: SITE_ACCESS_ORIGINS });
    } catch {
      // Without an answer, show the ordinary state rather than a false alarm.
    }
    if (seq !== tabRefreshSeq) return;
    await showNoPage(tab.title?.trim() || "This page", "", granted ? "unusable" : "site-access");
    return;
  }

  let url: URL;
  try {
    url = new URL(tab.url);
  } catch {
    url = new URL("about:blank");
  }
  if (!isHttpUrl(url)) {
    await showNoPage(tab.title?.trim() || tab.url, tab.url, "unusable");
    return;
  }

  // A URL means Chrome granted this site, so no banner shows here.
  const title = tab.title?.trim() || url.hostname;
  pageTitle.textContent = title;
  pageUrl.textContent = tab.url;
  renderPageGlyph();
  showPageArea("note");
  if (currentPage?.url !== tab.url || currentPage.tabId !== tab.id) resetPreservedNotes();
  currentPage = { url: tab.url, title, tabId: tab.id };
  // Run cards follow the page too, so another page's cards go with it.
  runCards.follow(tab.url);
  // The bar keeps the user's draft; its context line names this page.
  void probeSelection();
  // Another page's summary line, and its Open summary, go now rather than
  // when this page's lookup returns.
  if (summaryPageUrl !== tab.url) renderPageSummary(null, null);
  updateControls();
  if (!changingNoteIdentity) await notes.showPage({ url: tab.url, title });
  void refreshArtifact();
}

function reenableSiteAccess(): void {
  void chrome.tabs.create({ url: `chrome://extensions/?id=${chrome.runtime.id}` })
    .catch((error: unknown) => show(`Could not open Chrome's extension settings: ${describe(error)}`, true));
}

settingsButton.addEventListener("click", openSettings);
captureExplanationAction.addEventListener("click", () => {
  if (captureExplanationAction.dataset.action === "capture-site") {
    selectView("page");
    element<HTMLButtonElement>("capture-site").click();
  } else if (captureExplanationAction.dataset.action === "adapter-review" && pendingAdapterReview) {
    openAdapterReview(pendingAdapterReview);
  } else openSettings();
});
setupSettingsButton.addEventListener("click", openSettings);
pauseButton.addEventListener("click", () => void pauseCapture());
replayButton.addEventListener("click", () => void replayVisits());
discardButton.addEventListener("click", () => void discardPendingVisits());
dismissButton.addEventListener("click", () => void dismissNotices());
noteBody.addEventListener("input", () => notes.edited());
siteAccessReenableButton.addEventListener("click", reenableSiteAccess);
saveWhyButton.addEventListener("click", () => void saveWhy());
artifactWhy.addEventListener("keydown", (event) => {
  if (event.key !== "Enter" || event.isComposing) return;
  event.preventDefault();
  void saveWhy();
});
captureStripAction.addEventListener("click", () => {
  if (captureStripAction.dataset.action === "send") void replayVisits();
  else if (captureStripAction.dataset.action === "settings") openSettings();
});
artifactWhy.addEventListener("input", () => {
  whyEdited = true;
  updateControls();
});
olderNoteList.addEventListener("click", (event) => {
  const target = event.target as HTMLElement;
  const index = target.dataset.olderIndex;
  if (index === undefined) return;
  if (target.dataset.olderAction === "view") {
    void viewOlderNote(Number(index));
  } else {
    void useOlderNote(Number(index));
  }
});
copyOlderNoteViewButton.addEventListener("click", copyOlderNoteView);
closeOlderNoteViewButton.addEventListener("click", closeOlderNoteView);
preservedNotes.addEventListener("toggle", () => void listPreservedNotes());
element<HTMLButtonElement>("close-preserved-note").addEventListener("click", () => {
  preservedNotesSeq++;
  clearPreservedNoteView();
  preservedNotesStatus.textContent = "Choose a note to view read only.";
});
element<HTMLButtonElement>("copy-preserved-note").addEventListener("click", () => {
  void navigator.clipboard.writeText(preservedNoteBody.value).catch((error: unknown) => {
    preservedNotesStatus.textContent = `Could not copy: ${describe(error)}`;
  });
});
pageTab.addEventListener("click", () => selectView("page"));
recentTab.addEventListener("click", () => selectView("recent"));
for (const tab of [pageTab, recentTab]) {
  tab.addEventListener("keydown", (event) => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const target = event.key === "Home" ? pageTab : event.key === "End" ? recentTab
      : tab === pageTab ? recentTab : pageTab;
    selectView(target === pageTab ? "page" : "recent");
    target.focus();
  });
}
recentType.addEventListener("change", () => void loadRecent());
recentReading.addEventListener("change", () => void loadRecent());
recentMore.addEventListener("click", () => void loadMoreRecent());
recentReload.addEventListener("click", () => void loadRecent());
recentTitle.addEventListener("input", scheduleRecent);
for (const radio of worklogPeriodRadios.values()) {
  radio.addEventListener("change", () => {
    selectedWork.clear();
    void loadWorklog();
  });
}
curateWorklog.addEventListener("change", () => void loadWorklog());
worklogShareScope.addEventListener("change", () => {
  resetWorklogSelection();
  void loadWorklog();
});
worklogFocusSession.addEventListener("change", () => void loadWorklog());
worklogMore.addEventListener("click", () => void loadMoreWorklog());
worklogSelectAll.addEventListener("click", () => {
  if (!worklogReport || worklogReport.type === "worklog_reported") return;
  for (const row of worklogReport.choices) selectedWork.add(worklogChoiceKey(row));
  renderWorklogChoices();
  renderWorklog();
});
worklogClear.addEventListener("click", () => {
  selectedWork.clear();
  renderWorklogChoices();
  renderWorklog();
});
includeWorklogGaps.addEventListener("change", renderWorklog);
copyWorklogButton.addEventListener("click", copyWorklog);
summarizeButton.addEventListener("click", () => void summarize());
taskForm.addEventListener("submit", (event) => {
  event.preventDefault?.();
  void runTask();
});
// Slash is a panel shortcut only. Native text fields, editable descendants,
// composed text and modified shortcuts keep their ordinary keys.
document.addEventListener("keydown", (event) => {
  if (event.key !== "/" || event.defaultPrevented || event.isComposing || event.keyCode === 229 ||
      event.ctrlKey || event.altKey || event.metaKey || event.shiftKey || closed || taskStarting || taskBar.hidden) return;
  const target = event.target as HTMLElement | null;
  if (target && (["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName) ||
      target.isContentEditable || target.closest?.("[contenteditable]"))) return;
  event.preventDefault();
  taskInput.focus();
});

async function recallTask(direction: 1 | -1): Promise<void> {
  try {
    const text = await taskRecall.move(direction, taskInput.value);
    if (text === null) return;
    clearRunLine(taskRunArea);
    taskInput.value = text;
    const caret = direction === 1 ? 0 : text.length;
    taskInput.setSelectionRange(caret, caret);
  } catch (error) {
    if (followedTabId !== null) {
      showRunLine(taskRunArea, followedTabId, `Could not recall earlier instructions: ${describe(error)}. Try again.`, true);
    }
  }
}

// Enter runs the task; Shift+Enter adds a newline, and Enter that ends an
// IME composition is left to the input method.
taskInput.addEventListener("keydown", (event) => {
  if ((event.key === "ArrowUp" || event.key === "ArrowDown") && !taskStarting && !closed &&
      session.connected && harnessReady(session) && !event.defaultPrevented && !event.isComposing && event.keyCode !== 229 &&
      !event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey) {
    const caret = taskInput.selectionStart;
    if (caret === taskInput.selectionEnd &&
        (event.key === "ArrowUp" ? caret === 0 : caret === taskInput.value.length)) {
      event.preventDefault();
      void recallTask(event.key === "ArrowUp" ? 1 : -1);
      return;
    }
  }
  if (event.key !== "Enter" || event.shiftKey || event.isComposing || event.keyCode === 229) return;
  event.preventDefault?.();
  void runTask();
});
taskCommands.addEventListener("toggle", () => {
  if (taskCommands.open && commandRevision === null && !taskStarting) void loadTaskCommands();
});
taskCommandRefresh.addEventListener("click", () => { if (!taskStarting) void loadTaskCommands(); });
taskCommand.addEventListener("change", () => {
  taskCommandInsert.disabled = taskStarting || taskCommand.disabled || !taskCommand.value;
});
taskCommandInsert.addEventListener("click", () => {
  if (taskStarting || taskCommand.disabled || !taskCommand.value) return;
  const inserted = `/${taskCommand.value} ${taskInput.value}`;
  if (inserted.length > 4000) {
    taskCommandStatus.textContent = "Shorten the draft before inserting a command.";
    taskCommand.value = "";
    return;
  }
  taskRecall.reset();
  taskInput.value = inserted;
  taskCommand.value = "";
  taskCommandInsert.disabled = true;
  taskInput.focus();
});
taskUsePage.addEventListener("change", () => { renderTaskBar(); void probeSelection(); });
taskInput.addEventListener("input", () => taskRecall.edited());
taskInput.addEventListener("focus", () => void probeSelection());
// The selection may have changed in the page since the bar got focus: check
// again as Run is pressed and when the panel's window gets focus back.
taskRunButton.addEventListener("pointerdown", () => void probeSelection());
window.addEventListener("focus", () => void probeSelection());
taskHintSettings.addEventListener("click", openSettings);
taskOpenButton.addEventListener("click", openTasks);
// The tracker does not poll while the panel is hidden; showing it again
// catches up at once.
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "hidden") return;
  taskNoticeSignal("task_notice_opened");
  if (tracker !== null) void refreshTasks();
  void checkForUpdate();
});

// --- Newer build on disk (scripts/dev/deploy-live) ---------------------------
const updates = chromeManifests();

/** Shows the update notice while a newer build than the running one is on
 * disk (build-update.ts). Checked at startup, when the panel is shown again,
 * and on every status poll, since a deploy usually lands while it is open. */
async function checkForUpdate(): Promise<void> {
  if (closed) return;
  const disk = await newerBuildOnDisk(updates);
  if (!closed) updateWarning.hidden = disk === null;
}

/** Saves the note, then reloads the whole extension so the worker and this
 * panel both run the new build. A note that did not save keeps the panel. */
async function reloadForUpdate(): Promise<void> {
  updateReloadButton.disabled = true;
  updateMessage.textContent = "Saving your note…";
  const copies = notes.copyOutCount;
  await notes.flush();
  const refusal = notes.hasUnsavedEdit
    ? "Your note is not saved, so Brauser was not reloaded. Copy the note or fix the save problem, then reload."
    // A conflict keeps the edit in the copy-out box instead of saving it.
    : notes.copyOutCount > copies
      ? "Your note changed elsewhere, so your edit was not saved. It is kept in the note area; copy it if you need it, then reload."
      : notes.recoveryAtRisk
        ? "A local copy of your unsaved text could not be kept, so Brauser was not reloaded. Copy the text, then reload."
        : null;
  if (refusal !== null) {
    updateMessage.textContent = refusal;
    updateReloadButton.disabled = false;
    return;
  }
  chrome.runtime.reload();
}
updateReloadButton.addEventListener("click", () => void reloadForUpdate());
void checkForUpdate();
pageSummaryOpen.addEventListener("click", () => void openPageSummary());
chrome.runtime.onMessage.addListener((message, sender) => {
  if (sender.id === chrome.runtime.id && isPanelNotice(message) && showNotice(message)) {
    void clearStoredNotice();
  }
  return false;
});
session.watchPolicy(() => {
  // The config reload is debounced; invalidate history immediately so a
  // reply from the old home/revision cannot publish during that wait.
  draftPolicyPending = true;
  invalidateRecent();
  recentStatus.textContent = "Settings changed. Reloading Recent…";
  resetWorklogSelection();
  sharingDraftPanel?.render();
  worklogText.textContent = "Settings changed. Reloading worklog…";
  scheduleReload();
});
// The Tasks page may leave a Reuse instruction while this panel is open.
chrome.storage.onChanged.addListener((changes, areaName) => {
  if (areaName === "session" && REUSE_KEY in changes) void checkReuse();
});
chrome.tabs.onActivated.addListener(() => void refreshActiveTab());
// Withdrawing or restoring site access in Chrome changes what this tab shows.
chrome.permissions.onAdded.addListener(() => void refreshActiveTab());
chrome.permissions.onRemoved.addListener(() => void refreshActiveTab());
chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
  if (followsTabUpdate(tabId, changeInfo, tab, followedTabId)) void refreshActiveTab();
});
chrome.tabs.onReplaced?.addListener((addedTabId, removedTabId) => {
  if (followsTabReplacement(addedTabId, removedTabId, followedTabId)) void refreshActiveTab();
});
const pollTimer = setInterval(() => {
  void panelTick();
  void checkForUpdate();
}, STATUS_POLL_MS);
window.addEventListener("pagehide", () => {
  closed = true;
  sharingDraftPanel?.invalidate();
  taskRecall.reset();
  pendingTaskFocus = null;
  clearInterval(pollTimer);
  if (reloadTimer !== null) clearTimeout(reloadTimer);
  if (recentTimer !== null) clearTimeout(recentTimer);
  runCards.dispose();
  tracker?.dispose();
  // Best effort: MV3 gives no guarantee this completes before teardown.
  void notes.flush();
  session.host.disconnect();
});

// Independent of the host connection: a Reuse value left before this panel
// opened is claimed as soon as its own window is known.
void checkReuse();

void (async () => {
  busy = true;
  updateControls();
  let initialized = false;
  let reachedHost = false;
  try {
    const hello = await session.hello();
    reachedHost = true;
    showHostBuild(hello.host_version);
    await reloadConfig();
    if (hello.config_issue) show(hello.config_issue, true);
    initialized = true;
  } catch (error) {
    // Only a failed hello means the host is missing; later failures come from
    // a host that answered, so reinstalling it would not help.
    show(reachedHost
      ? `Could not read the host settings: ${describe(error)}. Open settings to check the configuration, then reopen the panel.`
      : `Native host unavailable: ${describe(error)}. Install or register the development host, then reopen the panel.`, true);
  } finally {
    busy = false;
    await session.refreshStatus().catch(() => undefined);
    updateControls();
    // A worker-policy failure can follow a successful host config read.
    // History can still load once connected, even if capture setup failed.
    retryWaitingHistory();
  }
  await refreshActiveTab();
  // list_runs makes the host start the registered service, which runs on
  // demand (spec §5), so the panel asks only when a harness is set up or a
  // run may still be there: one started before the harness was removed
  // still needs its card and Cancel.
  if (session.config?.agent || await runsMayExist(chrome.storage.local)) {
    // Tasks first, so the summary restore knows whether one is active; a
    // list failure notice from either stays under the bar.
    await refreshTasks();
    await restoreCards();
  }
  // A panel closed meanwhile leaves a stored notice for the next one.
  if (closed) return;
  await showStoredNotice(true);
  // Opening the panel is a natural moment to catch up; the worker does it.
  if (initialized) await replayVisits();
})();
