# Automatic summary eligibility follow-ups

**Goal:** Fix `brwr-sm7` and `brwr-sm9`: only current capture rules authorize
automatic summaries, and a browser left open can retry a viewer-only summary
on the next local day.

**Architecture:** Keep note identity and manual summary resolution intact.
Check automatic eligibility under the native config lock. Keep the worker's
bounded submitted-ID cache, with one local calendar day for the cache.

**Tech Stack:** Rust native host and TypeScript MV3 worker; offline fixtures.

**Sources:** DESIGN.md §5.2, the two beads, `host/src/native.rs`,
`extension/auto-summary.ts`, and `extension/tests/worker.test.mjs`.

## Design challenge

- **Challenger:** `/root/auto_summary_design`, a read-only Codex reviewer.
- **Alternative:** Resolve automatic keys directly from current adapters under
  one lock, rather than adding an unlocked current-match check after note-key
  resolution. Keep a single cache day rather than timestamps per artifact.
- **Outcome:** Adopt the single refreshed config lock for settings, identity
  and current matching, released before service IPC. Retain the existing key
  resolver's home-folder and request checks, and independently require a
  current adapter match on the original URL under that same lock. This keeps
  manual/note semantics and avoids a new policy enum in their common helper.
  Adopt centralized local-day rotation before candidate early returns and
  before submission insertion, including clearing undated legacy caches once.

## Open decisions

- **Current rules authorize automatic summaries:** resolved by the acceptance
  of `brwr-sm7` and DESIGN.md §5.2; preserved note contracts do not authorize.
- **Next-day retry:** resolved by `brwr-sm9` and DESIGN.md §5.2; use the local
  calendar day, with the host deciding whether the summary still blocks.
- **Retry timing:** existing activity synchronization on a later load/focus/
  navigation recreates candidates. No immediate midnight run is promised.

## Tasks

1. Reproduce removed/narrowed-policy authorization and next-day suppression
   with native, pure-state and worker regressions.
2. Enforce native eligibility under one lock; expire submitted IDs by day.
3. Run the full offline gate, read the complete diff, update contracts and
   append cycle documentation, then close only the completed beads.

## Review correction

The one Codex pass found a P2 at worker settlement: a request made yesterday
could receive `already_summarized` just after midnight and stamp today's
suppression cache. A failing-first worker test now delays that exact reply
across midnight. The worker retains the local calendar day produced by the
request timestamp callback and caches the answer only when the reply still
lands on that day. Otherwise it drops the candidate so a later activity
update can ask for today's decision. This uses the existing injected clock
and worker port seam. The complete gate is rerun after the correction.

The first full rerun exposed interaction between the new worker clock tests:
advancing the shared fixture's clock also reconsidered tabs from preceding
tests, while restoration could overlap an unfinished settlement. Each clock
test now begins with its own activity-session fixture, disables automatic
work during cleanup, and the next-day case waits for the settled day/cache
rather than merely a closed native port. Assertions and time limits are
unchanged. The complete worker file is checked before the final gate.

## Completion

All three tasks are complete. The final full gate passed after the midnight
correction and fixture isolation: 1,365 Rust-reported outcomes (94 explicit
IPC guards), all 41 extension files, both Clippy configurations, and the
accessibility command (2 fixture passes, 36 browser sandbox skips). The full
worker file reports 102 passing cases; 14 pure-state cases also pass in UTC
and Pacific time. Supplementary README checks passed. The complete diff was
read manually and one read-only Codex pass found only the corrected P2.
`brwr-q0i4` records the separate capture-pause follow-up; `brwr-rr93` records
the visible read-scope proposal. Platform smoke/dialog behavior is unchanged
and was not exercised on Linux. Driver review, external gates and publication
remain outside this cycle.
