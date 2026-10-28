import {
  PANEL_NOTICE_KEY,
  PANEL_NOTICE_MAX_AGE_MS,
  SITE_ACCESS_ORIGINS,
  isHttpUrl,
  isPanelNotice,
  localTimestamp,
  type PanelNotice,
  type WorkerStatus,
} from "./model.js";
import { ConfigSession, describe, element, setupProblem, worker } from "./settings.js";
import { PROTOCOL_VERSION, newRequestId } from "./native.js";
import { CaptureSitePanel } from "./capture-site-panel.js";
import { APP_NAME, storageKey } from "./brand.js";
import { hostBuildText } from "./build-info.js";
import { SETUP_WARNING } from "./agent.js";
import { summarizeFailureText, summarizeTab } from "./summarize.js";
import { RunCards, runsMayExist } from "./run-card.js";
import { recentSummaryText, summaryLine } from "./summary-text.js";
import { NoteEditor, type UnsavedText } from "./note-editor.js";
import { NoteRecovery } from "./note-recovery.js";
import {
  ArtifactRefresher, deliveredSince, followsTabReplacement, followsTabUpdate,
} from "./artifact-refresh.js";
import type {
  ArtifactListItem,
  ArtifactRefView,
  ArtifactView,
  ContainedArtifactView,
  OlderNote,
  SummaryView,
  WorklogResult,
} from "../protocol/ts/generated.js";

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
const noteConflictMessage = noteConflict.querySelector("p")!;
const noteUnsaved = element<HTMLTextAreaElement>("note-unsaved");
const discardUnsavedButton = element<HTMLButtonElement>("discard-unsaved");
const noteRecoveryWarning = element<HTMLParagraphElement>("note-recovery-warning");
const copyUnsavedButton = element<HTMLButtonElement>("copy-unsaved");
const artifactStatus = element<HTMLParagraphElement>("artifact-status");
const artifactCard = element<HTMLElement>("artifact-card");
const artifactHeading = element<HTMLHeadingElement>("artifact-heading");
const artifactIdentity = element<HTMLParagraphElement>("artifact-identity");
const artifactMeasures = element<HTMLParagraphElement>("artifact-measures");
const artifactLinks = element<HTMLParagraphElement>("artifact-links");
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
const summarizeButton = element<HTMLButtonElement>("summarize");
const runArea = element<HTMLDivElement>("run-area");
const pageSummary = element<HTMLDivElement>("page-summary");
const pageSummaryText = element<HTMLParagraphElement>("page-summary-text");
const pageSummaryOpen = element<HTMLButtonElement>("page-summary-open");

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
const SUMMARIZE_WARNING_SEEN_KEY = storageKey("summarize_warning_seen_v1");
const SUMMARIZE_NEEDS_HARNESS = "Set up a summary harness in settings first.";

// --- Page area (§5.3, §5.7) --------------------------------------------------
// The panel follows the active tab. `tabRefreshSeq` lets a refresh whose tab
// query was overtaken by a newer one be ignored; the note itself is owned by
// `notes` (note-editor.ts), which serializes every load and save.
let tabRefreshSeq = 0;
// The tab the panel last resolved as active; only its updates matter.
let followedTabId: number | null = null;
let currentPage: { url: string; title: string; tabId: number | undefined } | null = null;
let currentArtifact: ArtifactView | null = null;
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
// The run area's latest line and the tab it is about; another tab's line goes
// when the panel follows a different tab.
let runLine: { tabId: number; element: HTMLElement } | null = null;
// The last worker notice shown, since one can arrive both live and stored.
let shownNotice: string | null = null;
let recentTimer: ReturnType<typeof setTimeout> | null = null;
// The shown Recent rows' Open summary buttons, disabled with the host.
let recentOpenButtons: HTMLButtonElement[] = [];
let recentLoadSeq = 0;
let recentWaitingForHost = false;
let worklogWaitingForHost = false;
let worklogLoadSeq = 0;

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

function renderArtifact(artifact: ArtifactView | null, recognized: boolean, pageUrl: string | null = null): void {
  currentArtifact = artifact;
  artifactPageUrl = artifact ? pageUrl : null;
  artifactCard.hidden = artifact === null;
  artifactStatus.textContent = recognized ? "● Artifact recognized" : "○ Not an artifact";
  if (!artifact) {
    whyArtifactId = null;
    whyEdited = false;
    renderWithin(null);
    renderContained([]);
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
    renderArtifact(null, false);
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
  if (session.config?.capture_enabled === false) {
    return "Capture is paused, so this can't be saved. Use Resume capture in settings to turn it back on.";
  }
  if (session.status?.pause_pending === true) {
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

function updateControls(): void {
  captureSite?.render(currentPage, changing());
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
  const harnessReady = session.agentStatus?.state === "ready";
  summarizeButton.hidden = currentPage === null;
  summarizeButton.disabled = !harnessReady || summarizing || currentPage?.tabId === undefined;
  summarizeButton.title = harnessReady ? "" : SUMMARIZE_NEEDS_HARNESS;
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
  renderSetupWarning();
  if (session.status) renderQueue(session.status);
  updateControls();
  retryWaitingHistory();
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
    ? "Capture is enabled for the listed sites."
    : "Capture is paused. Resume it in settings.");
  if (session.status?.pause_pending && session.config?.capture_enabled &&
      !session.configIssue && !session.adapterIssue) {
    show("Capture is paused locally. Use Pause capture to finish saving this setting in the host.", true);
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
    void reloadConfig()
      .catch((error: unknown) => show(`Could not refresh settings: ${describe(error)}`, true))
      .finally(() => {
        busy = false;
        updateControls();
        // A note that could not load before (say, no folder yet) may load now.
        void notes.retry();
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
    const previous = session.status?.delivered_batches ?? null;
    const state = await session.refreshStatus();
    // A background sync delivered activity; the card's totals may be stale.
    if (deliveredSince(previous, state.delivered_batches)) void refreshArtifact(true);
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

function withoutRunAreaChild(child: Element): Element[] {
  return Array.from(runArea.children).filter((other) => other !== child);
}

function showRunLine(tabId: number, text: string, warning = false): void {
  const line = document.createElement("p");
  line.className = warning ? "run-line warning" : "run-line";
  line.textContent = text;
  const kept = runLine ? withoutRunAreaChild(runLine.element) : Array.from(runArea.children);
  runArea.replaceChildren(...kept, line);
  runLine = { tabId, element: line };
}

function clearRunLineUnlessFor(tabId: number | undefined): void {
  if (runLine?.tabId !== tabId) clearRunLine();
}

function clearRunLine(): void {
  if (!runLine) return;
  runArea.replaceChildren(...withoutRunAreaChild(runLine.element));
  runLine = null;
}

/** Shows SETUP_WARNING (spec §6.1) with the first summary ever, from any
 * path: the button, or the shortcut or context menu (showNotice). It does not
 * hold the run back. */
async function showSetupWarningOnce(): Promise<void> {
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
  dismiss.addEventListener("click", () => runArea.replaceChildren(...withoutRunAreaChild(box)));
  box.append(text, dismiss);
  runArea.replaceChildren(box, ...Array.from(runArea.children));
}

async function summarize(): Promise<void> {
  const page = currentPage;
  if (summarizing || !page || page.tabId === undefined) return;
  const tabId = page.tabId;
  summarizing = true;
  updateControls();
  showRunLine(tabId, "Reading this page…");
  try {
    await showSetupWarningOnce();
    // A page already being summarized answers with its run, which shows
    // (or keeps) the same card.
    const { run } = await summarizeTab(
      { id: tabId, url: page.url, title: page.title },
      { scripting: chrome.scripting, host: session.host, storage: chrome.storage.local, now: () => new Date() },
    );
    // Run cards are not per tab, so the card shows whichever tab is followed
    // now; only this tab's own line goes.
    if (runLine?.tabId === tabId) clearRunLine();
    runCards.show(run);
  } catch (error) {
    // The panel may follow another tab by now (a slow cold start); this
    // line is about the tab that was summarized, so it is dropped.
    if (followedTabId === tabId) showRunLine(tabId, summarizeFailureText(error), true);
  } finally {
    summarizing = false;
    updateControls();
  }
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
  void showSetupWarningOnce();
  if (notice.kind === "summarize_failed") {
    showRunLine(notice.tab_id, notice.message, true);
  } else {
    // The notice names only the run; list_runs brings its card, as it does
    // for any unfinished run.
    clearRunLine();
    void runCards.restore();
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

function showUnsavedText(entries: readonly UnsavedText[]): void {
  const only = entries.length === 1 ? entries[0]! : null;
  noteConflictMessage.textContent = only
    ? `${only.reason} Copy your text for ${only.url} back in below:`
    : "Some note text could not be saved. Copy it back in below:";
  noteUnsaved.value = only
    ? only.text
    : entries.map((entry) => `${entry.url}\n${entry.reason}\n\n${entry.text}`).join("\n\n");
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
});

function copyUnsavedText(): void {
  void navigator.clipboard.writeText(noteUnsaved.value).catch((error: unknown) => {
    showNoteStatus(`Could not copy: ${describe(error)}`, true);
  });
}

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
    const response: WorklogResult = await session.host.call({
      type: "worklog",
      protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(),
      start_date: range.start,
      end_date: range.end,
    }, "worklog_result");
    if (seq !== worklogLoadSeq) return;
    worklogText.textContent = response.text;
    copyWorklogButton.disabled = false;
  } catch (error) {
    if (seq !== worklogLoadSeq) return;
    worklogText.textContent = `Could not generate worklog: ${describe(error)}`;
  }
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
  pageUnusable.hidden = state !== "unusable";
  siteAccessWarning.hidden = state !== "site-access";
  noteEditor.hidden = state !== "note";
}

async function showNoPage(title: string, url: string, state: "none" | "unusable" | "site-access"): Promise<void> {
  currentPage = null;
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
  currentPage = { url: tab.url, title, tabId: tab.id };
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
setupSettingsButton.addEventListener("click", openSettings);
pauseButton.addEventListener("click", () => void pauseCapture());
replayButton.addEventListener("click", () => void replayVisits());
discardButton.addEventListener("click", () => void discardPendingVisits());
dismissButton.addEventListener("click", () => void dismissNotices());
noteBody.addEventListener("input", () => notes.edited());
siteAccessReenableButton.addEventListener("click", reenableSiteAccess);
copyUnsavedButton.addEventListener("click", copyUnsavedText);
discardUnsavedButton.addEventListener("click", () => notes.discardUnsaved());
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
pageTab.addEventListener("click", () => selectView("page"));
recentTab.addEventListener("click", () => selectView("recent"));
recentType.addEventListener("change", () => void loadRecent());
recentTitle.addEventListener("input", scheduleRecent);
worklogPeriod.addEventListener("change", () => void loadWorklog());
copyWorklogButton.addEventListener("click", copyWorklog);
summarizeButton.addEventListener("click", () => void summarize());
pageSummaryOpen.addEventListener("click", () => void openPageSummary());
chrome.runtime.onMessage.addListener((message, sender) => {
  if (sender.id === chrome.runtime.id && isPanelNotice(message) && showNotice(message)) {
    void clearStoredNotice();
  }
  return false;
});
session.watchPolicy(scheduleReload);
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
  // Best effort: MV3 gives no guarantee this completes before teardown.
  void notes.flush();
  session.host.disconnect();
});

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
  if (session.config?.agent || await runsMayExist(chrome.storage.local)) await runCards.restore();
  await showStoredNotice(true);
  // Opening the panel is a natural moment to catch up; the worker does it.
  if (initialized) await replayVisits();
})();
