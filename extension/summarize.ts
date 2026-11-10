// Summarize & log: reads a tab's main text and starts its summary run. The
// panel button, the `summarize` shortcut, and the context menu all use
// summarizeTab; the host and the service check every field again.
import type { AgentStatus, AutoSummarySkipReason, RunView } from "../protocol/ts/generated.js";
import { canonicalCapture } from "./adapter-templates.js";
import { serviceSetupText } from "./agent.js";
import { APP_NAME, BINARY_NAME } from "./brand.js";
import { EXTRACT_LIMITS, extractReadablePage, wellFormed, type ExtractedPage } from "./extract.js";
import { localTimestamp } from "./model.js";
import { HostError, PROTOCOL_VERSION, newRequestId, type HostClient } from "./native.js";
import { openTabsFor, type OpenTabsApi } from "./open-tabs.js";
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
  /** Where the other tabs in the summarized tab's window come from
   * (open-tabs.ts); without it a summary is sent with none. */
  tabs?: OpenTabsApi;
  /** Overrides ALL_FRAMES_TIMEOUT_MS (tests). */
  allFramesTimeoutMs?: number;
  /** Override WORD_WAIT_MS and WORD_POLL_MS (tests). */
  wordWaitMs?: number;
  wordPollMs?: number;
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

/** Whether `url` is a Miro board, as the host's `service::miro::miro_board`
 * recognises it (the shared vectors in host/tests/fixtures/miro-board-urls.json):
 * `https://miro.com/app/board/<token(12)>` with an optional trailing slash,
 * a `%3D` read as `=`. Like an Office document, it only lets a board with no
 * text start a run; the host decides what the run is told. */
export function isMiroBoardUrl(url: string): boolean {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }
  if (parsed.protocol !== "https:" || parsed.hostname !== "miro.com" || parsed.port !== "" ||
      parsed.username !== "" || parsed.password !== "") return false;
  const segments = parsed.pathname.split("/").slice(1);
  if (segments.at(-1) === "") segments.pop();
  if (segments.length !== 3 || segments[0] !== "app" || segments[1] !== "board") return false;
  return canonicalCapture("token(12)", segments[2]!.replaceAll(/%3d/gi, "=")) !== null;
}

/** The kind of Office document an address names. */
export type OfficeDocumentKind = "word" | "excel" | "powerpoint" | "unknown";

/** The kind of Office for the web document `url` is, or null when it is not
 * one, as the host's `service::office::office_document` recognises it (the
 * shared vectors in host/tests/fixtures/office-document-urls.json): an https
 * `*.sharepoint.com` address with a `:w:`, `:x:` or `:p:` path segment (which
 * names the kind), or a `/_layouts/15/Doc.aspx` or `WopiFrame.aspx` with a
 * non-blank `sourcedoc` or `file` (whose extension names it); or an https
 * `*.officeapps.live.com` viewer whose `src` is an https URL (kind from
 * `file`, else `src`'s last path segment). The host decides on its own what
 * the run's page.md and prompt say. */
export function officeDocumentKind(url: string): OfficeDocumentKind | null {
  let page: URL;
  try {
    page = new URL(url);
  } catch {
    return null;
  }
  if (page.protocol !== "https:") return null;
  // The first value of a key (ASCII case-insensitive), when not blank.
  const param = (name: string): string | null => {
    for (const [key, value] of page.searchParams) {
      if (key.toLowerCase() === name) return value.trim() === "" ? null : value;
    }
    return null;
  };
  const kindOfName = (name: string): OfficeDocumentKind => {
    const dot = name.lastIndexOf(".");
    const extension = dot < 0 ? "" : name.slice(dot + 1).trim().toLowerCase();
    if (["docx", "docm", "doc"].includes(extension)) return "word";
    if (["xlsx", "xlsm", "xlsb", "xls"].includes(extension)) return "excel";
    if (["pptx", "pptm", "ppt"].includes(extension)) return "powerpoint";
    return "unknown";
  };
  const host = page.hostname.toLowerCase();
  const file = param("file");
  if (host.endsWith(".officeapps.live.com")) {
    const src = param("src");
    if (src === null) return null;
    let source: URL;
    try {
      source = new URL(src);
    } catch {
      return null;
    }
    if (source.protocol !== "https:") return null;
    const named = file === null ? "unknown" : kindOfName(file);
    if (named !== "unknown") return named;
    return kindOfName(source.pathname.split("/").at(-1) ?? "");
  }
  if (!host.endsWith(".sharepoint.com")) return null;
  const shareLetters: Record<string, OfficeDocumentKind> = { w: "word", x: "excel", p: "powerpoint" };
  let share: OfficeDocumentKind | null = null;
  for (const segment of page.pathname.split("/")) {
    const match = /^(?::([wxp]):|%3a([wxp])%3a)$/i.exec(segment);
    if (match) {
      share = shareLetters[(match[1] ?? match[2]!).toLowerCase()]!;
      break;
    }
  }
  const path = page.pathname.toLowerCase();
  const docPage = (path.endsWith("/_layouts/15/doc.aspx") || path.endsWith("/_layouts/15/wopiframe.aspx"))
    && (param("sourcedoc") !== null || file !== null);
  if (share === null && !docPage) return null;
  return share ?? (file === null ? "unknown" : kindOfName(file));
}

/** Whether `url` is an Office for the web document address (officeDocumentKind
 * is not null). It only lets an Office page with no text start a run. */
export function isOfficeDocumentUrl(url: string): boolean {
  return officeDocumentKind(url) !== null;
}

// A top frame with at least this much trimmed text is the page's content,
// whatever its subframes hold (ads, embeds, comment widgets).
const TOP_FRAME_ENOUGH_CHARS = 200;
// A top frame Readability found no article in (suite bars, footers,
// noscript text) yields to a subframe holding this many times its text.
const CHROME_TOP_FRAME_RATIO = 5;
// How long the all-frames extractor may take before a slow or hung subframe
// is given up on and the top frame alone is read, as before 2026-10-28.
export const ALL_FRAMES_TIMEOUT_MS = 10_000;
// How long a Word document whose frames hold no rendered paragraphs yet (its
// "Word is starting." splash) is read again before it is sent with empty
// content, and how often it is read meanwhile.
export const WORD_WAIT_MS = 5_000;
export const WORD_POLL_MS = 500;

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
 * editors render the document in a cross-origin iframe. A top frame with
 * no article (`readable === false`) also yields to a subframe holding at
 * least CHROME_TOP_FRAME_RATIO times its text, so page chrome such as
 * SharePoint's suite bar never stands in for the document. A frame that
 * read Word paragraphs (`word`) is the document whatever the others hold;
 * `word` says whether the chosen frame is one. Missing,
 * malformed, or failed frame results are skipped. The chosen content may be
 * blank; readTab decides whether that page is unreadable. */
function choosePage(
  results: readonly { frameId: number; result?: unknown }[],
): { top: ExtractedPage; content: string; selection: string | null; truncated: boolean; word: boolean } | undefined {
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
  let longestSub: ExtractedPage | undefined;
  for (const page of pages) {
    if (page !== top && (!longestSub || length(page) > length(longestSub))) longestSub = page;
  }
  let chosen = top;
  if (longestSub && length(top) < TOP_FRAME_ENOUGH_CHARS) {
    if (length(longestSub) > length(top)) chosen = longestSub;
  } else if (longestSub && top.readable === false
    && length(longestSub) >= CHROME_TOP_FRAME_RATIO * length(top)) {
    chosen = longestSub;
  }
  const wordPage = pages.find((page) => page.word === true);
  if (wordPage) chosen = wordPage;
  // The chosen frame's own selection fits its content by construction; one
  // borrowed from another frame travels only if both still fit the cap.
  let selection = chosen.selection;
  let truncated = chosen.truncated === true;
  if (selection === null) {
    const borrowedPage = pages.find((page) => page.selection !== null);
    const borrowed = borrowedPage?.selection ?? null;
    if (borrowed !== null
      && encodedBytes(chosen.content) + encodedBytes(borrowed) <= EXTRACT_LIMITS.maxRequestBytes) {
      selection = borrowed;
      truncated ||= borrowedPage?.truncated === true;
    }
  }
  return { top, content: chosen.content, selection, truncated, word: chosen.word === true };
}

/** The page a run sends: what readTab read from a tab. */
export interface TabPage {
  url: string;
  title: string;
  content: string;
  truncated: boolean;
  selection: string | null;
}

/** Extracts every frame of `tab` with the bundled Readability and returns
 * the frame content choosePage picks, every string well-formed. A page whose
 * top frame cannot be read, or no frame of which yields text, throws
 * `HostError("unreadable")`, except that an Office document address
 * (isOfficeDocumentUrl) or a Miro board (isMiroBoardUrl) with no text is read
 * with empty content: the host names the document or board in page.md and
 * the agent reads it itself; a top frame that by then holds another address
 * (a navigation, or an SPA's pushState) throws `HostError("page_changed")`.
 * A Word document address none of whose frames has rendered its paragraphs
 * yet (Word's "Word is starting." splash) is read again every WORD_POLL_MS
 * until one has, for at most WORD_WAIT_MS; past that it is read with empty
 * content, never the splash. Other pages are read once.
 * It sends nothing to the host: Summarize & log and the task bar both read
 * pages through it. */
export async function readTab(
  tab: { id: number; url: string; title: string },
  deps: Pick<SummarizeDeps, "scripting" | "allFramesTimeoutMs" | "wordWaitMs" | "wordPollMs">,
): Promise<TabPage> {
  const target = { tabId: tab.id, allFrames: true };
  const timeoutMs = deps.allFramesTimeoutMs ?? ALL_FRAMES_TIMEOUT_MS;
  // Races `work` against `ms`, which then wins.
  const within = async <T>(work: Promise<T>, ms: number): Promise<T | "timeout"> => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timedOut = new Promise<"timeout">((resolve) => {
      timer = setTimeout(() => resolve("timeout"), ms);
    });
    return Promise.race([work, timedOut]).finally(() => clearTimeout(timer));
  };
  try {
    // A hung subframe holds the injection too; past the timeout the
    // extractor runs without waiting for it.
    await within(deps.scripting.executeScript({ target, files: ["vendor/readability.js"] }), timeoutMs);
  } catch {
    // The extractor falls back to body text where Readability did not load.
  }
  type FrameResults = { frameId: number; result?: unknown }[];
  const extract = (frames: typeof target | { tabId: number; frameIds: number[] }): Promise<FrameResults> =>
    deps.scripting.executeScript({ target: frames, func: extractReadablePage, args: [EXTRACT_LIMITS] });
  const read = async (budgetMs: number): Promise<ReturnType<typeof choosePage>> => {
    let results: FrameResults;
    try {
      // Chrome rejects when the main frame fails its up-front permission check
      // (the Web Store, other extensions' pages) or when exactly one frame was
      // targeted; otherwise it drops every frame that failed, the top frame
      // included, from the results and resolves. A missing top frame result
      // (say, a navigation mid-injection) reads as unreadable below.
      const all = await within(extract(target), budgetMs);
      // A slow or hung subframe holds the whole call, so the top frame is read
      // alone, as before every frame was.
      results = all === "timeout" ? await extract({ tabId: tab.id, frameIds: [0] }) : all;
    } catch {
      results = [];
    }
    return choosePage(Array.isArray(results) ? results : []);
  };
  let page = await read(timeoutMs);
  if (!page) throw new HostError("unreadable", UNREADABLE_MESSAGE);
  let content = page.content;
  let truncated = page.truncated;
  if (officeDocumentKind(tab.url) === "word" && !page.word) {
    // Word shows a splash before it renders the document; whatever a frame
    // reads meanwhile is not the document.
    const waitMs = deps.wordWaitMs ?? WORD_WAIT_MS;
    const pollMs = deps.wordPollMs ?? WORD_POLL_MS;
    const deadline = Date.now() + waitMs;
    while (!page.word && sameAddress(tab.url, page.top.href) && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, Math.min(pollMs, Math.max(0, deadline - Date.now()))));
      // A poll never outlasts the wait; a slow one reads the top frame alone.
      // A poll that lost the top frame (an injection racing Word's own
      // navigation) keeps the last read; a real navigation shows next time.
      const next = await read(Math.max(1, Math.min(timeoutMs, deadline - Date.now())));
      if (next) page = next;
    }
    content = page.word ? page.content : "";
    // Splash text is dropped whole, so nothing of it was cut.
    truncated = page.word ? page.truncated : false;
  }
  if (content.trim().length === 0 && !isOfficeDocumentUrl(tab.url) && !isMiroBoardUrl(tab.url)) {
    throw new HostError("unreadable", UNREADABLE_MESSAGE);
  }
  // The run is keyed by `tab.url`, so its content must come from that page;
  // subframe addresses differ from it by design.
  if (!sameAddress(tab.url, page.top.href)) throw new HostError("page_changed", PAGE_CHANGED_MESSAGE);
  return {
    // Every string is made well-formed again, since the tab title comes from
    // Chrome and a stale or older extractor may not have done it.
    url: wellFormed(tab.url),
    title: wellFormed(cutTitle(page.top.title || tab.title)),
    content: wellFormed(content),
    truncated,
    selection: page.selection === null ? null : wellFormed(page.selection),
  };
}

/** Reads `tab` (readTab, which throws `unreadable` or `page_changed` before
 * anything is sent), then starts or joins its summary run. */
export async function summarizeTab(
  tab: { id: number; url: string; title: string },
  deps: SummarizeDeps,
): Promise<{ run: RunView; existing: boolean }> {
  const page = await readTab(tab, deps);
  const openTabs = await openTabsFor(tab.id, deps.tabs);
  const response = await deps.host.call({
    type: "start_run",
    protocol_version: PROTOCOL_VERSION,
    request_id: newRequestId(),
    url: page.url,
    title: page.title,
    content: page.content,
    ...(page.truncated ? { capture_truncated: true } : {}),
    selection: page.selection,
    harness: null,
    record_key: null,
    ...(openTabs.length > 0 ? { open_tabs: openTabs } : {}),
    requested_at: localTimestamp(deps.now()),
  }, "run_started");
  await markRunsMayExist(deps.storage, deps.now().getTime());
  return { run: response.run, existing: response.existing };
}

/** What an automatic summary attempt came to: a run started (or joined), the
 * host's reason for skipping it, or a page that could not be read. */
export type AutoSummaryOutcome = "started" | AutoSummarySkipReason | "unreadable" | "page_changed";

/** Reads `tab` (readTab) and asks the host to summarize it automatically
 * (start_auto_summary). A failure to reach the host or the service throws, so
 * the caller keeps the tab for a later try. */
export async function autoSummarizeTab(
  tab: { id: number; url: string; title: string },
  deps: Omit<SummarizeDeps, "host"> & { host: Pick<HostClient, "callAny"> },
): Promise<AutoSummaryOutcome> {
  let page: TabPage;
  try {
    page = await readTab(tab, deps);
  } catch (error) {
    if (error instanceof HostError && (error.code === "unreadable" || error.code === "page_changed")) {
      return error.code;
    }
    throw error;
  }
  const openTabs = await openTabsFor(tab.id, deps.tabs);
  const response = await deps.host.callAny({
    type: "start_auto_summary",
    protocol_version: PROTOCOL_VERSION,
    request_id: newRequestId(),
    url: page.url,
    title: page.title,
    content: page.content,
    ...(page.truncated ? { capture_truncated: true } : {}),
    ...(openTabs.length > 0 ? { open_tabs: openTabs } : {}),
    requested_at: localTimestamp(deps.now()),
  }, ["run_started", "auto_summary_skipped"]);
  if (response.type === "auto_summary_skipped") return response.reason;
  await markRunsMayExist(deps.storage, deps.now().getTime());
  return "started";
}
