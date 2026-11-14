## 2026-11-14 — Prove the merge queue waits for its draft, from Mergify's own summary

**What to change:** after any change to repository settings, ownership, visibility or branch protection, and whenever a rule about what the merge queue tests is written down (AGENTS.md "Review and merge cadence", `.mergify.yml` comments), check one real batch: the draft's `Required conditions of queue rule default for merge` block, and the draft's open and close times against its CI run.
Keep `scripts/tests/mergify-gate.test.mjs`, which fails when the queue rule has no `merge_conditions`.

**Why:** branch protection had been injecting the required checks into Mergify's merge conditions, so drafts waited for CI (last seen on draft #455, 2026-11-13 11:29Z).
The move to a private user-owned repository lost protection, and from draft #464 (2026-11-14 04:11Z) every draft read "No extra conditions" and closed within about a minute: nine drafts, #464 to #485, plus #473's direct merge, merged on the PRs' own checks alone.
The docs still said the queue tested every PR on its draft.
Nobody noticed until a lane read draft #483's body while planning a CI change (brwr-3zqd).
#483's red CI after it closed turned out to be a 0-step billing refusal, so no broken code is known to have landed, but nothing would have stopped it.

**Where:** `.mergify.yml` `merge_conditions`; `gh pr view <draft> --json createdAt,closedAt,body`; the `Mergify Merge Queue` check run on a merged PR's head.
