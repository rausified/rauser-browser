## 2026-11-09 — Share history invalidation across panel views

Change: Give Recent and worklog a shared configuration-change lifecycle hook so each view invalidates at the policy notice and refreshes after the host configuration reload.

Why: Independent review found that the sharing view invalidated immediately while Recent waited for the debounced reload, allowing an old-home reply to publish. The local fix covers both, but separate subscriptions and reload checks can drift again as views grow.

Where: `extension/panel.ts` (`session.watchPolicy`, `scheduleReload`, `invalidateRecent`, `resetWorklogSelection`) and the held-configuration regressions in `extension/tests/panel/recent-worklog.test.mjs`.
- Review 2026-11-12: **Keep.** Coordinate Recent and worklog invalidation after policy changes; separate subscriptions can expose stale results.
