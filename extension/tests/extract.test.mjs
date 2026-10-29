import assert from "node:assert/strict";
import test from "node:test";
import { existsSync, readFileSync } from "node:fs";
import vm from "node:vm";

// Run after build:extension. Chrome serializes extractReadablePage and runs
// it in the page, so each test runs its source text in a fresh context that
// holds only the fakes it is given.
const { EXTRACT_LIMITS, extractReadablePage } = await import("../dist/extract.js");
const { readTab, summarizeTab, UNREADABLE_MESSAGE, PAGE_CHANGED_MESSAGE } = await import("../dist/summarize.js");

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
// `rendered` / `bodyRendered` false give the document element / body no
// layout boxes (a display:none iframe; a display:none body), and `bodyNodes`
// are the body's children. Without either option the fake has no
// getClientRects, as in the older tests.
function page({
  title = "Page title", article = null, articleTitle = "", bodyText = "", selection = "", href = PAGE_URL,
  mainHeadings = [], rendered, bodyRendered = rendered, bodyNodes = [],
} = {}) {
  const seen = { clonedDeep: null, options: null, parsed: 0 };
  const rects = (shown) => (shown === undefined ? {} : { getClientRects: () => (shown ? [{ width: 1, height: 1 }] : []) });
  const body = Object.assign(el("body", ...bodyNodes), { innerText: bodyText }, rects(bodyRendered));
  const document = {
    title,
    body,
    documentElement: { ...rects(rendered) },
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
        assert.deepEqual(request.target, { tabId: 7, allFrames: true });
        if (request.files) {
          assert.deepEqual(request.files, ["vendor/readability.js"]);
          return [];
        }
        return [{ frameId: 0, result: vm.runInNewContext(`(${request.func})(${JSON.stringify(request.args[0])})`, scope) }];
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

// What John's first task run read from a SharePoint PowerPoint for the web
// page (2026-10-28): a hidden Office token-factory iframe whose body holds
// only an inline script. In a frame that is not rendered, innerText returns
// textContent, script text included.
const TOKEN_SCRIPT = 'var settings = {"requestExecutorSettings":{},"tokenFactorySettings":{"upn":"user@example.com"}};';
const hiddenScriptFrame = (fields = {}) => page({
  title: "", bodyText: TOKEN_SCRIPT, bodyNodes: [el("script", TOKEN_SCRIPT)], rendered: false, selection: "x", ...fields,
});

test("a frame that is not rendered contributes no content or selection", () => {
  const { scope } = hiddenScriptFrame();
  const result = run(scope);
  assert.equal(result.content, "");
  assert.equal(result.selection, null);
  assert.equal(result.readable, false);
  assert.doesNotMatch(JSON.stringify(result), /tokenFactorySettings|user@example\.com/);
  // An article Readability finds in a hidden frame is not content either.
  const article = page({ article: el("p", "Hidden article"), rendered: false });
  assert.equal(run(article.scope).content, "");
  assert.equal(run(article.scope).readable, false);
});

test("the body fallback never includes script, style, noscript or template text", () => {
  // A rendered document whose body is not (display:none): innerText would
  // return textContent, so the fallback walks the body without those.
  const nodes = [
    el("p", "Visible ", el("script", "var leak = 1;"), "slide text"),
    el("style", ".leak { color: red }"),
    el("noscript", "Enable JavaScript leak"),
    el("template", el("p", "Template leak")),
    el("div", "Second block"),
  ];
  const leaky = nodes.map((node) => node.textContent).join("");
  const { scope } = page({ title: "Deck", bodyText: leaky, bodyNodes: nodes, rendered: true, bodyRendered: false });
  const result = run(scope);
  assert.equal(result.content, "Visible slide text\n\nSecond block");
  assert.doesNotMatch(result.content, /leak/i);
  assert.equal(result.readable, false);
});

test("a rendered page's body fallback is still its innerText", () => {
  const { scope } = page({
    title: "Doc", bodyText: "  Rendered text  ", bodyNodes: [el("script", "var leak = 1;")], rendered: true,
  });
  assert.equal(run(scope).content, "Rendered text");
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

// Summarize reads every frame: Word for the web and similar editors render
// the document in a cross-origin iframe, so the top frame holds no text.
const TAB = { id: 9, url: "https://cisco-my.sharepoint.com/:w:/r/personal/u/_layouts/15/Doc.aspx?sourcedoc=x", title: "Plan.docx" };
const framePage = (fields) => ({
  title: "", content: "", selection: null, truncated: false, readable: false, href: TAB.url, ...fields,
});

// Deps whose executeScript answers the extractor with `frames` (Chrome's
// per-frame results) and records every injection.
function framesDeps(frames, { readability = async () => [] } = {}) {
  const injections = [];
  const requests = [];
  return {
    injections,
    requests,
    deps: {
      scripting: {
        async executeScript(injection) {
          injections.push(injection);
          if (injection.files) return readability(injection);
          return typeof frames === "function" ? frames(injection) : frames;
        },
      },
      host: {
        async call(request) {
          requests.push(request);
          return { run: { title: request.title, url: request.url }, existing: false };
        },
      },
      storage: { async set() {} },
      now: () => new Date("2026-10-28T10:00:00Z"),
    },
  };
}

test("summarizeTab injects Readability and the extractor into every frame", async () => {
  const { deps, injections } = framesDeps([{ frameId: 0, result: framePage({ content: "Body" }) }]);
  await summarizeTab(TAB, deps);
  assert.equal(injections.length, 2);
  assert.deepEqual(injections[0], { target: { tabId: 9, allFrames: true }, files: ["vendor/readability.js"] });
  assert.deepEqual(injections[1].target, { tabId: 9, allFrames: true });
  assert.equal(injections[1].func, extractReadablePage);
  assert.deepEqual(injections[1].args, [EXTRACT_LIMITS]);
});

test("readTab reads the page summarizeTab would send, and sends nothing", async () => {
  const frames = [
    { frameId: 0, result: framePage({ title: "Top\uD800 title", content: "" }) },
    { frameId: 3, result: framePage({ content: "Office text\uDC00", selection: "picked" }) },
  ];
  const read = framesDeps(frames);
  const page = await readTab(TAB, read.deps);
  assert.deepEqual(page, { url: TAB.url, title: "Top\uFFFD title", content: "Office text\uFFFD", selection: "picked" });
  assert.equal(read.requests.length, 0);
  assert.deepEqual(read.injections.map((injection) => injection.files ?? injection.args), [
    ["vendor/readability.js"], [EXTRACT_LIMITS],
  ]);
  const summarized = framesDeps(frames);
  await summarizeTab(TAB, summarized.deps);
  const { url, title, content, selection } = summarized.requests[0];
  assert.deepEqual({ url, title, content, selection }, page);

  const untitled = framesDeps([{ frameId: 0, result: framePage({ title: "", content: "Body" }) }]);
  assert.equal((await readTab(TAB, untitled.deps)).title, TAB.title);
  const empty = framesDeps([{ frameId: 0, result: framePage({ content: " " }) }]);
  await assert.rejects(readTab(TAB, empty.deps), (error) => error.code === "unreadable" && error.message === UNREADABLE_MESSAGE);
  const moved = framesDeps([{ frameId: 0, result: framePage({ content: "Body", href: "https://elsewhere.example/" }) }]);
  await assert.rejects(readTab(TAB, moved.deps),
    (error) => error.code === "page_changed" && error.message === PAGE_CHANGED_MESSAGE);
});

test("an empty top frame summarizes the Office subframe's text under the top frame's title", async () => {
  const doc = "Quarterly plan\n\n" + "The document body. ".repeat(40);
  const { deps, requests } = framesDeps([
    { frameId: 0, result: framePage({ title: "Plan.docx", href: TAB.url }) },
    { frameId: 3, result: framePage({ title: "Word", content: doc, readable: true, href: "https://word-edit.officeapps.live.com/we/wordeditorframe.aspx?ui=en-US" }) },
  ]);
  await summarizeTab(TAB, deps);
  assert.equal(requests.length, 1);
  assert.equal(requests[0].content, doc);
  assert.equal(requests[0].title, "Plan.docx");
  assert.equal(requests[0].url, TAB.url);

  // With no top-frame title, the tab's title stands in, never the iframe's.
  const untitled = framesDeps([
    { frameId: 0, result: framePage({}) },
    { frameId: 3, result: framePage({ title: "Word", content: doc, href: "https://word-edit.officeapps.live.com/x" }) },
  ]);
  await summarizeTab(TAB, untitled.deps);
  assert.equal(untitled.requests[0].title, "Plan.docx");
});

test("a top frame with article text wins over a small ad frame", async () => {
  const article = "Article paragraph. ".repeat(20);
  const { deps, requests } = framesDeps([
    { frameId: 0, result: framePage({ title: "Article", content: article, readable: true }) },
    { frameId: 5, result: framePage({ title: "Ad", content: "Buy now ".repeat(100), href: "https://ads.example/frame" }) },
  ]);
  await summarizeTab(TAB, deps);
  assert.equal(requests[0].content, article);
  assert.equal(requests[0].title, "Article");

  // A short top frame still wins a tie, and when it is the longest.
  const tie = framesDeps([
    { frameId: 0, result: framePage({ content: "Short top" }) },
    { frameId: 5, result: framePage({ content: "Short sub", href: "https://ads.example/frame" }) },
  ]);
  await summarizeTab(TAB, tie.deps);
  assert.equal(tie.requests[0].content, "Short top");
});

test("the selection comes from the frame where the user selected text", async () => {
  const doc = "The document body. ".repeat(40);
  const { deps, requests } = framesDeps([
    { frameId: 0, result: framePage({}) },
    { frameId: 3, result: framePage({ content: doc, selection: "picked words", href: "https://word-edit.officeapps.live.com/x" }) },
  ]);
  await summarizeTab(TAB, deps);
  assert.equal(requests[0].selection, "picked words");

  // A selection in another frame than the content still travels.
  const article = "Article paragraph. ".repeat(20);
  const other = framesDeps([
    { frameId: 0, result: framePage({ content: article }) },
    { frameId: 5, result: framePage({ content: "x", selection: "from the frame", href: "https://embed.example/" }) },
  ]);
  await summarizeTab(TAB, other.deps);
  assert.equal(other.requests[0].content, article);
  assert.equal(other.requests[0].selection, "from the frame");
});

test("a frame that refuses injection or returns nothing is skipped", async () => {
  const doc = "The document body. ".repeat(40);
  const { deps, requests } = framesDeps([
    { frameId: 0, result: framePage({}) },
    { frameId: 2, error: new Error("Cannot access a chrome-extension:// URL of different extension") },
    { frameId: 4 },
    { frameId: 6, result: null },
    { frameId: 7, result: { content: 42 } },
    { frameId: 3, result: framePage({ content: doc, href: "https://word-edit.officeapps.live.com/x" }) },
  ], { readability: async () => { throw new Error("Cannot access contents of the page"); } });
  await summarizeTab(TAB, deps);
  assert.equal(requests.length, 1);
  assert.equal(requests[0].content, doc);
});

test("no frame with text is unreadable, and a missing top frame is too", async () => {
  const empty = framesDeps([
    { frameId: 0, result: framePage({ content: "   " }) },
    { frameId: 3, result: framePage({ content: "", href: "https://word-edit.officeapps.live.com/x" }) },
  ]);
  await assert.rejects(summarizeTab(TAB, empty.deps),
    (error) => error.code === "unreadable" && error.message === UNREADABLE_MESSAGE);
  assert.equal(empty.requests.length, 0);

  const noTop = framesDeps([
    { frameId: 3, result: framePage({ content: "Text", href: "https://word-edit.officeapps.live.com/x" }) },
  ]);
  await assert.rejects(summarizeTab(TAB, noTop.deps), (error) => error.code === "unreadable");

  const refused = framesDeps(() => { throw new Error("Cannot access contents of the page"); });
  await assert.rejects(summarizeTab(TAB, refused.deps), (error) => error.code === "unreadable");
  assert.equal(noTop.requests.length + refused.requests.length, 0);
});

test("only the top frame's address must still name the tab's page", async () => {
  const doc = "The document body. ".repeat(40);
  const moved = framesDeps([
    { frameId: 0, result: framePage({ href: "https://cisco-my.sharepoint.com/other" }) },
    { frameId: 3, result: framePage({ content: doc, href: TAB.url }) },
  ]);
  await assert.rejects(summarizeTab(TAB, moved.deps),
    (error) => error.code === "page_changed" && error.message === PAGE_CHANGED_MESSAGE);
  assert.equal(moved.requests.length, 0);
});

test("a hidden script-only frame loses to a visible frame with short text", async () => {
  // Every frame's result comes from the real extractor, as Chrome runs it.
  const extracted = (scope, href) => {
    scope.window.location.href = href;
    return run(scope);
  };
  const top = page({ title: "Deck.pptx", bodyText: "", rendered: true, href: TAB.url }).scope;
  const hidden = hiddenScriptFrame().scope;
  const slide = page({ bodyText: "Slide 1: Plan", rendered: true }).scope;
  const frames = [
    { frameId: 0, result: extracted(top, TAB.url) },
    { frameId: 2, result: extracted(hidden, "https://cisco-my.sharepoint.com/_layouts/15/tokenfactoryiframe") },
    { frameId: 4, result: extracted(slide, "https://powerpoint.officeapps.live.com/p/PowerPointFrame.aspx") },
  ];
  const { deps, requests } = framesDeps(frames);
  await summarizeTab(TAB, deps);
  assert.equal(requests[0].content, "Slide 1: Plan");
  assert.equal(requests[0].selection, null);
  assert.doesNotMatch(JSON.stringify(requests[0]), /tokenFactorySettings|user@example\.com/);
  // With no visible text anywhere, the hidden frame cannot make it readable.
  const alone = framesDeps(frames.slice(0, 2));
  await assert.rejects(readTab(TAB, alone.deps), (error) => error.code === "unreadable");
});

test("a selection borrowed from another frame never pushes start_run past the cap", async () => {
  const room = EXTRACT_LIMITS.maxRequestBytes - Buffer.byteLength(JSON.stringify(null));
  // The top frame's content filled its own cap, measured with no selection.
  const content = "a".repeat(room - 2);
  const { deps, requests } = framesDeps([
    { frameId: 0, result: framePage({ content }) },
    { frameId: 5, result: framePage({ content: "x", selection: "picked", href: "https://embed.example/" }) },
  ]);
  await summarizeTab(TAB, deps);
  const [request] = requests;
  const bytes = Buffer.byteLength(JSON.stringify(request.content)) + Buffer.byteLength(JSON.stringify(request.selection));
  assert.ok(bytes <= EXTRACT_LIMITS.maxRequestBytes);
});

test("a non-article top frame yields to a subframe with five times its text", async () => {
  const chrome = "Suite bar Sign in Footer ".repeat(16).slice(0, 400);
  const doc = "The document body. ".repeat(300).slice(0, 5000);
  const sub = (content, readable = true) => ({
    frameId: 3, result: framePage({ content, readable, href: "https://word-edit.officeapps.live.com/x" }),
  });
  const chromeTop = framesDeps([{ frameId: 0, result: framePage({ title: "Plan.docx", content: chrome }) }, sub(doc)]);
  await summarizeTab(TAB, chromeTop.deps);
  assert.equal(chromeTop.requests[0].content, doc);
  assert.equal(chromeTop.requests[0].title, "Plan.docx");

  // An article top frame wins whatever its subframes hold.
  const articleTop = framesDeps([{ frameId: 0, result: framePage({ content: chrome, readable: true }) }, sub(doc)]);
  await summarizeTab(TAB, articleTop.deps);
  assert.equal(articleTop.requests[0].content, chrome);

  // Under five times the top frame's text, the top frame stays.
  const close = framesDeps([{ frameId: 0, result: framePage({ content: chrome }) }, sub(doc.slice(0, 1000))]);
  await summarizeTab(TAB, close.deps);
  assert.equal(close.requests[0].content, chrome);
});

test("a hung subframe falls back to reading the top frame alone", async () => {
  const { deps, requests, injections } = framesDeps((injection) => injection.target.allFrames
    ? new Promise(() => {})
    : [{ frameId: 0, result: framePage({ content: "Top text" }) }]);
  deps.allFramesTimeoutMs = 20;
  await summarizeTab(TAB, deps);
  assert.deepEqual(injections.map((injection) => injection.target), [
    { tabId: 9, allFrames: true }, { tabId: 9, allFrames: true }, { tabId: 9, frameIds: [0] },
  ]);
  assert.equal(requests[0].content, "Top text");
});

test("a hung Readability injection is given up on and the page is read without it", async () => {
  const { deps, requests, injections } = framesDeps(
    [{ frameId: 0, result: framePage({ content: "Body text" }) }],
    { readability: () => new Promise(() => {}) },
  );
  deps.allFramesTimeoutMs = 20;
  await summarizeTab(TAB, deps);
  assert.deepEqual(injections.map((injection) => [injection.target, injection.files ?? injection.func]), [
    [{ tabId: 9, allFrames: true }, ["vendor/readability.js"]],
    [{ tabId: 9, allFrames: true }, extractReadablePage],
  ]);
  assert.equal(requests[0].content, "Body text");
});

test("the all-frames timeout is ten seconds", async () => {
  const { ALL_FRAMES_TIMEOUT_MS } = await import("../dist/summarize.js");
  assert.equal(ALL_FRAMES_TIMEOUT_MS, 10_000);
});
