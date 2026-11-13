// Replays a recorded Mergify timeline (PR #384, fixtures/mergify-timeline-384.json) through
// pr-watch, queue-pr and the PR keeper: every poll-time snapshot, every two minutes and at
// every recorded event, built from Mergify's comment edit history. #384 sat dequeued for
// 6h43m, then waited an hour behind a stale needs-merge label (AGENTS.md "Keep watching
// until it merges").
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { EXIT as WATCH, decide, track } from "../dev/pr-watch.mjs";
import { planQueue, snapshotFromGraphql } from "../dev/queue-pr.mjs";
import { decideKeeper, repairsToFile } from "../pr-keeper.mjs";
import { done, run } from "./pr-fixtures.mjs";

const TL = JSON.parse(readFileSync(new URL("./fixtures/mergify-timeline-384.json", import.meta.url), "utf8"));
const ms = (t) => Date.parse(t);
const iso = (n) => new Date(n).toISOString().replace(".000Z", "Z");
const latest = (items, t, key = "at") => items.filter((i) => ms(i[key]) <= ms(t)).at(-1) ?? null;

function inConflict(t) {
  return TL.conflict.some((w) => ms(w.from) <= ms(t) && ms(t) < ms(w.to));
}

/** The PR as one poll at time `t` would have read it, in queue-pr's GraphQL shape. */
function snapshotAt(t, { extraComments = [], removedLabels = [] } = {}) {
  const head = latest(TL.heads, t);
  const labels = new Set();
  for (const e of TL.labels) if (ms(e.at) <= ms(t)) e.op === "add" ? labels.add(e.label) : labels.delete(e.label);
  for (const { label, at } of removedLabels) if (ms(at) <= ms(t)) labels.delete(label);
  const comments = TL.comments.filter((c) => ms(c.createdAt) <= ms(t)).map((c) => {
    const v = latest(c.versions, t) ?? c.versions[0];
    return { author: { login: c.author }, createdAt: c.createdAt, updatedAt: v.at, body: v.body };
  }).concat(extraComments.filter((c) => ms(c.createdAt) <= ms(t)).map((c) => ({ author: { login: c.author }, createdAt: c.createdAt, updatedAt: c.createdAt, body: c.body })));
  const green = head.greenAt && ms(head.greenAt) <= ms(t);
  const opts = (more) => ({ completedAt: head.greenAt, startedAt: head.at, ...more });
  const contexts = green
    ? [done("PR owner", "SUCCESS", opts({ required: true })), done("Rust dependency policy", "SUCCESS", opts({ required: true })),
      done("Windows / Build and check (windows-latest)", "SUCCESS", opts()), done("Build and check", "SUCCESS", opts({ required: true }))]
    : [done("PR owner", "SUCCESS", { required: true, completedAt: head.at }), run("Rust dependency policy", "IN_PROGRESS", null, { required: true }),
      run("Windows / Build and check (windows-latest)", "IN_PROGRESS", null), run("Build and check", "QUEUED", null, { required: true })];
  const conflicting = inConflict(t);
  return snapshotFromGraphql({ repository: { pullRequest: {
    number: TL.number, state: ms(t) >= ms(TL.merged) ? "MERGED" : "OPEN", isDraft: false,
    mergeable: conflicting ? "CONFLICTING" : "MERGEABLE", mergeStateStatus: conflicting ? "DIRTY" : "CLEAN",
    headRefOid: head.oid, headRefName: "feat/example", baseRefName: "main", isCrossRepository: false, autoMergeRequest: null,
    labels: { nodes: [...labels].map((name) => ({ name })) }, comments: { nodes: comments },
    commits: { nodes: [{ commit: { statusCheckRollup: { contexts: { nodes: contexts } } } }] },
  } } });
}

/** Every two minutes from the first commit to the merge, plus one second after every event. */
function pollTimes() {
  const start = ms(TL.heads[0].at), end = ms(TL.merged) + 60000;
  const times = new Set();
  for (let t = start; t <= end; t += 120000) times.add(t);
  const events = [...TL.heads.map((h) => h.at), ...TL.labels.map((l) => l.at), ...TL.comments.flatMap((c) => c.versions.map((v) => v.at)), ...TL.conflict.flatMap((w) => [w.from, w.to])];
  for (const e of events) times.add(ms(e) + 1000);
  return [...times].filter((t) => t >= start && t <= end).sort((a, b) => a - b).map(iso);
}
const POLLS = pollTimes();

/** pr-watch's decisions over the timeline, with its own between-poll memory. */
function watchReplay() {
  const state = { dirtySince: null, idleSince: null };
  return POLLS.map((t) => {
    const snap = snapshotAt(t);
    track(state, snap, t);
    return { t, snap, d: decide(snap, { now: t, dirtySince: state.dirtySince, idleSince: state.idleSince }) };
  });
}
const firstAt = (rows, pred) => rows.find(pred)?.t ?? null;

test("replay: the fixture is synthetic (no real logins or links)", () => {
  const text = JSON.stringify(TL);
  assert.doesNotMatch(text, /jrausercisco|rausified|issuecomment-\d/);
  assert.ok(POLLS.length > 250, `${POLLS.length} polls`);
});

test("replay pr-watch: each phase of #384 gets the right exit", () => {
  const rows = watchReplay();
  const at = (t) => rows.filter((r) => r.t <= t).at(-1).d;
  // Labelled at 20:44, after its checks went green at 20:58 it is queued and checking.
  assert.match(at("2026-11-09T20:44:10Z").action, /^unqueued:/);
  assert.match(at("2026-11-09T21:00:00Z").action, /^checking:/);
  // Dequeued for a conflict pr-doctor labelled needs-merge: the session must merge main (exit 11).
  const conflict = at("2026-11-09T22:00:00Z");
  assert.equal(conflict.exitCode, WATCH.CONFLICT);
  assert.equal(conflict.action, "conflict-labeled");
  assert.match(conflict.message, /extension\/panel\.ts/);
  assert.equal(at("2026-11-10T03:50:00Z").action, "conflict-labeled");
  // Fixed at 03:52, but the label stayed: exit 16 naming it, until a person removed it at 05:02.
  for (const t of ["2026-11-10T03:55:00Z", "2026-11-10T04:30:00Z", "2026-11-10T05:02:00Z"]) {
    const d = at(t);
    assert.equal(d.exitCode, WATCH.QUEUE_BLOCKED, `${t}: ${d.message}`);
    assert.match(d.message, /gh pr edit 384 --remove-label needs-merge/);
  }
  assert.match(at("2026-11-10T05:04:00Z").action, /^checking:/);
  assert.equal(rows.at(-1).d.exitCode, WATCH.DONE);
});

test("replay queue-pr: never a second queue comment while Mergify holds a request", () => {
  for (const t of POLLS) {
    const snap = snapshotAt(t);
    const plan = planQueue(snap, { now: t });
    const kinds = plan.actions.map((a) => a.kind);
    if (["requested", "waiting", "checking", "merged"].includes(snap.mergify.state)) assert.ok(!kinds.includes("queue"), `${t}: ${plan.message}`);
    // While the PR conflicts, queue-pr posts nothing.
    if (inConflict(t)) assert.deepEqual(kinds, [], `${t}: ${plan.message}`);
  }
  // It would have queued in the two gaps: before the session's first queue comment, and
  // after the 03:52 push, while the dequeue on the old head was still Mergify's last word.
  const wants = POLLS.filter((t) => planQueue(snapshotAt(t), { now: t }).actions.some((a) => a.kind === "queue"));
  assert.ok(wants.every((t) => t < "2026-11-09T20:44:19Z" || (t > "2026-11-10T03:52:43Z" && t < "2026-11-10T04:00:44Z")), wants.join(" "));
  assert.ok(wants.some((t) => t > "2026-11-10T03:52:43Z"));
  assert.match(planQueue(snapshotAt("2026-11-10T03:55:00Z"), { now: "2026-11-10T03:55:00Z" }).message, /left the queue on 1a63e38.*head moved/);
});

test("replay keeper: one repair bead per head while it conflicts, and needs-merge cleared at the first poll after the fix", () => {
  let removedAt = null;
  const repairs = new Set(), beads = [];
  for (const t of POLLS) {
    const snap = snapshotAt(t, { removedLabels: removedAt ? [{ label: "needs-merge", at: removedAt }] : [] });
    const d = decideKeeper(snap, { now: t, mergeTreeClean: !inConflict(t), trusted: ["doctor-bot"] });
    for (const a of d.actions) {
      if (a.kind === "repair") {
        repairs.add(a.ref);
        for (const filed of repairsToFile([{ number: 384, ...a }], beads)) beads.push({ external_ref: filed.ref, status: "open" });
      }
      if (a.kind === "unlabel") { assert.equal(removedAt, null, "removed once"); removedAt = t; }
      if (a.kind === "queue") assert.ok(!["requested", "waiting", "checking", "merged"].includes(snap.mergify.state), t);
    }
    if (inConflict(t) && snap.labels.includes("needs-merge")) assert.deepEqual(d.alerts.map((a) => a.kind), ["needs-person"], t);
  }
  // The 03:52:15 push was mid-fix (still conflicting): a new head, but no second bead.
  assert.deepEqual([...repairs], ["gh-pr-384-1a63e38ae071", "gh-pr-384-37ef5ec97a3b"]);
  assert.deepEqual(beads.map((b) => b.external_ref), ["gh-pr-384-1a63e38ae071"]);
  assert.ok(removedAt > "2026-11-10T03:52:43Z" && removedAt <= "2026-11-10T03:54:44Z", removedAt);
  // A person removed it at 05:02:46: the keeper saves about 68 minutes of this delay.
  assert.ok((ms("2026-11-10T05:02:46Z") - ms(removedAt)) / 60000 > 60);
});
