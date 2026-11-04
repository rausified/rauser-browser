## 2026-11-03 — Test pr-watch on recorded Mergify payloads

Change: keep a few Mergify Merge Queue check runs (title, summary and the
embedded `mergify.v1` payload) captured from real PRs as fixture files under
`scripts/tests/fixtures/mergify/`, one per phase seen (ready, waiting,
queued, batch wait, dequeued, merged), and have `scripts/tests/pr-watch.test.mjs`
build its checks from those instead of hand-written titles.

Why: #294's "comment `@Mergifyio queue`" advice never fired. Its test checked
a hand-built "Merge queue is ready" title with no payload, while Mergify's
real check carries a payload whose `state` is "waiting" under that title, and
`mergifyQueueStatus` read the payload first. #265 (2026-11-04) showed the
generic message instead; #298 fixed it with one real-shaped fixture.

Where: `scripts/tests/pr-watch.test.mjs` (the `mq()` helper and
`WAITING_META`), a capture step (`gh api repos/<repo>/commits/<sha>/check-runs`)
recorded in a fixture README, and `scripts/dev/pr-watch.mjs`
`mergifyQueueStatus`.
