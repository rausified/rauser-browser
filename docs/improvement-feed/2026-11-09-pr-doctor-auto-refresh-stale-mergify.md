## 2026-11-09 — Let pr-doctor refresh a stale Mergify evaluation

Change: When pr-doctor's scheduled run finds a PR labelled `queue` whose "Mergify Merge Queue" check is still waiting on required checks that are already SUCCESS, have it post `@Mergifyio refresh` once, using `staleQueueConditions` from `scripts/dev/pr-watch.mjs`, instead of waiting for the owning session to read pr-watch's advice.

Why: Mergify missed check-run completions on #329 and #343 on 2026-11-09. Each PR sat green and labelled until someone commented a refresh. pr-watch now detects this and says so, but only a session that is still watching acts on it. An idle owner leaves the PR stuck, the same gap as a green PR that is never queued.

Where: `scripts/pr-doctor.mjs` and `.github/workflows/pr-doctor.yml`, reusing the pr-watch helper and its tests.
