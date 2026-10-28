// Summarize & log: reads a tab's main text and starts its summary run. The
// panel button, the `summarize` shortcut, and the context menu all use
// summarizeTab; the host and the service check every field again.
import type { AgentStatus, RunView } from "../protocol/ts/generated.js";
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
/** Why Summarize & log, and Capture Site's Improve with AI, are unavailable:
 * both run the harness set up in settings. */
export const NEEDS_HARNESS_MESSAGE = "Set up a summary harness in settings first.";
// The start_run schema's title bound.
const MAX_TITLE_CHARS = 2048;

export interface SummarizeDeps {
  scripting: typeof chrome.scripting;
  host: Pick<HostClient, "call">;
  /** Where an accepted run is recorded, so a later panel restores it. */
  storage: Pick<ChromeStorageArea, "set">;
  now(): Date;
}

/** Whether settings has a harness that answered its probe, so a run can
 * start. Summarize & log and Improve with AI both wait for it. */
export function harnessReady(session: { agentStatus: AgentStatus | null }): boolean {
  return session.agentStatus?.state === "ready";
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

/** Extracts `tab`'s top frame with the bundled Readability, then starts (or
 * joins) its summary run. A page that cannot be read throws
 * `HostError("unreadable")`, and a tab that by then holds another address
 * (a navigation, or an SPA's pushState) throws `HostError("page_changed")`;
 * neither sends anything. */
export async function summarizeTab(
  tab: { id: number; url: string; title: string },
  deps: SummarizeDeps,
): Promise<{ run: RunView; existing: boolean }> {
  const target = { tabId: tab.id, frameIds: [0] };
  let page: ExtractedPage | undefined;
  try {
    await deps.scripting.executeScript({ target, files: ["vendor/readability.js"] });
    const [injection] = await deps.scripting.executeScript({
      target, func: extractReadablePage, args: [EXTRACT_LIMITS],
    });
    page = injection?.result;
  } catch {
    // Chrome refuses some pages (the Web Store, other extensions' pages), and
    // a page can navigate away mid-injection.
    page = undefined;
  }
  if (!page || typeof page.content !== "string" || page.content === "") {
    throw new HostError("unreadable", UNREADABLE_MESSAGE);
  }
  // The run is keyed by `tab.url`, so its content must come from that page.
  if (!sameAddress(tab.url, page.href)) throw new HostError("page_changed", PAGE_CHANGED_MESSAGE);
  const response = await deps.host.call({
    type: "start_run",
    protocol_version: PROTOCOL_VERSION,
    request_id: newRequestId(),
    // Every string is made well-formed again, since the tab title comes from
    // Chrome and a stale or older extractor may not have done it.
    url: wellFormed(tab.url),
    title: wellFormed(cutTitle(page.title || tab.title)),
    content: wellFormed(page.content),
    selection: page.selection === null ? null : wellFormed(page.selection),
    harness: null,
    record_key: null,
    requested_at: localTimestamp(deps.now()),
  }, "run_started");
  await markRunsMayExist(deps.storage, deps.now().getTime());
  return { run: response.run, existing: response.existing };
}
