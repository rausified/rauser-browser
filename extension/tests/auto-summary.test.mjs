import assert from "node:assert/strict";
import test from "node:test";

const {
  MAX_SUBMITTED, dropCandidate, dueTabs, emptyAutoSummaryState, markSubmitted, nextDueInMs,
  noteArtifact, noteAttention, noteClosed, noteLoaded,
} = await import("../dist/auto-summary.js");

const doc = (n) => ({ artifact_id: `gdoc:docs.google.com/${n}`, url: `https://docs.google.com/d/${n}`, title: `Doc ${n}` });
const SEC = 1_000;

test("dwellZeroIsDueOnLoadEvenInBackground", () => {
  const state = emptyAutoSummaryState();
  noteArtifact(state, 1, doc("a"), 0);
  assert.deepEqual(dueTabs(state, 0, 0), [], "not loaded yet");
  noteLoaded(state, 1);
  assert.deepEqual(dueTabs(state, 0, 0), [1]);
});

test("dwellCountsOnlyFocusedTime", () => {
  const state = emptyAutoSummaryState();
  noteArtifact(state, 1, doc("a"), 0);
  noteLoaded(state, 1);
  // 30 s in the background, then 20 s focused: 20 s of 45.
  noteAttention(state, 2, 0);
  noteAttention(state, 1, 30 * SEC);
  assert.deepEqual(dueTabs(state, 50 * SEC, 45 * SEC), []);
  assert.equal(nextDueInMs(state, 50 * SEC, 45 * SEC), 25 * SEC);
  // Focus moves away and back: only focused stretches count.
  noteAttention(state, null, 50 * SEC);
  assert.deepEqual(dueTabs(state, 500 * SEC, 45 * SEC), []);
  assert.equal(nextDueInMs(state, 500 * SEC, 45 * SEC), null, "nothing counts while unfocused");
  noteAttention(state, 1, 600 * SEC);
  assert.deepEqual(dueTabs(state, 625 * SEC, 45 * SEC), [1]);
});

test("navigatingToAnotherArtifactResetsTheCandidate", () => {
  const state = emptyAutoSummaryState();
  noteArtifact(state, 1, doc("a"), 0);
  noteLoaded(state, 1);
  noteAttention(state, 1, 0);
  noteArtifact(state, 1, doc("a"), 5 * SEC);
  assert.equal(state.candidates["1"].loaded, true, "the same artifact is a no-op");
  noteArtifact(state, 1, doc("b"), 10 * SEC);
  assert.equal(state.candidates["1"].artifact_id, doc("b").artifact_id);
  assert.equal(state.candidates["1"].loaded, false);
  assert.equal(state.candidates["1"].focused_ms, 0);
  noteArtifact(state, 1, null, 20 * SEC);
  assert.equal(state.candidates["1"], undefined);
});

test("twoTabsOneArtifactSubmitOnce", () => {
  const state = emptyAutoSummaryState();
  for (const tab of [1, 2]) {
    noteArtifact(state, tab, doc("a"), tab);
    noteLoaded(state, tab);
  }
  assert.deepEqual(dueTabs(state, 10, 0), [1, 2]);
  markSubmitted(state, 1);
  assert.deepEqual(dueTabs(state, 10, 0), [], "the other tab's candidate for it goes too");
  assert.deepEqual(state.submitted, [doc("a").artifact_id]);
  noteArtifact(state, 3, doc("a"), 20);
  assert.equal(state.candidates["3"], undefined, "a submitted artifact makes no candidate");
});

test("submittedIsCappedOldestFirst", () => {
  const state = emptyAutoSummaryState();
  for (let n = 0; n < MAX_SUBMITTED + 3; n++) {
    noteArtifact(state, 1, doc(n), n);
    markSubmitted(state, 1);
  }
  assert.equal(MAX_SUBMITTED, 500);
  assert.equal(state.submitted.length, MAX_SUBMITTED);
  assert.equal(state.submitted[0], doc(3).artifact_id);
  assert.equal(state.submitted.at(-1), doc(MAX_SUBMITTED + 2).artifact_id);
});

test("notSetUpLeavesTheArtifactEligible", () => {
  const state = emptyAutoSummaryState();
  noteArtifact(state, 1, doc("a"), 0);
  noteLoaded(state, 1);
  dropCandidate(state, 1);
  assert.deepEqual(dueTabs(state, 1, 0), []);
  assert.deepEqual(state.submitted, []);
  noteArtifact(state, 1, doc("a"), 2);
  assert.equal(state.candidates["1"].artifact_id, doc("a").artifact_id);
});

test("dueTabsIsFifo", () => {
  const state = emptyAutoSummaryState();
  noteArtifact(state, 7, doc("late"), 30);
  noteArtifact(state, 3, doc("early"), 10);
  noteArtifact(state, 5, doc("middle"), 20);
  for (const tab of [7, 3, 5]) noteLoaded(state, tab);
  assert.deepEqual(dueTabs(state, 40, 0), [3, 5, 7]);
});

test("nextDueInMsGivesTheShortestRemainingDwell", () => {
  const state = emptyAutoSummaryState();
  noteArtifact(state, 1, doc("a"), 0);
  noteArtifact(state, 2, doc("b"), 0);
  noteLoaded(state, 1);
  noteLoaded(state, 2);
  noteAttention(state, 1, 0);
  noteAttention(state, 2, 10 * SEC);
  noteAttention(state, 1, 20 * SEC);
  // Tab 1 has 10 s + running; tab 2 has 10 s and is not focused.
  assert.equal(nextDueInMs(state, 25 * SEC, 30 * SEC), 15 * SEC);
  assert.equal(nextDueInMs(emptyAutoSummaryState(), 0, 30 * SEC), null);
  assert.equal(nextDueInMs(state, 25 * SEC, 0), 0, "dwell 0: due now");
});

test("closing a tab drops its candidate", () => {
  const state = emptyAutoSummaryState();
  noteArtifact(state, 1, doc("a"), 0);
  noteClosed(state, 1);
  assert.deepEqual(state.candidates, {});
});
