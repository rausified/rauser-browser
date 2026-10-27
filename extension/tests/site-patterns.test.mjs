import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { readFileSync } from "node:fs";

const {
  inspectSitePatterns, httpOrigin, isUrlShape, readPatternResult,
  prunePatternSamples, samplePattern, SAMPLE_TTL_MS, MAX_SHAPE_COUNT,
} = await import("../dist/site-patterns.js");

const shape = (url) => inspectSitePatterns(url).current;

function privateShape(value) {
  assert.ok(isUrlShape(value), value);
  const literals = value.replace(/\{(?:num|key|uuid|hex32|hex40|hex64|tailhex32|id|slug|param)\}/g, "");
  assert.doesNotMatch(literals, /[A-Z0-9%#]/);
  for (const part of literals.split(/[/?&=]/)) assert.ok(part.length <= 20);
}

test("one shaper covers Jira, SharePoint, GitHub and named future tool URL forms", () => {
  const cases = [
    ["https://acme.atlassian.net/browse/PROJ-42", "/browse/{key}"],
    ["https://acme.atlassian.net/jira/software/projects/PROJ/boards/7?selectedIssue=PROJ-42", "/jira/software/projects/{slug}/boards/{num}?{param}={key}"],
    ["https://acme.sharepoint.com/sites/work/_layouts/15/Doc.aspx?sourcedoc=%7B123e4567-e89b-12d3-a456-426614174000%7D", "/sites/work/_layouts/{num}/{slug}?sourcedoc={uuid}"],
    ["https://acme-my.sharepoint.com/personal/person/_layouts/15/Doc.aspx?sourcedoc=123e4567-e89b-12d3-a456-426614174000", "/personal/person/_layouts/{num}/{slug}?sourcedoc={uuid}"],
    ["https://github.com/Acme/Widget/pull/42/files", "/{slug}/{slug}/pull/{num}/files"],
    ["https://github.com/acme/widget/issues/42", "/acme/widget/issues/{num}"],
    ["https://wiki.example/display/SPACE/Title", "/display/{slug}/{slug}"],
    ["https://wiki.example/wiki/spaces/SPACE/pages/123456/Release-Plan", "/wiki/spaces/{slug}/pages/{num}/{slug}"],
    ["https://www.notion.so/Release-Plan-0123456789abcdef0123456789abcdef", "/{tailhex32}"],
    ["https://www.notion.so/0123456789abcdef0123456789abcdef", "/{hex32}"],
    ["https://docs.google.com/document/u/0/d/aBcD123-Secret/edit", "/document/u/{num}/d/{id}/edit"],
    ["https://wiki.example/pages/Customer%20Roadmap", "/pages/{slug}"],
    ["https://wiki.example/pages/VeryLongMixedCaseCustomerRoadmap", "/pages/{slug}"],
    ["https://wiki.example/items/123e4567-e89b-12d3-a456-426614174000", "/items/{uuid}"],
    ["https://wiki.example/items/" + "a".repeat(40), "/items/{hex40}"],
    ["https://wiki.example/items/" + "a".repeat(64), "/items/{hex64}"],
  ];
  for (const [url, expected] of cases) {
    assert.equal(shape(url), expected, url);
    privateShape(expected);
  }
});

test("every recorded starter fixture URL produces a valid private shape", () => {
  let checked = 0;
  const vectors = JSON.parse(readFileSync(new URL("../../protocol/adapter-templates/vectors.json", import.meta.url), "utf8"));
  for (const vector of vectors.cases.filter((row) => row.doc?.$fixture === "starters" && typeof row.url === "string")) {
    const origin = httpOrigin(vector.url);
    if (!origin || new URL(vector.url).pathname.split("/").length > 33) {
      assert.equal(shape(vector.url), null, vector.id);
    } else {
      privateShape(shape(vector.url));
      checked += 1;
    }
  }
  assert.ok(checked >= 20, `expected recorded URLs, saw ${checked}`);
});

test("queries contain only shaped values and private keys, and fragments disappear", () => {
  const value = shape("https://work.example/route-word/_layouts/end_?customer=secret&privateKey=CustomerName&sort=ASC&page=42#TOP-SECRET");
  assert.equal(value, "/route-word/_layouts/end_?customer={slug}&page={num}&sort={slug}&{param}={slug}");
  privateShape(value);
  assert.equal(shape("https://work.example/a?z=7&a=ABC-12"), shape("https://work.example/a?a=ABC-12&z=9"));
  assert.equal(shape("https://work.example/a?x=one&x=two"), "/a?x={slug}&x={slug}");
  assert.equal(shape("https://work.example//a/"), "//a/");
  for (const bad of ["chrome://newtab", "file:///private/x", "not a url", "https://user:password@work.example/a", "https://work.example/" + "x".repeat(8192), "https://work.example/" + "a/".repeat(33), "https://work.example/?" + "a=x&".repeat(9)]) {
    assert.equal(shape(bad), null, bad.slice(0, 100));
  }
  assert.equal(httpOrigin("https://user:password@work.example/a"), null);
  assert.equal(httpOrigin("http://localhost:8123/a"), "http://localhost:8123");
});

function inject({ href = "https://work.example/pages/42", origin = "https://work.example", expected = href, links = [], topFrame = true, afterRead } = {}) {
  let reads = 0;
  const location = { href, origin };
  const window = {};
  window.top = topFrame ? window : {};
  const anchors = links.map((link) => ({ get href() { reads += 1; afterRead?.(location); return link; } }));
  const context = {
    URL, location, window, expected,
    document: { links: new Proxy(anchors, { get(target, key) { assert.notEqual(key, "length", "reading length walks every link"); return target[key]; } }) },
  };
  // Chrome serializes the function in isolation: this catches accidental
  // dependencies on imports or module-scope helpers.
  const result = vm.runInNewContext(`(${inspectSitePatterns.toString()})(expected, true)`, context);
  return { result: JSON.parse(JSON.stringify(result)), reads };
}

test("the isolated link reader never returns raw hrefs and generalizes singleton literals", () => {
  const { result } = inject({ links: [
    "https://work.example/tickets/123456/details", "https://work.example/tickets/987654/history",
    "https://work.example/customer-secret/42", "https://work.example/customer-secret/42",
    "https://other.example/remote/9", "https://user:pw@work.example/secret/1", "javascript:alert(1)",
  ] });
  assert.equal(result.current, "/pages/{num}");
  assert.ok(result.links.some((row) => row.shape === "/tickets/{num}/{slug}" && row.count === 2));
  assert.ok(result.links.some((row) => row.shape === "/{slug}/{num}" && row.count === 2));
  assert.doesNotMatch(JSON.stringify(result), /123456|987654|customer-secret|https:|remote|secret\/1/);
  for (const row of result.links) privateShape(row.shape);
});

test("page identity, frame and origin checks reject stale or embedded documents", () => {
  for (const options of [
    { expected: "https://work.example/pages/43" },
    { expected: "https://work.example/pages/42#changed" },
    { topFrame: false },
    { origin: "https://other.example" },
    { afterRead: (location) => { location.href += "#changed"; }, links: ["https://work.example/items/1"] },
  ]) {
    const { result, reads } = inject(options);
    assert.deepEqual(result, { current: null, links: [] });
    if (!options.afterRead) assert.equal(reads, 0);
  }
});

test("the link reader reads at most 500 anchors and returns at most 50 shapes", () => {
  const links = Array.from({ length: 600 }, (_, i) => `https://work.example/route/${"x/".repeat(Math.floor(i / 4) % 30)}${i % 2 ? "123" : "ABC-123"}?${i % 3 ? "ref" : "mode"}=7`);
  const { result, reads } = inject({ links });
  assert.equal(reads, 500);
  assert.equal(result.links.length, 50);
  assert.ok(result.links.every((row) => row.count >= 1 && row.count <= 500));
  const offsite = inject({ links: [...Array(500).fill("https://other.example/a"), "https://work.example/secret/99"] });
  assert.equal(offsite.reads, 500);
  assert.deepEqual(offsite.result.links, []);
});

test("untrusted shapes cannot smuggle titles, identifiers or extra metadata into retained results", () => {
  for (const value of [null, {}, "/items/123", "/Title", "/" + "x".repeat(21), "/a?key=secret", "/a?Key={slug}", "/a#{slug}", "/a?key={slug}?x={num}", "/a?{param}={param}", "/{token}", "/a-b-c/{num}"]) {
    assert.equal(isUrlShape(value), false, String(value));
  }
  for (const value of [{}, { current: null, links: [] }, { current: "/a", links: Array(51).fill({ shape: "/a", count: 1 }) }, { current: "/a", links: [{ shape: "/a/123", count: 1 }] }, { current: "/a", links: [{ shape: "/a", count: 501 }] }]) {
    assert.throws(() => readPatternResult(value));
  }
  assert.deepEqual(readPatternResult({ current: "/items/{num}", links: [{ shape: "/a/{num}", count: 1, title: "secret" }], url: "secret" }), {
    current: "/items/{num}", links: [{ shape: "/a/{num}", count: 1 }],
  });
});

test("samples cap distinct patterns and counts, expire at seven days and purge disabled origins", () => {
  const origin = "https://work.example";
  const now = Date.now();
  const samples = {};
  const words = Array.from({ length: 24 }, (_, i) => `route${String.fromCharCode(97 + i)}`);
  for (const [index, word] of words.entries()) samplePattern(samples, origin, `/${word}/{num}`, now + index);
  assert.equal(samples[origin].length, 20);
  assert.equal(samples[origin].at(-1).shape, "/routee/{num}");
  const newest = samples[origin][0];
  newest.count = MAX_SHAPE_COUNT;
  samplePattern(samples, origin, newest.shape, now + 24);
  assert.equal(newest.count, MAX_SHAPE_COUNT);
  samplePattern(samples, origin, "/private/123", now);
  assert.equal(samples[origin].length, 20);
  samples[origin][0].last_seen = now - SAMPLE_TTL_MS;
  assert.equal(prunePatternSamples(samples, [origin], now + 24)[origin].length, 19);
  assert.deepEqual(prunePatternSamples(samples, [], now + 24), {});
  assert.deepEqual(prunePatternSamples(samples, [origin], now + SAMPLE_TTL_MS + 24), {});
});

test("corrupt samples are discarded and origin, shape and metadata budgets are independently bounded", () => {
  const origin = "https://work.example";
  const now = Date.now();
  const row = { shape: "/items/{num}", count: 1, last_seen: now };
  const stored = { [origin]: [
    { ...row, shape: "/items/42" }, { ...row, count: 1.5 }, { ...row, last_seen: now + 1 },
    { ...row, count: MAX_SHAPE_COUNT + 1 }, { ...row, title: "secret", url: "secret" }, row,
    { ...row, shape: "/other/{slug}", last_seen: now - SAMPLE_TTL_MS },
    null,
  ], "https://disabled.example": [row] };
  assert.deepEqual(prunePatternSamples(stored, [origin], now), { [origin]: [row] });
  for (const value of [null, [], "bad", { [origin]: "bad" }]) assert.deepEqual(prunePatternSamples(value, [origin], now), {});
  const many = Object.fromEntries(Array.from({ length: 100 }, (_, i) => [`https://site${i}.example`, [row]]));
  assert.equal(Object.keys(prunePatternSamples(many, Object.keys(many), now)).length, 64);
  assert.deepEqual(prunePatternSamples({ [origin]: [...Array(20).fill(null), row] }, [origin], now), {});
});

// One fixture pins this shaper and the host's Rust parser
// (adapter-templates/src/shape.rs) to the same canonical shapes.
const shapeFixture = JSON.parse(readFileSync(new URL("../../protocol/capture-site/shapes.json", import.meta.url), "utf8"));
const built = (value) => typeof value === "string" ? value : value.$build.map((part) =>
  typeof part === "string" ? part : part.repeat.repeat(part.times)).join("");

test("shared shape fixture matches the in-page shaper", () => {
  assert.ok(shapeFixture.accept.length >= 30);
  for (const row of shapeFixture.accept) {
    const expected = built(row.shape);
    assert.equal(inspectSitePatterns(built(row.url)).current, expected, row.note);
    assert.ok(isUrlShape(expected), row.note);
    privateShape(expected);
  }
});

test("isUrlShape agrees with the shared reject list", () => {
  assert.ok(shapeFixture.reject.length >= 20);
  for (const row of shapeFixture.reject) assert.equal(isUrlShape(built(row.shape)), false, row.note);
});
