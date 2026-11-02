import assert from "node:assert/strict";
import test from "node:test";

// Run after build:extension.
const {
  artifactLook, capturePill, compactDuration, displayUrl, importanceLabel, importanceTone, pageLook, summaryTimeLabel,
} = await import("../dist/panel-look.js");

test("known artifact types have fixed glyphs, hues and labels", () => {
  assert.deepEqual(artifactLook("github.pr"), { glyph: "PR", hue: "cyan", label: "Pull request" });
  assert.deepEqual(artifactLook("jira.issue"), { glyph: "JI", hue: "indigo", label: "Jira issue" });
  assert.deepEqual(artifactLook("web.page"), { glyph: "W", hue: "neutral", label: "Web page" });
});

test("unknown artifact types derive a stable look from the type", () => {
  const look = artifactLook("acme.design_doc");
  assert.equal(look.glyph, "AC");
  assert.equal(look.label, "Design doc");
  assert.deepEqual(artifactLook("acme.design_doc"), look, "the hue is stable");
  assert.ok(["cyan", "indigo", "amber", "violet", "green"].includes(look.hue));
  assert.equal(artifactLook("...").glyph, "?");
});

test("an unrecognized page's look comes from its host", () => {
  assert.deepEqual(pageLook("https://www.example.dev/post"), { glyph: "E", hue: "neutral", label: "Web page" });
  assert.equal(pageLook("not a url").glyph, "·");
  assert.equal(pageLook(null).glyph, "·");
});

test("addresses drop their scheme only", () => {
  assert.equal(displayUrl("https://github.com/a/b/pull/1"), "github.com/a/b/pull/1");
  assert.equal(displayUrl("HTTP://x.dev/?q=1"), "x.dev/?q=1");
  assert.equal(displayUrl("ftp://x"), "ftp://x");
});

test("importance chips", () => {
  assert.equal(importanceTone("high"), "high");
  assert.equal(importanceTone("medium"), "plain");
  assert.equal(importanceLabel("low"), "Low importance");
});

test("summary time labels: today, this week, earlier, unreadable", () => {
  const now = new Date(2026, 10, 2, 12, 0); // Mon 2 Nov 2026, local
  assert.equal(summaryTimeLabel(new Date(2026, 10, 2, 9, 14).toISOString(), now), "09:14");
  assert.equal(summaryTimeLabel(new Date(2026, 9, 30, 8, 5).toISOString(), now), "Fri 08:05");
  assert.equal(summaryTimeLabel(new Date(2026, 9, 20, 8, 5).toISOString(), now), "2026-10-20");
  assert.equal(summaryTimeLabel("someday", now), "someday");
});

test("compact durations", () => {
  assert.equal(compactDuration(0), "0m");
  assert.equal(compactDuration(30), "<1m");
  assert.equal(compactDuration(48 * 60), "48m");
  assert.equal(compactDuration(72 * 60), "1h 12m");
  assert.equal(compactDuration(120 * 60), "2h");
});

test("the capture pill reports global capture, a stuck backlog first", () => {
  const base = { captureOn: true, queued: 0, deliveryProblem: false };
  assert.equal(capturePill(base).tone, "on");
  assert.equal(capturePill(base).label, "Capture on");
  assert.equal(capturePill({ ...base, captureOn: false }).label, "Capture off");
  assert.equal(capturePill({ ...base, queued: 2 }).tone, "on", "a healthy backlog is not stuck");
  const stuck = capturePill({ ...base, queued: 6, deliveryProblem: true });
  assert.deepEqual([stuck.tone, stuck.label], ["pending", "6 pending"]);
  assert.equal(capturePill({ ...base, queued: 1, deliveryProblem: true }).detail, "1 pending capture record waiting to be sent");
});
