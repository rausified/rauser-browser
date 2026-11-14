## 2026-11-02 — Explain limited reading beside a summary overview

Change: show the validated whole/partial/page-only reading scope in the run
card and Tasks detail, as proposed in `brwr-rr93`. Why: fixing next-day retry
(`brwr-sm9`) exposed that a viewer-only summary can look finished in the panel
while the host deliberately allows another attempt tomorrow. A short label
would help a person judge the overview and understand a later retry. Where:
`protocol/schema.json`, `host/src/service/record.rs`, `extension/run-card.ts`
and the built run-card/Tasks tests. Keep the label text-only and omit it for
legacy answers without scope.
- +1 2026-11-02: brwr-sm5 now tells agents when browser capture was capped,
  including automatic summaries. Carry the validated resulting scope into saved
  summary choices in Recent and This page too (`brwr-30xv`), where people decide
  whether to reopen an artifact without opening Tasks detail. Keep this distinct
  from the capture flag: a capped page can still yield a whole-artifact summary
  after the agent retrieves it. The run-card projection is pending held PR #249.

- Done 2026-11-02: `brwr-rr93` (cycle commit prepared for driver).
- Review 2026-11-12: **Done.** Shows reading coverage next to summary overview so a viewer-only result is not mistaken for complete reading.
