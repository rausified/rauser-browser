## 2026-11-09 — Review live adapter precedence changes

Change: guard or explicitly review identity changes when Settings reorders
unchanged overlapping adapters. Why: the `brwr-obqb` fix protects recognition
narrowing, but contract membership alone does not detect a different winner
when two live adapters trade places. The validator skips contracts still live
verbatim, while capture uses first-match order. A person changing precedence
should see which recorded identities would move before saving. Where:
`host/src/config.rs` transition checks and the Settings precedence editor.
Tracked as `brwr-qbub`, discovered from `brwr-obqb`; reproduce with a real
revision-checked save before implementing, retaining disjoint reorder and
contained-before-container behavior.
