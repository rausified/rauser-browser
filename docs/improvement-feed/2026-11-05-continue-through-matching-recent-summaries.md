## 2026-11-05 — Continue through matching Recent summaries

Change: let Recent continue beyond its first 32 matching artifact-day rows,
preserving the chosen title, type and saved-summary reading filters. Why:
`brwr-y1q2` now finds limited readings before the response cap, and its
365-day fixture demonstrates that a substantial history still exceeds the
visible result set. A person should be able to review later matches without
guessing a narrower title. Where: `host/src/index/query.rs`, `list_artifacts`
in `protocol/schema.json`, and Recent in `extension/panel.ts`. Keep bounded
responses, stable day/artifact identities and stale-response protection.
Tracked as `brwr-a30j`, discovered from `brwr-y1q2`.
- Review 2026-11-12: **Done.** Let Recent load more matching results 32 at a time. People can reach older summaries without changing their search.
- Done 2026-11-12: verified on main at 4cd617d; `extension/panel.ts` pages matching Recent results.
