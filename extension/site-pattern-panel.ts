import { describe, element, worker } from "./settings.js";
import { httpOrigin, inspectSitePatterns, readPatternResult, type SampledShape } from "./site-patterns.js";

interface PatternPage { url: string; tabId: number | undefined }

/** A click-only inspection. No grant, draft, config write or storage handoff
 * is created here. Navigating, canceling or closing drops all link shapes. */
export class SitePatternPanel {
  private readonly area = element<HTMLDetailsElement>("site-pattern-area");
  private readonly start = element<HTMLButtonElement>("site-pattern-review");
  private readonly review = element<HTMLElement>("site-pattern-result");
  private readonly message = element<HTMLElement>("site-pattern-message");
  private readonly rows = element<HTMLElement>("site-pattern-rows");
  private readonly close = element<HTMLButtonElement>("site-pattern-close");
  private page: PatternPage | null = null;
  private target: PatternPage | null = null;

  constructor() {
    this.start.addEventListener("click", () => void this.inspect());
    this.area.addEventListener("toggle", () => { if (!this.area.open) this.cancel(); });
    this.close.addEventListener("click", () => {
      this.cancel();
      this.start.focus();
    });
    // A reload or document replacement keeps the tab ID and can keep the URL,
    // so the URL alone cannot tell the review that its links are stale. Any
    // new load or URL change in the reviewed tab discards the review,
    // including one still in flight (an A-to-B-to-A navigation).
    chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
      if (this.target && tabId === this.target.tabId &&
        (changeInfo.status === "loading" || changeInfo.url !== undefined)) this.cancel();
    });
  }

  render(page: PatternPage | null, busy: boolean): void {
    this.page = page;
    this.area.hidden = !page || !httpOrigin(page.url);
    this.start.disabled = busy || page?.tabId === undefined;
    if (this.target && !this.samePage(page, this.target)) this.cancel();
  }

  private samePage(a: PatternPage | null, b: PatternPage): boolean {
    return a?.url === b.url && a.tabId === b.tabId;
  }

  private cancel(): void {
    this.target = null;
    this.rows.replaceChildren();
    this.review.hidden = true;
    this.start.hidden = false;
  }

  private async inspect(): Promise<void> {
    if (!this.page || this.page.tabId === undefined || this.start.disabled || this.start.hidden) return;
    const target = { ...this.page };
    this.target = target;
    this.review.hidden = false;
    this.start.hidden = true;
    this.message.textContent = "Reviewing page patterns…";
    this.rows.replaceChildren();
    try {
      // Recheck incognito and the active tab before reading any page hrefs.
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tab || tab.incognito !== false || tab.id !== target.tabId || tab.url !== target.url || this.target !== target) {
        throw new Error("The active page changed. Review the current page again.");
      }
      const results = await chrome.scripting.executeScript({
        target: { tabId: target.tabId!, frameIds: [0] }, world: "ISOLATED",
        func: inspectSitePatterns, args: [target.url, true],
      });
      if (this.target !== target) return;
      const patterns = readPatternResult(results.find((result) => result.frameId === 0)?.result);
      // Stored samples are optional here; a failure to read them must not
      // discard the page's own shapes.
      let samples: SampledShape[] = [];
      let samplesError: unknown = null;
      try {
        samples = await worker<SampledShape[]>({ kind: "get_site_patterns", origin: httpOrigin(target.url)! });
      } catch (error) {
        samplesError = error;
      }
      const [current] = await chrome.tabs.query({ active: true, currentWindow: true });
      if (this.target !== target) return;
      if (!this.samePage(this.page, target) || current?.incognito !== false || current.id !== target.tabId || current.url !== target.url) {
        this.cancel();
        return;
      }
      this.message.textContent = "Review only — capture rules stay as they are. Numbers and identifiers are replaced by placeholders; short lowercase route words remain. Links are read only on this click and forgotten when you close this review.";
      const add = (text: string): void => {
        const row = document.createElement("li");
        row.textContent = text;
        this.rows.append(row);
      };
      add(`Current page: ${patterns.current}`);
      if (samplesError !== null) add(`Visited patterns unavailable: ${describe(samplesError)}`);
      for (const row of samples) add(`Visited pattern (${row.count} navigation${row.count === 1 ? "" : "s"}): ${row.shape}`);
      for (const row of patterns.links) add(`Link pattern (${row.count} link${row.count === 1 ? "" : "s"}; not evidence of a visit): ${row.shape}`);
      if (!patterns.links.length) add("No same-origin link patterns found.");
    } catch (error) {
      if (this.target === target) this.message.textContent = `Could not review this page: ${describe(error)}`;
    }
  }
}
