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

export function extractReadablePage(limits: ExtractLimits): ExtractedPage {
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

  const DROPPED = new Set(["IMG", "SCRIPT", "STYLE"]);
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

  let title = "";
  let content = "";
  let readable = false;
  try {
    if (typeof Readability === "function") {
      const article = new Readability(document.cloneNode(true), { serializer: (el) => el }).parse();
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
  if (!readable) {
    content = (document.body?.innerText ?? "").trim();
  }
  const fullTitle = title || document.title || "";
  title = cut(fullTitle, Math.min(fullTitle.length, limits.maxTitleChars));

  const picked = window.getSelection()?.toString() ?? "";
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
