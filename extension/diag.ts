// Local diagnostics: a structured logger for the worker and the extension
// pages. Each entry goes to the console with a Brauser prefix and to one
// bounded ring buffer in trusted-only chrome.storage.local, which the settings
// page can copy for a bug report. The request_id field on native requests
// joins these entries with the host's host.jsonl and service.jsonl lines.
//
// Privacy: entries never carry page text, titles, note text, prompts, answers,
// or agent output. A URL appears only as its origin (diagOrigin). Field values
// are reduced to primitives and error name/message pairs before they are kept.
import { APP_NAME, storageKey } from "./brand.js";

export type DiagLevel = "debug" | "info" | "warn" | "error";
export type DiagValue = string | number | boolean | null | DiagError;

export interface DiagError {
  name: string;
  message: string;
  code?: string;
  stack?: string;
}

export interface DiagEntry {
  ts: string;
  level: DiagLevel;
  ctx: string;
  event: string;
  fields?: Record<string, DiagValue>;
}

/** The storage.local key holding the persisted ring buffer. */
export const DIAG_STORAGE_KEY = storageKey("diag_log_v1");
export const DIAG_MAX_ENTRIES = 500;
export const DIAG_MAX_BYTES = 256 * 1024;
/** How long entries wait in memory before one batched storage write. */
export const DIAG_FLUSH_MS = 1_000;
// A single string field is cut here, so one entry cannot crowd out the rest.
const MAX_STRING = 1_000;
const DIAG_LOCK = `${APP_NAME}-diag-log`;

/**
 * The origin of `url` (scheme, host, and port), the only form of a URL a
 * diagnostic may hold. Opaque origins keep only their scheme, and anything
 * that does not parse becomes "invalid".
 */
export function diagOrigin(url: unknown): string {
  if (typeof url !== "string" || url === "") return "none";
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return "invalid";
  }
  return parsed.origin === "null" ? parsed.protocol : parsed.origin;
}

function cut(text: string): string {
  return text.length > MAX_STRING ? `${text.slice(0, MAX_STRING)}…` : text;
}

/**
 * An error as a diagnostic value: its name and message, plus a string `code`
 * when it has one (HostError). The stack is left out unless asked for.
 */
export function serializeError(error: unknown, options: { stack?: boolean } = {}): DiagError {
  if (error instanceof Error) {
    const result: DiagError = { name: error.name, message: cut(error.message) };
    const code = (error as { code?: unknown }).code;
    if (typeof code === "string") result.code = code;
    if (options.stack && typeof error.stack === "string") result.stack = cut(error.stack);
    return result;
  }
  return { name: typeof error, message: cut(String(error)) };
}

/** Reduce `value` to something safe to keep: primitives pass, errors become
 * name and message, and any other object is recorded only by its kind. */
function diagValue(value: unknown): DiagValue | undefined {
  if (value === undefined) return undefined;
  if (value === null || typeof value === "boolean") return value;
  if (typeof value === "number") return Number.isFinite(value) ? value : String(value);
  if (typeof value === "string") return cut(value);
  if (typeof value === "bigint") return value.toString();
  if (value instanceof Error) return serializeError(value);
  return `[${Array.isArray(value) ? "array" : typeof value}]`;
}

function entryBytes(entry: DiagEntry): number {
  return new TextEncoder().encode(JSON.stringify(entry)).length;
}

function isEntry(value: unknown): value is DiagEntry {
  if (typeof value !== "object" || value === null) return false;
  const entry = value as Record<string, unknown>;
  return typeof entry.ts === "string" && typeof entry.level === "string" &&
    typeof entry.ctx === "string" && typeof entry.event === "string";
}

/**
 * Keep at most `maxEntries` entries and `maxBytes` of JSON. Oldest debug
 * entries go first, so routine detail cannot push out a warning; then the
 * oldest of any level.
 */
export function capEntries(
  entries: readonly DiagEntry[], maxEntries = DIAG_MAX_ENTRIES, maxBytes = DIAG_MAX_BYTES,
): DiagEntry[] {
  const sizes = entries.map(entryBytes);
  let bytes = sizes.reduce((total, size) => total + size, 0);
  let count = entries.length;
  const drop = new Set<number>();
  const over = () => count > maxEntries || bytes > maxBytes;
  for (const debugFirst of [true, false]) {
    for (let index = 0; index < entries.length && over(); index += 1) {
      if (drop.has(index) || (debugFirst && entries[index]?.level !== "debug")) continue;
      drop.add(index);
      count -= 1;
      bytes -= sizes[index] ?? 0;
    }
  }
  return entries.filter((_, index) => !drop.has(index));
}

interface DiagStorage {
  get(key: string): Promise<Record<string, unknown>>;
  set(items: Record<string, unknown>): Promise<void>;
  remove?(keys: string): Promise<void>;
  setAccessLevel?(options: { accessLevel: "TRUSTED_CONTEXTS" }): Promise<void>;
}

type DiagConsole = Pick<Console, DiagLevel>;

export interface DiagOptions {
  ctx?: string;
  /** The storage area; defaults to chrome.storage.local when present. */
  storage?: () => DiagStorage | null;
  console?: DiagConsole | null;
  now?: () => Date;
  flushMs?: number;
}

function chromeLocal(): DiagStorage | null {
  const api = (globalThis as { chrome?: { storage?: { local?: DiagStorage } } }).chrome;
  return api?.storage?.local ?? null;
}

/** Run `action` under an origin-wide lock where the platform has one, so the
 * worker and the pages do not overwrite each other's batches. */
function withDiagLock<T>(action: () => Promise<T>): Promise<T> {
  const locks = (globalThis as { navigator?: { locks?: LockManager } }).navigator?.locks;
  return locks?.request ? locks.request(DIAG_LOCK, { mode: "exclusive" }, action) : action();
}

/**
 * The logger. Entries wait in memory and reach storage in one write at most
 * every {@link DIAG_FLUSH_MS}, so a hot path never adds a storage write per
 * event. Persisting is best-effort: a failed write is dropped silently.
 */
export class DiagLog {
  private ctx: string;
  private readonly storage: () => DiagStorage | null;
  private readonly sink: DiagConsole | null;
  private readonly now: () => Date;
  private readonly flushMs: number;
  private pending: DiagEntry[] = [];
  private timer: ReturnType<typeof setTimeout> | null = null;
  private chain: Promise<void> = Promise.resolve();
  private trusted: Promise<boolean> | null = null;

  constructor(options: DiagOptions = {}) {
    this.ctx = options.ctx ?? "extension";
    this.storage = options.storage ?? chromeLocal;
    this.sink = options.console === undefined ? console : options.console;
    this.now = options.now ?? (() => new Date());
    this.flushMs = options.flushMs ?? DIAG_FLUSH_MS;
  }

  /** Name the context ("worker", "panel", "options") later entries carry. */
  setContext(ctx: string): void {
    this.ctx = ctx;
  }

  debug(event: string, fields?: Record<string, unknown>): void { this.log("debug", event, fields); }
  info(event: string, fields?: Record<string, unknown>): void { this.log("info", event, fields); }
  warn(event: string, fields?: Record<string, unknown>): void { this.log("warn", event, fields); }
  error(event: string, fields?: Record<string, unknown>): void { this.log("error", event, fields); }

  log(level: DiagLevel, event: string, fields?: Record<string, unknown>): void {
    const entry: DiagEntry = { ts: this.now().toISOString(), level, ctx: this.ctx, event };
    if (fields) {
      const kept: Record<string, DiagValue> = {};
      for (const [name, value] of Object.entries(fields)) {
        const reduced = diagValue(value);
        if (reduced !== undefined) kept[name] = reduced;
      }
      if (Object.keys(kept).length > 0) entry.fields = kept;
    }
    try {
      if (entry.fields) this.sink?.[level](`${APP_NAME} ${event}`, entry.fields);
      else this.sink?.[level](`${APP_NAME} ${event}`);
    } catch {
      // A console that throws must not break the caller.
    }
    this.pending.push(entry);
    // Storage that never comes back must not grow memory without bound.
    if (this.pending.length > DIAG_MAX_ENTRIES) this.pending.splice(0, this.pending.length - DIAG_MAX_ENTRIES);
    this.schedule();
  }

  private schedule(): void {
    if (this.timer !== null) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.flush();
    }, this.flushMs);
    // Node test runs need not wait for a pending flush to exit.
    (this.timer as { unref?: () => void }).unref?.();
  }

  /** Write every pending entry now. Never rejects. */
  flush(): Promise<void> {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.chain = this.chain.then(() => this.write()).catch(() => undefined);
    return this.chain;
  }

  private area(): DiagStorage | null {
    try {
      return this.storage();
    } catch {
      return null;
    }
  }

  private trustedStorage(storage: DiagStorage): Promise<boolean> {
    // Content scripts must not read diagnostics; without trusted-only
    // access, nothing is written.
    this.trusted ??= Promise.resolve(storage.setAccessLevel?.({ accessLevel: "TRUSTED_CONTEXTS" }))
      .then(() => true, () => {
        this.trusted = null;
        return false;
      });
    return this.trusted;
  }

  private async write(): Promise<void> {
    if (this.pending.length === 0) return;
    const storage = this.area();
    if (!storage || !await this.trustedStorage(storage)) return;
    const batch = this.pending;
    this.pending = [];
    try {
      await withDiagLock(async () => {
        const stored = await storage.get(DIAG_STORAGE_KEY);
        await storage.set({ [DIAG_STORAGE_KEY]: capEntries([...readEntries(stored[DIAG_STORAGE_KEY]), ...batch]) });
      });
    } catch {
      // Diagnostics are best-effort; the console copy remains.
    }
  }

  /** The persisted entries, oldest first, after writing pending ones. */
  async entries(): Promise<DiagEntry[]> {
    await this.flush();
    const storage = this.area();
    if (!storage) return [...this.pending];
    try {
      const stored = await storage.get(DIAG_STORAGE_KEY);
      return [...readEntries(stored[DIAG_STORAGE_KEY]), ...this.pending];
    } catch {
      return [...this.pending];
    }
  }

  /** Forget every entry, persisted and pending. */
  async clear(): Promise<void> {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    this.pending = [];
    const storage = this.area();
    if (!storage) return;
    this.chain = this.chain.then(() => withDiagLock(async () => {
      if (storage.remove) await storage.remove(DIAG_STORAGE_KEY);
      else await storage.set({ [DIAG_STORAGE_KEY]: [] });
    })).catch(() => undefined);
    await this.chain;
  }
}

function readEntries(value: unknown): DiagEntry[] {
  return Array.isArray(value) ? value.filter(isEntry) : [];
}

/** The logger every extension module shares. */
export const diag = new DiagLog();

interface ErrorEventLike { error?: unknown; message?: unknown }
interface RejectionEventLike { reason?: unknown }

/**
 * Name this context and log its uncaught errors and unhandled rejections at
 * error. A page also writes its pending entries when it is hidden, since it
 * may be closed before the next timed write.
 */
export function initDiag(ctx: string): void {
  diag.setContext(ctx);
  const target = globalThis as {
    addEventListener?: (type: string, listener: (event: unknown) => void) => void;
  };
  if (typeof target.addEventListener !== "function") return;
  target.addEventListener("error", (event) => {
    const { error, message } = event as ErrorEventLike;
    diag.error("uncaught error", { error: error ?? message });
  });
  target.addEventListener("unhandledrejection", (event) => {
    diag.error("unhandled rejection", { error: (event as RejectionEventLike).reason });
  });
  target.addEventListener("pagehide", () => { void diag.flush(); });
}

export interface DiagHeader {
  extension_version: string;
  build_id: string;
  protocol_version: number;
  host_version?: string;
}

/** The copyable report: a header line, then one JSON line per entry. */
export function diagnosticsText(header: DiagHeader, entries: readonly DiagEntry[], now = new Date()): string {
  const first = { type: `${APP_NAME.toLowerCase()}_diagnostics`, exported_at: now.toISOString(), ...header, entries: entries.length };
  return [first, ...entries].map((line) => JSON.stringify(line)).join("\n") + "\n";
}
