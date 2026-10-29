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
import { ConfigSession, captureStateText, describe, element, leaseFor, setupProblem, worker } from "./settings.js";
import { evaluate } from "./adapter-templates.js";
import { HostError, PROTOCOL_VERSION, newRequestId } from "./native.js";
import { CaptureSitePanel } from "./capture-site-panel.js";
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
import { recentSummaryText, summaryLine } from "./summary-text.js";
import { NoteEditor } from "./note-editor.js";
import { NoteRecovery, type RecoveryText } from "./note-recovery.js";
import {
  ArtifactRefresher, deliveredSince, followsTabReplacement, followsTabUpdate,
} from "./artifact-refresh.js";
import type {
  ArtifactListItem,
  ArtifactRefView,
  ArtifactView,
  ContainedArtifactView,
  OlderNote,
  RunView,
  SummaryView,
  WorklogReported,
} from "../protocol/ts/generated.js";

initDiag("panel");

const status = element<HTMLDivElement>("status");
const buildId = element<HTMLSpanElement>("build-id");
const hostBuild = element<HTMLSpanElement>("host-build");

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
const recentStatus = element<HTMLParagraphElement>("recent-status");
const recentList = element<HTMLDivElement>("recent-list");
const worklogPeriod = element<HTMLSelectElement>("worklog-period");
const worklogText = element<HTMLElement>("worklog-text");
const copyWorklogButton = element<HTMLButtonElement>("copy-worklog");
const worklogGaps = element<HTMLElement>("worklog-gaps");
const worklogGapStatus = element<HTMLElement>("worklog-gap-status");
const worklogGapList = element<HTMLElement>("worklog-gap-list");
const includeWorklogGaps = element<HTMLInputElement>("worklog-include-gaps");
const summarizeButton = element<HTMLButtonElement>("summarize");
const runArea = element<HTMLDivElement>("run-area");
const pageSummary = element<HTMLDivElement>("page-summary");
const pageSummaryText = element<HTMLParagraphElement>("page-summary-text");
const pageSummaryOpen = element<HTMLButtonElement>("page-summary-open");
const taskBar = element<HTMLElement>("task-bar");
const taskForm = element<HTMLFormElement>("task-form");
const taskInput = element<HTMLTextAreaElement>("task-input");
const taskRunButton = element<HTMLButtonElement>("task-run");
const taskContext = element<HTMLParagraphElement>("task-context");
const taskHint = element<HTMLParagraphElement>("task-hint");
const taskHintText = element<HTMLSpanElement>("task-hint-text");
const taskHintSettings = element<HTMLButtonElement>("task-hint-settings");
const taskAnnounce = element<HTMLParagraphElement>("task-announce");
const taskRunArea = element<HTMLDivElement>("task-run-area");
const taskOpenButton = element<HTMLButtonElement>("task-open");

const session = new ConfigSession(render);
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
let recentTimer: ReturnType<typeof setTimeout> | null = null;
// The shown Recent rows' Open summary buttons, disabled with the host.
let recentOpenButtons: HTMLButtonElement[] = [];
let recentLoadSeq = 0;
let recentWaitingForHost = false;
let worklogWaitingForHost = false;
let worklogLoadSeq = 0;
let worklogReport: WorklogReported | null = null;

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
  pageSummaryText.textContent = summary !== null && summaryPageUrl !== null ? summaryLine(summary) : "";
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
    if (!compiled) {
      reason = "Capture rules could not be read. Repair them in settings.";
    } else {
      const result = evaluate(compiled, currentPage.url, currentPage.title);
      if (result.match) {
        captureExplanation.hidden = true;
        captureExplanationText.textContent = "";
        return;
      }
      switch (result.reason) {
        case "out_of_scope":
          reason = "This page is outside the workspaces or accounts chosen for capture. Review this site's scope in settings.";
          break;
        case "no_adapter":
          reason = "This site has no capture rules. Review Capture Site to choose which pages to remember.";
          captureExplanationAction.textContent = "Review Capture Site";
          captureExplanationAction.dataset.action = "capture-site";
          captureExplanationAction.disabled ||= element<HTMLButtonElement>("capture-site").disabled;
          break;
        case "no_rule":
          reason = "This page does not match this site's capture rules. Only matching pages are remembered. Review the rules in settings.";
          break;
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
  dismissButton.disabled = changing() || !state || (!state.last_error &&
    !state.overflow_count && !state.rejected_count);
  saveWhyButton.disabled = savingWhy || whySaveBlocked() !== null;
  const ready = harnessReady(session);
  summarizeButton.hidden = currentPage === null;
  summarizeButton.disabled = !ready || summarizing || currentPage?.tabId === undefined;
  summarizeButton.title = ready ? "" : NEEDS_HARNESS_MESSAGE;
  renderTaskBar();
  pageSummaryOpen.disabled = !connected;
  for (const open of recentOpenButtons) open.disabled = !connected;
  const paused = whyPausedReason();
  whyPausedHint.hidden = paused === null;
  if (paused !== null) whyPausedHint.textContent = paused;
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
  const captureOn = session.config?.capture_enabled === true && session.configIssue === null &&
    session.adapterIssue === null &&
    state.pause_pending !== true;
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
  const reconnected = connected && !artifactHostConnected;
  const generation = session.status?.delivered_batches ?? null;
  const delivered = generation !== null && deliveredSince(artifactDeliveryGeneration, generation);
  // Advance before asking the host: its response can notify us again.
  artifactHostConnected = connected;
  artifactDeliveryGeneration = generation;
  renderSetupWarning();
  if (session.status) renderQueue(session.status);
  updateControls();
  retryWaitingHistory();
  if (!closed && (reconnected || delivered)) void refreshArtifact(true);
}

// History needs only the host configuration, not the worker policy, so
// Recent opened before the host answered loads as soon as the session
// connects, even while policy installation is still pending or later fails.
function retryWaitingHistory(): void {
  if (!session.connected || recentView.hidden) return;
  if (recentWaitingForHost) void loadRecent();
  if (worklogWaitingForHost) void loadWorklog();
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
  if (response.config_issue || response.adapter_issue) {
    show(response.config_issue ?? response.adapter_issue ?? "Adapter policy is invalid.", true);
  }
  else show(session.config!.capture_enabled
    ? captureStateText(session.config!)
    : "Capture is paused. Resume it in settings.");
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
  if (pageState === "site-access") {
    return { text: "Re-enable site access in Chrome so the agent can read this page.", settings: false };
  }
  if (currentPage === null || currentPage.tabId === undefined) return { text: "Tasks work on http(s) pages.", settings: false };
  return null;
}

function renderTaskBar(): void {
  const blocked = taskBlocked();
  taskBar.hidden = followedTabId === null;
  taskRunButton.disabled = blocked !== null || taskStarting;
  const label = taskStarting ? "Starting…" : "Run";
  if (taskRunButton.textContent !== label) taskRunButton.textContent = label;
  taskInput.readOnly = taskStarting;
  taskHint.hidden = blocked === null;
  if (taskHintText.textContent !== (blocked?.text ?? "")) taskHintText.textContent = blocked?.text ?? "";
  taskHintSettings.hidden = blocked?.settings !== true;
  renderTaskContext();
}

/** "Uses this page: <title>", naming the selection when the page has one,
 * so the user sees which page a task will get. */
function renderTaskContext(): void {
  const page = currentPage;
  let text = "";
  if (page !== null && page.tabId !== undefined) {
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
  if (page === null || page.tabId === undefined || !harnessReady(session)) {
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
  if (taskStarting || taskBlocked() !== null || !page || page.tabId === undefined) return;
  if (cleanInstruction(instruction) === "") return;
  const tabId = page.tabId;
  taskStarting = true;
  updateControls();
  showRunLine(taskRunArea, tabId, "Reading this page…");
  try {
    await showSetupWarningOnce(taskRunArea);
    const run = await startTask(
      { id: tabId, url: page.url, title: page.title },
      instruction,
      { scripting: chrome.scripting, host: session.host, storage: chrome.storage.local, now: () => new Date() },
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
  } finally {
    taskStarting = false;
    updateControls();
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
    now: () => Date.now(),
    visible: () => document.visibilityState !== "hidden",
    changed: ({ running, newestId }) => {
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
  const instruction = await takeReuse(chrome.storage.session, windowId).catch(() => null);
  if (!closed && instruction !== null && !taskStarting) {
    taskInput.value = instruction;
    taskInput.focus();
  }
}

/** Restores the summary cards. The run record stays while a task is active,
 * or when the task list failed and nothing is known. */
function restoreCards(): Promise<void> {
  return restoreRunCards([runCards], taskRunArea, {
    host: session.host, now: () => Date.now(), storage: chrome.storage.local, openSettings,
  }, { keepRecord: tasksRunning > 0 || trackerError !== null });
}

/** Shows a notice from the worker's shortcut or context-menu summary when it
 * is about the followed tab. Returns whether it is now shown. */
function showNotice(notice: PanelNotice): boolean {
  if (notice.tab_id !== followedTabId) return false;
  const id = `${notice.kind}:${notice.tab_id}:${notice.at}`;
  if (id === shownNotice) return true;
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
  const current = isPanelNotice(stored) && Date.now() - stored.at < PANEL_NOTICE_MAX_AGE_MS ? stored : null;
  if (current && current.tab_id !== followedTabId && !dropOthers) return;
  await clearStoredNotice();
  if (current) showNotice(current);
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
  if (worklogPeriod.value === "yesterday") {
    now.setDate(now.getDate() - 1);
    const day = localDate(now);
    return { start: day, end: day };
  }
  if (worklogPeriod.value === "week") {
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
  const link = document.createElement("a");
  link.className = "recent-link";
  link.href = item.url;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  const title = document.createElement("strong");
  title.textContent = item.title;
  const details = document.createElement("span");
  details.textContent = `${item.artifact_type} · focused ${duration(item.focused_seconds)} · background ${duration(item.background_seconds)}${item.opens ? ` · opened ${item.opens}×` : ""}`;
  link.append(title, details);
  if (item.why) {
    const why = document.createElement("span");
    why.textContent = `Why: ${item.why}`;
    link.append(why);
  }
  row.append(link);
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

function renderRecent(items: readonly ArtifactListItem[]): void {
  recentList.replaceChildren();
  recentOpenButtons = [];
  let day = "";
  for (const item of items) {
    if (item.local_date !== day) {
      day = item.local_date;
      const heading = document.createElement("h3");
      heading.textContent = day;
      recentList.append(heading);
    }
    recentList.append(recentRow(item));
  }
  if (!items.length) {
    const empty = document.createElement("p");
    empty.textContent = "No matching artifact activity yet.";
    recentList.append(empty);
  }
}

async function loadRecent(): Promise<void> {
  const seq = ++recentLoadSeq;
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
      limit: 32,
    }, "artifact_list_result");
    if (seq !== recentLoadSeq) return;
    const selected = recentType.value;
    recentType.replaceChildren(new Option("All types", ""));
    for (const type of response.artifact_types) recentType.add(new Option(type, type));
    recentType.value = selected;
    renderRecent(response.items);
    recentStatus.textContent = `${response.items.length} artifact-day entr${response.items.length === 1 ? "y" : "ies"}${response.truncated ? " · first 32 shown" : ""}`;
  } catch (error) {
    if (seq !== recentLoadSeq) return;
    recentStatus.textContent = `Could not load Recent: ${describe(error)}`;
    renderRecent([]);
  }
}

async function loadWorklog(): Promise<void> {
  const seq = ++worklogLoadSeq;
  worklogReport = null;
  worklogGaps.hidden = true;
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
  worklogText.textContent = "Generating…";
  copyWorklogButton.disabled = true;
  try {
    const response: WorklogReported = await session.host.call({
      type: "worklog_report",
      protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(),
      start_date: range.start,
      end_date: range.end,
    }, "worklog_reported");
    if (seq !== worklogLoadSeq) return;
    if (response.start_date !== range.start || response.end_date !== range.end) throw new Error("The host returned a different worklog period. Try again.");
    worklogReport = response;
    renderWorklog();
    copyWorklogButton.disabled = false;
  } catch (error) {
    if (seq !== worklogLoadSeq) return;
    worklogText.textContent = error instanceof HostError && error.code === "invalid_request" && /unsupported request/.test(error.message)
      ? "Rebuild the Brauser host and extension together, then reopen Recent to review capture gaps."
      : `Could not generate worklog: ${describe(error)}`;
  }
}

function renderWorklog(): void {
  const report = worklogReport;
  if (!report) return;
  const lines = report.gaps.map((gap) =>
    `${gap.count} completed activity-hour record${gap.count === 1 ? "" : "s"} missing from ${gap.first_occurred_at} through ${gap.last_occurred_at}.`);
  const hasGaps = lines.length > 0 || report.gaps_truncated;
  worklogGaps.hidden = !hasGaps;
  worklogGapStatus.textContent = "Known capture gaps intersect this period. Counts cover each full reported range; ranges may overlap. Missing activity is excluded from totals.";
  if (report.gaps_truncated) lines.push("More capture gaps are recorded; only the first 32 notices are shown.");
  worklogGapList.replaceChildren();
  for (const line of lines) {
    const item = document.createElement("li");
    item.textContent = line;
    worklogGapList.append(item);
  }
  worklogText.textContent = report.text + (hasGaps && includeWorklogGaps.checked
    ? `\n\nCapture gaps: this worklog is incomplete. Counts cover full reported ranges, may overlap, and are not added to activity totals.\n${lines.join("\n")}`
    : "");
}

function scheduleRecent(): void {
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
  if (recent) {
    void Promise.all([loadRecent(), loadWorklog()]);
  }
}

function copyWorklog(): void {
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
recentType.addEventListener("change", () => void loadRecent());
recentTitle.addEventListener("input", scheduleRecent);
worklogPeriod.addEventListener("change", () => void loadWorklog());
includeWorklogGaps.addEventListener("change", renderWorklog);
copyWorklogButton.addEventListener("click", copyWorklog);
summarizeButton.addEventListener("click", () => void summarize());
taskForm.addEventListener("submit", (event) => {
  event.preventDefault?.();
  void runTask();
});
// Enter runs the task; Shift+Enter adds a newline, and Enter that ends an
// IME composition is left to the input method.
taskInput.addEventListener("keydown", (event) => {
  if (event.key !== "Enter" || event.shiftKey || event.isComposing || event.keyCode === 229) return;
  event.preventDefault?.();
  void runTask();
});
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
  if (tracker !== null && document.visibilityState !== "hidden") void refreshTasks();
});
pageSummaryOpen.addEventListener("click", () => void openPageSummary());
chrome.runtime.onMessage.addListener((message, sender) => {
  if (sender.id === chrome.runtime.id && isPanelNotice(message) && showNotice(message)) {
    void clearStoredNotice();
  }
  return false;
});
session.watchPolicy(scheduleReload);
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
const pollTimer = setInterval(() => void panelTick(), STATUS_POLL_MS);
window.addEventListener("pagehide", () => {
  closed = true;
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
    // A summary list that works clears the notice; the task list's failure
    // still stands.
    if (trackerError !== null && !closed) showListFailure(taskRunArea, trackerError, openSettings);
  }
  // A panel closed meanwhile leaves a stored notice for the next one.
  if (closed) return;
  await showStoredNotice(true);
  // Opening the panel is a natural moment to catch up; the worker does it.
  if (initialized) await replayVisits();
})();
