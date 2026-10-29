import assert from "node:assert/strict";
import test from "node:test";

import { STATUS_CONTEXT, decideStatuses, mergeGroupStatus, parseArgs } from "../merge-hold.mjs";

function pr(number, head, labels = [], sha = `sha${number}`) {
  return { number, head: { ref: head, sha }, labels: labels.map((name) => ({ name })) };
}

test("with no final-stretch PR every open PR passes merge-hold", () => {
  const statuses = decideStatuses([pr(1, "cookie/cycle-1"), pr(2, "feat/x")]);
  assert.deepEqual(statuses.map((s) => [s.sha, s.state]), [["sha1", "success"], ["sha2", "success"]]);
  assert.ok(statuses.every((s) => s.context === STATUS_CONTEXT));
});

test("a final-stretch PR holds Cookie's PRs as pending and names itself", () => {
  const statuses = decideStatuses([pr(5, "feat/big", ["final-stretch"]), pr(6, "cookie/cookie-overnight/cycle-3-abc")]);
  const cookie = statuses.find((s) => s.sha === "sha6");
  assert.equal(cookie.state, "pending");
  assert.match(cookie.description, /#5/);
});

test("every other session's PR is held too, and the labeled PR passes", () => {
  const statuses = decideStatuses([pr(5, "feat/big", ["final-stretch"]), pr(7, "fix/other")]);
  assert.deepEqual(statuses.map((s) => [s.sha, s.state]), [["sha5", "success"], ["sha7", "pending"]]);
});

test("a Cookie PR that is itself in its final stretch is not held by its own label", () => {
  const statuses = decideStatuses([pr(8, "cookie/cycle-9", ["final-stretch"])]);
  assert.equal(statuses[0].state, "success");
});

test("several final-stretch PRs all pass, so two final stretches never hold each other", () => {
  const statuses = decideStatuses([
    pr(5, "feat/a", ["final-stretch"]), pr(9, "feat/b", ["final-stretch"]), pr(6, "cookie/cycle-1"),
  ]);
  assert.deepEqual(statuses.map((s) => [s.sha, s.state]), [["sha5", "success"], ["sha9", "success"], ["sha6", "pending"]]);
  const held = statuses.find((s) => s.sha === "sha6");
  assert.match(held.description, /#5/);
  assert.match(held.description, /#9/);
  assert.ok(held.description.length <= 140, "GitHub caps status descriptions at 140 characters");
});

const QUEUE_SHA = "0123456789abcdef0123456789abcdef01234567";

test("a merge queue entry passes merge-hold on its merge_group commit, since it passed the hold to be enqueued", () => {
  const status = mergeGroupStatus(QUEUE_SHA);
  assert.equal(status.sha, QUEUE_SHA);
  assert.equal(status.state, "success");
  assert.equal(status.context, STATUS_CONTEXT);
  assert.ok(status.description.length > 0 && status.description.length <= 140);
});

test("the merge_group status refuses anything but a full commit SHA", () => {
  for (const sha of ["", "abc123", "refs/heads/main", `${QUEUE_SHA}0`, QUEUE_SHA.toUpperCase().replace(/./, "g")]) {
    assert.throws(() => mergeGroupStatus(sha), /full commit SHA/, JSON.stringify(sha));
  }
});

test("--merge-group <sha> selects the merge_group path; no arguments keeps the open-PR sweep", () => {
  assert.deepEqual(parseArgs(["--merge-group", QUEUE_SHA]), { mergeGroupSha: QUEUE_SHA });
  assert.deepEqual(parseArgs([]), { mergeGroupSha: null });
  assert.throws(() => parseArgs(["--merge-group"]), /--merge-group needs a commit SHA/);
  assert.throws(() => parseArgs(["--sweep"]), /unknown argument/);
});
