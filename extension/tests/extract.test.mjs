import assert from "node:assert/strict";
import test from "node:test";
import { existsSync, readFileSync } from "node:fs";
import vm from "node:vm";

// Run after build:extension. Chrome serializes extractReadablePage and runs
// it in the page, so each test runs its source text in a fresh context that
// holds only the fakes it is given.
const { EXTRACT_LIMITS, extractReadablePage } = await import("../dist/extract.js");
const { summarizeTab } = await import("../dist/summarize.js");

// The host refuses a frame over this many bytes (`frame::MAX_INBOUND_BYTES`).
const MAX_INBOUND_BYTES = 4 * 1024 * 1024;

const PAGE_URL = "https://read.example/article";

const text = (value) => ({ nodeType: 3, nodeValue: value, childNodes: [], textContent: value });
const el = (tag, ...children) => ({
  nodeType: 1,
  tagName: tag.toUpperCase(),
  childNodes: children.map((child) => (typeof child === "string" ? text(child) : child)),
  get textContent() {
    return this.childNodes.map((child) => child.textContent).join("");
  },
});

// A page whose Readability finds `article` (null: no article), with
// `bodyText` as the body's innerText and `selection` as the user's selection.
function page({
  title = "Page title", article = null, articleTitle = "", bodyText = "", selection = "", href = PAGE_URL,
  mainHeadings = [],
} = {}) {
  const seen = { clonedDeep: null, options: null, parsed: 0 };
  const document = {
    title,
    body: { innerText: bodyText },
    querySelectorAll: () => mainHeadings,
    cloneNode(deep) {
      seen.clonedDeep = deep;
      return { clone: true };
    },
  };
  const window = { getSelection: () => ({ toString: () => selection }), location: { href } };
  class Readability {
    constructor(doc, options) {
      assert.deepEqual(doc, { clone: true });
      seen.options = options;
    }
    parse() {
      seen.parsed += 1;
      if (article === null) return null;
      return { title: articleTitle, content: seen.options.serializer(article) };
    }
  }
  return { scope: { document, window, Readability, TextEncoder }, seen };
}

const run = (scope, limits = EXTRACT_LIMITS) =>
  vm.runInNewContext(`(${extractReadablePage})(${JSON.stringify(limits)})`, scope);

function heading(value, excluded = false) {
  return { innerText: value, textContent: value, closest: () => excluded ? {} : null };
}

test("a current main heading wins over stale document and Readability titles", () => {
  for (const article of [null, el("p", "The pull request's current content")]) {
    const { scope } = page({
      title: "Pull requests · acme/project", articleTitle: "Pull requests · acme/project", article,
      bodyText: "Current body", mainHeadings: [heading("  Fix the\n release   #35  ")],
    });
    assert.equal(run(scope).title, "Fix the release #35");
  }
});

test("missing or ambiguous main headings retain article and document title fallbacks", () => {
  for (const mainHeadings of [[], [heading(" ")], [heading("Hidden", true)], [heading("First"), heading("Second")]]) {
    assert.equal(run(page({ article: el("p", "Body"), articleTitle: "Article title", mainHeadings }).scope).title,
      "Article title");
    assert.equal(run(page({ title: "Document title", bodyText: "Body", mainHeadings }).scope).title,
      "Document title");
  }
  assert.equal(run(page({ bodyText: "Body", mainHeadings: [heading("Same"), heading(" Same ")] }).scope).title, "Same");
});

test("heading titles keep the title cap and surrogate replacement", () => {
  const { scope } = page({ bodyText: "Body", mainHeadings: [heading("\uD800" + "😀".repeat(2000))] });
  const title = run(scope).title;
  assert.equal(title[0], "\uFFFD");
  assert.equal(title.length, EXTRACT_LIMITS.maxTitleChars - 1);
  assert.ok(title.endsWith("😀"));
});

test("summarizeTab sends the item title after list-to-item SPA navigation with stale metadata", async () => {
  const oldTitle = "Pull requests · acme/project";
  const itemTitle = "Fix the release #35";
  const mainHeadings = [heading("Pull requests")];
  const article = el("div", el("p", "List content"));
  const { scope } = page({
    title: oldTitle, articleTitle: oldTitle, article, mainHeadings, href: "https://github.com/acme/project/pulls",
  });
  assert.equal(run(scope).content, "List content");
  // pushState updates the URL and body before either tab/document metadata.
  scope.window.location.href = "https://github.com/acme/project/pull/35";
  article.childNodes = [el("h1", itemTitle), el("p", "Item content")];
  mainHeadings.splice(0, 1, heading(itemTitle));
  assert.equal(scope.document.title, oldTitle);
  const requests = [];
  const deps = {
    scripting: {
      async executeScript(request) {
        assert.deepEqual(request.target, { tabId: 7, frameIds: [0] });
        if (request.files) {
          assert.deepEqual(request.files, ["vendor/readability.js"]);
          return [];
        }
        return [{ result: vm.runInNewContext(`(${request.func})(${JSON.stringify(request.args[0])})`, scope) }];
      },
    },
    host: {
      async call(request, expected) {
        assert.equal(request.type, "start_run");
        assert.equal(expected, "run_started");
        requests.push(request);
        return { run: { title: request.title, url: request.url }, existing: false };
      },
    },
    storage: { async set() {} },
    now: () => new Date("2026-10-28T10:00:00Z"),
  };
  const result = await summarizeTab({ id: 7, url: scope.window.location.href, title: oldTitle }, deps);
  assert.equal(requests.length, 1);
  assert.equal(requests[0].title, itemTitle);
  assert.equal(requests[0].url, scope.window.location.href);
  assert.equal(requests[0].content, `# ${itemTitle}\n\nItem content`);
  assert.equal(result.run.title, itemTitle);
  // A caller still on the list URL must never attach item content to it.
  await assert.rejects(summarizeTab({ id: 7, url: "https://github.com/acme/project/pulls", title: oldTitle }, deps),
    (error) => error.code === "page_changed");
  assert.equal(requests.length, 1);
});

test("extracts article blocks as text and drops images", () => {
  const article = el("div",
    el("h1", "The ", el("em", "big"), " story"),
    el("p", "First   paragraph\n with a ", el("a", "link"), ".", el("img")),
    el("h3", "Details"),
    el("ul", el("li", "one"), el("li", el("p", "two"))),
    el("pre", "fn main() {\n    run();\n}"),
    el("p", "Line", el("br"), "break"),
    el("script", "alert(1)"),
    el("style", "p { color: red }"),
    el("p", el("img")),
  );
  const { scope, seen } = page({ article, articleTitle: "Article title", selection: "  picked words " });
  const result = run(scope);
  assert.equal(seen.clonedDeep, true);
  assert.equal(seen.options.serializer(article), article);
  assert.deepEqual({ ...result }, {
    title: "Article title",
    content: [
      "# The big story",
      "First paragraph with a link.",
      "### Details",
      "- one",
      "- two",
      "fn main() {\n    run();\n}",
      "Line\nbreak",
    ].join("\n\n"),
    selection: "  picked words ",
    truncated: false,
    readable: true,
    href: PAGE_URL,
  });
});

test("falls back to body text when Readability finds no article", () => {
  const { scope, seen } = page({ title: "Doc title", bodyText: "\n  Body line one\nBody line two  \n" });
  const result = run(scope);
  assert.equal(seen.parsed, 1);
  assert.deepEqual({ ...result }, {
    title: "Doc title", content: "Body line one\nBody line two", selection: null, truncated: false, readable: false,
    href: PAGE_URL,
  });
  // An article that holds no text falls back as well.
  const empty = page({ title: "Doc title", article: el("div", el("img")), bodyText: "Body" });
  assert.deepEqual({ ...run(empty.scope) }, {
    title: "Doc title", content: "Body", selection: null, truncated: false, readable: false, href: PAGE_URL,
  });
});

test("maximal extraction stays under the host frame cap", () => {
  // Every third character escapes six-fold in JSON or is a quote; the rest
  // take three UTF-8 bytes. Neither measure in characters bounds the frame.
  const unit = "界\"\u0001";
  const content = unit.repeat(1_000_000 / 3) + "界";
  const selection = "\"\u0001界".repeat(21_845) + "\"";
  assert.equal(content.length, 1_000_000);
  assert.equal(selection.length, 65_536);
  const { scope } = page({ article: el("div", el("pre", content)), articleTitle: "界".repeat(5000), selection });
  const result = run(scope);
  assert.equal(result.truncated, true);
  assert.equal(result.readable, true);
  assert.ok(content.startsWith(result.content));
  assert.ok(result.content.length > 100_000);
  assert.ok(selection.startsWith(result.selection));
  const encoded = (value) => new TextEncoder().encode(JSON.stringify(value)).length;
  assert.ok(encoded(result.selection) <= EXTRACT_LIMITS.maxSelectionBytes);
  assert.ok(encoded(result.content) + encoded(result.selection) <= EXTRACT_LIMITS.maxRequestBytes);
  assert.ok(result.title.length <= EXTRACT_LIMITS.maxTitleChars);
  const frame = JSON.stringify({
    type: "start_run", protocol_version: 8, request_id: "x".repeat(128), url: `https://example.com/${"a".repeat(8000)}`,
    title: result.title, content: result.content, selection: result.selection, harness: null, record_key: null,
    requested_at: "2026-10-27T09:15:00-04:00",
  });
  assert.ok(new TextEncoder().encode(frame).length < MAX_INBOUND_BYTES);

  // Plain ASCII never escapes, so the schema's character bounds bind first.
  const ascii = page({ article: el("pre", "a".repeat(2_000_000)), selection: "b".repeat(70_000) });
  const plain = run(ascii.scope);
  assert.equal(plain.truncated, true);
  assert.equal(plain.content.length, EXTRACT_LIMITS.maxContentChars);
  assert.equal(plain.selection.length, EXTRACT_LIMITS.maxSelectionChars - 2);
});

test("truncation never splits a surrogate pair", () => {
  const { scope } = page({ article: el("pre", "😀".repeat(10)), selection: "😀".repeat(10) });
  const result = run(scope, { ...EXTRACT_LIMITS, maxRequestBytes: 30, maxSelectionBytes: 11 });
  assert.equal(result.selection, "😀😀");
  assert.equal(result.content, "😀😀😀😀");
  assert.equal(result.truncated, true);
});

test("reports the address of the document it read", () => {
  // The page may have navigated, or changed its URL with pushState, since the
  // caller chose it; summarizeTab compares this address with the run's.
  const { scope } = page({ article: el("p", "Hello"), href: "https://read.example/elsewhere" });
  assert.equal(run(scope).href, "https://read.example/elsewhere");
});

// JSON.stringify writes a lone surrogate as a \\udXXXX escape, which a strict
// parser such as serde_json rejects, failing the whole start_run.
const LONE_ESCAPE = /\\ud[89a-f][0-9a-f]{2}/i;

test("lone surrogates in page text, selection and title become U+FFFD", () => {
  const { scope } = page({
    article: el("div", el("p", "high \uD800 low \uDC00 pair 😀 end\uD83D")),
    articleTitle: "\uDE00Title\uD800",
    selection: "pick \uDBFF ed 😀",
  });
  const result = run(scope);
  assert.equal(result.content, "high \uFFFD low \uFFFD pair 😀 end\uFFFD");
  assert.equal(result.title, "\uFFFDTitle\uFFFD");
  assert.equal(result.selection, "pick \uFFFD ed 😀");
  assert.doesNotMatch(JSON.stringify(result), LONE_ESCAPE);

  // The body-text fallback, and a cut next to a replaced half, too.
  const body = page({ title: "Doc\uDC00", bodyText: "Body \uD800\uD800😀" });
  const fallback = run(body.scope, { ...EXTRACT_LIMITS, maxContentChars: 8 });
  assert.equal(fallback.title, "Doc\uFFFD");
  assert.equal(fallback.content, "Body \uFFFD\uFFFD");
  assert.equal(fallback.truncated, true);
  assert.doesNotMatch(JSON.stringify(fallback), LONE_ESCAPE);
});

test("an empty page is unreadable, not an empty run", () => {
  const { scope } = page({ title: "", article: el("div", el("p", "  "), el("img")), bodyText: " \n\t " });
  assert.deepEqual({ ...run(scope) }, {
    title: "", content: "", selection: null, truncated: false, readable: false, href: PAGE_URL,
  });
  const nobody = page({ bodyText: "" });
  nobody.scope.document.body = null;
  assert.deepEqual({ ...run(nobody.scope) }, {
    title: "Page title", content: "", selection: null, truncated: false, readable: false, href: PAGE_URL,
  });
});

test("runs with only document, window and Readability in scope", () => {
  // TextEncoder and JSON are the other globals the plan allows; a fresh
  // context has no Node, Element, NodeFilter, getComputedStyle, or module.
  const { scope } = page({ article: el("p", "Hello"), selection: "" });
  const context = vm.createContext({ ...scope });
  assert.deepEqual(Object.keys(context).sort(), ["Readability", "TextEncoder", "document", "window"]);
  assert.equal(vm.runInContext("typeof Node + typeof Element + typeof module", context), "undefinedundefinedundefined");
  const result = vm.runInContext(`(${extractReadablePage})(${JSON.stringify(EXTRACT_LIMITS)})`, context);
  assert.equal(result.content, "Hello");
  assert.equal(result.readable, true);

  // Without Readability (its script failed to load), the body text is used.
  const missing = page({ bodyText: "Body" });
  delete missing.scope.Readability;
  assert.equal(run(missing.scope).readable, false);
  assert.equal(run(missing.scope).content, "Body");
});

test("the limits match the host frame cap and the start_run schema", () => {
  assert.deepEqual({ ...EXTRACT_LIMITS }, {
    maxRequestBytes: 3 * 1024 * 1024, maxSelectionBytes: 64 * 1024,
    maxContentChars: 1_048_576, maxSelectionChars: 65_536, maxTitleChars: 2048,
  });
  const schema = JSON.parse(readFileSync(new URL("../../protocol/schema.json", import.meta.url), "utf8"));
  const start = schema.$defs.StartRunRequest.properties;
  assert.equal(start.content.maxLength, EXTRACT_LIMITS.maxContentChars);
  assert.equal(start.selection.oneOf[0].maxLength, EXTRACT_LIMITS.maxSelectionChars);
  assert.equal(start.title.maxLength, EXTRACT_LIMITS.maxTitleChars);
});

test("the vendored license ships with the bundle", () => {
  const dist = new URL("../dist/vendor/", import.meta.url);
  assert.ok(existsSync(new URL("readability.LICENSE.md", dist)));
  assert.match(readFileSync(new URL("readability.LICENSE.md", dist), "utf8"), /Apache License/);
  assert.match(readFileSync(new URL("readability.js", dist), "utf8"), /function Readability\(doc, options\)/);
});
