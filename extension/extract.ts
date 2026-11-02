// Reads a page's main text for a summary run. Chrome serializes
// extractReadablePage into the page, after injecting the bundled Readability,
// so the function is self-contained: it uses only document, window,
// TextEncoder, JSON, and the global Readability. It reports the address of
// the document it read, which the caller checks against the run's page.

export interface ExtractLimits {
  // Encoded bytes: each field is measured as UTF-8 JSON, as it travels.
  maxRequestBytes: number;
  maxSelectionBytes: number;
  // The start_run schema's character bounds, which the host also enforces.
  maxContentChars: number;
  maxSelectionChars: number;
  maxTitleChars: number;
}

export interface ExtractedPage {
  title: string;
  content: string;
  selection: string | null;
  truncated: boolean;
  readable: boolean;
  // window.location.href when the extractor ran.
  href: string;
}

// 3 MiB of encoded content and selection leaves the rest of the host's 4 MiB
// inbound frame for the envelope, whatever the script or escaping.
export const EXTRACT_LIMITS: ExtractLimits = {
  maxRequestBytes: 3 * 1024 * 1024,
  maxSelectionBytes: 64 * 1024,
  maxContentChars: 1_048_576,
  maxSelectionChars: 65_536,
  maxTitleChars: 2048,
};

// A lone surrogate: half a pair with no other half. JSON.stringify writes one
// as a \udXXXX escape, which strict parsers such as the host's serde_json
// reject, so each is replaced with U+FFFD before anything is sent.
const LONE_SURROGATE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g;

/** `value` with every lone surrogate replaced by U+FFFD. */
export function wellFormed(value: string): string {
  return value.replace(LONE_SURROGATE, "\uFFFD");
}

export function extractReadablePage(limits: ExtractLimits): ExtractedPage {
  // The same replacement as wellFormed, repeated here because Chrome runs
  // only this function's own source in the page.
  const lone = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g;
  const whole = (value: string): string => value.replace(lone, "\uFFFD");
  const encoder = new TextEncoder();
  const encoded = (value: string | null): number => encoder.encode(JSON.stringify(value)).length;
  // A cut never ends on half a surrogate pair.
  const cut = (value: string, end: number): string => {
    const code = value.charCodeAt(end - 1);
    return value.slice(0, code >= 0xd800 && code <= 0xdbff ? end - 1 : end);
  };
  // The longest prefix of at most maxChars UTF-16 units that `fits`. Each
  // candidate is cut whole, since JSON escapes a lone surrogate six-fold and
  // the measure would no longer grow with the prefix.
  const fit = (value: string, maxChars: number, fits: (prefix: string) => boolean): string => {
    const longest = cut(value, Math.min(value.length, maxChars));
    if (fits(longest)) return longest;
    let low = 0;
    let high = longest.length;
    while (low < high) {
      const mid = Math.ceil((low + high) / 2);
      if (fits(cut(longest, mid))) low = mid;
      else high = mid - 1;
    }
    return cut(longest, low);
  };

  // NOSCRIPT and TEMPLATE hold no rendered text either; Readability already
  // strips them from an article; the body fallback uses rendered innerText.
  const DROPPED = new Set(["IMG", "SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE"]);
  const BLOCKS = new Set([
    "ADDRESS", "ARTICLE", "ASIDE", "BLOCKQUOTE", "DD", "DETAILS", "DIV", "DL", "DT", "FIGCAPTION", "FIGURE",
    "FOOTER", "HEADER", "HR", "MAIN", "NAV", "OL", "P", "SECTION", "SUMMARY", "TABLE", "TBODY", "TD", "TFOOT",
    "TH", "THEAD", "TR", "UL",
  ]);
  const blocks: string[] = [];
  let current = "";
  let prefix = "";
  // Ends the current block. A prefix waits for the first block with text, so
  // an `li` holding a `p` still reads "- text".
  const flush = (): void => {
    const lines = current.split("\n").map((line) => line.replace(/ +/g, " ").trim()).filter((line) => line !== "");
    current = "";
    if (lines.length === 0) return;
    blocks.push(prefix + lines.join("\n"));
    prefix = "";
  };
  const walk = (node: Node): void => {
    if (node.nodeType === 3) {
      current += (node.nodeValue ?? "").replace(/\s+/g, " ");
      return;
    }
    if (node.nodeType !== 1) return;
    const tag = (node as Element).tagName.toUpperCase();
    if (DROPPED.has(tag)) return;
    if (tag === "BR") {
      current += "\n";
      return;
    }
    if (tag === "PRE") {
      flush();
      const pre = (node.textContent ?? "").replace(/^\n+|\s+$/g, "");
      if (pre.trim() !== "") {
        blocks.push(prefix + pre);
        prefix = "";
      }
      return;
    }
    const heading = /^H([1-6])$/.exec(tag);
    const block = heading !== null || tag === "LI" || BLOCKS.has(tag);
    if (block) {
      flush();
      if (heading !== null) prefix = "#".repeat(Number(heading[1])) + " ";
      else if (tag === "LI") prefix = "- ";
    }
    for (const child of Array.from(node.childNodes)) walk(child);
    if (block) {
      flush();
      prefix = "";
    }
  };

  // SPA navigation can update the body before document.title (and the
  // metadata Readability uses). Prefer one unambiguous main-page heading;
  // ignore hidden/navigation headings and fall back when a page has several.
  const headings = new Set<string>();
  for (const heading of document.querySelectorAll("main h1, article h1, [role='main'] h1")) {
    if (heading.closest("[hidden], [aria-hidden='true'], nav, aside")) continue;
    const text = ((heading as HTMLElement).innerText ?? heading.textContent ?? "").replace(/\s+/g, " ").trim();
    if (text !== "") headings.add(text);
  }
  const headingTitle = headings.size === 1 ? [...headings][0] : "";

  // Whether an element has layout boxes. An element that is not being
  // rendered (in a display:none iframe, or under display:none) has no client
  // rects, which is exactly when innerText returns textContent, inline
  // script and style text included. checkVisibility would also count
  // visibility and opacity, which innerText does not. An element without
  // getClientRects (not a real DOM) counts as rendered.
  const hasBoxes = (node: Element | null | undefined): boolean =>
    typeof node?.getClientRects !== "function" || node.getClientRects().length > 0;
  // A frame whose document is not rendered (Office's hidden token-factory
  // iframe) shows the user nothing, so it contributes no content or
  // selection and can never be the frame a summary reads.
  const rendered = hasBoxes(document.documentElement);

  // Word for the web renders each paragraph as a `p.Paragraph` inside a
  // `.OutlineElement` under #PagesContainer. Readability cannot read that
  // frame: on a clone with no layout it picks Fluent's screen-reader keytip
  // layer ("Control Period, fzczazmzs02h, zzh, …", whose commas outscore the
  // document), and without that layer it still keeps only part of the
  // document. So a Word frame is read paragraph by paragraph: role=heading
  // with aria-level gives a heading, an `li` outline element a list item
  // indented by its aria-level, and aria-hidden list-marker glyphs are
  // skipped. Word renders only the pages near the viewport, so a long
  // document's later pages are absent; the host's Office document section
  // tells the agent to read the document itself when the page falls short.
  const wordLines: { text: string; item: boolean }[] = [];
  if (rendered) {
    // Text the reader sees: hidden, aria-hidden, and boxless (display:none)
    // elements contribute nothing, and a `br` is a line break.
    const visibleText = (node: Node): string => {
      if (node.nodeType === 3) return node.nodeValue ?? "";
      if (node.nodeType !== 1) return "";
      const element = node as Element;
      if (element.tagName.toUpperCase() === "BR") return "\n";
      if (element.getAttribute("aria-hidden") === "true" || element.getAttribute("hidden") !== null) return "";
      if (!hasBoxes(element)) return "";
      return Array.from(node.childNodes).map(visibleText).join("");
    };
    for (const paragraph of Array.from(document.querySelectorAll("#PagesContainer .Paragraph"))) {
      const outline = paragraph.closest(".OutlineElement");
      if (!outline || paragraph.closest("[hidden], [aria-hidden='true']")) continue;
      const line = visibleText(paragraph).replace(/[\u200b\ufeff]/g, "").split("\n")
        .map((part) => part.replace(/\s+/g, " ").trim()).filter((part) => part !== "").join("\n");
      if (line === "") continue;
      const level = Number(paragraph.getAttribute("aria-level"));
      const depth = Math.max(1, Number(outline.getAttribute("aria-level")) || 1);
      if (paragraph.getAttribute("role") === "heading" && level >= 1 && level <= 6) {
        wordLines.push({ text: "#".repeat(level) + " " + line, item: false });
      } else if (outline.tagName.toUpperCase() === "LI") {
        wordLines.push({ text: "  ".repeat(depth - 1) + "- " + line, item: true });
      } else {
        wordLines.push({ text: line, item: false });
      }
    }
  }

  let title = "";
  let content = "";
  let readable = false;
  if (wordLines.length > 0) {
    // Consecutive list items stay one block, as the article walker writes them.
    content = wordLines.map((line, index) =>
      (index === 0 ? "" : line.item && wordLines[index - 1]?.item === true ? "\n" : "\n\n") + line.text).join("");
    readable = true;
  }
  try {
    if (!readable && rendered && typeof Readability === "function") {
      const clone = document.cloneNode(true) as Document;
      // Readability re-parses the clone's HTML when it retries, and Chrome
      // checks every inline handler attribute it parses against the page's
      // CSP: a strict page logs a violation per handler, charged to this
      // extension. Handlers carry no text, so the clone drops them first.
      for (const node of Array.from(clone.querySelectorAll("*"))) {
        for (const name of node.getAttributeNames()) {
          if (/^on/i.test(name)) node.removeAttribute(name);
        }
      }
      const article = new Readability(clone, { serializer: (el) => el }).parse();
      if (article !== null) {
        title = article.title ?? "";
        if (typeof article.content === "object" && article.content !== null) walk(article.content as Node);
        flush();
        content = blocks.join("\n\n");
        readable = content !== "";
      }
    }
  } catch {
    // An unparseable page reads as body text below.
  }
  if (!readable && rendered && document.body) {
    // A boxless body (display:contents or display:none) can return raw
    // textContent from innerText. The rendered root still applies the
    // browser's visibility filtering, including hidden descendants.
    const source = hasBoxes(document.body) ? document.body : document.documentElement;
    content = (source.innerText ?? "").trim();
  }
  // Replaced before measuring, so a cut never meets a lone half either.
  content = whole(content);
  const fullTitle = whole(headingTitle || title || document.title || "");
  title = cut(fullTitle, Math.min(fullTitle.length, limits.maxTitleChars));

  const picked = rendered ? whole(window.getSelection()?.toString() ?? "") : "";
  let selection: string | null = picked.trim() === "" ? null : picked;
  let truncated = false;
  if (selection !== null) {
    const kept = fit(selection, limits.maxSelectionChars, (prefix) => encoded(prefix) <= limits.maxSelectionBytes);
    truncated = kept.length < selection.length;
    selection = kept;
  }
  const room = limits.maxRequestBytes - encoded(selection);
  const kept = fit(content, limits.maxContentChars, (prefix) => encoded(prefix) <= room);
  truncated ||= kept.length < content.length;
  return { title, content: kept, selection, truncated, readable, href: String(window.location.href) };
}
