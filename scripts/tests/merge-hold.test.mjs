import assert from "node:assert/strict";
import test from "node:test";

import { HELD_PREFIXES, STATUS_CONTEXT, decideStatuses } from "../merge-hold.mjs";

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

test("the final-stretch PR itself and other sessions' PRs are never held", () => {
  const statuses = decideStatuses([pr(5, "feat/big", ["final-stretch"]), pr(7, "fix/other")]);
  assert.deepEqual(statuses.map((s) => [s.sha, s.state]), [["sha5", "success"], ["sha7", "success"]]);
});

test("a Cookie PR that is itself in its final stretch is not held by its own label", () => {
  const statuses = decideStatuses([pr(8, "cookie/cycle-9", ["final-stretch"])]);
  assert.equal(statuses[0].state, "success");
});

test("several final-stretch PRs are all named, and a Cookie PR is held until all are done", () => {
  const statuses = decideStatuses([
    pr(5, "feat/a", ["final-stretch"]), pr(9, "feat/b", ["final-stretch"]), pr(6, "cookie/cycle-1"),
  ]);
  const cookie = statuses.find((s) => s.sha === "sha6");
  assert.equal(cookie.state, "pending");
  assert.match(cookie.description, /#5/);
  assert.match(cookie.description, /#9/);
  assert.ok(cookie.description.length <= 140, "GitHub caps status descriptions at 140 characters");
});

test("Cookie's branches are the held prefix", () => {
  assert.deepEqual(HELD_PREFIXES, ["cookie/"]);
});
