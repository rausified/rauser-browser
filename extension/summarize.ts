// Summarize & log: reads a tab's main text and starts its summary run. The
// panel button, the `summarize` shortcut, and the context menu all use
// summarizeTab; the host and the service check every field again.
import type { RunView } from "../protocol/ts/generated.js";
import { APP_NAME, BINARY_NAME } from "./brand.js";
import { EXTRACT_LIMITS, extractReadablePage, type ExtractedPage } from "./extract.js";
import { localTimestamp } from "./model.js";
import { HostError, PROTOCOL_VERSION, newRequestId, type HostClient } from "./native.js";

export const SUMMARIZE_MENU_ID = `${BINARY_NAME}-summarize`;
export const SUMMARIZE_MENU_TITLE = `Summarize with ${APP_NAME}`;
export const UNREADABLE_MESSAGE = `${APP_NAME} can't read this page's content.`;
export const NOT_A_WEB_PAGE_MESSAGE = "Summaries work on http(s) pages.";
// The start_run schema's title bound.
const MAX_TITLE_CHARS = 2048;

export interface SummarizeDeps {
  scripting: typeof chrome.scripting;
  host: Pick<HostClient, "call">;
  now(): Date;
}

/** Extracts `tab`'s top frame with the bundled Readability, then starts (or
 * joins) its summary run. A page that cannot be read throws
 * `HostError("unreadable")` and sends nothing. */
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
  const response = await deps.host.call({
    type: "start_run",
    protocol_version: PROTOCOL_VERSION,
    request_id: newRequestId(),
    url: tab.url,
    title: (page.title || tab.title).slice(0, MAX_TITLE_CHARS),
    content: page.content,
    selection: page.selection,
    harness: null,
    record_key: null,
    requested_at: localTimestamp(deps.now()),
  }, "run_started");
  return { run: response.run, existing: response.existing };
}
