// Built side-panel tests: page summary. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  ARTICLE_URL, CAPTURING, PANEL_HTML, artifact, buttonIn, descendants, install, noteLoaded,
  recognized, settle, until,
} from "./harness.mjs";

// --- The page card (redesign, 2026-11-02) ----------------------------------

test("the page card shows the page's title and URL beside the artifact status", () => {
  for (const id of ["page-title", "page-url"]) {
    const tag = PANEL_HTML.match(new RegExp(`<p id="${id}"[^>]*>`))?.[0];
    assert.ok(tag, `${id} stays in the panel`);
    assert.doesNotMatch(tag, /visually-hidden|\bhidden\b/, `${id} is visible`);
  }
  const status = PANEL_HTML.match(/<p id="artifact-status"[^>]*>/)?.[0];
  assert.ok(status && !/visually-hidden|hidden/.test(status), "the artifact status stays visible");
  assert.match(PANEL_HTML, /<span id="page-glyph"[^>]*aria-hidden="true"/, "the page tile is decorative");
});

// --- The page's existing summary (Task 51) ---------------------------------

const SUMMARY = {
  summarized_at: "2026-10-27T10:00:00-07:00", document_type: "specification",
  category: "technical specification", importance: "high",
  summary_path: "summaries/2026-10-27 Spec.md", harness: "codex",
};

const opensSummary = (request) => request.type === "open_summary"
  ? { type: "summary_opened", relative_path: SUMMARY.summary_path }
  : null;

test("page and Recent show the overview and type as literal text", async () => {
  const overview = '<img src="https://remote.example/pixel"> A decision on the launch.';
  const summary = { ...SUMMARY, overview };
  for (const recognizedPage of [false, true]) {
    const page = await install({
      tab: { id: 1, url: ARTICLE_URL, title: "Article" },
      intercept: (request) => {
        if (request.type === "get_artifact") return {
          ...(recognizedPage ? recognized(artifact("doc:a", "Article")) : { type: "artifact_result", recognized: false, artifact: null }),
          summary,
        };
        if (request.type === "list_artifacts") return {
          type: "artifact_list_result", artifact_types: ["web.page"], truncated: false,
          items: [{ local_date: "2026-10-27", artifact_id: "doc:a", artifact_type: "web.page", title: "Article",
            url: ARTICLE_URL, focused_seconds: 0, background_seconds: 0, opens: 0, why: null, summary }],
        };
        return opensSummary(request);
      },
    });
    await until(() => !page.el("page-summary").hidden, "the overview");
    // The overview is prose; its labels are chips, all literal text.
    assert.equal(page.el("page-summary-text").textContent, overview);
    assert.deepEqual(page.el("page-summary-chips").children.map((chip) => chip.textContent),
      ["specification", "technical specification", "High importance"]);
    assert.equal(page.el("page-summary-chips").children[2].dataset.tone, "high");
    // Summarize again is the card's re-summarize icon button: the same
    // #summarize control, moved into the card head, with its label kept
    // for assistive technology.
    assert.equal(page.el("summarize-label").textContent, "Summarize again");
    assert.equal(page.el("summarize").dataset.place, "card");
    assert.ok(page.el("page-summary-head").children.includes(page.el("summarize")));
    page.el("tab-recent").click();
    await until(() => page.el("recent-list").textContent.includes("Summarized"), "the Recent overview");
    assert.ok(page.el("recent-list").textContent.includes(`Summarized · ${overview} · specification`));
    assert.ok(descendants(page.el("recent-list")).every((node) => node.tagName !== "IMG"));
    assert.equal(page.el("page-summary-text").children.length, 0);
  }
});

test("an unrecognized page with a summary shows its line and opens it", async () => {
  let fail = false;
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Article" },
    intercept: (request) => {
      if (request.type === "get_artifact") {
        return { type: "artifact_result", recognized: false, artifact: null, summary: SUMMARY };
      }
      if (request.type === "open_summary" && fail) return { type: "error", code: "conflict", message: "not a summary" };
      return opensSummary(request);
    },
  });
  await until(() => page.el("page-summary").hidden === false, "the page's summary line");
  assert.equal(page.el("page-summary-text").textContent, "specification · technical specification · high importance");
  assert.equal(page.el("artifact-card").hidden, true, "an unrecognized page has no artifact card");
  assert.equal(page.el("artifact-status").textContent, "○ Not an artifact");

  page.el("page-summary-open").click();
  await until(() => page.sent("open_summary").length === 1, "open_summary");
  assert.equal(page.sent("open_summary")[0].url, ARTICLE_URL);
  assert.equal(page.sent("open_summary")[0].summary_path, SUMMARY.summary_path);

  fail = true;
  page.el("page-summary-open").click();
  await until(() => /Could not open the summary: not a summary/.test(page.el("status").textContent), "the open failure");
});

test("a recognized page shows its adapter artifact's summary", async () => {
  // An adapter artifact with a summary and no measured activity yet.
  const summaryOnly = {
    ...artifact("doc:a", "Doc A"), first_seen: "2026-10-27", last_seen: "2026-10-27",
    focused_seconds: 0, background_seconds: 0, opens: 0, days_opened: 0,
  };
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact"
      ? { ...recognized(summaryOnly), summary: { ...SUMMARY, document_type: "report", importance: "low" } }
      : opensSummary(request),
  });
  await until(() => page.el("artifact-heading").textContent === "Doc A", "the artifact card");
  assert.equal(page.el("artifact-card").hidden, false);
  assert.equal(page.el("artifact-measures").textContent, "No activity measured yet");
  assert.equal(page.el("page-summary").hidden, false);
  assert.equal(page.el("page-summary-text").textContent, "report · technical specification · low importance");
  page.el("page-summary-open").click();
  await until(() => page.sent("open_summary").length === 1, "open_summary");
  assert.equal(page.sent("open_summary")[0].url, "https://docs.example/a");
});

test("a page without a summary hides the line", async () => {
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Article" },
    intercept: (request) => {
      if (request.type !== "get_artifact") return null;
      if (request.url.endsWith("/broken")) return { type: "error", code: "internal", message: "index busy" };
      return {
        type: "artifact_result", recognized: false, artifact: null,
        summary: request.url === ARTICLE_URL ? SUMMARY : null,
      };
    },
  });
  assert.equal(page.el("page-summary").hidden, true, "hidden before any lookup");
  await until(() => page.el("page-summary").hidden === false, "the summarized page's line");

  page.followLink("https://read.example/plain", "Plain");
  await until(() => page.sent("get_artifact").some((request) => request.url.endsWith("/plain")), "the next lookup");
  await until(() => page.el("page-summary").hidden === true, "the line to hide");
  assert.equal(page.el("page-summary-text").textContent, "");

  // A failed lookup for another page never shows the previous page's line.
  page.followLink(ARTICLE_URL, "Article");
  await until(() => page.el("page-summary").hidden === false, "the line again");
  page.followLink("https://read.example/broken", "Broken");
  await until(() => /unavailable: index busy/.test(page.el("artifact-status").textContent), "the lookup error");
  assert.equal(page.el("page-summary").hidden, true);
  page.el("page-summary-open").click();
  await settle();
  assert.equal(page.sent("open_summary").length, 0);
});

test("a summarized Recent row says so and renders agent text as text", async () => {
  const hostile = "<img src=https://evil.example/x.png>";
  const row = (title, url, summary) => ({
    local_date: "2026-10-27", artifact_id: `web:${title}`, artifact_type: "web.page", title, url,
    focused_seconds: 0, background_seconds: 0, opens: 0, why: null, summary,
  });
  const items = [
    row("<b>Spec</b>", "https://read.example/spec", { ...SUMMARY, document_type: hostile, importance: "medium" }),
    row("Plain page", "https://read.example/plain", null),
  ];
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Article" },
    intercept: (request) => request.type === "list_artifacts"
      ? { type: "artifact_list_result", items, artifact_types: ["web.page"], truncated: false }
      : opensSummary(request),
  });
  await noteLoaded(page);
  page.el("tab-recent").click();
  await until(() => page.el("recent-list").textContent.includes("Plain page"), "the Recent rows");

  const [summarized, plain] = page.el("recent-list").children.filter((child) => child.tagName !== "H3");
  assert.ok(summarized.textContent.includes("<b>Spec</b>"));
  assert.ok(summarized.textContent.includes(`Summarized · ${hostile}, medium importance`));
  assert.ok(!plain.textContent.includes("Summarized"));
  assert.equal(buttonIn(plain, "Open summary"), null);
  // Every piece of agent text is a text node, never markup.
  assert.ok(descendants(summarized).every((node) => !/^(IMG|B)$/.test(node.tagName)));

  // The button's accessible name says which row it opens.
  assert.equal(buttonIn(summarized, "Open summary").getAttribute("aria-label"), "Open summary: <b>Spec</b>");

  buttonIn(summarized, "Open summary").click();
  await until(() => page.sent("open_summary").length === 1, "open_summary");
  assert.equal(page.sent("open_summary")[0].url, "https://read.example/spec");
  // The row's own summary file, which the URL alone may no longer resolve to.
  assert.equal(page.sent("open_summary")[0].summary_path, SUMMARY.summary_path);
});

test("following a new tab clears the previous page's summary before its lookup returns", async () => {
  let release;
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Article" },
    intercept: (request) => {
      if (request.type !== "get_artifact") return opensSummary(request);
      if (request.url === ARTICLE_URL) return { type: "artifact_result", recognized: false, artifact: null, summary: SUMMARY };
      return new Promise((resolve) => {
        release = () => resolve({ type: "artifact_result", recognized: false, artifact: null, summary: null });
      });
    },
  });
  await until(() => page.el("page-summary").hidden === false, "the page's summary line");
  page.followLink("https://read.example/slow", "Slow");
  await until(() => page.sent("get_artifact").some((request) => request.url.endsWith("/slow")), "the next lookup");
  await settle();
  // The lookup is still out: the previous page's line is gone already.
  assert.equal(page.el("page-summary").hidden, true);
  assert.equal(page.el("page-summary-text").textContent, "");
  page.el("page-summary-open").click();
  await settle();
  assert.equal(page.sent("open_summary").length, 0);
  release();
  await settle();
  assert.equal(page.el("page-summary").hidden, true);
});

test("Open summary buttons are disabled while the host is disconnected", async () => {
  const items = [{
    local_date: "2026-10-27", artifact_id: "web:spec", artifact_type: "web.page", title: "Spec",
    url: "https://read.example/spec", focused_seconds: 0, background_seconds: 0, opens: 0, why: null,
    summary: SUMMARY,
  }];
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Article" },
    intercept: (request) => {
      if (request.type === "get_artifact") return { type: "artifact_result", recognized: false, artifact: null, summary: SUMMARY };
      if (request.type === "list_artifacts") return { type: "artifact_list_result", items, artifact_types: ["web.page"], truncated: false };
      return opensSummary(request);
    },
  });
  await until(() => page.el("page-summary").hidden === false, "the page's summary line");
  await noteLoaded(page);
  page.el("tab-recent").click();
  await until(() => page.el("recent-list").textContent.includes("Spec"), "the Recent row");
  const recentOpen = () => buttonIn(page.el("recent-list"), "Open summary");
  assert.equal(page.el("page-summary-open").disabled, false);
  assert.equal(recentOpen().disabled, false);

  page.hostExits();
  page.state.hostDead = true;
  page.titleChanged("Article (1)");
  await settle();
  page.titleChanged("Article (2)");
  await until(() => /unavailable/i.test(page.el("artifact-status").textContent), "the host-unavailable status");
  assert.equal(page.el("page-summary-open").disabled, true);
  assert.equal(recentOpen().disabled, true);

  // The host is back; the next page's calls reach it.
  page.state.hostDead = false;
  page.followLink("https://read.example/next", "Next");
  await until(() => page.el("page-summary-open").disabled === false, "Open summary to enable again");
  assert.equal(recentOpen().disabled, false);
});
