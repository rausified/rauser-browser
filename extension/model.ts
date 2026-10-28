import type {
  ActivityEvent, AdapterDocument, OverflowMarker, VisitEvent, VisitOutcome,
} from "../protocol/ts/generated.js";
import {
  compileDocument,
  validateDocument,
  type CompiledDocument,
  type CompileResult,
} from "./adapter-templates.js";
import { storageKey } from "./brand.js";

export const POLICY_LEASE_MS = 24 * 60 * 60 * 1_000;
export const MAX_QUEUED_VISITS = 256;
export const MAX_QUEUE_BYTES = 512 * 1_024;
// Extension pages watch this key to notice config saved by another page.
export const POLICY_STORAGE_KEY = storageKey("policy_v1");
// The only extension pages whose messages the worker accepts.
export const TRUSTED_PAGES = ["panel.html", "options.html"] as const;
// The fixed folders the host keeps inside the home folder (DESIGN.md §6.1).
// The host refuses any other names, so settings never chooses them.
export const HOME_LAYOUT = {
  pages_dir: "notes",
  log_dir: "logs",
  artifacts_dir: "artifacts",
  summaries_dir: "summaries",
  later_dir: "later",
} as const;

// A summary started outside the panel (the shortcut or the context menu), for
// the panel to show. The worker keeps the latest in chrome.storage.session, so
// a panel that had not loaded yet still sees it, and sends it to open pages.
export const PANEL_NOTICE_KEY = storageKey("panel_notice_v1");
// The panel shows a stored notice only this soon after it was written.
export const PANEL_NOTICE_MAX_AGE_MS = 2 * 60_000;
// The site access the manifest grants at install (host_permissions).
export const SITE_ACCESS_ORIGINS = ["http://*/*", "https://*/*"];

export type PanelNotice =
  | { kind: "run_started"; run_id: string; tab_id: number; at: number }
  | { kind: "summarize_failed"; message: string; tab_id: number; at: number };

export function isPanelNotice(value: unknown): value is PanelNotice {
  if (typeof value !== "object" || value === null) return false;
  const notice = value as Record<string, unknown>;
  if (typeof notice.tab_id !== "number" || !Number.isInteger(notice.tab_id) ||
      typeof notice.at !== "number" || !Number.isFinite(notice.at)) {
    return false;
  }
  return (notice.kind === "run_started" && typeof notice.run_id === "string") ||
    (notice.kind === "summarize_failed" && typeof notice.message === "string");
}

export interface PolicyLease {
  revision: string;
  expires_at: number;
  capture_enabled: boolean;
  // Capture origins are the adapter origins; a v5 lease's `sites` is ignored.
  adapters: AdapterDocument;
}

export interface QueuedVisit {
  // VisitEvent remains accepted only so an upgrade can drain M1's durable
  // queue. New automatic capture produces ActivityEvent records.
  event: VisitEvent | ActivityEvent;
  dedupe_key: string;
  attempts: number;
  retry_after: number;
  // Items stored before these fields existed lack them. A missing tab ID is
  // never patched, and a missing dispatch flag counts as already dispatched.
  tab_id?: number;
  // Once the panel has received an item, its event is frozen: the host hashes
  // the entry text, so a later edit would break interrupted-write recovery.
  dispatched?: boolean;
  // All records in one native delivery retain its ID across retries, making
  // both the log append and an attached overflow marker idempotent.
  batch_id?: string;
  // The overflow evidence sent with this batch is frozen too. Without this,
  // drops that happen after a lost acknowledgement could be folded into a
  // replay whose batch ID already names a smaller marker in the log.
  batch_overflow?: OverflowMarker | null;
}

export interface QueueState {
  version: 1;
  items: QueuedVisit[];
  // A standalone marker survives worker restarts and lost host replies.
  overflow_delivery?: { batch_id: string; overflow: OverflowMarker; retry_after: number };
  overflow_count: number;
  overflow_first_at: string | null;
  overflow_last_at: string | null;
  rejected_count: number;
  // A persistent notice that stays until the user dismisses it.
  last_error: string | null;
  // The latest retryable failure. It clears once no pending visit has failed.
  retry_error: string | null;
}

export interface WorkerStatus {
  queued: number;
  live_activity: number;
  next_retry_at: number | null;
  overflow_count: number;
  overflow_first_at: string | null;
  overflow_last_at: string | null;
  rejected_count: number;
  last_error: string | null;
  retry_error: string | null;
  policy_expires_at: number | null;
  pause_pending: boolean;
  pause_token: string | null;
  // Origins an earlier build removed or saw revoked whose adapters the host
  // config may still hold. Capture stays off for them until the settings page
  // writes them out of the host config.
  pending_removals: string[];
  // Why the worker's last background exchange with the host failed, or null
  // once one succeeds.
  host_error: string | null;
  // Acknowledged delivery batches so far; changes whenever the host's
  // artifact history may have.
  delivered_batches: number;
}

export type WorkerRequest =
  | { kind: "install_policy"; lease: PolicyLease; resume_after_confirmation: boolean; resume_after_pause_token: string | null }
  | { kind: "suspend_policy" }
  | { kind: "pause_capture" }
  | { kind: "get_pending_ids" }
  | { kind: "discard_pending"; event_ids: string[] }
  | { kind: "get_pending" }
  | { kind: "get_activity" }
  | { kind: "get_site_patterns"; origin: string }
  | { kind: "clear_site_patterns"; origin: string }
  | { kind: "ack_visit"; event_id: string; outcome: VisitOutcome; reason: string | null }
  | { kind: "get_status" }
  | { kind: "sync_now" }
  | { kind: "clear_notices" }
  | { kind: "clear_pending_removals" };

export interface WorkerReply<T> {
  ok: boolean;
  value: T | null;
  error: string | null;
}

export function emptyQueue(): QueueState {
  return {
    version: 1,
    items: [],
    overflow_count: 0,
    overflow_first_at: null,
    overflow_last_at: null,
    rejected_count: 0,
    last_error: null,
    retry_error: null,
  };
}

// Formats local wall-clock time with its numeric UTC offset, such as
// 2026-09-28T09:15:00-04:00. The host files each visit under this local date.
export function localTimestamp(date: Date): string {
  const year = date.getFullYear();
  if (!Number.isFinite(year) || year < 0 || year > 9_999) {
    throw new RangeError("Visit time is outside the supported range");
  }
  const pad = (value: number, width = 2): string => String(value).padStart(width, "0");
  const offset = -date.getTimezoneOffset();
  const sign = offset < 0 ? "-" : "+";
  const magnitude = Math.abs(offset);
  return `${pad(year, 4)}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}` +
    `T${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}` +
    `${sign}${pad(Math.floor(magnitude / 60))}:${pad(magnitude % 60)}`;
}

export function isHttpUrl(url: URL): boolean {
  return url.protocol === "http:" || url.protocol === "https:";
}

/** Whether the lease allows automatic capture at all right now. */
export function leaseActive(lease: PolicyLease, now = Date.now()): boolean {
  return lease.capture_enabled && lease.expires_at > now;
}

/** Parse a URL automatic capture may consider: HTTP(S), no credentials. */
export function captureUrl(raw: string): URL | null {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return null;
  }
  if (!isHttpUrl(url) || url.username || url.password) return null;
  return url;
}

export function isPolicyLease(value: unknown): value is PolicyLease {
  return isPolicyLeaseShape(value) && "ok" in validateDocument(value.adapters);
}

function isPolicyLeaseShape(value: unknown): value is PolicyLease {
  if (typeof value !== "object" || value === null) return false;
  const lease = value as Record<string, unknown>;
  const adapters = lease.adapters;
  return (
    typeof lease.revision === "string" &&
    lease.revision.length > 0 &&
    typeof lease.expires_at === "number" &&
    Number.isFinite(lease.expires_at) &&
    typeof lease.capture_enabled === "boolean" &&
    typeof adapters === "object" &&
    adapters !== null &&
    Array.isArray((adapters as Record<string, unknown>).artifacts) &&
    ((adapters as Record<string, unknown>).artifacts as unknown[]).every((artifact) =>
      typeof artifact === "object" && artifact !== null &&
      typeof (artifact as Record<string, unknown>).origin === "string")
  );
}

type AdapterCompiler = (adapters: unknown) => CompileResult;

/**
 * Owns the compiled adapter document for the worker's current host revision.
 *
 * `install` always validates because extension messages are untrusted.
 * `restore` validates a stored lease once after worker startup and then trusts
 * the revision that the host minted, avoiding a full compile on every storage
 * read. The lease's active and adapter-origin checks still run before the
 * compiled document is used.
 */
export class PolicyLeaseCache {
  private cached: { revision: string; doc: CompiledDocument } | null = null;

  constructor(private readonly compile: AdapterCompiler = compileDocument) {}

  install(value: unknown): PolicyLease | null {
    if (!isPolicyLeaseShape(value)) return null;
    const compiled = this.compile(value.adapters);
    if (!compiled.ok) return null;
    this.cached = { revision: value.revision, doc: compiled.doc };
    return value;
  }

  restore(value: unknown): PolicyLease | null {
    if (!isPolicyLeaseShape(value)) {
      this.cached = null;
      return null;
    }
    if (this.cached?.revision === value.revision) return value;
    return this.install(value);
  }

  compiledFor(lease: PolicyLease): CompiledDocument | null {
    return this.cached?.revision === lease.revision ? this.cached.doc : null;
  }
}

export function isQueuedVisit(value: unknown): value is QueuedVisit {
  if (typeof value !== "object" || value === null) return false;
  const item = value as Record<string, unknown>;
  const event = item.event;
  if (typeof event !== "object" || event === null) return false;
  const visit = event as Record<string, unknown>;
  const common =
    typeof visit.event_id === "string" &&
    typeof visit.url === "string" &&
    (visit.title === null || typeof visit.title === "string") &&
    typeof visit.occurred_at === "string" &&
    visit.incognito === false;
  const activity =
    typeof visit.artifact_id === "string" &&
    typeof visit.artifact_type === "string" &&
    typeof visit.opens === "number" &&
    typeof visit.focused_seconds === "number" &&
    typeof visit.background_seconds === "number" &&
    Number.isInteger(visit.opens) && visit.opens >= 0 &&
    Number.isInteger(visit.focused_seconds) && visit.focused_seconds >= 0 &&
    Number.isInteger(visit.background_seconds) && visit.background_seconds >= 0 &&
    Array.isArray(visit.modes) && visit.modes.length <= 3 &&
    visit.modes.every((mode) => typeof mode === "object" && mode !== null &&
      ["view", "edit", "review"].includes(String((mode as Record<string, unknown>).mode)) &&
      typeof (mode as Record<string, unknown>).url === "string") &&
    Array.isArray(visit.trails) && visit.trails.length <= 10 &&
    visit.trails.every((trail) => typeof trail === "object" && trail !== null &&
      (typeof (trail as Record<string, unknown>).from_artifact_id === "string" ||
        (trail as Record<string, unknown>).from_artifact_id === null) &&
      (typeof (trail as Record<string, unknown>).from_url === "string" ||
        (trail as Record<string, unknown>).from_url === null) &&
      typeof (trail as Record<string, unknown>).transition === "string") &&
    Array.isArray(visit.refs) && visit.refs.length <= 20 &&
    visit.refs.every((ref) => typeof ref === "object" && ref !== null &&
      typeof (ref as Record<string, unknown>).artifact_type === "string" &&
      typeof (ref as Record<string, unknown>).value === "string");
  const legacy = visit.artifact_id === undefined && visit.artifact_type === undefined &&
    visit.opens === undefined && visit.focused_seconds === undefined &&
    visit.background_seconds === undefined && visit.modes === undefined &&
    visit.trails === undefined && visit.refs === undefined;
  const overflow = item.batch_overflow;
  const marker = typeof overflow === "object" && overflow !== null
    ? overflow as Record<string, unknown>
    : null;
  const batchOverflow = overflow === undefined || overflow === null || (marker !== null &&
    typeof marker.count === "number" && Number.isInteger(marker.count) &&
    marker.count > 0 && marker.count <= 0xffff_ffff &&
    typeof marker.first_occurred_at === "string" &&
    typeof marker.last_occurred_at === "string");
  return (
    typeof item.dedupe_key === "string" &&
    typeof item.attempts === "number" &&
    Number.isInteger(item.attempts) &&
    item.attempts >= 0 &&
    typeof item.retry_after === "number" &&
    Number.isFinite(item.retry_after) &&
    (item.tab_id === undefined || (typeof item.tab_id === "number" && Number.isInteger(item.tab_id))) &&
    (item.dispatched === undefined || typeof item.dispatched === "boolean") &&
    (item.batch_id === undefined || typeof item.batch_id === "string") &&
    batchOverflow &&
    common && (legacy || activity)
  );
}
