## 2026-11-09 — Read Mergify's neutral "Dequeued" titles as dequeued

Change: Have `mergifyQueueStatus` in `scripts/dev/pr-watch.mjs` treat a title starting with "Dequeued" as phase `dequeued`, whatever the check's conclusion. Give the advice by reason: for "conflict with base branch", merge main in; for "queue conditions no longer match", name the unmet condition, then re-comment `@Mergifyio queue`.

Why: PR #346 ("Dequeued — conflict with base branch") and #349 ("Dequeued — queue conditions no longer match") finished their Mergify Merge Queue check as COMPLETED/NEUTRAL, not FAILURE. pr-watch only marks a dequeue from a failed state, so it reads both as phase `unknown` and says "waiting for Mergify to admit it" for a PR Mergify has already removed from the queue.

Where: `scripts/dev/pr-watch.mjs` (`mergifyQueueStatus`, `decide`), its phase table in `scripts/tests/pr-watch.test.mjs`, and the title list in AGENTS.md "Review and merge cadence".
- Done 2026-11-09: #364
