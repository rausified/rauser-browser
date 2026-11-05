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
