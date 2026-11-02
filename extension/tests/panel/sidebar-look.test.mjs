// Built side-panel tests: the redesigned page card, header capture pill and
// capture strip action (DESIGN.md §5.7). Run after build:extension; the fake
// DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  ARTICLE_URL, CAPTURING, CONFIG, artifact, install, noteLoaded, recognized, settle, typeWhy, until,
} from "./harness.mjs";

const PR_URL = "https://github.com/acme-corp/widgets/pull/7";

test("a recognized page shows its type's tile, kind chip and short measures", async () => {
  const page = await install({
    tab: { id: 1, url: PR_URL, title: "Fix the thing" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact"
      ? recognized({ ...artifact("pr:7", "Fix the thing"), artifact_type: "github.pr", focused_seconds: 42 * 60, opens: 3 })
      : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  assert.equal(page.el("page-title").textContent, "Fix the thing");
  assert.equal(page.el("page-url").textContent, PR_URL);
  assert.equal(page.el("page-glyph").textContent, "PR");
  assert.equal(page.el("page-glyph").dataset.hue, "cyan");
  assert.equal(page.el("artifact-kind-label").textContent, "Pull request");
  assert.equal(page.el("artifact-measures-short").textContent, "42m focused · 3 opens");
  assert.equal(page.el("artifact-details").hidden, false, "the details row comes with the card");
  assert.equal(page.el("summarize").textContent, "Summarize & log");
});

test("an unrecognized page offers Save Page through the same Summarize & log button", async () => {
  const page = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Article" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact"
      ? { type: "artifact_result", recognized: false, artifact: null, summary: null }
      : undefined,
  });
  await until(() => page.el("artifact-status").textContent === "○ Not an artifact", "the verdict");
  assert.equal(page.el("artifact-card").hidden, true);
  assert.equal(page.el("artifact-details").hidden, true);
  assert.equal(page.el("summarize").textContent, "Save Page");
  assert.equal(page.el("page-glyph").dataset.hue, "neutral");
});

test("Enter in Why I'm here saves it, and a composing Enter does not", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(artifact("doc:a", "Doc A")) : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  await noteLoaded(page);
  await settle();
  typeWhy(page, "reviewing for launch");
  let prevented = 0;
  page.el("artifact-why").dispatch("keydown", { key: "Enter", isComposing: true, preventDefault: () => prevented++ });
  await settle();
  assert.equal(page.sent("record_why").length, 0);
  page.el("artifact-why").dispatch("keydown", { key: "Enter", isComposing: false, preventDefault: () => prevented++ });
  await until(() => page.sent("record_why").length === 1, "record_why");
  assert.equal(prevented, 1);
});

test("the header pill and strip action follow capture state", async () => {
  const on = await install({ tab: { id: 1, url: ARTICLE_URL, title: "Article" }, config: CAPTURING });
  await until(() => on.el("capture-pill-label").textContent === "Capture on", "the on pill");
  assert.equal(on.el("capture-pill").dataset.tone, "on");
  assert.equal(on.el("capture-strip-action").hidden, true, "nothing to do with an empty buffer");

  const stuck = await install({
    tab: { id: 1, url: ARTICLE_URL, title: "Article" }, config: CAPTURING,
    workerStatus: { queued: 6, retry_error: "host unreachable" },
  });
  await until(() => stuck.el("capture-pill-label").textContent === "6 pending", "the pending pill");
  assert.equal(stuck.el("capture-pill").dataset.tone, "pending");
  assert.equal(stuck.el("capture-strip-action").hidden, false);
  assert.equal(stuck.el("capture-strip-action").textContent, "Send now");

  const paused = await install({ tab: { id: 1, url: ARTICLE_URL, title: "Article" }, config: CONFIG });
  await until(() => paused.el("capture-strip-action").textContent === "Resume in settings", "the resume action");
  assert.equal(paused.el("capture-pill-label").textContent, "Capture off");
  assert.ok(paused.el("capture-strip-summary").textContent.includes("Capture off"),
    "the strip keeps its Capture on/off wording for the smoke");
});
