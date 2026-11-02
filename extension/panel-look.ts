// Presentation helpers for the side panel's visual design (DESIGN.md §5.7):
// the short glyph and hue that stand for an artifact type or site, the label
// for a type, the summary's time label and the header capture pill. Pure
// functions only, so the panel and its tests share one source.
import type { ImportanceLevel } from "../protocol/ts/generated.js";

/** One of a small fixed palette; `panel.css` styles `.hue-<name>`. */
export type Hue = "cyan" | "indigo" | "amber" | "violet" | "green" | "neutral";

export interface Look {
  /** One to two characters, shown in a small tile. */
  glyph: string;
  hue: Hue;
  /** A short human label for the type, such as "Pull request". */
  label: string;
}

const KNOWN: Readonly<Record<string, Look>> = {
  "github.pr": { glyph: "PR", hue: "cyan", label: "Pull request" },
  "github.issue": { glyph: "IS", hue: "green", label: "Issue" },
  "github.repo": { glyph: "GH", hue: "neutral", label: "Repository" },
  "jira.issue": { glyph: "JI", hue: "indigo", label: "Jira issue" },
  "jira.filter": { glyph: "JF", hue: "indigo", label: "Jira filter" },
  "confluence.page": { glyph: "C", hue: "indigo", label: "Confluence page" },
  "miro.board": { glyph: "MB", hue: "amber", label: "Miro board" },
  "sharepoint.doc": { glyph: "DO", hue: "violet", label: "Document" },
  "web.page": { glyph: "W", hue: "neutral", label: "Web page" },
};

const HUES: readonly Hue[] = ["cyan", "indigo", "amber", "violet", "green"];

function hashHue(text: string): Hue {
  let hash = 0;
  for (const char of text) hash = (hash * 31 + (char.codePointAt(0) ?? 0)) >>> 0;
  return HUES[hash % HUES.length] ?? "neutral";
}

/** Title-cases an adapter type's last part: "acme.design_doc" → "Design doc". */
function typeLabel(type: string): string {
  const last = type.split(".").pop() ?? type;
  const words = last.replace(/[_-]+/g, " ").trim();
  if (!words) return type;
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** The look for an adapter's artifact type; unknown types get a stable hue. */
export function artifactLook(type: string): Look {
  const known = KNOWN[type];
  if (known) return known;
  const site = type.split(".")[0] ?? "";
  const letters = site.replace(/[^a-z0-9]/gi, "");
  return {
    glyph: (letters.slice(0, 2) || "?").toUpperCase(),
    hue: hashHue(type),
    label: typeLabel(type),
  };
}

/** The look for a page no adapter recognizes, from its URL's host. */
export function pageLook(url: string | null): Look {
  let host: string;
  try {
    host = url ? new URL(url).hostname : "";
  } catch {
    host = "";
  }
  host = host.replace(/^www\./, "");
  const first = host.match(/[a-z0-9]/i)?.[0];
  return { glyph: first ? first.toUpperCase() : "·", hue: "neutral", label: "Web page" };
}

/** A URL shown without its scheme, the way the design shows addresses. */
export function displayUrl(url: string): string {
  return url.replace(/^https?:\/\//i, "");
}

/** The tone a summary's importance chip takes. */
export function importanceTone(importance: ImportanceLevel): "high" | "plain" {
  return importance === "high" ? "high" : "plain";
}

/** "High importance", from the protocol's lower-case level. */
export function importanceLabel(importance: ImportanceLevel): string {
  return `${importance.charAt(0).toUpperCase()}${importance.slice(1)} importance`;
}

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

function pad(value: number): string {
  return String(value).padStart(2, "0");
}

/** When a summary was made, in local time: "09:14" today, "Mon 09:14" this
 * week, "2026-10-21" earlier. An unreadable timestamp is shown as written. */
export function summaryTimeLabel(summarizedAt: string, now: Date = new Date()): string {
  const at = new Date(summarizedAt);
  if (Number.isNaN(at.getTime())) return summarizedAt;
  const time = `${pad(at.getHours())}:${pad(at.getMinutes())}`;
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (at.getTime() >= startOfToday && at.getTime() <= now.getTime() + 60_000) return time;
  const sixDaysAgo = startOfToday - 6 * 86_400_000;
  if (at.getTime() >= sixDaysAgo && at.getTime() < startOfToday) return `${WEEKDAYS[at.getDay()]} ${time}`;
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`;
}

/** Compact focused time for a Recent row: "1h 12m", "48m", "<1m". */
export function compactDuration(seconds: number): string {
  if (seconds < 60) return seconds > 0 ? "<1m" : "0m";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest ? `${hours}h ${rest}m` : `${hours}h`;
}

export type PillTone = "on" | "off" | "pending";

export interface CapturePill {
  tone: PillTone;
  label: string;
  /** The longer explanation for the pill's tooltip and accessible name. */
  detail: string;
}

export interface CapturePillInput {
  /** Capture is switched on in a healthy host configuration. */
  captureOn: boolean;
  /** Pending capture records in the worker's buffer. */
  queued: number;
  /** Delivery reported a problem (retry, host or last error). */
  deliveryProblem: boolean;
}

/** The header pill: whether capture is on, and a pending backlog when
 * delivery is stuck. Whether this page is recognized is the page card's
 * artifact status, not the pill's. */
export function capturePill(input: CapturePillInput): CapturePill {
  if (input.queued > 0 && input.deliveryProblem) {
    const records = `${input.queued} pending`;
    return { tone: "pending", label: records, detail: `${records} capture record${input.queued === 1 ? "" : "s"} waiting to be sent` };
  }
  if (!input.captureOn) return { tone: "off", label: "Capture off", detail: "Capture is off" };
  return { tone: "on", label: "Capture on", detail: "Capturing on sites with adapters" };
}
