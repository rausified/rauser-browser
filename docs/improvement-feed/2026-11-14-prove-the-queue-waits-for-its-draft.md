## 2026-11-14 — Prove the merge queue waits for its draft, from Mergify's own summary

**What to change:** when a rule about what the merge queue tests is written down (AGENTS.md "Review and merge cadence", `.mergify.yml` comments), check it against one real merged PR: its `Mergify Merge Queue` check run and its draft's `Required conditions of queue rule default for merge` block, and the draft's CI run times against the draft's close time.
Keep `scripts/tests/ci-usage.test.mjs`'s check that the queue rule has `merge_conditions` (two-step CI) and cannot check a PR in place.

**Why:** AGENTS.md and `.mergify.yml` said every PR merges once CI passes on its queue draft.
It did not: the rule had no `merge_conditions`, the draft showed "No extra conditions" for merge, and draft #483 closed 20 seconds after opening, with #479 and #480 merged, while its CI failed 8 minutes later and main's runs went red.
The queue was gating on each PR's own checks only.
That was found only while moving Windows off PR pushes (brwr-3zqd), which is unsafe without the draft gate.

**Where:** `.mergify.yml` `merge_conditions`; `gh api repos/<owner>/<repo>/commits/<pr head>/check-runs` (the Mergify check's summary); `gh pr view <draft> --json createdAt,closedAt,body`.
