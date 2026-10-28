// Summarize & log: reads a tab's main text and starts its summary run. The
// panel button, the `summarize` shortcut, and the context menu all use
// summarizeTab; the host and the service check every field again.
import type { RunView } from "../protocol/ts/generated.js";
import { serviceSetupText } from "./agent.js";
import { APP_NAME, BINARY_NAME } from "./brand.js";
import { EXTRACT_LIMITS, extractReadablePage, wellFormed, type ExtractedPage } from "./extract.js";
import { localTimestamp } from "./model.js";
import { HostError, PROTOCOL_VERSION, newRequestId, type HostClient } from "./native.js";
import { markRunsMayExist } from "./run-card.js";

export const SUMMARIZE_MENU_ID = `${BINARY_NAME}-summarize`;
export const SUMMARIZE_MENU_TITLE = `Summarize with ${APP_NAME}`;
export const UNREADABLE_MESSAGE = `${APP_NAME} can't read this page's content.`;
export const NOT_A_WEB_PAGE_MESSAGE = "Summaries work on http(s) pages.";
export const SITE_ACCESS_NEEDED_MESSAGE =
  `${APP_NAME} needs site access to summarize this page — re-enable it in Chrome's extension settings for ${APP_NAME}.`;
export const PAGE_CHANGED_MESSAGE = "The page changed before it could be read. Summarize it again.";
// The start_run schema's title bound.
const MAX_TITLE_CHARS = 2048;

export interface SummarizeDeps {
  scripting: typeof chrome.scripting;
  host: Pick<HostClient, "call">;
  /** Where an accepted run is recorded, so a later panel restores it. */
  storage: Pick<ChromeStorageArea, "set">;
  now(): Date;
}

/** What a failed summarizeTab tells the user, the same on every path: the
 * panel button, the shortcut, and the context menu. */
export function summarizeFailureText(error: unknown): string {
  if (error instanceof HostError && (error.code === "unreadable" || error.code === "page_changed")) return error.message;
  if (error instanceof HostError && error.code === "service_unavailable") return serviceSetupText(error.message);
  return `Could not start the summary: ${error instanceof Error ? error.message : String(error)}`;
}

/** At most MAX_TITLE_CHARS UTF-16 units, never ending on half a pair. */
function cutTitle(title: string): string {
  if (title.length <= MAX_TITLE_CHARS) return title;
  const code = title.charCodeAt(MAX_TITLE_CHARS - 1);
  return title.slice(0, code >= 0xd800 && code <= 0xdbff ? MAX_TITLE_CHARS - 1 : MAX_TITLE_CHARS);
}

/** Whether two addresses name the same page once parsed as the URL standard
 * does (case, default port), as Chrome reports tab URLs. */
function sameAddress(a: string, b: unknown): boolean {
  if (typeof b !== "string") return false;
  try {
    return new URL(a).href === new URL(b).href;
  } catch {
    return false;
  }
}

// A top frame with at least this much trimmed text is the page's content,
// whatever its subframes hold (ads, embeds, comment widgets).
const TOP_FRAME_ENOUGH_CHARS = 200;

/** A frame's extractor result, if it is one. */
function framePage(value: unknown): ExtractedPage | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const page = value as Partial<ExtractedPage>;
  if (typeof page.content !== "string") return undefined;
  return value as ExtractedPage;
}

const encodedBytes = (value: string | null): number => new TextEncoder().encode(JSON.stringify(value)).length;

/** What every frame's extractor read: the top frame's page (its title and
 * address stand for the tab), and the content and selection to send. The
 * content is the top frame's when it has at least TOP_FRAME_ENOUGH_CHARS or
 * the most text, else the longest subframe's: Word for the web and similar
 * editors render the document in a cross-origin iframe. Missing,
 * malformed, or failed frame results are skipped. */
function choosePage(
  results: readonly { frameId: number; result?: unknown }[],
): { top: ExtractedPage; content: string; selection: string | null } | undefined {
  let top: ExtractedPage | undefined;
  const pages: ExtractedPage[] = [];
  for (const entry of results) {
    const page = framePage(entry?.result);
    if (!page) continue;
    if (entry.frameId !== 0) pages.push(page);
    else if (!top) {
      top = page;
      pages.unshift(page);
    }
  }
  if (!top) return undefined;
  const length = (page: ExtractedPage): number => page.content.trim().length;
  let chosen = top;
  if (length(top) < TOP_FRAME_ENOUGH_CHARS) {
    for (const page of pages) if (length(page) > length(chosen)) chosen = page;
  }
  if (length(chosen) === 0) return undefined;
  // The chosen frame's own selection fits its content by construction; one
  // borrowed from another frame travels only if both still fit the cap.
  let selection = chosen.selection;
  if (selection === null) {
    const borrowed = pages.find((page) => page.selection !== null)?.selection ?? null;
    if (borrowed !== null
      && encodedBytes(chosen.content) + encodedBytes(borrowed) <= EXTRACT_LIMITS.maxRequestBytes) {
      selection = borrowed;
    }
  }
  return { top, content: chosen.content, selection };
}

/** Extracts every frame of `tab` with the bundled Readability, then starts
 * (or joins) its summary run with the frame content choosePage picks. A page
 * no frame of which yields text, or whose top frame cannot be read, throws
 * `HostError("unreadable")`; a top frame that by then holds another address
 * (a navigation, or an SPA's pushState) throws `HostError("page_changed")`;
 * neither sends anything. */
export async function summarizeTab(
  tab: { id: number; url: string; title: string },
  deps: SummarizeDeps,
): Promise<{ run: RunView; existing: boolean }> {
  const target = { tabId: tab.id, allFrames: true };
  try {
    await deps.scripting.executeScript({ target, files: ["vendor/readability.js"] });
  } catch {
    // The extractor falls back to body text where Readability did not load.
  }
  let results: { frameId: number; result?: unknown }[];
  try {
    // Chrome skips a subframe it cannot inject and rejects only when the top
    // frame fails: some pages are refused (the Web Store, other extensions'
    // pages), and a page can navigate away mid-injection.
    results = await deps.scripting.executeScript({
      target, func: extractReadablePage, args: [EXTRACT_LIMITS],
    });
  } catch {
    results = [];
  }
  const page = choosePage(Array.isArray(results) ? results : []);
  if (!page) throw new HostError("unreadable", UNREADABLE_MESSAGE);
  // The run is keyed by `tab.url`, so its content must come from that page;
  // subframe addresses differ from it by design.
  if (!sameAddress(tab.url, page.top.href)) throw new HostError("page_changed", PAGE_CHANGED_MESSAGE);
  const response = await deps.host.call({
    type: "start_run",
    protocol_version: PROTOCOL_VERSION,
    request_id: newRequestId(),
    // Every string is made well-formed again, since the tab title comes from
    // Chrome and a stale or older extractor may not have done it.
    url: wellFormed(tab.url),
    title: wellFormed(cutTitle(page.top.title || tab.title)),
    content: wellFormed(page.content),
    selection: page.selection === null ? null : wellFormed(page.selection),
    harness: null,
    record_key: null,
    requested_at: localTimestamp(deps.now()),
  }, "run_started");
  await markRunsMayExist(deps.storage, deps.now().getTime());
  return { run: response.run, existing: response.existing };
}
