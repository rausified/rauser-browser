// The Capture Site advanced-editor handoff. When the user opens advanced
// settings from a drafted review, the panel stores the drafted adapters and
// the private URL shapes in trusted `storage.session` under a one-time ID
// with a 30-minute expiry, then opens the settings page with that ID. The
// entry never holds a raw URL, a link href or a tab title. Settings reads it
// once and deletes it; the worker deletes expired entries.
import type { ShapeCount } from "../protocol/ts/generated.js";
import { httpOrigin, isUrlShape } from "./site-patterns.js";

export const DRAFT_HANDOFF_PREFIX = "captureSiteDraft:";
export const DRAFT_HANDOFF_TTL_MS = 30 * 60_000;
// The host checks at most 8 drafted adapters of at most 16 rules each.
const MAX_ADAPTERS = 8;
const MAX_RULES = 16;
const MAX_SHAPES = 128;
const ID = /^[A-Za-z0-9-]{1,64}$/;

type Table = Record<string, unknown>;

export interface DraftHandoff {
  id: string;
  origin: string;
  /** Milliseconds since the epoch when the panel stored the entry. */
  created: number;
  adapters: Table[];
  /** Per handed-off adapter, the canonical JSON of each drafted rule. An
   * editor rule that is not among them is a user edit. */
  baseline_rules: string[][];
  visited: ShapeCount[];
  links: ShapeCount[];
}

/** JSON with object keys sorted, so equal values compare equal as text. */
export function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value !== null && typeof value === "object") {
    const record = value as Record<string, unknown>;
    return `{${Object.keys(record).sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(record[key])}`).join(",")}}`;
  }
  return JSON.stringify(value) ?? "undefined";
}

function isTable(value: unknown): value is Table {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function rulesOf(table: Table): unknown[] {
  return Array.isArray(table.match) ? table.match as unknown[] : [];
}

/** A new handoff entry for drafted `adapters`; every rule they hold now is
 * a drafted rule. */
export function draftHandoff(
  id: string, origin: string, adapters: readonly Table[], visited: readonly ShapeCount[],
  links: readonly ShapeCount[], now: number,
): DraftHandoff {
  const tables = adapters.map((table) => structuredClone(table));
  return {
    id, origin, created: now, adapters: tables,
    baseline_rules: tables.map((table) => rulesOf(table).map(canonicalJson)),
    visited: visited.map(({ shape, count }) => ({ shape, count })),
    links: links.map(({ shape, count }) => ({ shape, count })),
  };
}

export function handoffKey(id: string): string {
  return `${DRAFT_HANDOFF_PREFIX}${id}`;
}

export function isHandoffId(id: string): boolean {
  return ID.test(id);
}

function shapes(value: unknown): value is ShapeCount[] {
  return Array.isArray(value) && value.length <= MAX_SHAPES && value.every((row) =>
    isTable(row) && isUrlShape(row.shape) && Number.isInteger(row.count) && (row.count as number) >= 1);
}

function valid(value: unknown, id: string): value is DraftHandoff {
  if (!isTable(value) || value.id !== id || typeof value.origin !== "string" ||
      httpOrigin(value.origin) !== value.origin || typeof value.created !== "number" ||
      !Number.isFinite(value.created)) return false;
  const { adapters, baseline_rules: baseline } = value;
  return Array.isArray(adapters) && adapters.length <= MAX_ADAPTERS &&
    adapters.every((table) => isTable(table) && table.origin === value.origin && rulesOf(table).length <= MAX_RULES) &&
    Array.isArray(baseline) && baseline.length === adapters.length &&
    baseline.every((rules) => Array.isArray(rules) && rules.length <= MAX_RULES &&
      rules.every((rule) => typeof rule === "string")) &&
    shapes(value.visited) && shapes(value.links);
}

function live(entry: DraftHandoff, now: number): boolean {
  return entry.created <= now && now - entry.created < DRAFT_HANDOFF_TTL_MS;
}

export async function storeDraftHandoff(area: ChromeStorageArea, entry: DraftHandoff): Promise<void> {
  if (!isHandoffId(entry.id)) throw new Error("Invalid draft handoff ID");
  await area.set({ [handoffKey(entry.id)]: entry });
}

/** Read the entry once and delete it. A missing, malformed or expired entry
 * answers null; it is deleted all the same. */
export async function takeDraftHandoff(
  area: ChromeStorageArea, id: string, now: number,
): Promise<DraftHandoff | null> {
  if (!isHandoffId(id)) return null;
  const key = handoffKey(id);
  const value = (await area.get(key))[key];
  await area.remove?.(key);
  return valid(value, id) && live(value, now) ? value : null;
}

/** Delete every expired or malformed handoff entry. */
export async function pruneDraftHandoffs(area: ChromeStorageArea, now: number): Promise<void> {
  const all = await area.get(null);
  const stale = Object.entries(all)
    .filter(([key, value]) => key.startsWith(DRAFT_HANDOFF_PREFIX) &&
      !(valid(value, key.slice(DRAFT_HANDOFF_PREFIX.length)) && live(value, now)))
    .map(([key]) => key);
  if (stale.length) await area.remove?.(stale);
}
