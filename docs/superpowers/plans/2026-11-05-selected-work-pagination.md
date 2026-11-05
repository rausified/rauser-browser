# Continue through selected work

**Goal:** `brwr-p71k`. Let a person reach every representable artifact-day choice
in a busy day or week, then copy exactly the checked preview. Preserve the
200-row and 96-KiB serialized-row limits on each response.

**Architecture:** Reuse the direct-activity choice renderer; add stateless
snapshot-bound pages within one index read transaction and accumulate only
explicitly loaded choices in the panel.

**Tech Stack:** Existing Rust/rusqlite index and native host, generated JSON
protocol types, TypeScript DOM UI, and offline Rust/Node/Playwright fixtures.

**Sources:** `DESIGN.md` §5.1 Selected work for sharing, `brwr-p71k`, and the
existing choice limits and selection lifecycle in `extension/panel.ts`.

## Open decisions

- Resolved by the existing selected-work contract and this bead: loading more
  work is explicit, new choices start unchecked, and Copy uses only checked
  displayed rows. No content, settings or history writes are introduced.
- Resolved by Cookie within this delivery: all-work choices gain pagination;
  focus-session entry pagination remains `brwr-6ldz`; catalog pagination
  remains `brwr-t4gf`.
- Resolved by Cookie: changed history refuses continuation. Refresh keeps
  later pages only if their full snapshot still matches; otherwise it drops
  them and prunes selection to refreshed rows, with an explicit notice.

## Design challenge

2026-11-05, reviewer `/root/pagination_design_challenge`: adopt stateless
snapshot-bound offsets. Keyset-only pagination and event-count/latest-event
watermarks can mix old and new totals for existing artifact-days; persisted
snapshots add lifecycle state. Hash framed, versioned deterministic values:
date scope, storage identity and adapter scope, all ordered logical choices
before omission, and gap information. Offsets count logical rows, including
oversized rows. A normal row that exceeds the remaining byte budget begins
the next page; it is never permanently skipped. Report permanent omissions
separately from continuation, retaining that warning across loaded pages.
Require a matching snapshot for positive offsets and reject invalid offsets.

## Delivery

1. Add optional paging metadata to `worklog_choices`, preserving unpaged wire
   shapes and the independent focus-session flow.
2. Share the existing choice renderer with deterministic bounded pages from
   one index read transaction, with validated snapshot continuation.
3. Add **Load more work**, unchecked append, exact checked Copy, and guards
   for refresh, period/config changes, errors and late replies.
4. Verify busy/byte-bound/omitted rows, stale history and scope, legacy reads,
   selection, keyboard behavior and full local gates.
5. Read the entire diff afresh, complete one independent review, record
   results and documentation, and leave publication to Cookie's driver.

## Cycle 8 progress (quarantined)

Baseline required health gate passed on `08bc0d2e`. Implementation and focused
checks are complete. The independent Codex review found overlapping-refresh
snapshot loss, missing runtime metadata validation, native disabled-button
focus loss, and gap toggling re-enabling controls while loading. All four
were corrected; the first two were reproduced with failing tests before the
fixes. The panel now keeps a scope-bound validated snapshot independently of
pending refreshes, validates nested page metadata, and uses guarded
`aria-disabled` for the loading button. Full final gates passed; local implementation is complete.


The large 451-row replay regression was narrowed to cache-loss reconstruction;
a small fixture still checks actual reindex snapshot stability. Pagination
does not need hundreds of regenerated Markdown artifacts. Final validation
runs the required gate before the Dolt tooling fixtures, after a parallel
attempt encountered high VM I/O pressure and a tooling-fixture error. No
product behavior, timeout or assertion was relaxed.


Final local validation passed: 1,634 workspace Rust-reported passes (seven
new; 94 explicit IPC guards and two ignored), all 48 extension test files,
62 focused worklog cases (14 new), both Clippy configurations, and a11y two
fixture passes/51 explicit sandbox browser skips (one new keyboard journey).
README extras passed: 10,696 inert Markdown renderings, 12 script test files,
252 Python tooling cases (92 platform skips), and queue consistency. The
previously errored Dolt fixture passed alone in 11.6 seconds, then in the full
serial tooling run. A parallel attempt during heavy I/O was interrupted;
no assertion or timeout was weakened. Final whole-diff manual review and one
independent Codex pass are complete; all four review findings were corrected.
Driver owns unsandboxed browser/IPC acceptance and publication.

## Repair after the cycle 8 commit gate

The driver rejected cycle 8 twice on the real-browser final-page focus
assertion (`keyboard.test.mjs:70`). Its local browser skips were not browser
acceptance. Recovered the complete prescribed `08bc0d2e..b4484a59` diff rather
than abandoning it. The missing browser behavior was inherited disabled state:
the checkbox's fieldset remained disabled until `finally`, so Chrome ignored
the earlier `focus()` call. Enable the fieldset before appending/focusing the
new page; keep the final cleanup for errors and late replies.

The existing offline append/Copy regression now models disabled-fieldset
focus refusal with a test-scoped method mock. It fails on the recovered code
and passes after the ordering fix. The real-browser keyboard assertion is
unchanged. This is a repair-only cycle; the driver retains responsibility for
real Chromium/IPC acceptance and publication. Validation results follow.

Repair validation is now complete: the required protocol/build/fmt/Clippy,
offline dependency install, typecheck/lint, extension build/tests, workspace
Rust tests, scripted-dialog Clippy and required a11y command all pass locally.
Counts remain 1,634 Rust-reported passes (94 explicit IPC guards, two ignored),
48 extension files, 62 focused worklog cases and two a11y fixture passes with
51 explicit browser skips. Supplementary checks pass: 10,696 inert Markdown
renderings, 12 script test files and 252 Python tooling tests (92 platform
skips). Read the complete recovered/repaired diff afresh with no additional
code findings. Queue transport/export and its live consistency check remain
with the driver, as this run requires. Restored two absent follow-up records
as `brwr-6ldz` and `brwr-1b6o`; shared disabled-focus modeling is `brwr-tcd5`.
