## 2026-11-09 — Queue a PR when it opens

Change: Have a session add the `queue` label and post `@Mergifyio queue` in the same step that opens the PR, instead of after its checks go green. Make `scripts/dev/pr-watch.mjs` print that command as the first action for an unqueued PR whose checks are still pending, not only once they pass.

Why: PR #326 (starters D3) went green at about 23:55 on 2026-11-08 and sat unqueued for about 7 hours. The session that owned it went idle between opening it and queuing it, and nothing else moves a green PR forward. Mergify already holds a queued PR until its required checks pass, so queuing at open costs nothing and closes that gap. PR #324 (D2), queued at open, merged 25 minutes after it opened.

Where: `AGENTS.md` "Review and merge cadence", the `merge` section of `worker-contract.json`, and `scripts/dev/pr-watch.mjs` with its tests.
- Review 2026-11-12: **Keep.** Put new PRs in Mergify queue immediately so green checks can advance without an active session; one PR sat seven hours.
