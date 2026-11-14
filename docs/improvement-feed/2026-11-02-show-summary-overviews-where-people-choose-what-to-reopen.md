## 2026-11-02 — Show summary overviews where people choose what to reopen

Change: finish brwr-sm3 by carrying SUMMARY.md's overview into run answers and
saved-summary views, then show it in the run card, artifact card and Recent,
with today's wording for older records. Why: brwr-sm1 now preserves the
recorded path into an artifact in its saved summary, but the person deciding
which summary to reopen still sees only its type and importance. The existing
one-sentence overview is the useful decision aid at that point. Where:
protocol/schema.json, host/src/summary.rs and service/record.rs,
extension/summary-text.ts, extension/run-card.ts and the panel. Use the
existing brwr-sm3 rather than create another proposal for the same work.

- Done 2026-11-02: brwr-sm3 carries overview through events, views and run answers; panel, Recent and run cards show it with legacy fallbacks.
- Review 2026-11-12: **Done.** Shows saved overview where a person decides what to reopen.
