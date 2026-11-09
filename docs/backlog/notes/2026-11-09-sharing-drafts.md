# 2026-11-09 — Named sharing drafts handoff

`brwr-1b6o` implements browser-local named selected-work snapshots and explicit
evidence review. The source contract is DESIGN.md §5.1 and the implementation
plan is `docs/superpowers/plans/2026-11-09-sharing-drafts.md`. The one independent
review found `brwr-74bl`, fixed test-first without adding a new seam.

`brwr-cc3h` proposes inert word/URL change highlighting to reduce rereading
before review. `brwr-f774` remains the broader guard-checker quality proposal;
this cycle's `brwr-sivz` only repairs 36 missing guards discovered by the
supplementary script gate. Do not confuse explicit sandbox skips with execution.
The driver must run real Chromium and strict local-IPC acceptance.

Assigned Re-learn work remains behind its existing apply/recovery, durable
unalias, authoritative-history location and downgrade decisions. No apply
authority was inferred, and none of those items was closed. Queue transport,
consistency and publication remain driver-owned; no queue file belongs in Git.

Local verification uses an offline npm cache and `target/cookie-health-isolated`
for Cargo because the shared target lock was held. These are ignored build
artifacts, not product defaults. No runtime, credential or live API was touched.
