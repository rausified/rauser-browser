import type { ActivityEvent, ActivityMode, ActivityTrail } from "../protocol/ts/generated.js";
import type { EvaluateResult, Mode } from "./adapter-templates.js";
import { localTimestamp } from "./model.js";

export type ArtifactMatch = Extract<EvaluateResult, { match: true }>;

export interface ActivityTotal {
  event_id: string;
  artifact_id: string;
  artifact_type: string;
  hour_start: string;
  url: string;
  title: string | null;
  opens: number;
  focused_ms: number;
  background_ms: number;
  modes: ActivityMode[];
  trails: ActivityTrail[];
  refs: { artifact_type: string; value: string }[];
}

export interface ActivityTab {
  artifact_id: string;
  artifact_type: string;
  url: string;
  title: string | null;
  mode: Mode | null;
  window_id: number;
}

export interface ActivityState {
  version: 1;
  totals: ActivityTotal[];
  tabs: Record<string, ActivityTab>;
  pending_sources: Record<string, ActivityTrail>;
  active_by_window: Record<string, number>;
  last_focused_window: number | null;
  chrome_focused: boolean;
  idle: boolean;
  measured_at: number;
}

export interface NavigationContext {
  tab_id: number;
  window_id: number;
  url: string;
  title: string | null;
  transition: string;
  qualifiers: string[];
  reload: boolean;
}

const MAX_TRAILS = 10;
const MAX_REFS = 20;

export function emptyActivityState(now = Date.now()): ActivityState {
  return {
    version: 1, totals: [], tabs: {}, pending_sources: {}, active_by_window: {},
    last_focused_window: null, chrome_focused: false, idle: false, measured_at: now,
  };
}

function hourStart(at: number): string {
  const date = new Date(at);
  const local = localTimestamp(date);
  return `${local.slice(0, 13)}:00:00${local.slice(19)}`;
}

function measuredTab(state: ActivityState): { tab: ActivityTab; kind: "focused" | "background" } | null {
  if (state.idle || state.last_focused_window === null) return null;
  const id = state.active_by_window[String(state.last_focused_window)];
  if (id === undefined) return null;
  const tab = state.tabs[String(id)];
  return tab ? { tab, kind: state.chrome_focused ? "focused" : "background" } : null;
}

function totalFor(
  state: ActivityState,
  tab: ActivityTab,
  at: number,
  uuid: () => string,
): ActivityTotal {
  const hour = hourStart(at);
  let total = state.totals.find((candidate) =>
    candidate.artifact_id === tab.artifact_id && candidate.hour_start === hour);
  if (!total) {
    total = {
      event_id: uuid(), artifact_id: tab.artifact_id, artifact_type: tab.artifact_type,
      hour_start: hour, url: tab.url, title: tab.title, opens: 0,
      focused_ms: 0, background_ms: 0, modes: [], trails: [], refs: [],
    };
    state.totals.push(total);
  }
  return total;
}

/** Attribute elapsed time in local-hour slices. Offset changes naturally make
 * a distinct key because the numeric offset is part of hour_start. */
export function settleActivity(
  state: ActivityState,
  now: number,
  uuid: () => string,
): void {
  if (!Number.isFinite(now) || now <= state.measured_at) {
    state.measured_at = Math.max(state.measured_at, now);
    return;
  }
  const measured = measuredTab(state);
  if (measured) {
    let cursor = state.measured_at;
    while (cursor < now) {
      const boundary = new Date(cursor);
      boundary.setMinutes(60, 0, 0);
      let end = Math.min(now, boundary.getTime());
      // A UTC-offset jump can occur before the next local-hour boundary.
      // Minute slices keep that change in its own offset-qualified bucket.
      end = Math.min(end, cursor + 60_000);
      const offset = new Date(cursor).getTimezoneOffset();
      if (end - cursor > 1 && new Date(end - 1).getTimezoneOffset() !== offset) {
        let low = cursor + 1;
        let high = end;
        while (low < high) {
          const middle = Math.floor((low + high) / 2);
          if (new Date(middle).getTimezoneOffset() === offset) low = middle + 1;
          else high = middle;
        }
        end = low;
      }
      const total = totalFor(state, measured.tab, cursor, uuid);
      const field = measured.kind === "focused" ? "focused_ms" : "background_ms";
      total[field] += end - cursor;
      cursor = end;
    }
  }
  state.measured_at = now;
}

function addDistinct<T>(values: T[], value: T, same: (left: T, right: T) => boolean, cap: number): void {
  if (values.length < cap && !values.some((existing) => same(existing, value))) values.push(value);
}

export function recordNavigation(
  state: ActivityState,
  context: NavigationContext,
  match: ArtifactMatch | null,
  now: number,
  uuid: () => string,
): void {
  settleActivity(state, now, uuid);
  const key = String(context.tab_id);
  const previous = state.tabs[key];
  if (!match) {
    delete state.tabs[key];
    delete state.pending_sources[key];
    return;
  }
  const title = match.title ?? context.title;
  const tab: ActivityTab = {
    artifact_id: match.artifact_id, artifact_type: match.type, url: context.url,
    title: title ?? null, mode: match.mode, window_id: context.window_id,
  };
  state.tabs[key] = tab;
  const changed = previous?.artifact_id !== match.artifact_id;
  const contributes = match.mode !== null || (match.refs?.length ?? 0) > 0 ||
    (changed && !context.reload);
  const currentHour = hourStart(now);
  let total = state.totals.find((candidate) =>
    candidate.artifact_id === tab.artifact_id && candidate.hour_start === currentHour);
  if (!total && contributes) total = totalFor(state, tab, now, uuid);
  if (total) {
    total.url = context.url;
    total.title = title ?? null;
    if (match.mode) {
      addDistinct(total.modes, { mode: match.mode, url: context.url },
        (a, b) => a.mode === b.mode, 3);
    }
    for (const ref of match.refs ?? []) {
      addDistinct(total.refs, { artifact_type: ref.type, value: ref.value },
        (a, b) => a.artifact_type === b.artifact_type && a.value === b.value, MAX_REFS);
    }
  }
  if (!changed || context.reload) {
    delete state.pending_sources[key];
    return;
  }
  if (!total) throw new Error("cold open did not create an activity total");
  total.opens += 1;
  const forwardBack = context.qualifiers.includes("forward_back");
  const pending = state.pending_sources[key];
  const sameTabSource = ["link", "form_submit"].includes(context.transition)
    ? previous : undefined;
  const trail: ActivityTrail = forwardBack
    ? { from_artifact_id: null, from_url: null, transition: "forward_back" }
    : pending ?? {
      from_artifact_id: sameTabSource?.artifact_id ?? null,
      from_url: sameTabSource?.url ?? null,
      transition: context.transition,
    };
  addDistinct(total.trails, trail,
    (a, b) => a.from_artifact_id === b.from_artifact_id && a.from_url === b.from_url &&
      a.transition === b.transition,
    MAX_TRAILS);
  delete state.pending_sources[key];
}

export function createdNavigationTarget(
  state: ActivityState,
  sourceTabId: number,
  targetTabId: number,
  transition = "link",
): void {
  const source = state.tabs[String(sourceTabId)];
  if (source) {
    state.pending_sources[String(targetTabId)] = {
      from_artifact_id: source.artifact_id, from_url: source.url, transition,
    };
  }
}

export function activateTab(
  state: ActivityState, tabId: number, windowId: number, now: number, uuid: () => string,
): void {
  settleActivity(state, now, uuid);
  state.active_by_window[String(windowId)] = tabId;
  state.last_focused_window = windowId;
}

export function focusWindow(
  state: ActivityState, windowId: number | null, now: number, uuid: () => string,
): void {
  settleActivity(state, now, uuid);
  state.chrome_focused = windowId !== null;
  if (windowId !== null) state.last_focused_window = windowId;
}

export function setIdle(
  state: ActivityState, idle: boolean, now: number, uuid: () => string,
): void {
  settleActivity(state, now, uuid);
  state.idle = idle;
}

export function removeTab(state: ActivityState, tabId: number, now: number, uuid: () => string): void {
  settleActivity(state, now, uuid);
  const key = String(tabId);
  delete state.tabs[key];
  delete state.pending_sources[key];
  for (const [windowId, active] of Object.entries(state.active_by_window)) {
    if (active === tabId) delete state.active_by_window[windowId];
  }
}

export function replaceTab(state: ActivityState, added: number, removed: number): void {
  const oldKey = String(removed);
  const newKey = String(added);
  if (state.tabs[oldKey]) state.tabs[newKey] = state.tabs[oldKey];
  if (state.pending_sources[oldKey]) state.pending_sources[newKey] = state.pending_sources[oldKey];
  delete state.tabs[oldKey];
  delete state.pending_sources[oldKey];
  for (const windowId of Object.keys(state.active_by_window)) {
    if (state.active_by_window[windowId] === removed) state.active_by_window[windowId] = added;
  }
}

export function freezeCompleted(
  state: ActivityState,
  now: number,
  uuid: () => string,
): ActivityEvent[] {
  settleActivity(state, now, uuid);
  const current = hourStart(now);
  const frozen = state.totals.filter((total) => total.hour_start !== current);
  state.totals = state.totals.filter((total) => total.hour_start === current);
  return frozen.map((total) => ({
    event_id: total.event_id, artifact_id: total.artifact_id,
    artifact_type: total.artifact_type, url: total.url, title: total.title,
    occurred_at: total.hour_start, incognito: false, opens: total.opens,
    focused_seconds: Math.floor(total.focused_ms / 1_000),
    background_seconds: Math.floor(total.background_ms / 1_000),
    modes: total.modes, trails: total.trails, refs: total.refs,
  }));
}
