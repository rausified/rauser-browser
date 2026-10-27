// Private URL shapes for Capture Site (DESIGN §5.1). A shape contains route
// words and placeholder classes, never a page title or an identifier value.
export interface CountedShape {
  shape: string;
  count: number;
}

export interface SampledShape extends CountedShape {
  last_seen: number;
}

export interface SitePatterns {
  current: string | null;
  links: CountedShape[];
}

export const SAMPLE_TTL_MS = 7 * 24 * 60 * 60_000;
export const MAX_SITE_SHAPES = 20;
export const MAX_SHAPE_COUNT = 1_000_000;

/** This function is also the entire injected script. Keep its runtime
 * dependencies inside the function: Chrome serializes it without closures.
 * With readLinks=false it is pure and does not access page globals. */
export function inspectSitePatterns(expectedUrl: string, readLinks = false): SitePatterns {
  const empty = (): SitePatterns => ({ current: null, links: [] });
  const literal = (value: string): boolean => value.length > 0 && value.length <= 20 &&
    /^[a-z_-]+$/.test(value) && /[a-z]/.test(value) && (value.match(/[-_]/g)?.length ?? 0) <= 1;
  const placeholder = (value: string): string => {
    if (/^\d+$/.test(value)) return "{num}";
    if (/^[A-Z][A-Z0-9]+-\d+$/.test(value)) return "{key}";
    if (/^\{?[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\}?$/i.test(value)) return "{uuid}";
    for (const length of [32, 40, 64]) {
      if (value.length === length && /^[0-9a-f]+$/i.test(value)) return `{hex${length}}`;
    }
    if (/^.+-[0-9a-f]{32}$/i.test(value)) return "{tailhex32}";
    if (/\d/.test(value) && /^[A-Za-z0-9_-]+$/.test(value)) return "{id}";
    return "{slug}";
  };
  const parse = (raw: string): URL | null => {
    if (typeof raw !== "string" || raw.length > 8_192) return null;
    try {
      const url = new URL(raw);
      return (url.protocol === "https:" || url.protocol === "http:") && !url.username && !url.password ? url : null;
    } catch { return null; }
  };
  const shape = (url: URL): string | null => {
    // Don't decode path names: a percent-encoded name is always a slug.
    const parts = url.pathname.split("/").slice(1);
    if (parts.length > 32) return null;
    const path = "/" + parts.map((part) => part === "" ? "" : literal(part) ? part : placeholder(part)).join("/");
    const query: string[] = [];
    for (const [key, value] of url.searchParams) {
      if (query.length >= 8) return null;
      query.push(`${literal(key) ? key : "{param}"}=${placeholder(value)}`);
    }
    query.sort();
    const result = path + (query.length ? `?${query.join("&")}` : "");
    return result.length <= 2_048 ? result : null;
  };
  const url = parse(expectedUrl);
  if (!url) return empty();
  const current = shape(url);
  if (!current) return empty();
  if (!readLinks) return { current, links: [] };
  // The panel observed a particular top-frame URL. A same-origin navigation
  // (including an SPA or hash change) still invalidates that observation.
  if (window.top !== window || location.href !== expectedUrl || location.origin !== url.origin) return empty();
  const counts = new Map<string, number>();
  // document.links is a live collection the browser walks lazily, so reading
  // its first 500 entries does not materialize every link on a huge page the
  // way a static querySelectorAll result would. Count every inspected link,
  // including off-origin ones, to bound work on pages that plant a very large
  // number of unrelated links. Never read its length: that walks it all.
  const anchors = document.links;
  for (let index = 0; index < 500; index += 1) {
    const anchor = anchors[index];
    if (!anchor) break;
    const linked = parse(anchor.href);
    if (!linked || linked.origin !== url.origin) continue;
    const value = shape(linked);
    if (value) counts.set(value, (counts.get(value) ?? 0) + 1);
  }
  // A literal must recur in distinct shapes. Repeated links to one customer
  // page must not turn that customer's name into a trusted route word.
  const occurrences = new Map<string, number>();
  for (const value of counts.keys()) {
    const words = new Set(value.split("?")[0]!.split("/").filter(literal));
    for (const word of words) occurrences.set(word, (occurrences.get(word) ?? 0) + 1);
  }
  const generalized = new Map<string, number>();
  for (const [value, count] of counts) {
    const [path, query] = value.split("?");
    const normalized = path!.split("/").map((part) => literal(part) && occurrences.get(part) === 1 ? "{slug}" : part).join("/") +
      (query === undefined ? "" : `?${query}`);
    generalized.set(normalized, (generalized.get(normalized) ?? 0) + count);
  }
  // Recheck after reading the DOM as well as before it. Link shapes never
  // enter storage, samples, native messages or the delivery buffer.
  if (location.href !== expectedUrl || location.origin !== url.origin) return empty();
  return { current, links: [...generalized].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .slice(0, 50).map(([value, count]) => ({ shape: value, count })) };
}

export function httpOrigin(raw: string): string | null {
  try {
    const url = new URL(raw);
    return (url.protocol === "http:" || url.protocol === "https:") && !url.username && !url.password ? url.origin : null;
  } catch { return null; }
}

/** Validate storage/page-script results without accepting arbitrary strings
 * that could turn the sample into a second browsing-history buffer. */
export function isUrlShape(value: unknown): value is string {
  if (typeof value !== "string" || value.length > 2_048 || !value.startsWith("/")) return false;
  const literal = (part: string): boolean => part.length > 0 && part.length <= 20 &&
    /^[a-z_-]+$/.test(part) && /[a-z]/.test(part) && (part.match(/[-_]/g)?.length ?? 0) <= 1;
  const placeholder = (part: string): boolean => /^(\{(?:num|key|uuid|hex32|hex40|hex64|tailhex32|id|slug)\})$/.test(part);
  const [path, query, extra] = value.split("?");
  if (extra !== undefined) return false;
  const parts = path!.split("/").slice(1);
  if (parts.length > 32 || !parts.every((part) => part === "" || literal(part) || placeholder(part))) return false;
  if (query === undefined) return true;
  const parameters = query.split("&");
  return parameters.length <= 8 && parameters.every((parameter) => {
    const [key, val, rest] = parameter.split("=");
    return rest === undefined && (literal(key!) || key === "{param}") && placeholder(val!);
  });
}

export function readPatternResult(value: unknown): SitePatterns {
  if (typeof value !== "object" || value === null) throw new Error("Site pattern review was unavailable. Try again on this page.");
  const result = value as Partial<SitePatterns>;
  if (!isUrlShape(result.current) || !Array.isArray(result.links) || result.links.length > 50 ||
      !result.links.every((row) => row && isUrlShape(row.shape) && Number.isInteger(row.count) && row.count > 0 && row.count <= 500)) {
    throw new Error("The page changed or its patterns were unavailable. Review the current page again.");
  }
  return { current: result.current, links: result.links.map(({ shape, count }) => ({ shape, count })) };
}

export type PatternSamples = Record<string, SampledShape[]>;

export function prunePatternSamples(value: unknown, origins: readonly string[], now: number): PatternSamples {
  const result: PatternSamples = {};
  if (typeof value !== "object" || value === null || Array.isArray(value)) return result;
  const stored = value as Record<string, unknown>;
  for (const origin of [...new Set(origins)].slice(0, 64)) {
    if (httpOrigin(origin) !== origin) continue;
    const rows = stored[origin];
    if (!Array.isArray(rows)) continue;
    const seen = new Set<string>();
    const valid: SampledShape[] = [];
    // Storage is untrusted too. Do not traverse an unbounded injected list.
    for (const row of rows.slice(0, MAX_SITE_SHAPES)) {
      if (typeof row !== "object" || row === null || !isUrlShape(row.shape) || seen.has(row.shape) ||
          !Number.isInteger(row.count) || row.count < 1 || row.count > MAX_SHAPE_COUNT ||
          !Number.isSafeInteger(row.last_seen) || row.last_seen > now || row.last_seen <= now - SAMPLE_TTL_MS) continue;
      seen.add(row.shape);
      valid.push({ shape: row.shape, count: row.count, last_seen: row.last_seen });
    }
    if (valid.length) result[origin] = valid;
  }
  return result;
}

export function samplePattern(samples: PatternSamples, origin: string, shape: string, now: number): void {
  if (!isUrlShape(shape)) return;
  const rows = samples[origin] ?? [];
  const existing = rows.find((row) => row.shape === shape);
  if (existing) {
    existing.count = Math.min(MAX_SHAPE_COUNT, existing.count + 1);
    existing.last_seen = now;
  } else {
    rows.push({ shape, count: 1, last_seen: now });
  }
  rows.sort((a, b) => b.last_seen - a.last_seen || a.shape.localeCompare(b.shape));
  samples[origin] = rows.slice(0, MAX_SITE_SHAPES);
}
