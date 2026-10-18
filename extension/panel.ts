import {
  exactOriginPattern,
  isHttpUrl,
  localTimestamp,
  type WorkerStatus,
} from "./model.js";
import { ConfigSession, describe, element, setupProblem, worker } from "./settings.js";
import { PROTOCOL_VERSION, newRequestId } from "./native.js";
import { APP_NAME } from "./brand.js";
import { recordNoteOrigin } from "./grants.js";
import { NoteEditor, type UnsavedText } from "./note-editor.js";
import type {
  ArtifactListItem,
  ArtifactView,
  OlderNote,
  WorklogResult,
} from "../protocol/ts/generated.js";

const status = element<HTMLDivElement>("status");
const settingsButton = element<HTMLButtonElement>("open-settings");
const setupWarning = element<HTMLDivElement>("setup-warning");
const setupReason = element<HTMLSpanElement>("setup-reason");
const setupSettingsButton = element<HTMLButtonElement>("setup-open-settings");
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
const noteUnavailable = element<HTMLParagraphElement>("note-unavailable");
const noteBody = element<HTMLTextAreaElement>("note-body");
const noteGrantOrigin = element<HTMLButtonElement>("note-grant-origin");
const noteConflict = element<HTMLDivElement>("note-conflict");
const noteConflictMessage = noteConflict.querySelector("p")!;
const noteUnsaved = element<HTMLTextAreaElement>("note-unsaved");
const copyUnsavedButton = element<HTMLButtonElement>("copy-unsaved");
const artifactStatus = element<HTMLParagraphElement>("artifact-status");
const artifactCard = element<HTMLElement>("artifact-card");
const artifactHeading = element<HTMLHeadingElement>("artifact-heading");
const artifactIdentity = element<HTMLParagraphElement>("artifact-identity");
const artifactMeasures = element<HTMLParagraphElement>("artifact-measures");
const artifactLinks = element<HTMLParagraphElement>("artifact-links");
const artifactMode = element<HTMLSpanElement>("artifact-mode");
const artifactWhy = element<HTMLInputElement>("artifact-why");
const saveWhyButton = element<HTMLButtonElement>("save-why");
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

const session = new ConfigSession(render);
let busy = false;
let replaying = false;
let tickRunning = false;
let closed = false;
let reloadTimer: ReturnType<typeof setTimeout> | null = null;
// The worker delivers capture records and renews its lease on its own; the panel only
// polls to show the queue and to settle Chrome grant revocations.
const STATUS_POLL_MS = 30_000;
const NOTE_SAVE_DEBOUNCE_MS = 1_000;

// --- Page area (§5.3, §5.7) --------------------------------------------------
// The panel follows the active tab. `tabRefreshSeq` lets a refresh whose tab
// query was overtaken by a newer one be ignored; the note itself is owned by
// `notes` (note-editor.ts), which serializes every load and save.
let tabRefreshSeq = 0;
let grantOrigin: string | null = null;
let currentPage: { url: string; title: string } | null = null;
let currentArtifact: ArtifactView | null = null;
let olderNoteEntries: OlderNote[] = [];
// Tracks the artifact the "Why I'm here" field was last rendered for, so an
// unrelated tab/title refresh for the SAME artifact never overwrites text the
// user has not saved yet; switching to a different artifact always resets it.
let whyArtifactId: string | null = null;
let whyEdited = false;
let recentTimer: ReturnType<typeof setTimeout> | null = null;
let recentLoadSeq = 0;
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

function renderArtifact(artifact: ArtifactView | null, recognized: boolean): void {
  currentArtifact = artifact;
  artifactCard.hidden = artifact === null;
  artifactStatus.textContent = recognized ? "● Artifact recognized" : "○ Not an artifact";
  if (!artifact) {
    whyArtifactId = null;
    whyEdited = false;
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
  artifactMeasures.textContent = artifact.first_seen
    ? `Focused ${duration(artifact.focused_seconds)} over ${artifact.days_opened} day${artifact.days_opened === 1 ? "" : "s"} · ${duration(artifact.background_seconds)} background · last ${artifact.last_seen}`
    : "Recognized; no delivered activity yet.";
  artifactLinks.textContent = artifact.linked_count
    ? `${artifact.linked_count} linked artifact${artifact.linked_count === 1 ? "" : "s"}`
    : "No linked artifacts yet.";
  if (artifact.artifact_id !== whyArtifactId) {
    whyArtifactId = artifact.artifact_id;
    whyEdited = false;
  }
  // A refresh triggered by an unrelated tab/title event for this same
  // artifact must never clobber context the user is still typing.
  if (!whyEdited) {
    artifactWhy.value = artifact.why ?? "";
  }
}

async function refreshArtifact(): Promise<void> {
  const page = currentPage;
  if (!page || !session.connected || !session.config?.storage) {
    renderArtifact(null, false);
    return;
  }
  try {
    const response = await session.host.call({
      type: "get_artifact",
      protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(),
      url: page.url,
      title: page.title,
    }, "artifact_result");
    if (page !== currentPage) return;
    renderArtifact(response.artifact, response.recognized);
  } catch (error) {
    if (page !== currentPage) return;
    // A transient fetch failure must not discard an in-progress "Why I'm
    // here" draft or the card's last-known state (renderArtifact(null, ...)
    // would reset both); only report the error.
    artifactStatus.textContent = `Artifact status unavailable: ${describe(error)}`;
  }
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
  if (!page || !artifact || !text || changing()) return;
  saveWhyButton.disabled = true;
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
    await refreshArtifact();
    show("Artifact context saved.");
  } catch (error) {
    show(`Could not save artifact context: ${describe(error)}`, true);
  } finally {
    saveWhyButton.disabled = false;
  }
}

function renderOlderNotes(entries: readonly OlderNote[]): void {
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
    const useButton = document.createElement("button");
    useButton.type = "button";
    useButton.className = "secondary compact";
    useButton.dataset.olderIndex = String(index);
    useButton.dataset.olderAction = "use";
    useButton.textContent = "Use as artifact note";
    item.append(label, viewButton, useButton);
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
  return busy || replaying || tickRunning;
}

function updateControls(): void {
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
  if (state.locally_removed_sites.length) {
    messages.push("Some sites are locally off until their removal is saved in the host.");
  }
  if (!state.navigation_ready && session.config?.capture_enabled) {
    messages.push("Chrome navigation listener is unavailable; activity capture has not started.");
  }
  queueWarning.textContent = messages.join(" ");
  // Open the strip on its own once there is something worth surfacing.
  if (messages.length && !captureStrip.open) captureStrip.open = true;
}

function render(): void {
  renderSetupWarning();
  if (session.status) renderQueue(session.status);
  updateControls();
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
    : "Capture is off.");
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
        void refreshArtifact();
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
    const state = await session.refreshStatus();
    if (state.revoked_origins.length) await session.reconcileRevocations();
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
    show("Capture paused. Pending capture records cleared. Enable a site in settings to resume.");
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
}, describe, NOTE_SAVE_DEBOUNCE_MS);

async function refreshGrantButton(origin: string): Promise<void> {
  try {
    const granted = await chrome.permissions.contains({ origins: [exactOriginPattern(origin)] });
    grantOrigin = granted ? null : origin;
    noteGrantOrigin.hidden = granted;
  } catch {
    grantOrigin = null;
    noteGrantOrigin.hidden = true;
  }
}

function requestNoteOrigin(): void {
  const origin = grantOrigin;
  if (!origin) return;
  // This call must run in the click stack to keep Chrome's user gesture.
  const granted = chrome.permissions.request({ origins: [exactOriginPattern(origin)] });
  void granted.then((ok) => {
    if (ok) {
      grantOrigin = null;
      noteGrantOrigin.hidden = true;
      // Removing a logging site for this origin must not revoke the grant.
      return recordNoteOrigin(origin);
    } else {
      showNoteStatus("Chrome access was declined.", true);
    }
  }).catch((error: unknown) => showNoteStatus(describe(error), true));
}

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

function recentRow(item: ArtifactListItem): HTMLElement {
  const row = document.createElement("a");
  row.className = "recent-row";
  row.href = item.url;
  row.target = "_blank";
  row.rel = "noopener noreferrer";
  const title = document.createElement("strong");
  title.textContent = item.title;
  const details = document.createElement("span");
  details.textContent = `${item.artifact_type} · focused ${duration(item.focused_seconds)} · background ${duration(item.background_seconds)}${item.opens ? ` · opened ${item.opens}×` : ""}`;
  row.append(title, details);
  if (item.why) {
    const why = document.createElement("span");
    why.textContent = `Why: ${item.why}`;
    row.append(why);
  }
  return row;
}

function renderRecent(items: readonly ArtifactListItem[]): void {
  recentList.replaceChildren();
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
  if (!session.connected || !session.config?.storage) return;
  const seq = ++recentLoadSeq;
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
  if (!session.connected || !session.config?.storage) return;
  const seq = ++worklogLoadSeq;
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

/** Follows the active tab (§5.7). The header and the note editor are pointed
 * at the tab this call found in the same step, so they cannot disagree;
 * `notes` saves the page being left before loading the new one. */
async function refreshActiveTab(): Promise<void> {
  const seq = ++tabRefreshSeq;
  let tab: ChromeTab | undefined;
  try {
    [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  } catch {
    tab = undefined;
  }
  if (seq !== tabRefreshSeq) return; // A newer refresh has the current tab.

  if (!tab || tab.incognito) {
    currentPage = null;
    renderArtifact(null, false);
    pageTitle.textContent = "No page yet";
    pageUrl.textContent = "";
    pageUnusable.hidden = true;
    noteUnavailable.hidden = true;
    noteGrantOrigin.hidden = true;
    await notes.showPage(null);
    return;
  }

  if (tab.url === undefined) {
    currentPage = null;
    renderArtifact(null, false);
    // §5.3: without an origin grant or a live activeTab, the panel cannot
    // see this tab's address at all.
    pageTitle.textContent = tab.title?.trim() || "This page";
    pageUrl.textContent = "";
    pageUnusable.hidden = true;
    noteUnavailable.hidden = false;
    noteGrantOrigin.hidden = true;
    await notes.showPage(null);
    return;
  }

  let url: URL;
  try {
    url = new URL(tab.url);
  } catch {
    url = new URL("about:blank");
  }
  if (!isHttpUrl(url)) {
    currentPage = null;
    renderArtifact(null, false);
    pageTitle.textContent = tab.title?.trim() || tab.url;
    pageUrl.textContent = tab.url;
    pageUnusable.hidden = false;
    noteUnavailable.hidden = true;
    noteGrantOrigin.hidden = true;
    await notes.showPage(null);
    return;
  }

  pageTitle.textContent = tab.title?.trim() || url.hostname;
  pageUrl.textContent = tab.url;
  pageUnusable.hidden = true;
  noteUnavailable.hidden = true;
  currentPage = { url: tab.url, title: tab.title?.trim() || url.hostname };
  void refreshGrantButton(url.origin);
  await notes.showPage({ url: tab.url, title: tab.title?.trim() || url.hostname });
  void refreshArtifact();
}

settingsButton.addEventListener("click", openSettings);
setupSettingsButton.addEventListener("click", openSettings);
pauseButton.addEventListener("click", () => void pauseCapture());
replayButton.addEventListener("click", () => void replayVisits());
discardButton.addEventListener("click", () => void discardPendingVisits());
dismissButton.addEventListener("click", () => void dismissNotices());
noteBody.addEventListener("input", () => notes.edited());
noteGrantOrigin.addEventListener("click", requestNoteOrigin);
copyUnsavedButton.addEventListener("click", copyUnsavedText);
saveWhyButton.addEventListener("click", () => void saveWhy());
artifactWhy.addEventListener("input", () => {
  whyEdited = true;
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
session.watchPolicy(scheduleReload);
chrome.tabs.onActivated.addListener(() => void refreshActiveTab());
chrome.tabs.onUpdated.addListener((_tabId, changeInfo) => {
  if (changeInfo.title !== undefined || changeInfo.url !== undefined || changeInfo.status === "complete") {
    void refreshActiveTab();
  }
});
const pollTimer = setInterval(() => void panelTick(), STATUS_POLL_MS);
window.addEventListener("pagehide", () => {
  closed = true;
  clearInterval(pollTimer);
  if (reloadTimer !== null) clearTimeout(reloadTimer);
  if (recentTimer !== null) clearTimeout(recentTimer);
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
  }
  await refreshActiveTab();
  // Opening the panel is a natural moment to catch up; the worker does it.
  if (initialized) await replayVisits();
})();
