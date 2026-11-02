// Auto-summary candidates (DESIGN.md "Auto-summary"): which open tabs show a
// recognized artifact that should be summarized, and when each is due. Pure
// state; the worker keeps it in chrome.storage.session and sends
// start_auto_summary for due tabs, oldest first.

/** A tab showing a recognized artifact that has not been submitted yet. */
export interface AutoCandidate {
  artifact_id: string;
  url: string;
  title: string | null;
  /** When the tab first showed this artifact, for first-in, first-out order. */
  queued_at: number;
  /** The page finished loading (tabs.onUpdated status "complete"). */
  loaded: boolean;
  /** Focused time already counted, in milliseconds. */
  focused_ms: number;
  /** When the current focused stretch began, or null while unfocused. */
  focused_since: number | null;
}

export interface AutoSummaryState {
  version: 1;
  candidates: Record<string, AutoCandidate>;
  /** Artifacts already submitted this browser session, oldest first. */
  submitted: string[];
  /** No attempt before this time (ms), after the host could not be reached. */
  retry_after?: number;
  /** Artifacts not to try again before a time (ms): a refusal, an unreadable
   * page, or a run just started. Cleared when the host's config changes. */
  skipped?: Record<string, number>;
  /** The lease revision the skips were recorded under. */
  revision?: string;
}

/** How many submitted artifact IDs are remembered. */
export const MAX_SUBMITTED = 500;

export function emptyAutoSummaryState(): AutoSummaryState {
  return { version: 1, candidates: {}, submitted: [] };
}

/** The tab now shows `match` (null: no recognized artifact). The same
 * artifact keeps its candidate; another one replaces it; one already
 * submitted makes none. */
export function noteArtifact(
  state: AutoSummaryState,
  tabId: number,
  match: { artifact_id: string; url: string; title: string | null } | null,
  now: number,
): void {
  const key = String(tabId);
  const current = state.candidates[key];
  if (match && current?.artifact_id === match.artifact_id) return;
  delete state.candidates[key];
  if (!match || state.submitted.includes(match.artifact_id)) return;
  if ((state.skipped?.[match.artifact_id] ?? 0) > now) return;
  state.candidates[key] = {
    artifact_id: match.artifact_id, url: match.url, title: match.title,
    queued_at: now, loaded: false, focused_ms: 0, focused_since: null,
  };
}

/** The tab's page finished loading. */
export function noteLoaded(state: AutoSummaryState, tabId: number): void {
  const candidate = state.candidates[String(tabId)];
  if (candidate) candidate.loaded = true;
}

/** `focusedTabId` is now the focused tab (null: none, such as Chrome in the
 * background or the user idle). Ends every other focused stretch. */
export function noteAttention(state: AutoSummaryState, focusedTabId: number | null, now: number): void {
  for (const [key, candidate] of Object.entries(state.candidates)) {
    const focused = focusedTabId !== null && key === String(focusedTabId);
    if (candidate.focused_since !== null && !focused) {
      candidate.focused_ms += Math.max(0, now - candidate.focused_since);
      candidate.focused_since = null;
    } else if (candidate.focused_since === null && focused) {
      candidate.focused_since = now;
    }
  }
}

export function noteClosed(state: AutoSummaryState, tabId: number): void {
  delete state.candidates[String(tabId)];
}

function focusedMs(candidate: AutoCandidate, now: number): number {
  const running = candidate.focused_since === null ? 0 : Math.max(0, now - candidate.focused_since);
  return candidate.focused_ms + running;
}

function byQueue(state: AutoSummaryState): [string, AutoCandidate][] {
  return Object.entries(state.candidates).sort(([, a], [, b]) => a.queued_at - b.queued_at);
}

/** Loaded tabs focused for at least `dwellMs` in total (any loaded tab when
 * `dwellMs` is 0), oldest candidate first. */
export function dueTabs(state: AutoSummaryState, now: number, dwellMs: number): number[] {
  return byQueue(state)
    .filter(([, candidate]) => candidate.loaded && (dwellMs === 0 || focusedMs(candidate, now) >= dwellMs))
    .map(([key]) => Number(key));
}

/** Milliseconds until the next loaded candidate becomes due, 0 when one is
 * due now, or null when none can become due without more focus. */
export function nextDueInMs(state: AutoSummaryState, now: number, dwellMs: number): number | null {
  let next: number | null = null;
  for (const candidate of Object.values(state.candidates)) {
    if (!candidate.loaded) continue;
    const remaining = Math.max(0, dwellMs - focusedMs(candidate, now));
    if (remaining > 0 && candidate.focused_since === null) continue;
    next = next === null ? remaining : Math.min(next, remaining);
  }
  return next;
}

/** The tab's artifact was submitted: no tab is a candidate for it again
 * this session. */
export function markSubmitted(state: AutoSummaryState, tabId: number): void {
  const candidate = state.candidates[String(tabId)];
  if (!candidate) return;
  for (const [key, other] of Object.entries(state.candidates)) {
    if (other.artifact_id === candidate.artifact_id) delete state.candidates[key];
  }
  state.submitted = state.submitted.filter((id) => id !== candidate.artifact_id);
  state.submitted.push(candidate.artifact_id);
  if (state.submitted.length > MAX_SUBMITTED) {
    state.submitted.splice(0, state.submitted.length - MAX_SUBMITTED);
  }
}

/** Forget the tab's candidate without recording its artifact, so a later
 * open tries again. */
export function dropCandidate(state: AutoSummaryState, tabId: number): void {
  delete state.candidates[String(tabId)];
}

/** Do not try the tab's artifact again before `until` (ms), in any tab. */
export function markSkipped(state: AutoSummaryState, tabId: number, until: number): void {
  const candidate = state.candidates[String(tabId)];
  if (!candidate) return;
  for (const [key, other] of Object.entries(state.candidates)) {
    if (other.artifact_id === candidate.artifact_id) delete state.candidates[key];
  }
  state.skipped = { ...state.skipped, [candidate.artifact_id]: until };
  const ids = Object.keys(state.skipped);
  // Bounded like `submitted`: the soonest-expiring skips go first.
  if (ids.length > MAX_SUBMITTED) {
    ids.sort((a, b) => state.skipped![a]! - state.skipped![b]!);
    for (const id of ids.slice(0, ids.length - MAX_SUBMITTED)) delete state.skipped[id];
  }
}

/** Forget every skip, so each artifact is tried again on its next open. */
export function clearSkips(state: AutoSummaryState): void {
  delete state.skipped;
}
