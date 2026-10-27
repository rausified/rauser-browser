/** The page the panel's artifact card describes: the active tab's address
 * and title, as `get_artifact` is asked about them. */
export interface ArtifactPage {
  url: string;
  title: string;
}

export function samePage(a: ArtifactPage | null, b: ArtifactPage | null): boolean {
  return a !== null && b !== null && a.url === b.url && a.title === b.title;
}

/**
 * Whether a `tabs.onUpdated` event can change what the panel shows: only a
 * title, address, or load completion of the tab the panel follows. Other
 * tabs' events (a background tab's unread counter, another page finishing
 * loading) are ignored. Before the panel knows its tab, an active tab may be
 * the one it shows.
 */
export function followsTabUpdate(
  tabId: number,
  changeInfo: { title?: string; url?: string; status?: string },
  tab: { active?: boolean },
  followedTabId: number | null,
): boolean {
  if (followedTabId === null ? tab.active !== true : tabId !== followedTabId) return false;
  return changeInfo.title !== undefined || changeInfo.url !== undefined ||
    changeInfo.status === "complete";
}

/** Whether `tabs.onReplaced` swapped out the tab the panel follows. Chrome
 * can replace the active tab (prerender, Instant) without `onActivated`, and
 * {@link followsTabUpdate} would then ignore the replacement's updates. */
export function followsTabReplacement(
  _addedTabId: number,
  removedTabId: number,
  followedTabId: number | null,
): boolean {
  return followedTabId === null || removedTabId === followedTabId;
}

/** Whether the worker delivered capture records between two status polls:
 * its delivery generation moved. Delivery can change the shown artifact's
 * totals, which {@link ArtifactRefresher} would otherwise not ask about
 * again. Queue length cannot tell: it can be empty before and after. */
export function deliveredSince(previous: number | null, generation: number): boolean {
  return previous !== null && generation !== previous;
}

/**
 * Keeps the artifact card's `get_artifact` requests in step with the page
 * without asking the host more than it must: each request reads the index
 * under the host's exclusive configuration lock, which capture delivery and
 * note saves also wait for. A page whose answer is shown or on its way is
 * not asked again unless the caller forces it (its history changed). Bursts
 * coalesce: at most one request is in flight, followed by at most one more
 * for whichever page is current when it finishes. An answer is shown only
 * while its page, compared by address and title rather than object
 * identity, is still the current one.
 */
export class ArtifactRefresher<R> {
  private asked: ArtifactPage | null = null;
  private wanted = false;
  private looping = false;
  private running: Promise<void> = Promise.resolve();

  constructor(
    private readonly fetch: (page: ArtifactPage) => Promise<R>,
    private readonly current: () => ArtifactPage | null,
    private readonly render: (page: ArtifactPage, result: R) => void,
    private readonly fail: (page: ArtifactPage, error: unknown) => void,
  ) {}

  /** Refresh the card for the current page. The promise settles once an
   * answer at least as new as this call has been handled. */
  refresh(force = false): Promise<void> {
    const page = this.current();
    if (!page || (!force && samePage(this.asked, page))) return this.idle();
    this.wanted = true;
    if (!this.looping) this.running = this.loop();
    return this.running;
  }

  /** The card no longer shows a page; the next refresh asks again. */
  clear(): void {
    this.asked = null;
    this.wanted = false;
  }

  private idle(): Promise<void> {
    return this.looping ? this.running : Promise.resolve();
  }

  private async loop(): Promise<void> {
    this.looping = true;
    try {
      while (this.wanted) {
        this.wanted = false;
        const page = this.current();
        if (!page) {
          this.asked = null;
          return;
        }
        this.asked = page;
        try {
          const result = await this.fetch(page);
          if (samePage(page, this.current())) this.render(page, result);
        } catch (error) {
          // Ask again next time rather than keep a page with no answer.
          if (this.asked === page) this.asked = null;
          if (samePage(page, this.current())) this.fail(page, error);
        }
      }
    } finally {
      this.looping = false;
    }
  }
}
