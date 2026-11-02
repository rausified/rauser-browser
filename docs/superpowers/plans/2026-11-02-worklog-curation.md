# Curate work before copying a worklog

**Goal:** Let a person select captured artifact-day entries for a standup or status update and copy exactly the preview, without changing source history (`brwr-ppn`).

**Architecture:** A new additive protocol-v8 `worklog_choices` query uses the existing locked, host-validated one-to-seven-day history path. The request also carries the current config revision, checked after refresh under that lock before opening history. It supplies independent direct-activity artifact-day rows, rendered by the host, and period-wide capture gaps from one index snapshot. The panel retains an explicit in-memory selection and concatenates only selected rows.

**Tech Stack:** Existing Rust index and native protocol; generated Rust/TypeScript; plain panel DOM and mocked transport tests.

**Sources:** `DESIGN.md` §5.1; bead `brwr-ppn`; existing worklog and capture-gap tests.

## Design challenge

- **Challenger:** Codex session `curation_design_challenge`, read-only, 2026-11-02 (AGENTS.md sharing-boundary challenge).
- **Alternative:** Independent host-rendered artifact-day rows and browser concatenation instead of sending selections back for another server rendering pass. Do not reuse ordinary aggregate headers or global latest artifact context.
- **Outcome:** Adopted. Row titles and exact URLs come from the latest direct visit within that day; global Why and rolled-up container copies are excluded. Selection is scoped to storage and period, cleared on disconnect, intersected on refresh, and never extends to newly arrived rows. Deterministic 200-row/96 KiB serialized-row bounds report truncation. Period-wide gaps retain the existing explicit inclusion choice.

## Open decisions

- **Selection/preview:** Resolved by Cookie as coordinator from the bead's explicit row-selection acceptance, 2026-11-02: opt-in “Choose work to share,” initially no rows selected, “Select all shown” and “Clear,” exact text-only preview with dates, measures and links. Ordinary worklog stays available outside this mode.
- **Containers/context:** Resolved by the design challenge from the exclusion requirement: selected rows show only direct measured activity and same-day context, without rolled-up child copies or global Why. UI explains direct container activity.
- **Refresh/lifetime:** Resolved from stable-identity acceptance: retain artifact/day selections in memory only for the same storage and period; removed rows lose selection; new rows start unchecked. Clear on disconnect/storage change/period change. Copy is disabled during loading, errors and an empty selection.
- **Bounds/compatibility:** Resolved by existing bounded-history conventions: at most 200 choices and 96 KiB serialized rows, visible truncation, no partial URL. A host without the new request gives rebuild guidance; ordinary worklog requests remain unchanged.
- No unresolved product or policy decisions; related-open-tabs scope remains outside this task.

## Tasks

1. Add the independent wire query and host-derived, bounded direct artifact-day choices with scoped context and tests.
2. Add panel selection, refresh preservation and exact preview/clipboard behavior with built-panel tests.
3. Cover browser keyboard/a11y interaction; run the full offline gate and review the complete diff.
4. Update current design/history, feature narrative, improvement feed, queue and cycle handoff.

## Archived validation and completion

Initial gate passed before implementation, with explicit IPC and browser sandbox skips. Final results and remaining platform acceptance are recorded below at closeout.

The independent offline review found no P1/P2 issue and one P3: delivery refresh
removed the focused checkbox. The implementation now restores that artifact/day
focus after same-scope refresh only while focus has not moved elsewhere; two
mocked cases and the real keyboard fixture cover restoration and non-stealing.

Tasks 1–4 are complete locally. Both the initial gate and final gate on the
focus-corrected implementation passed every required and README command.
Final cargo workspace results: 1,380 reported outcomes, including 94 explicit
IPC sandbox skips; all 41 extension files pass, including 25 individual
worklog cases. Six Rust and thirteen panel regressions are new. Accessibility
reports two fixture passes and 37 explicit real-Chromium sandbox skips; the
new keyboard/polling case awaits the driver's unsandboxed gate/CI. Queue and
current-state checks pass. The independent offline review and Cookie's full
diff review are complete; no outstanding correctness finding remains.

No live API, service, external host or remote Git operation was used. The
network-backed `codex exec review` command was not run under this cycle's
explicit offline guardrail; the read-only Codex session supplied independent
review and the driver owns its separate final review/publication. Pagination
is queued as `brwr-p71k`; `brwr-sm2` awaits its recorded product decisions.

## Driver rejection and recovery (2026-11-02)

The unsandboxed cycle-6 gate rejected the recovered change: 38 of 39
accessibility cases passed, but the new curation keyboard journey tried to
reach inactive Recent using Tab. Its `tabindex="-1"` deliberately keeps it
outside the tab order. This was a test-navigation defect, not evidence that
the product should change its existing composite keyboard behavior.

Cookie recovered the complete base-to-quarantine patch from
`quarantine/cycle-6-20261102T202047Z` onto base `f29565ce9834` and reopened
`brwr-ppn` during repair. The journey now Tabs to selected This page, presses
ArrowRight, and asserts Recent has focus before opening curation. The fixture
helper documentation and TESTING.md explain the entry contract. No new
feature scope was added. The earlier sandbox skips did not establish browser
acceptance; the repaired case still requires the driver's unsandboxed run.

Recovery validation completed: the required full command sequence passed on
the recovered tree and again after repair; all supplementary README commands
passed. Rust reports 1,380 outcomes including 94 explicit IPC sandbox skips;
all 41 extension test files pass. Accessibility reports 2 fixture passes and
37 explicit browser sandbox skips, not 39 browser passes. Fresh complete-diff
review found no additional correctness issue. Queue/current-state checks pass.
`brwr-ppn` is closed locally with driver real-browser acceptance and publication
still required. Follow-up `brwr-1xmv` records shared composite keyboard entry;
`brwr-p71k` still covers pagination. No new feature was started in this repair
cycle. The Git metadata mount is read-only; the driver owns the final commit.

## Integration recovery on current main (2026-11-02)

This cycle starts from clean `e9f7408c`, with no quarantine report and with
`brwr-ppn` explicitly reopened because none of its product code reached that
baseline. Its required health gate passed before work began. Recover the
feature-only delta from `f42018a3` plus the overlapping-period review fix in
`00891487`; do not restore archived queue or append-only bookkeeping files.
The four-task design, recorded challenge and resolved decisions above still
apply. The new panel CSS is retained, with choice styles placed beside its
worklog styles. The keyboard journey now uses current main's shared
`enterPanelTab` helper, already used by tab and task-recall journeys.

Focused host and built-panel checks pass. Full current-baseline validation
and final review results are recorded at this cycle's closeout below; archived
results above describe earlier attempts, not current browser acceptance.

Current recovery closeout: the entire required command sequence and every
supplementary README check pass. Cargo reports 1,380 outcomes, including 94
explicit sandbox IPC skips; all 44 extension files pass. Accessibility has
2 fixture passes and 40 explicit Chromium sandbox skips. Six Rust and fourteen
built-panel regressions plus the keyboard journey are restored relative to
this baseline. Fresh complete diff review found no new correctness issue.
The network-backed `codex exec review` is excluded by this run's explicit
offline guardrail; Cookie reviewed the complete diff locally, and the driver
owns independent review and unsandboxed browser/IPC acceptance before publishing.

`brwr-ppn` is closed locally and exported. `brwr-1xmv` stays blocked on its
explicit real-Chromium acceptance; its helper now also serves the recovered
curation journey. Pagination remains `brwr-p71k`. No migration, live install,
external API, remote Git operation or assigned-work gate was changed.
