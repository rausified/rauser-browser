import type { ActivityEvent, ActivityTrail, OverflowMarker, VisitEvent, VisitOutcome } from "../protocol/ts/generated.js";
import {
  MAX_QUEUE_BYTES,
  MAX_QUEUED_VISITS,
  POLICY_STORAGE_KEY,
  PolicyLeaseCache,
  TRUSTED_PAGES,
  captureUrl,
  emptyQueue,
  isHttpUrl,
  isPolicyLease,
  isQueuedVisit,
  leaseActive,
  PANEL_NOTICE_KEY,
  SITE_ACCESS_ORIGINS,
  type PanelNotice,
  type PolicyLease,
  type QueuedVisit,
  type QueueState,
  type WorkerReply,
  type WorkerRequest,
  type WorkerStatus,
} from "./model.js";
import { BINARY_NAME, storageKey } from "./brand.js";
import { diag, initDiag } from "./diag.js";
import { evaluate, type EvaluateResult } from "./adapter-templates.js";
import { withLatestHostState } from "./coordination.js";
import { HostClient, PROTOCOL_VERSION, newRequestId } from "./native.js";
import { leaseFor, withConfigMutationLock } from "./settings.js";
import { httpOrigin, inspectSitePatterns, prunePatternSamples, samplePattern } from "./site-patterns.js";
import { pruneDraftHandoffs } from "./capture-site-handoff.js";
import {
  NOT_A_WEB_PAGE_MESSAGE, SITE_ACCESS_NEEDED_MESSAGE, SUMMARIZE_MENU_ID, SUMMARIZE_MENU_TITLE,
  summarizeFailureText, summarizeTab,
} from "./summarize.js";
import {
  activateTab, createdNavigationTarget, emptyActivityState, focusWindow, freezeCompleted,
  recordNavigation, removeTab, replaceTab, setIdle, settleActivity,
  type ActivityState,
} from "./activity.js";

// First, so errors in the rest of the worker are logged.
initDiag("worker");

const QUEUE_KEY = storageKey("queue_v1");
const POLICY_KEY = POLICY_STORAGE_KEY;
const PAUSE_KEY = storageKey("pause_pending_v1");
const PAUSE_TOKEN_KEY = storageKey("pause_token_v1");
// Origins whose sites were removed or revoked under an earlier build but may
// still have adapters in the host config. Capture stays off for them until a
// received lease no longer carries an adapter on any of them.
const PENDING_REMOVALS_KEY = storageKey("pending_site_removals_v1");
// Honored as pending removals until migratePendingRemovals carries them over.
const LEGACY_REVOCATIONS_KEY = storageKey("revocations_v1");
const LEGACY_REMOVED_SITES_KEY = storageKey("locally_removed_sites_v1");
// Written by the per-origin note grants of earlier builds.
const LEGACY_NOTE_ORIGINS_KEY = storageKey("note_origins_v1");
// The per-list cap earlier builds enforced on each legacy key.
const LEGACY_MAX_REMOVALS = 256;
// Migration merges both legacy lists, so the pending list holds their full union.
const MAX_PENDING_REMOVALS = 2 * LEGACY_MAX_REMOVALS;
const HOST_ERROR_KEY = storageKey("host_error_v1");
// Counts acknowledged delivery batches, so pages can tell the index changed.
const DELIVERED_KEY = storageKey("delivered_batches_v1");
const ACTIVITY_TOTALS_KEY = storageKey("activity_totals_v1");
const ACTIVITY_SESSION_KEY = storageKey("activity_session_v1");
const SITE_PATTERNS_KEY = storageKey("site_patterns_v1");
// Matches MAX_URL_BYTES in host/src/capture.rs, so the host never rejects a
// visit the worker chose to buffer.
const MAX_VISIT_URL_BYTES = 8_192;
const MAX_TITLE_CHARS = 300;
// The worker, not the panel, starts the host to deliver visits and renew the
// policy lease, so capture keeps working while no extension page is open.
// The alarm wakes the worker to retry deliveries and renew the lease in time.
const SYNC_ALARM = `${BINARY_NAME}-sync`;
const SYNC_PERIOD_MINUTES = 5;
// A one-shot sync soon after a capture. Chrome runs alarms no sooner than 30
// seconds out, which also covers the title grace period.
const DELIVER_ALARM = `${BINARY_NAME}-deliver`;
const DELIVER_DELAY_MINUTES = 0.5;
const LEASE_RENEW_WINDOW_MS = 60 * 60_000;
const encoder = new TextEncoder();

let storageReady: Promise<void> | null = null;
let sequence: Promise<unknown> = Promise.resolve();
// The last valid lease this worker received from a page through
// install_policy, kept even while a local pause holds it out of storage. A
// lease stored by an earlier build may have been narrowed locally, so only a
// received lease shows what the host config holds.
let receivedLease: PolicyLease | null = null;
const policyLeases = new PolicyLeaseCache();

// These bounded shapes stay in trusted extension storage, wholly separate
// from activity and delivery. Sampling failures must not interrupt capture.
async function patternSamples(lease: PolicyLease | null) {
  const stored = await chrome.storage.local.get(SITE_PATTERNS_KEY);
  const origins = lease && leaseActive(lease)
    ? lease.adapters.artifacts.map((adapter) => String(adapter.origin)) : [];
  return prunePatternSamples(stored[SITE_PATTERNS_KEY], origins, Date.now());
}

async function pruneSitePatterns(lease: PolicyLease | null): Promise<void> {
  await chrome.storage.local.set({ [SITE_PATTERNS_KEY]: await patternSamples(lease) });
}

// Removal, pause and suspension promise to clear samples. If the pruned rows
// cannot be written, delete every sample instead; report only when even
// deletion fails, so the caller can retry rather than revive the rows.
async function purgeSitePatterns(lease: PolicyLease | null): Promise<void> {
  try {
    await pruneSitePatterns(lease);
  } catch {
    await chrome.storage.local.remove(SITE_PATTERNS_KEY);
  }
}

function patternFailure(): void {
  diag.warn("site patterns unavailable");
}

function activitySessionStorage(): ChromeStorageArea {
  return chrome.storage.session;
}

async function readActivity(): Promise<ActivityState> {
  const [local, session] = await Promise.all([
    chrome.storage.local.get(ACTIVITY_TOTALS_KEY),
    activitySessionStorage().get(ACTIVITY_SESSION_KEY),
  ]);
  const totals = local[ACTIVITY_TOTALS_KEY];
  const context = session[ACTIVITY_SESSION_KEY];
  const fresh = emptyActivityState();
  if (typeof totals === "object" && totals !== null &&
      (totals as { version?: unknown }).version === 1 &&
      Array.isArray((totals as { totals?: unknown }).totals)) {
    fresh.totals = structuredClone((totals as { totals: ActivityState["totals"] }).totals);
  }
  if (typeof context === "object" && context !== null &&
      (context as { version?: unknown }).version === 1) {
    const saved = context as Partial<ActivityState>;
    if (saved.tabs && saved.pending_sources && saved.active_by_window &&
        typeof saved.chrome_focused === "boolean" && typeof saved.idle === "boolean" &&
        typeof saved.measured_at === "number") {
      fresh.tabs = structuredClone(saved.tabs);
      fresh.pending_sources = structuredClone(saved.pending_sources);
      fresh.active_by_window = structuredClone(saved.active_by_window);
      fresh.last_focused_window = saved.last_focused_window ?? null;
      fresh.chrome_focused = saved.chrome_focused;
      fresh.idle = saved.idle;
      fresh.measured_at = saved.measured_at;
    }
  }
  return fresh;
}

async function writeActivity(state: ActivityState): Promise<void> {
  const { totals, ...context } = state;
  await Promise.all([
    chrome.storage.local.set({ [ACTIVITY_TOTALS_KEY]: { version: 1, totals } }),
    activitySessionStorage().set({ [ACTIVITY_SESSION_KEY]: context }),
  ]);
}

async function narrowActivity(lease: PolicyLease | null): Promise<void> {
  const activity = await readActivity();
  settleActivity(activity, Date.now(), () => crypto.randomUUID());
  const allowed = (url: string): boolean => lease !== null && matchingArtifact(lease, url) !== null;
  activity.totals = activity.totals.filter((total) => allowed(total.url));
  if (lease !== null) {
    for (const total of activity.totals) total.trails = trailsUnder(lease, total.trails);
  }
  for (const [tabId, tab] of Object.entries(activity.tabs)) {
    if (!allowed(tab.url)) delete activity.tabs[tabId];
  }
  for (const [tabId, source] of Object.entries(activity.pending_sources)) {
    if (source.from_url === null || !allowed(source.from_url)) delete activity.pending_sources[tabId];
  }
  await writeActivity(activity);
}

/** The trails the host would accept under this lease. The host re-resolves
 * every trail source and rejects the whole event when one no longer resolves
 * to the same artifact (DESIGN.md §5.1 Trails), so a source on a removed or
 * pending-removal origin is dropped here instead of costing the hour. */
function trailsUnder(lease: PolicyLease, trails: ActivityTrail[]): ActivityTrail[] {
  return trails.filter((trail) => trail.from_url === null
    ? trail.from_artifact_id === null
    : trail.from_artifact_id !== null &&
      matchingArtifact(lease, trail.from_url)?.artifact_id === trail.from_artifact_id);
}

/** Apply trailsUnder to queued activity the panel has not yet received.
 * A dispatched event stays frozen: the host may already hold its bytes. */
function narrowQueuedTrails(items: QueuedVisit[], lease: PolicyLease): void {
  for (const item of items) {
    if (item.dispatched !== false || !isActivityEvent(item.event)) continue;
    const trails = trailsUnder(lease, item.event.trails);
    if (trails.length !== item.event.trails.length) item.event = { ...item.event, trails };
  }
}

function activityContext(state: ActivityState): Omit<ActivityState, "totals"> {
  const { totals, ...context } = state;
  void totals;
  return context;
}

function isActivityEvent(event: VisitEvent | ActivityEvent): event is ActivityEvent {
  return "artifact_id" in event;
}

function ensureTrustedStorage(): Promise<void> {
  storageReady ??= Promise.all([
    chrome.storage.local.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" }),
    chrome.storage.session.setAccessLevel?.({ accessLevel: "TRUSTED_CONTEXTS" }),
  ]).then(async () => {
    await updatePressureBadge(await readQueue());
  });
  return storageReady;
}

function serialize<T>(work: () => Promise<T>): Promise<T> {
  const next = sequence.then(work, work);
  sequence = next.then(
    () => undefined,
    () => undefined,
  );
  return next;
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

async function readPolicy(): Promise<PolicyLease | null> {
  return (await readPolicyState()).lease;
}

type LegacyPolicyLease = Omit<PolicyLease, "adapters"> & { origins: string[] };

function legacyPolicyLease(value: unknown): LegacyPolicyLease | null {
  if (typeof value !== "object" || value === null || "adapters" in value) return null;
  // Protocol v4 leases had the current policy fields and a site list, but no
  // adapter document. Adding an empty document lets the current validator
  // check that old shape without treating arbitrary corrupt state as an
  // upgrade signal. The site origins decide which navigations renew it.
  const sites = (value as Record<string, unknown>).sites;
  if (!Array.isArray(sites)) return null;
  const origins: string[] = [];
  for (const site of sites) {
    const origin = typeof site === "object" && site !== null
      ? (site as Record<string, unknown>).origin
      : undefined;
    if (typeof origin !== "string") return null;
    origins.push(origin);
  }
  const candidate = { ...value, adapters: { format: 1, artifacts: [] } };
  if (!isPolicyLease(candidate)) return null;
  return {
    revision: candidate.revision,
    expires_at: candidate.expires_at,
    capture_enabled: candidate.capture_enabled,
    origins,
  };
}

async function readPolicyState(): Promise<{
  lease: PolicyLease | null;
  legacyLease: LegacyPolicyLease | null;
}> {
  const stored = await chrome.storage.local.get(POLICY_KEY);
  const value = stored[POLICY_KEY];
  const lease = policyLeases.restore(value);
  return {
    lease,
    // A well-formed lease from the previous extension version is renewed from
    // the host. Missing, suspended, or malformed state stays inert.
    legacyLease: lease ? null : legacyPolicyLease(value),
  };
}

function isOverflowDelivery(value: unknown): value is NonNullable<QueueState["overflow_delivery"]> {
  if (typeof value !== "object" || value === null) return false;
  const delivery = value as Record<string, unknown>;
  const marker = delivery.overflow as Record<string, unknown> | null;
  return typeof delivery.batch_id === "string" && delivery.batch_id.length <= 64 &&
    typeof delivery.retry_after === "number" && Number.isFinite(delivery.retry_after) &&
    typeof marker === "object" && marker !== null &&
    Number.isInteger(marker.count) && Number(marker.count) > 0 && Number(marker.count) <= 0xffff_ffff &&
    typeof marker.first_occurred_at === "string" && typeof marker.last_occurred_at === "string";
}

async function readQueue(): Promise<QueueState> {
  const stored = await chrome.storage.local.get(QUEUE_KEY);
  const value = stored[QUEUE_KEY];
  if (value === undefined) return emptyQueue();
  if (typeof value !== "object" || value === null) {
    throw new Error("Capture queue is unreadable; capture is paused");
  }
  const state = value as Record<string, unknown>;
  if (
    state.version !== 1 ||
    !Array.isArray(state.items) ||
    !state.items.every(isQueuedVisit) ||
    !(state.overflow_delivery === undefined || isOverflowDelivery(state.overflow_delivery)) ||
    typeof state.overflow_count !== "number" ||
    !(state.overflow_first_at === undefined || state.overflow_first_at === null ||
      typeof state.overflow_first_at === "string") ||
    !(state.overflow_last_at === undefined || state.overflow_last_at === null ||
      typeof state.overflow_last_at === "string") ||
    typeof state.rejected_count !== "number" ||
    !(state.last_error === null || typeof state.last_error === "string") ||
    !(state.retry_error === undefined || state.retry_error === null ||
      typeof state.retry_error === "string")
  ) {
    throw new Error("Capture queue is unreadable; capture is paused");
  }
  // Queues stored before retry_error existed kept retry failures in last_error.
  return {
    ...(value as QueueState),
    overflow_first_at: (state.overflow_first_at as string | null | undefined) ?? null,
    overflow_last_at: (state.overflow_last_at as string | null | undefined) ?? null,
    retry_error: (state.retry_error as string | null | undefined) ?? null,
  };
}

async function writeQueue(state: QueueState): Promise<void> {
  await chrome.storage.local.set({ [QUEUE_KEY]: state });
  await updatePressureBadge(state);
}

function queueBytes(state: QueueState): number {
  return encoder.encode(JSON.stringify(state.items)).length;
}

function underPressure(state: QueueState): boolean {
  return state.items.length >= Math.ceil(MAX_QUEUED_VISITS * 0.8) ||
    queueBytes(state) >= Math.ceil(MAX_QUEUE_BYTES * 0.8);
}

async function updatePressureBadge(state: QueueState): Promise<void> {
  const warning = underPressure(state) || state.overflow_count > 0;
  await chrome.action.setBadgeBackgroundColor({ color: "#b45309" });
  await chrome.action.setBadgeText({ text: warning ? "!" : "" });
}

/** Fold a duplicate, not-yet-dispatched hourly total instead of consuming a
 * second queue slot. This repairs duplicate freezes after worker interruption
 * without ever mutating a record the host may already have hashed. */
function coalesceActivity(queue: QueueState, event: ActivityEvent): boolean {
  const existing = queue.items.find((item) =>
    item.dispatched !== true && isActivityEvent(item.event) &&
    item.event.artifact_id === event.artifact_id &&
    item.event.artifact_type === event.artifact_type &&
    item.event.occurred_at === event.occurred_at);
  if (!existing || !isActivityEvent(existing.event)) return false;
  const merged = structuredClone(existing.event);
  merged.opens = Math.min(100_000, merged.opens + event.opens);
  merged.focused_seconds = Math.min(7_200,
    merged.focused_seconds + event.focused_seconds);
  merged.background_seconds = Math.min(7_200,
    merged.background_seconds + event.background_seconds);
  // Refs must come from the title we keep: the host re-evaluates that
  // evidence, so refs from an older title would reject the entire hour.
  const refs = event.title !== null ? event.refs : merged.refs;
  merged.title = event.title ?? merged.title;
  merged.url = event.url;
  merged.modes = [...merged.modes, ...event.modes]
    .filter((mode, index, all) => all.findIndex((other) =>
      other.mode === mode.mode && other.url === mode.url) === index).slice(0, 3);
  merged.trails = [...merged.trails, ...event.trails]
    .filter((trail, index, all) => all.findIndex((other) =>
      other.transition === trail.transition &&
      other.from_artifact_id === trail.from_artifact_id && other.from_url === trail.from_url) === index)
    .slice(0, 10);
  merged.refs = refs
    .filter((ref, index, all) => all.findIndex((other) =>
      other.artifact_type === ref.artifact_type && other.value === ref.value) === index).slice(0, 20);
  const before = existing.event;
  existing.event = merged;
  if (queueBytes(queue) > MAX_QUEUE_BYTES) {
    existing.event = before;
    return false;
  }
  return true;
}

/** Freeze completed local hours and move them into the durable delivery
 * queue. The total and queue are published in one storage.local update so a
 * worker stop cannot lose the handoff. */
async function freezeActivity(now = Date.now()): Promise<number> {
  const activity = await readActivity();
  const frozen = freezeCompleted(activity, now, () => crypto.randomUUID());
  if (!frozen.length) {
    await writeActivity(activity);
    return 0;
  }
  const queue = await readQueue();
  let accepted = 0;
  for (const event of frozen) {
    const item: QueuedVisit = {
      event, dedupe_key: event.event_id, attempts: 0, retry_after: 0, dispatched: false,
    };
    if (underPressure(queue) && coalesceActivity(queue, event)) {
      accepted += 1;
      continue;
    }
    const next = [...queue.items, item];
    if (next.length > MAX_QUEUED_VISITS ||
        encoder.encode(JSON.stringify(next)).length > MAX_QUEUE_BYTES) {
      queue.overflow_count = Math.min(0xffff_ffff, queue.overflow_count + 1);
      queue.overflow_first_at ??= event.occurred_at;
      queue.overflow_last_at = event.occurred_at;
      queue.last_error = "Activity queue is full; completed hours were not buffered";
      continue;
    }
    queue.items = next;
    accepted += 1;
  }
  await Promise.all([
    chrome.storage.local.set({
      [ACTIVITY_TOTALS_KEY]: { version: 1, totals: activity.totals },
      [QUEUE_KEY]: queue,
    }),
    activitySessionStorage().set({ [ACTIVITY_SESSION_KEY]: activityContext(activity) }),
  ]);
  await updatePressureBadge(queue);
  return accepted;
}

async function readStoredPendingRemovals(): Promise<string[]> {
  const stored = await chrome.storage.local.get(PENDING_REMOVALS_KEY);
  const value = stored[PENDING_REMOVALS_KEY];
  if (value === undefined) return [];
  // This list is a safety gate. If it is corrupt, fail closed.
  if (!Array.isArray(value) || value.length > MAX_PENDING_REMOVALS ||
      !value.every((entry) => typeof entry === "string")) {
    throw new Error("Pending site removal state is unreadable; capture is paused");
  }
  return value;
}

function legacyOrigins(value: unknown, pick: (entry: unknown) => unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.map(pick).filter((origin): origin is string => typeof origin === "string");
}

function legacyRemovedSite(value: unknown): value is { origin: string; path_prefix: string } {
  if (typeof value !== "object" || value === null) return false;
  const site = value as Record<string, unknown>;
  if (typeof site.origin !== "string" || typeof site.path_prefix !== "string") return false;
  try {
    const url = new URL(site.origin);
    if (!isHttpUrl(url) || url.username || url.password || url.origin !== site.origin) return false;
  } catch {
    return false;
  }
  const path = site.path_prefix;
  return path.startsWith("/") && !path.includes("?") && !path.includes("#") &&
    !path.includes("\\") && !path.split("/").some((part) => part === "." || part === "..");
}

/** The earlier build's local site removals, validated exactly as it read
 * them. That build paused capture on a malformed list; so does this one,
 * and migration keeps the key rather than deleting the evidence. */
function legacyRemovedSites(value: unknown): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > LEGACY_MAX_REMOVALS || !value.every(legacyRemovedSite)) {
    throw new Error("Local site removal state is unreadable; capture is paused");
  }
  return value.map((site) => site.origin);
}

/** Origins an earlier build removed or saw revoked. Until migration has
 * carried them into the pending list and deleted the legacy keys, they keep
 * gating capture, so a failed migration write cannot reopen them. */
async function readLegacyRemovals(): Promise<string[]> {
  const stored = await chrome.storage.local.get([LEGACY_REVOCATIONS_KEY, LEGACY_REMOVED_SITES_KEY]);
  return [
    ...legacyOrigins(stored[LEGACY_REVOCATIONS_KEY], (entry) => entry),
    ...legacyRemovedSites(stored[LEGACY_REMOVED_SITES_KEY]),
  ];
}

/** Every origin capture must stay off for: the pending list plus any legacy
 * removal state not yet migrated. */
async function readPendingRemovals(): Promise<string[]> {
  return [...new Set([...await readStoredPendingRemovals(), ...await readLegacyRemovals()])].sort();
}

/** Carry origins that an earlier build removed or saw revoked, but whose
 * adapters the host may still hold, into the one pending-removal list. */
async function migratePendingRemovals(): Promise<void> {
  const stored = await chrome.storage.local.get([LEGACY_REVOCATIONS_KEY, LEGACY_REMOVED_SITES_KEY]);
  if (!(LEGACY_REVOCATIONS_KEY in stored) && !(LEGACY_REMOVED_SITES_KEY in stored)) return;
  const origins = await readPendingRemovals();
  // Never drop a removal gate. A union too large to store stays in the
  // legacy keys, which keep gating capture until it can migrate.
  if (origins.length > MAX_PENDING_REMOVALS) {
    throw new Error("Pending site removals exceed the migration limit; legacy state kept");
  }
  // Delete the legacy keys only after the merged list is durably written.
  await chrome.storage.local.set({ [PENDING_REMOVALS_KEY]: origins });
  await chrome.storage.local.remove([LEGACY_REVOCATIONS_KEY, LEGACY_REMOVED_SITES_KEY]);
  // Only a lease received after the list changed may clear it.
  receivedLease = null;
}

/** The lease capture may use: every adapter on a pending-removal origin is dropped. */
function withoutPendingRemovals(lease: PolicyLease, pending: string[]): PolicyLease {
  if (!pending.length) return lease;
  const removed = new Set(pending);
  return {
    ...lease,
    adapters: {
      ...lease.adapters,
      artifacts: lease.adapters.artifacts.filter((artifact) =>
        typeof artifact.origin !== "string" || !removed.has(artifact.origin)),
    },
  };
}

function holdsPendingRemoval(lease: PolicyLease, pending: string[]): boolean {
  const removed = new Set(pending);
  return lease.adapters.artifacts.some((artifact) =>
    typeof artifact.origin === "string" && removed.has(artifact.origin));
}

async function pausePending(): Promise<boolean> {
  const stored = await chrome.storage.local.get(PAUSE_KEY);
  return stored[PAUSE_KEY] === true;
}

async function readPauseToken(): Promise<string | null> {
  const stored = await chrome.storage.local.get(PAUSE_TOKEN_KEY);
  const value = stored[PAUSE_TOKEN_KEY];
  if (value === undefined) return null;
  if (typeof value !== "string") throw new Error("Local pause state is unreadable; capture is paused");
  return value;
}

/** The current lease for capture and delivery, or null when capture may not
 * run. It never asks Chrome about permissions: Brauser holds HTTP(S) access
 * from install. */
async function activeLease(): Promise<PolicyLease | null> {
  const pending = await readPendingRemovals();
  const lease = await readPolicy();
  if (!lease) return null;
  if (await pausePending()) return null;
  if (!lease.capture_enabled) {
    const queue = await readQueue();
    if (queue.items.length) {
      queue.items = [];
      await writeQueue(queue);
    }
    await narrowActivity(null);
    return null;
  }
  return withoutPendingRemovals(lease, pending);
}

function withoutHash(rawUrl: string): string | null {
  try {
    const url = new URL(rawUrl);
    url.hash = "";
    return url.href;
  } catch {
    return null;
  }
}

/** Whether the lease has an adapter on this URL's origin. Adapters are
 * origin-wide, so a site is on exactly when it has one. */
function leaseCoversOrigin(lease: PolicyLease, url: URL): boolean {
  return lease.adapters.artifacts.some((artifact) => artifact.origin === url.origin);
}

/** Match only while the lease is active, and only on an origin that has an
 * adapter in this lease. A URL whose origin has no adapter here never
 * matches, even when the compiled document for its revision still has one. */
function matchingArtifact(
  lease: PolicyLease,
  rawUrl: string,
  title?: string,
): Extract<EvaluateResult, { match: true }> | null {
  if (!leaseActive(lease)) return null;
  const url = captureUrl(rawUrl);
  if (!url || !leaseCoversOrigin(lease, url)) return null;
  const compiled = policyLeases.compiledFor(lease);
  if (!compiled) return null;
  const found = evaluate(compiled, rawUrl, title);
  return found.match ? found : null;
}

/** Update the durable hourly activity total for a qualifying navigation. */
async function captureNavigation(
  details: ChromeNavigationDetails,
  useCurrentTitle: boolean,
): Promise<boolean> {
  if (details.frameId !== 0 || details.documentLifecycle !== "active" || details.tabId < 0) {
    return false;
  }
  await ensureTrustedStorage();
  // Most navigations are for sites the user never enabled. Check the cached
  // lease first so they cost one storage read.
  // A navigation that does not capture still ends the tab's tracked entry,
  // so the previous page stops gaining focused time.
  const navigatedAway = async (): Promise<false> => {
    const activity = await readActivity();
    if (activity.tabs[String(details.tabId)]) {
      recordNavigation(activity, {
        tab_id: details.tabId, window_id: activity.tabs[String(details.tabId)]?.window_id ?? -1,
        url: "", title: null, transition: details.transitionType ?? "unknown",
        qualifiers: details.transitionQualifiers ?? [], reload: false,
      }, null, details.timeStamp, () => crypto.randomUUID());
      await writeActivity(activity);
    }
    return false;
  };
  const cached = await readPolicy();
  const url = captureUrl(details.url);
  if (!cached || !url || !leaseCoversOrigin(cached, url)) return navigatedAway();
  // The stored lease is the host's as sent, so it can still hold adapters on
  // pending-removal origins that activeLease() filters out.
  let lease: PolicyLease | null;
  try {
    lease = await activeLease();
  } catch (error) {
    // Unreadable pause or removal state must still stop the previous page
    // from gaining focused time before the failure propagates.
    await navigatedAway();
    throw error;
  }
  if (!lease || !leaseActive(lease) || !leaseCoversOrigin(lease, url)) return navigatedAway();
  if (encoder.encode(details.url).length > MAX_VISIT_URL_BYTES) return navigatedAway();

  let tab: ChromeTab;
  try {
    tab = await chrome.tabs.get(details.tabId);
  } catch {
    // The navigation still committed, so the previous page stops gaining time.
    return navigatedAway();
  }
  if (tab.incognito !== false) return navigatedAway();

  // Sample only a live public top-frame navigation on an enabled origin.
  // Shape before storing; neither the URL nor its title enters this table.
  const shape = inspectSitePatterns(details.url).current;
  if (shape) {
    try {
      const samples = await patternSamples(lease);
      // The lease can expire while the tab and samples are read; recheck so
      // an expired policy never gains a sample.
      const now = Date.now();
      if (leaseActive(lease, now)) {
        samplePattern(samples, url.origin, shape, now);
        await chrome.storage.local.set({ [SITE_PATTERNS_KEY]: samples });
      }
    } catch { patternFailure(); }
  }
  const match = matchingArtifact(lease, details.url);
  if (!match) return navigatedAway();

  const visitedAt = new Date(details.timeStamp);
  if (!Number.isFinite(visitedAt.getTime())) return false;
  // At commit, the tab still shows the previous document's title. The title
  // arrives later through tabs.onUpdated. A same-document history update
  // starts from the tab's current title, but an SPA usually sets the new
  // route's title after pushState, so that title may still be the old one.
  let title: string | null = null;
  if (useCurrentTitle && tab.url) {
    // A page title is optional. The navigation URL remains authoritative.
    const current = withoutHash(tab.url);
    if (current !== null && current === withoutHash(details.url)) {
      title = tab.title?.slice(0, MAX_TITLE_CHARS) ?? null;
    }
  }

  const activity = await readActivity();
  recordNavigation(activity, {
    tab_id: details.tabId, window_id: tab.windowId ?? -1, url: details.url, title,
    transition: details.transitionType ?? "unknown",
    qualifiers: details.transitionQualifiers ?? [],
    reload: details.transitionType === "reload",
  }, match, details.timeStamp, () => crypto.randomUUID());
  await writeActivity(activity);
  await freezeActivity(details.timeStamp);
  return true;
}

async function patchTitle(tabId: number, rawTitle: string): Promise<void> {
  let tab: ChromeTab;
  try {
    tab = await chrome.tabs.get(tabId);
  } catch {
    return;
  }
  if (tab.incognito !== false || !tab.url) return;
  await ensureTrustedStorage();
  const stored = await readPolicy();
  if (!stored) return;
  const lease = withoutPendingRemovals(stored, await readPendingRemovals());
  const current = withoutHash(tab.url);
  if (current === null) return;
  const activity = await readActivity();
  const mapped = activity.tabs[String(tabId)];
  const title = rawTitle.slice(0, MAX_TITLE_CHARS);
  const titleMatch = mapped && withoutHash(mapped.url) === current
    ? matchingArtifact(lease, mapped.url, title) : null;
  if (mapped && titleMatch) {
    mapped.title = title;
    const hour = [...activity.totals].reverse().find((total) =>
      total.artifact_id === mapped.artifact_id);
    if (hour) {
      hour.title = titleMatch.title ?? title;
      hour.url = mapped.url;
      hour.refs = [];
      for (const ref of titleMatch.refs ?? []) {
        if (hour.refs.length >= 20) break;
        if (!hour.refs.some((known) => known.artifact_type === ref.type && known.value === ref.value)) {
          hour.refs.push({ artifact_type: ref.type, value: ref.value });
        }
      }
    }
    await writeActivity(activity);
  }
  // Drain compatibility: M1 visits that were buffered before upgrade may
  // still accept their late title until first dispatch.
  const queue = await readQueue();
  let latest: QueuedVisit | undefined;
  for (const item of queue.items) {
    if (item.tab_id === tabId) latest = item;
  }
  // Only the tab's newest visit may take the title, and only before the panel
  // has seen it. A dispatched event may already be partly written by the host.
  if (!latest || latest.dispatched !== false || withoutHash(latest.event.url) !== current ||
      !matchingArtifact(lease, latest.event.url)) {
    return;
  }
  if (latest.event.title === title) return;
  latest.event = { ...latest.event, title };
  await writeQueue(queue);
}

async function installPolicy(
  lease: PolicyLease,
  resumeAfterConfirmation: boolean,
  resumeAfterPauseToken: string | null,
  seedOpenTabs = false,
): Promise<WorkerStatus> {
  // Reject an expired message before it can replace the compiled document for
  // the active revision. Otherwise the unchanged stored lease would reuse the
  // rejected message's matcher on its next read.
  if (typeof lease !== "object" || lease === null ||
      typeof lease.expires_at !== "number" || lease.expires_at <= Date.now() ||
      !policyLeases.install(lease)) {
    throw new Error("Host policy lease is invalid or expired");
  }
  // Record what the host holds before a pending pause can return early, so a
  // removal saved during the pause can still clear. Capture stays blocked.
  receivedLease = lease;
  if (await pausePending()) {
    if (!resumeAfterConfirmation) return currentStatus();
    // A pause requested while a save was in flight must win over an older
    // enable operation, even if that enable later commits its host config.
    if (lease.capture_enabled && resumeAfterPauseToken !== await readPauseToken()) {
      return currentStatus();
    }
  }
  // Store the lease as the host sent it, so clear_pending_removals can tell
  // whether the host still holds an adapter on a pending-removal origin.
  const effectiveLease = withoutPendingRemovals(lease, await readPendingRemovals());
  const queue = await readQueue();
  queue.items = queue.items.filter((item) =>
    effectiveLease.capture_enabled && matchingArtifact(effectiveLease, item.event.url));
  narrowQueuedTrails(queue.items, effectiveLease);
  await chrome.storage.local.set({
    [POLICY_KEY]: lease,
    [PAUSE_KEY]: false,
    [QUEUE_KEY]: queue,
  });
  await narrowActivity(effectiveLease.capture_enabled ? effectiveLease : null);
  // The narrower policy is already committed, so a purge failure cannot
  // leave a removed origin capturing.
  await purgeSitePatterns(effectiveLease.capture_enabled ? effectiveLease : null);
  // A page installing policy may have just enabled capture on an open tab.
  // Background renewal must leave its in-flight navigation unmapped until
  // captureNavigation records the real open after renewal completes.
  if (effectiveLease.capture_enabled && seedOpenTabs) await rebuildActivityMap();
  return currentStatus(await activeLease());
}

async function currentStatus(
  lease: PolicyLease | null = null,
  queue: QueueState | null = null,
): Promise<WorkerStatus> {
  const policy = lease ?? (await readPolicy());
  const paused = await pausePending();
  const captureActive = !paused && policy?.capture_enabled === true &&
    policy.expires_at > Date.now();
  // A panel/status wake may be the first event after an hour boundary. Make
  // that wake perform the same local handoff as the sync alarm, so completed
  // totals are never presented as current-hour activity.
  if (captureActive) await freezeActivity();
  const visits = captureActive ? await readQueue() : queue ?? (await readQueue());
  const activity = await readActivity();
  if (captureActive) {
    settleActivity(activity, Date.now(), () => crypto.randomUUID());
    await writeActivity(activity);
  }
  let nextRetryAt = captureActive
    ? visits.items
      .filter((item) => matchingArtifact(policy, item.event.url) !== null)
      .reduce<number | null>((earliest, item) =>
        earliest === null ? item.retry_after : Math.min(earliest, item.retry_after), null)
    : null;
  if (captureActive && visits.overflow_delivery) {
    nextRetryAt = Math.min(nextRetryAt ?? Infinity, visits.overflow_delivery.retry_after);
  }
  return {
    queued: visits.items.length,
    live_activity: activity.totals.length,
    next_retry_at: nextRetryAt,
    overflow_count: visits.overflow_count,
    overflow_first_at: visits.overflow_first_at,
    overflow_last_at: visits.overflow_last_at,
    rejected_count: visits.rejected_count,
    last_error: visits.last_error,
    // Retry notices stay visible while an event or standalone loss marker
    // is still waiting for acknowledgement.
    retry_error: visits.overflow_delivery || visits.items.some((item) => item.attempts > 0)
      ? visits.retry_error : null,
    policy_expires_at: policy?.expires_at ?? null,
    pause_pending: paused,
    pause_token: await readPauseToken(),
    pending_removals: await readPendingRemovals(),
    host_error: await readHostError(),
    delivered_batches: await readDeliveredBatches(),
  };
}

interface ReadyBatch {
  batch_id: string;
  items: QueuedVisit[];
  overflow: OverflowMarker | null;
}

/** Take one stable bounded batch. Items retain their batch ID across retry, so
 * a lost acknowledgement resends exactly the request the host saw. */
async function takeReady(): Promise<ReadyBatch | null> {
  const lease = await activeLease();
  if (!lease || lease.expires_at <= Date.now() || !lease.capture_enabled) return null;
  const queue = await readQueue();
  const currentOverflow = !queue.overflow_delivery && queue.overflow_count > 0 &&
      queue.overflow_first_at && queue.overflow_last_at
    ? {
      count: queue.overflow_count,
      first_occurred_at: queue.overflow_first_at,
      last_occurred_at: queue.overflow_last_at,
    }
    : null;
  const ready = queue.items.filter(
    (item) =>
      item.retry_after <= Date.now() && matchingArtifact(lease, item.event.url) !== null,
  );
  const first = ready[0];
  if (!first) {
    // Deliver ready activity before retrying standalone evidence: a marker
    // refused by an older host or an unavailable log must not block capture.
    if (queue.overflow_delivery && queue.overflow_delivery.retry_after <= Date.now()) {
      return { ...queue.overflow_delivery, items: [] };
    }
    if (queue.items.length > 0 || !currentOverflow) return null;
    queue.overflow_delivery = {
      batch_id: crypto.randomUUID(), overflow: currentOverflow, retry_after: 0,
    };
    await writeQueue(queue);
    return { ...queue.overflow_delivery, items: [] };
  }
  const batchId = first.batch_id ?? crypto.randomUUID();
  const localDay = first.event.occurred_at.slice(0, 10);
  const batch = ready.filter((item) =>
    first.batch_id ? item.batch_id === batchId :
      item.batch_id === undefined && item.event.occurred_at.startsWith(localDay)).slice(0, 64);
  const carrier = batch[0];
  if (!carrier) return null;
  const storedOverflow = batch.find((item) => item.batch_overflow !== undefined);
  // A batch created by an older extension has no recoverable overflow
  // snapshot. Replaying it without a marker is conservative: existing
  // overflow stays visible for a later fresh batch instead of risking an
  // acknowledgement that clears evidence the host never wrote.
  const overflow = first.batch_id === undefined
    ? currentOverflow
    : storedOverflow?.batch_overflow ?? null;
  // Freeze these events before they are sent to the host.
  if (batch.some((item) => item.dispatched !== true || item.batch_id !== batchId ||
      storedOverflow === undefined)) {
    // Pending removals can narrow the lease without an install_policy.
    narrowQueuedTrails(batch, lease);
    for (const item of batch) {
      item.dispatched = true;
      item.batch_id = batchId;
    }
    // One carrier is enough; ackBatch moves it if a partial retry ever drops
    // this item while retaining another member of the batch.
    if (storedOverflow === undefined) carrier.batch_overflow = overflow;
    await writeQueue(queue);
  }
  return { batch_id: batchId, items: batch, overflow };
}

async function ackBatch(
  batch: ReadyBatch,
  results: Array<{ event_id: string; outcome: VisitOutcome; reason: string | null }>,
  overflowRecorded: boolean,
): Promise<WorkerStatus> {
  const queue = await readQueue();
  const expected = new Set(batch.items.map((item) => item.event.event_id));
  if (results.length !== expected.size || results.some((result) => !expected.has(result.event_id))) {
    throw new Error("Host returned outcomes for a different capture batch");
  }
  const byId = new Map(results.map((result) => [result.event_id, result]));
  const retained: QueuedVisit[] = [];
  for (const item of queue.items) {
    if (item.batch_id !== batch.batch_id) {
      retained.push(item);
      continue;
    }
    const result = byId.get(item.event.event_id);
    if (!result) throw new Error("Host omitted a capture batch outcome");
    if (result.outcome === "retryable") {
      item.attempts += 1;
      item.retry_after = Date.now() +
        Math.min(60 * 60_000, 5_000 * 2 ** Math.min(item.attempts, 10));
      queue.retry_error = result.reason ?? "The host could not save this capture batch yet";
      retained.push(item);
    } else if (result.outcome === "rejected") {
      queue.rejected_count += 1;
      queue.last_error = result.reason ?? "A capture record was rejected by host policy";
    }
  }
  queue.items = retained;
  const retryingBatch = retained.filter((item) => item.batch_id === batch.batch_id);
  const retryCarrier = retryingBatch[0];
  if (retryCarrier && overflowRecorded) {
    for (const item of retryingBatch) delete item.batch_overflow;
    retryCarrier.batch_overflow = null;
  } else if (retryCarrier &&
      !retryingBatch.some((item) => item.batch_overflow !== undefined)) {
    retryCarrier.batch_overflow = batch.overflow;
  }
  // Dismiss can reset the counters while this batch is in the host. Only
  // subtract the frozen snapshot when the live range still starts with it;
  // otherwise every current drop happened after that reset and is new.
  const snapshotStillCounted = batch.overflow !== null &&
    queue.overflow_count >= batch.overflow.count &&
    queue.overflow_first_at === batch.overflow.first_occurred_at;
  if (overflowRecorded && snapshotStillCounted) {
    const recorded = batch.overflow?.count ?? 0;
    queue.overflow_count = Math.max(0, queue.overflow_count - recorded);
    if (queue.overflow_count === 0) {
      queue.overflow_first_at = null;
      queue.overflow_last_at = null;
    } else if (batch.overflow) {
      // New drops arrived while the host was writing. Their precise first
      // hour was not in the acknowledged snapshot; retain a conservative
      // range beginning at the last hour already marked rather than losing
      // evidence or clearing the newer count.
      queue.overflow_first_at = batch.overflow.last_occurred_at;
    }
  }
  if (batch.items.length === 0 && queue.overflow_delivery?.batch_id === batch.batch_id) {
    if (overflowRecorded) delete queue.overflow_delivery;
    else {
      queue.overflow_delivery.retry_after = Date.now() + 5_000;
      queue.retry_error = "The host could not save capture overflow evidence yet";
    }
  }
  if (!queue.overflow_delivery && !queue.items.some((entry) => entry.attempts > 0)) {
    queue.retry_error = null;
  }
  await writeQueue(queue);
  return currentStatus(null, queue);
}

// Kept for open extension pages from the previous build while they drain an
// in-flight single-record acknowledgement during an upgrade.
async function ackVisit(
  eventId: string,
  outcome: VisitOutcome,
  reason: string | null,
): Promise<WorkerStatus> {
  const queue = await readQueue();
  const item = queue.items.find((entry) => entry.event.event_id === eventId);
  if (!item) return currentStatus(null, queue);
  if (outcome === "retryable") {
    item.attempts += 1;
    item.retry_after = Date.now() + Math.min(60 * 60_000, 5_000 * 2 ** Math.min(item.attempts, 10));
    queue.retry_error = reason ?? "The host could not save this capture record yet";
  } else {
    queue.items = queue.items.filter((entry) => entry.event.event_id !== eventId);
    if (outcome === "rejected") {
      queue.rejected_count += 1;
      queue.last_error = reason ?? "A capture record was rejected by host policy";
    }
    if (!queue.items.some((entry) => entry.attempts > 0)) queue.retry_error = null;
  }
  await writeQueue(queue);
  return currentStatus(null, queue);
}

async function suspendPolicy(): Promise<WorkerStatus> {
  // A repairable host config issue is not a site removal. Remove the
  // capture lease immediately, but leave queued visits and an explicit
  // local pause untouched until a valid host policy can reconcile them.
  await chrome.storage.local.set({ [POLICY_KEY]: null });
  await purgeSitePatterns(null);
  return currentStatus();
}

// --- Background sync with the host -----------------------------------------
// Everything below runs outside `serialize`, because it waits on the host and
// on the config lock; each storage step inside it is serialized on its own.

interface SyncNeeds {
  renew: boolean;
  deliver: boolean;
}

async function syncNeeds(): Promise<SyncNeeds> {
  const state = await readPolicyState();
  const lease = state.lease;
  if (await pausePending()) return { renew: false, deliver: false };
  if (state.legacyLease) {
    return { renew: state.legacyLease.capture_enabled, deliver: false };
  }
  if (!lease || !lease.capture_enabled) {
    return { renew: false, deliver: false };
  }
  await freezeActivity();
  const now = Date.now();
  const renew = lease.expires_at - now <= LEASE_RENEW_WINDOW_MS;
  const queue = await readQueue();
  const deliver = queue.items.some((item) => item.retry_after <= now) ||
    (queue.overflow_delivery ? queue.overflow_delivery.retry_after <= now :
      queue.items.length === 0 && queue.overflow_count > 0);
  return { renew, deliver };
}

/** Whether this URL needs a host renewal before capture can decide. */
async function leaseNeedsRenewalFor(url: string): Promise<boolean> {
  const state = await readPolicyState();
  const parsed = captureUrl(url);
  if (!parsed) return false;
  if (state.legacyLease) {
    return state.legacyLease.capture_enabled &&
      state.legacyLease.origins.includes(parsed.origin) &&
      !await pausePending();
  }
  const lease = state.lease;
  if (!lease || !lease.capture_enabled || lease.expires_at > Date.now()) return false;
  // Ask as if the lapsed lease were still current: only an origin it has an
  // adapter for needs a renewal.
  if (!leaseCoversOrigin(lease, parsed)) return false;
  return !await pausePending();
}

/** Read the host config and publish it as the lease, as a page's reload does. */
async function renewPolicy(host: HostClient): Promise<void> {
  await withLatestHostState(
    withConfigMutationLock,
    () => host.call({
      type: "get_config", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
    }, "config_result"),
    async (response) => {
      const config = response.config;
      if (response.config_issue || response.adapter_issue ||
          (!config.storage && config.capture_enabled)) {
        await serialize(suspendPolicy);
        return;
      }
      await serialize(() => installPolicy(
        leaseFor(config, response.revision), !config.capture_enabled, null));
    },
  );
}

/** Send due visits; returns why the host could not be reached, if it could not. */
async function deliverPending(host: HostClient): Promise<string | null> {
  // One extra send covers overflow that arrives during the final full batch.
  for (let processed = 0; processed <= MAX_QUEUED_VISITS; processed += 64) {
    // Re-read eligibility before each send so a site removal or pause
    // cannot keep draining a stale pending snapshot.
    const batch = await serialize(takeReady);
    if (!batch) return null;
    let failure: string | null = null;
    try {
      const response = await host.call({
        type: "record_batch", protocol_version: PROTOCOL_VERSION,
        request_id: newRequestId(), batch_id: batch.batch_id,
        events: batch.items.map((item) => isActivityEvent(item.event)
          ? { kind: "activity", visit: null, activity: item.event }
          : { kind: "visit", visit: item.event, activity: null }),
        overflow: batch.overflow,
      }, "batch_recorded");
      if (response.batch_id !== batch.batch_id) {
        throw new Error("Host acknowledged a different capture batch");
      }
      diag.debug("capture batch delivered", {
        batch_id: batch.batch_id, events: batch.items.length,
        retryable: response.results.filter((result) => result.outcome === "retryable").length,
        overflow_recorded: response.overflow_recorded,
      });
      await serialize(async () => {
        await ackBatch(batch, response.results, response.overflow_recorded);
        await chrome.storage.local.set({ [DELIVERED_KEY]: await readDeliveredBatches() + 1 });
      });
      if (response.results.some((result) => result.outcome === "retryable") ||
          (batch.items.length === 0 && !response.overflow_recorded)) return null;
    } catch (error) {
      failure = describe(error);
      await serialize(() => ackBatch(batch, batch.items.map((item) => ({
        event_id: item.event.event_id, outcome: "retryable", reason: failure,
      })), false));
      return failure;
    }
  }
  return null;
}

async function runSync(): Promise<void> {
  await ensureTrustedStorage();
  const needs = await serialize(syncNeeds);
  if (!needs.renew && !needs.deliver) return;
  // One host process per sync; it exits when this port closes.
  const host = new HostClient();
  let failure: string | null = null;
  const started = performance.now();
  diag.info("host sync started", { renew: needs.renew, deliver: needs.deliver });
  try {
    // A failed renewal still leaves the current lease, if it has not
    // expired, good enough to deliver under.
    if (needs.renew) await renewPolicy(host).catch((error: unknown) => { failure = describe(error); });
    const undelivered = await deliverPending(host);
    failure ??= undelivered;
  } catch (error) {
    failure ??= describe(error);
  } finally {
    host.disconnect();
    const fields = { outcome: failure === null ? "ok" : "failed", elapsed_ms: Math.round(performance.now() - started) };
    if (failure === null) diag.info("host sync finished", fields);
    else diag.warn("host sync finished", { ...fields, error: failure });
    await chrome.storage.local.set({ [HOST_ERROR_KEY]: failure });
  }
}

let syncing: Promise<void> | null = null;

/** Start a sync, or join the one already running. Never rejects. */
function syncWithHost(): Promise<void> {
  syncing ??= runSync()
    .catch((error: unknown) => diag.warn("host sync failed", { error }))
    .finally(() => { syncing = null; });
  return syncing;
}

async function readDeliveredBatches(): Promise<number> {
  const stored = await chrome.storage.local.get(DELIVERED_KEY);
  const value = stored[DELIVERED_KEY];
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : 0;
}

async function readHostError(): Promise<string | null> {
  const stored = await chrome.storage.local.get(HOST_ERROR_KEY);
  const value = stored[HOST_ERROR_KEY];
  return typeof value === "string" ? value : null;
}

async function handleMessage(message: WorkerRequest): Promise<unknown> {
  await ensureTrustedStorage();
  switch (message.kind) {
    case "install_policy":
      return installPolicy(message.lease, message.resume_after_confirmation,
        message.resume_after_pause_token, true);
    case "suspend_policy":
      return suspendPolicy();
    case "pause_capture": {
      const queue = await readQueue().catch(() => emptyQueue());
      queue.items = [];
      await chrome.storage.local.set({
        [PAUSE_KEY]: true, [PAUSE_TOKEN_KEY]: crypto.randomUUID(),
        [POLICY_KEY]: null, [QUEUE_KEY]: queue,
      });
      await narrowActivity(null);
      // Pausing promises to clear these samples.
      await purgeSitePatterns(null);
      return currentStatus(null, queue);
    }
    case "get_pending_ids":
      return (await readQueue()).items.map((item) => item.event.event_id);
    case "discard_pending": {
      if (!Array.isArray(message.event_ids) ||
          message.event_ids.length > MAX_QUEUED_VISITS ||
          !message.event_ids.every((id) => typeof id === "string")) {
        throw new Error("Invalid pending-record discard request");
      }
      const selected = new Set(message.event_ids);
      const queue = await readQueue();
      queue.items = queue.items.filter((item) => !selected.has(item.event.event_id));
      await writeQueue(queue);
      return currentStatus(null, queue);
    }
    case "get_pending":
      return (await takeReady())?.items ?? [];
    case "get_activity": {
      await freezeActivity();
      return (await readActivity()).totals;
    }
    case "get_site_patterns":
    case "clear_site_patterns": {
      if (typeof message.origin !== "string" || httpOrigin(message.origin) !== message.origin) {
        throw new Error("Invalid site pattern origin");
      }
      // Clearing is local deletion, so even unreadable pause/removal state
      // cannot prevent it. Reading still reports that policy failure.
      const lease = message.kind === "clear_site_patterns" ? await activeLease().catch(() => null) : await activeLease();
      const samples = await patternSamples(lease);
      if (message.kind === "clear_site_patterns") delete samples[message.origin];
      await chrome.storage.local.set({ [SITE_PATTERNS_KEY]: samples });
      return samples[message.origin] ?? [];
    }
    case "ack_visit":
      return ackVisit(message.event_id, message.outcome, message.reason);
    case "get_status":
    case "sync_now":
      // sync_now is answered in the message listener, after the sync finishes.
      await activeLease();
      return currentStatus();
    case "clear_notices": {
      const queue = await readQueue();
      queue.last_error = null;
      queue.retry_error = null;
      queue.overflow_count = 0;
      queue.overflow_first_at = null;
      queue.overflow_last_at = null;
      queue.rejected_count = 0;
      await writeQueue(queue);
      return currentStatus(null, queue);
    }
    case "clear_pending_removals": {
      // Clear only once the host's lease no longer has an adapter on any
      // pending origin. Until then, capture stays off for those origins.
      const pending = await readPendingRemovals();
      if (pending.length && receivedLease && !holdsPendingRemoval(receivedLease, pending)) {
        await chrome.storage.local.remove([
          PENDING_REMOVALS_KEY, LEGACY_REVOCATIONS_KEY, LEGACY_REMOVED_SITES_KEY,
        ]);
      }
      return currentStatus();
    }
    default:
      throw new Error("Unknown worker request");
  }
}

function isWorkerRequest(value: unknown): value is WorkerRequest {
  return typeof value === "object" && value !== null && typeof (value as { kind?: unknown }).kind === "string";
}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (sender.id !== chrome.runtime.id ||
      !TRUSTED_PAGES.some((page) => sender.url === chrome.runtime.getURL(page))) {
    return false;
  }
  if (!isWorkerRequest(message)) {
    sendResponse({ ok: false, value: null, error: "Unknown worker request" } satisfies WorkerReply<null>);
    return false;
  }
  // A sync waits on the host and the config lock, so it must not hold the
  // serialized chain that other messages and navigation capture share.
  const before = message.kind === "sync_now" ? syncWithHost() : Promise.resolve();
  void before.then(() => serialize(() => handleMessage(message)))
    .then((value) => sendResponse({ ok: true, value, error: null } satisfies WorkerReply<unknown>))
    .catch((error: unknown) => {
      diag.info("worker request failed", { kind: message.kind, error });
      sendResponse({ ok: false, value: null, error: describe(error) } satisfies WorkerReply<null>);
    });
  return true;
});

async function captureAndDeliver(details: ChromeNavigationDetails, useCurrentTitle: boolean): Promise<void> {
  if (details.frameId !== 0 || details.documentLifecycle !== "active" || details.tabId < 0) return;
  // After Chrome was closed past the lease, renew before capturing, so the
  // first visits of the day are not dropped for want of a fresh lease.
  const capture = async (): Promise<boolean | "expired"> => {
    await ensureTrustedStorage();
    return await leaseNeedsRenewalFor(details.url) ? "expired" : captureNavigation(details, useCurrentTitle);
  };
  let captured = await serialize(capture);
  if (captured === "expired") {
    await syncWithHost();
    captured = await serialize(() => captureNavigation(details, useCurrentTitle));
  }
  if (!captured) return;
  // Keep an existing alarm, so steady browsing cannot keep pushing delivery
  // of a newly completed hour back.
  if (!await chrome.alarms.get(DELIVER_ALARM)) {
    await chrome.alarms.create(DELIVER_ALARM, { delayInMinutes: DELIVER_DELAY_MINUTES });
  }
}

const onCommitted = (details: ChromeNavigationDetails): void => {
  void captureAndDeliver(details, false).catch((error: unknown) => {
    diag.warn("navigation capture paused", { error });
  });
};

const onHistoryStateUpdated = (details: ChromeNavigationDetails): void => {
  void captureAndDeliver(details, true).catch((error: unknown) => {
    diag.warn("SPA capture paused", { error });
  });
};

// Register synchronously at top level so MV3 can wake this worker for
// navigation. webNavigation is a required permission.
chrome.webNavigation.onCommitted.addListener(onCommitted);
chrome.webNavigation.onHistoryStateUpdated.addListener(onHistoryStateUpdated);

// Chrome reports a tab's title only for origins this extension was granted.
chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  const title = changeInfo.title;
  if (title === undefined) return;
  void serialize(() => patchTitle(tabId, title)).catch((error: unknown) => {
    diag.warn("title update skipped", { error });
  });
});

chrome.tabs.onActivated.addListener((info) => {
  void serialize(async () => {
    const activity = await readActivity();
    activateTab(activity, info.tabId, info.windowId, Date.now(), () => crypto.randomUUID());
    await writeActivity(activity);
  }).catch((error: unknown) => diag.warn("tab attention update skipped", { error }));
});

chrome.tabs.onRemoved?.addListener((tabId) => {
  void serialize(async () => {
    const activity = await readActivity();
    removeTab(activity, tabId, Date.now(), () => crypto.randomUUID());
    await writeActivity(activity);
  }).catch((error: unknown) => diag.warn("tab removal update skipped", { error }));
});

const moveActivityTab = (added: number, removed: number): void => {
  void serialize(async () => {
    const activity = await readActivity();
    replaceTab(activity, added, removed);
    await writeActivity(activity);
  }).catch((error: unknown) => diag.warn("discarded-tab update skipped", { error }));
};
chrome.tabs.onReplaced?.addListener(moveActivityTab);
chrome.webNavigation.onTabReplaced?.addListener((details) =>
  moveActivityTab(details.tabId, details.replacedTabId));

chrome.webNavigation.onCreatedNavigationTarget?.addListener((details) => {
  void serialize(async () => {
    const activity = await readActivity();
    createdNavigationTarget(activity, details.sourceTabId, details.tabId);
    await writeActivity(activity);
  }).catch((error: unknown) => diag.warn("trail source skipped", { error }));
});

chrome.windows?.onFocusChanged.addListener((windowId) => {
  void serialize(async () => {
    const activity = await readActivity();
    focusWindow(activity, windowId === chrome.windows?.WINDOW_ID_NONE ? null : windowId,
      Date.now(), () => crypto.randomUUID());
    await writeActivity(activity);
  }).catch((error: unknown) => diag.warn("window attention update skipped", { error }));
});

chrome.idle?.setDetectionInterval(300);
chrome.idle?.onStateChanged.addListener((state) => {
  void serialize(async () => {
    const activity = await readActivity();
    setIdle(activity, state !== "active", Date.now(), () => crypto.randomUUID());
    await writeActivity(activity);
  }).catch((error: unknown) => diag.warn("idle update skipped", { error }));
});

// Capture Site's advanced-editor handoffs expire after 30 minutes. Settings
// deletes the one it opens; worker start and maintenance delete the rest.
function pruneCaptureSiteDrafts(): void {
  void pruneDraftHandoffs(chrome.storage.session, Date.now())
    .catch((error: unknown) => diag.warn("capture-site draft cleanup skipped", { error }));
}

chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === SYNC_ALARM || alarm.name === DELIVER_ALARM) {
    void syncWithHost();
    void serialize(pruneCurrentSitePatterns).catch(patternFailure);
    pruneCaptureSiteDrafts();
  }
});

chrome.runtime.onStartup.addListener(() => {
  pruneCaptureSiteDrafts();
  void syncWithHost();
});

pruneCaptureSiteDrafts();

export async function rebuildActivityMap(): Promise<void> {
  if (!chrome.tabs.query || !chrome.webNavigation.getFrame) return;
  await ensureTrustedStorage();
  const lease = await activeLease();
  if (!lease?.capture_enabled) return;
  const activity = await readActivity();
  const now = Date.now();
  if (lease.expires_at <= now) {
    // An expired lease is not authority to measure the restored interval.
    activity.measured_at = now;
    await writeActivity(activity);
    return;
  }
  // storage.session survives an ordinary MV3 worker suspension. Settle the
  // restored interval before refreshing the browser state below; otherwise
  // every worker restart discards the dwell time since its previous event.
  // A browser/extension restart clears storage.session, so readActivity()
  // starts its fresh cursor at now and this remains a no-op in that case.
  settleActivity(activity, now, () => crypto.randomUUID());
  const [tabs, focused, idle] = await Promise.all([
    chrome.tabs.query({}),
    chrome.windows?.getLastFocused() ?? Promise.resolve({ id: undefined, focused: false }),
    chrome.idle?.queryState(300) ?? Promise.resolve("active" as const),
  ]);
  activity.chrome_focused = focused.focused;
  activity.last_focused_window = focused.id ?? activity.last_focused_window;
  activity.idle = idle !== "active";
  for (const tab of tabs) {
    if (tab.id === undefined || tab.incognito !== false || activity.tabs[String(tab.id)]) continue;
    const frame = await chrome.webNavigation.getFrame({ tabId: tab.id, frameId: 0 }).catch(() => null);
    const url = frame?.url;
    const match = url ? matchingArtifact(lease, url) : null;
    if (!url || !match) continue;
    activity.tabs[String(tab.id)] = {
      artifact_id: match.artifact_id, artifact_type: match.type, url,
      // Only keep tab metadata after recognizing the live top-frame URL,
      // and only if the tab snapshot still names the same page.
      title: tab.status === "complete" && tab.url && withoutHash(tab.url) === withoutHash(url)
        ? tab.title?.slice(0, MAX_TITLE_CHARS) ?? null : null,
      mode: match.mode, window_id: tab.windowId ?? -1,
    };
    if (tab.active) activity.active_by_window[String(tab.windowId ?? -1)] = tab.id;
  }
  await writeActivity(activity);
}

export function queueActivityMapRebuild(): Promise<void> {
  return serialize(rebuildActivityMap);
}

// Queue behind this pass so a navigation waking the worker cannot race a
// missing-map rebuild. Chrome APIs that older test doubles omit simply skip it.
void queueActivityMapRebuild().catch((error: unknown) =>
  diag.warn("activity-map rebuild skipped", { error }));

// Registering the alarm again would restart its period on every wake.
void chrome.alarms.get(SYNC_ALARM).then(async (existing) => {
  if (!existing) await chrome.alarms.create(SYNC_ALARM, { periodInMinutes: SYNC_PERIOD_MINUTES });
})
  .catch((error: unknown) => diag.warn("sync alarm setup failed", { error }));

void chrome.sidePanel.setPanelBehavior({ openPanelOnActionClick: true }).catch((error: unknown) => {
  diag.warn("side panel setup failed", { error });
});

// --- Summarize & log from the shortcut and the context menu ----------------

/** Starts `tab`'s summary run and says how it went, for the panel. */
async function summaryNotice(tab: ChromeTab, tabId: number): Promise<PanelNotice> {
  const failed = (message: string): PanelNotice =>
    ({ kind: "summarize_failed", message, tab_id: tabId, at: Date.now() });
  const raw = tab.url;
  let url: URL | null;
  try {
    url = raw === undefined ? null : new URL(raw);
  } catch {
    url = null;
  }
  if (raw === undefined) return failed(await noAddressMessage());
  if (url === null || !isHttpUrl(url)) return failed(NOT_A_WEB_PAGE_MESSAGE);
  const host = new HostClient();
  try {
    const { run } = await summarizeTab(
      { id: tabId, url: raw, title: tab.title ?? "" },
      { scripting: chrome.scripting, host, storage: chrome.storage.local, now: () => new Date() },
    );
    return { kind: "run_started", run_id: run.run_id, tab_id: tabId, at: Date.now() };
  } catch (error) {
    return failed(summarizeFailureText(error));
  } finally {
    host.disconnect();
  }
}

/** Why a tab has no address. Without `tabs`, Chrome gives no URL for a page
 * outside Brauser's site access: one it never covers (chrome://, New Tab), or
 * any page once the user withdrew that access in Chrome. Unlike capture, this
 * one user-started check asks Chrome, as the panel's banner does. */
async function noAddressMessage(): Promise<string> {
  try {
    if (!await chrome.permissions.contains({ origins: SITE_ACCESS_ORIGINS })) return SITE_ACCESS_NEEDED_MESSAGE;
  } catch {
    // Without an answer, show the ordinary message rather than a false alarm.
  }
  return NOT_A_WEB_PAGE_MESSAGE;
}

async function publishNotice(notice: PanelNotice): Promise<void> {
  await chrome.storage.session.set({ [PANEL_NOTICE_KEY]: notice });
  try {
    await chrome.runtime.sendMessage(notice);
  } catch {
    // No panel is open yet; it reads the stored notice when it loads.
  }
}

function summarizeFromGesture(tab: ChromeTab | undefined): void {
  const tabId = tab?.id;
  if (tab === undefined || tabId === undefined) return;
  // Chrome opens the side panel only inside the user's gesture, which ends at
  // the first await, so this call comes before any.
  void chrome.sidePanel.open({ tabId }).catch((error: unknown) =>
    diag.warn("could not open the side panel", { error }));
  void summaryNotice(tab, tabId).then(publishNotice).catch((error: unknown) =>
    diag.warn("summary notice failed", { error }));
}

chrome.commands.onCommand.addListener((command, tab) => {
  if (command === "summarize") summarizeFromGesture(tab);
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === SUMMARIZE_MENU_ID) summarizeFromGesture(tab);
});

function runMigration(): Promise<void> {
  return serialize(migratePendingRemovals).catch((error: unknown) =>
    diag.warn("pending site removal migration failed", { error }));
}

// Earlier builds added a "note" page context-menu item and stored per-origin
// note grants; clear both on install or update so nothing stale survives.
// Chrome keeps menu items across worker restarts, so the summarize item is
// registered here only.
chrome.runtime.onInstalled.addListener((details) => {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create(
      { id: SUMMARIZE_MENU_ID, title: SUMMARIZE_MENU_TITLE, contexts: ["page", "selection"] },
      () => {
        const error = chrome.runtime.lastError;
        if (error) diag.warn("context menu setup failed", { error: error.message });
      },
    );
  });
  void chrome.storage.local.remove(LEGACY_NOTE_ORIGINS_KEY).catch((error: unknown) =>
    diag.warn("note grant cleanup failed", { error }));
  if (details?.reason === "update") void runMigration();
});

// Once per worker start, in case the update event was missed.
void runMigration();

async function pruneCurrentSitePatterns(): Promise<void> {
  await ensureTrustedStorage();
  let lease: PolicyLease | null;
  try {
    lease = await activeLease();
  } catch (error) {
    // Unreadable policy state enables no origin, so samples still expire.
    await pruneSitePatterns(null);
    throw error;
  }
  await pruneSitePatterns(lease);
}

// Expiry and policy removals apply on a cold worker start, too.
void serialize(pruneCurrentSitePatterns).catch(patternFailure);
