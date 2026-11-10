// Related open tabs (DESIGN.md §5.2): the other tabs in the summarized tab's
// own window, offered to the summary agent as titles and addresses only.
// Never page text: no tab is read or injected, and Chrome reports a tab's
// url and title only for the http(s) pages Brauser's host access covers.
// The host and the service check every entry again and keep only safe ones.
import type { OpenTab } from "../protocol/ts/generated.js";
import { wellFormed } from "./extract.js";
import { isHttpUrl } from "./model.js";

/** The most tabs a summary start carries (the schema's `maxItems`). */
export const MAX_OPEN_TABS = 20;
/** A longer title is cut to this many UTF-16 units (the schema's bound). */
export const MAX_OPEN_TAB_TITLE_CHARS = 300;
/** A longer address is left out, not cut: a cut address names another page. */
export const MAX_OPEN_TAB_URL_CHARS = 2048;

export type OpenTabsApi = Pick<typeof chrome.tabs, "get" | "query">;

function cut(value: string, max: number): string {
  if (value.length <= max) return value;
  const code = value.charCodeAt(max - 1);
  return value.slice(0, code >= 0xd800 && code <= 0xdbff ? max - 1 : max);
}

function webAddress(raw: string | undefined): string | null {
  if (!raw || raw.length > MAX_OPEN_TAB_URL_CHARS) return null;
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return null;
  }
  if (!isHttpUrl(url) || url.username || url.password) return null;
  return wellFormed(raw);
}

/** The other readable http(s) tabs in `tabId`'s window, most recently used
 * first, at most MAX_OPEN_TABS: never `tabId` itself or another tab showing
 * the same address, never an incognito tab (and none at all for an
 * incognito window), and never a tab Chrome gives no address for. Any
 * failure gives none, so the summary still starts. */
export async function openTabsFor(tabId: number, tabs: OpenTabsApi | undefined): Promise<OpenTab[]> {
  if (!tabs) return [];
  try {
    const self = await tabs.get(tabId);
    if (self.incognito !== false || self.windowId === undefined) return [];
    const others = await tabs.query({ windowId: self.windowId });
    const seen = new Set<string>();
    const own = webAddress(self.url);
    if (own) seen.add(own);
    const candidates = others
      .filter((tab) => tab.id !== tabId && tab.incognito === false)
      // Most recently used first; Chrome's tab order breaks ties.
      .map((tab, order) => ({ tab, order }))
      .sort((a, b) => (b.tab.lastAccessed ?? 0) - (a.tab.lastAccessed ?? 0) || a.order - b.order);
    const kept: OpenTab[] = [];
    for (const { tab } of candidates) {
      if (kept.length === MAX_OPEN_TABS) break;
      const url = webAddress(tab.url);
      if (!url || seen.has(url)) continue;
      seen.add(url);
      kept.push({ url, title: wellFormed(cut(tab.title ?? "", MAX_OPEN_TAB_TITLE_CHARS)) });
    }
    return kept;
  } catch {
    return [];
  }
}
