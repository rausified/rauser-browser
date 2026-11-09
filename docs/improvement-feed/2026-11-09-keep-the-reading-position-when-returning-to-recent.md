## 2026-11-09 — Keep the reading position when returning to Recent

Change: Preserve the loaded Recent pages and scroll/focus position when a reader switches between Page and Recent within an unchanged panel session. Offer an explicit Refresh action; retain the filter, home, configuration and host-exit invalidation rules.

Why: `brwr-a30j` removes the first-32 ceiling, but selecting Recent again currently calls `loadRecent()` and discards every appended page. Following up on a large set of limited readings would force a reader to page through the same entries again.

Where: `extension/panel.ts` (`selectView`, `loadRecent`, `recentResult`), `extension/panel.html`, and the Recent panel/keyboard tests. Tracked as `brwr-0401` with explicit refresh and stale-scope acceptance.
