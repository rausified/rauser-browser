import assert from "node:assert/strict";
import test from "node:test";

import {
  MARKER, classifyPr, commentBody, resolveAppendOnly, stripGenerated,
} from "../pr-doctor.mjs";

// --- resolveAppendOnly: diff3 hunks whose base is empty ----------------------

const hunk = (ours, base, theirs) =>
  `<<<<<<< ours\n${ours}||||||| base\n${base}=======\n${theirs}>>>>>>> theirs\n`;

test("both sides appending at the same point keeps both, main's (theirs) first", () => {
  const text = "# Log\n\n" + hunk("## B from the PR\n", "", "## A from main\n") + "tail\n";
  const result = resolveAppendOnly(text);
  assert.equal(result.resolved, true);
  assert.equal(result.text, "# Log\n\n## A from main\n\n## B from the PR\ntail\n");
});

test("list-item appends are joined without inserting a blank line", () => {
  const text = hunk("- +1 PR evidence\n", "", "- +1 main evidence\n");
  assert.equal(resolveAppendOnly(text).text, "- +1 main evidence\n- +1 PR evidence\n");
});

test("a hunk where either side changed base text is left for a person", () => {
  const text = "a\n" + hunk("changed by PR\n", "original\n", "changed by main\n") + "z\n";
  const result = resolveAppendOnly(text);
  assert.equal(result.resolved, false);
  assert.equal(result.remaining, 1);
  assert.equal(result.text, text, "an unresolvable file is returned unchanged");
});

test("one unresolvable hunk keeps the whole file unresolved", () => {
  const text = hunk("x\n", "", "y\n") + hunk("p\n", "q\n", "r\n");
  const result = resolveAppendOnly(text);
  assert.equal(result.resolved, false);
  assert.equal(result.remaining, 1);
});

test("two-way markers without a base section are never resolved", () => {
  const text = "<<<<<<< ours\nx\n=======\ny\n>>>>>>> theirs\n";
  assert.equal(resolveAppendOnly(text).resolved, false);
});

test("text with no markers is already resolved and unchanged", () => {
  assert.deepEqual(resolveAppendOnly("plain\n"), { resolved: true, remaining: 0, text: "plain\n" });
});

test("a truncated hunk is not resolved", () => {
  assert.equal(resolveAppendOnly("<<<<<<< ours\nx\n||||||| base\n=======\ny\n").resolved, false);
});

// --- stripGenerated: the backlog's generated blocks are emptied --------------

test("generated backlog blocks are emptied, markers and prose kept", () => {
  const text = [
    "intro",
    "<!-- BEGIN GENERATED section=plan: edit the beads, then run python3 scripts/render_backlog.py -->",
    "- generated line",
    "<!-- END GENERATED section=plan -->",
    "prose",
    "",
  ].join("\n");
  assert.equal(stripGenerated(text), [
    "intro",
    "<!-- BEGIN GENERATED section=plan: edit the beads, then run python3 scripts/render_backlog.py -->",
    "<!-- END GENERATED section=plan -->",
    "prose",
    "",
  ].join("\n"));
});

// --- classifyPr --------------------------------------------------------------

const pr = (over = {}) => ({
  number: 1, baseRefName: "main", isDraft: false, isCrossRepository: false,
  mergeable: "MERGEABLE", labels: [], ...over,
});

test("a conflicting same-repo PR into main is a candidate", () => {
  assert.equal(classifyPr(pr({ mergeable: "CONFLICTING" })), "conflicting");
});

test("drafts, forks, other bases and opted-out PRs are skipped", () => {
  assert.equal(classifyPr(pr({ mergeable: "CONFLICTING", isDraft: true })), "skip");
  assert.equal(classifyPr(pr({ mergeable: "CONFLICTING", isCrossRepository: true })), "skip");
  assert.equal(classifyPr(pr({ mergeable: "CONFLICTING", baseRefName: "release" })), "skip");
  assert.equal(classifyPr(pr({ mergeable: "CONFLICTING", labels: [{ name: "pr-doctor:skip" }] })), "skip");
});

test("unknown mergeability is pending and a mergeable PR is clean", () => {
  assert.equal(classifyPr(pr({ mergeable: "UNKNOWN" })), "pending");
  assert.equal(classifyPr(pr()), "clean");
});

// --- commentBody -------------------------------------------------------------

test("every comment carries the sticky marker", () => {
  for (const outcome of ["repaired", "needs-person", "report-only", "resolved"]) {
    assert.ok(commentBody({ outcome, files: ["a.md"], resolvable: true, sha: "abc1234" }).startsWith(MARKER));
  }
});

test("a needs-person comment lists the files and says the queued PR is waiting", () => {
  const body = commentBody({ outcome: "needs-person", files: ["host/src/x.rs", "DESIGN.md"], queued: true });
  assert.match(body, /host\/src\/x\.rs/);
  assert.match(body, /DESIGN\.md/);
  assert.match(body, /labeled `queue` and waits/);
  assert.doesNotMatch(body, /@\w/, "the comment never mentions anyone");
});

test("a repaired comment names the merge commit and the auto-resolved files", () => {
  const body = commentBody({ outcome: "repaired", files: ["docs/history.md"], sha: "abc1234" });
  assert.match(body, /abc1234/);
  assert.match(body, /docs\/history\.md/);
});

// --- Fail-closed edge cases (review of #258) ---------------------------------

test("CRLF marker lines are never half-resolved: the file stays unresolved and unchanged", () => {
  const text = "<<<<<<< ours\r\nx\r\n||||||| base\r\n=======\r\ny\r\n>>>>>>> theirs\r\n";
  const result = resolveAppendOnly(text);
  if (result.resolved) {
    assert.doesNotMatch(result.text, /^(<{7}|={7}|>{7}|\|{7})/m, "a resolution never keeps markers");
  } else {
    assert.equal(result.text, text);
  }
});

test("a final hunk with no trailing newline still resolves both sides", () => {
  const text = "<<<<<<< ours\nB\n||||||| base\n=======\nA\n>>>>>>> theirs";
  const result = resolveAppendOnly(text);
  assert.equal(result.resolved, true);
  assert.equal(result.text, "A\nB\n");
});

test("marker-looking text inside a side leaves the hunk to a person", () => {
  const nested = "<<<<<<< ours\n<<<<<<< not really\n||||||| base\n=======\ny\n>>>>>>> theirs\n";
  assert.equal(resolveAppendOnly(nested).resolved, false);
  // A Markdown setext heading underline inside a side looks like a separator.
  const setext = "<<<<<<< ours\nTitle\n||||||| base\n=======\nOther\n=======\n>>>>>>> theirs\n";
  assert.equal(resolveAppendOnly(setext).resolved, false);
});
