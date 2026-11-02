# Task entry and instruction recall

**Goal:** Make the task bar reachable from Chrome and reusable from the keyboard (brwr-2b5, brwr-18u).

**Architecture:** Existing worker-to-panel notices carry a fixed focus intent; execution remains exclusively inside the panel. Recall reads retained task records through the existing history and get-run helpers and keeps only this panel's draft/edit state in memory.

**Tech Stack:** TypeScript MV3 extension, built-panel fake DOM tests, local browser fixtures.

**Sources:** DESIGN.md §5.7–5.8; task-bar plan decisions 21, 26, 28; the two beads' acceptance criteria.

## Design challenge

- **Challenger:** Cookie's task_entry_design_challenge reviewer (required because gestures cross the worker/panel boundary).
- **Alternative:** A focus-only notice and a small panel-memory recall state machine, bounded to 20 list pages / 200 task entries, instead of gesture-carried instructions or a second persistent history. Hold focus through startup; reject stale/future notices; fetch full instructions and cancel delayed replacements after edits.
- **Outcome:** Adopted before task 1. Reuse the existing list-page and terminal-cursor helpers; preserve drafts on focus, and leave execution only to Enter/Run. This avoids duplicate instruction storage and the truncated list preview trap.

## Open decisions

- **Shortcut:** Ctrl+Shift+8 / Command+Shift+8, beside the existing summarize shortcut on 9; remappable through Chrome. **Status:** resolved by Cookie as coordinator, 2026-11-02, under the cycle's autonomous implementation authority.
- **Existing drafts:** A fresh bar stays empty when focused; an existing typed draft is preserved, following DESIGN.md's cross-tab draft rule. A gesture never copies selection or page text. **Status:** resolved by Cookie, 2026-11-02; interpret the bead's "empty" as no automatic prefill, not deletion of user work.
- **Slash:** Unmodified / focuses the bar only from non-editable panel controls; native text entry and IME keep their keys. **Status:** resolved by Cookie, 2026-11-02.
- **Recall:** ArrowUp on an empty field or at a collapsed start navigates older task instructions; ArrowDown at a collapsed end navigates newer and returns to the draft. Preserve edits at every visited position. No new persistent instruction store; the service's retention applies. **Status:** resolved by Cookie, 2026-11-02.
- **Unavailable history:** Leave the draft untouched and show a retryable message; never use list_runs' truncated instruction as executable text. **Status:** resolved by Cookie, 2026-11-02.

## Tasks

1. Add the Chrome command/menu and fixed focus notice, with worker and panel tests proving there is no task start or page-derived prefill.
2. Add lazy service-backed recall with draft/edit preservation and cancellation of stale asynchronous replacements; cover history ordering, full instructions, pruning and failure.
3. Exercise panel wiring and local real-browser keyboard fixtures, then run the complete required gate and review the full diff.
4. Update current design, user instructions, history, cycle write-ups, improvement feed, and exported queue; prepare the commit and hand off to the driver.

## Implementation and validation handoff

The four tasks are implemented. `task-recall.ts` shares the history page and terminal cursor helpers, fetches full instructions lazily, and preserves draft/visited edits in panel memory. The worker's new task gesture sends only focus intent. Focus waits through startup/read-only starts; recall cancellation covers typing, Reuse, submission and teardown. The complete diff review found a stale error after successful recall retry and old active runs jumping ahead of newer finished runs across pages. Both regressions failed against the previous built extension. The fixes clear the stale line on success and defer old active metadata until the terminal cursor reaches its chronological place.

`brwr-1xmv` was taken alongside this work for the shared real-keyboard composite entry helper. It is implemented in the two panel journeys, but its explicit real-Chromium acceptance stays open until the driver runs outside the sandbox. `brwr-ppn` was reopened after auditing its database-only historical closure against baseline 80abc368: the selected-work code is absent here. Its pagination follow-up now has a real prerequisite edge. No quarantined product code was silently recovered.

Focused validation: 51 recall/bar cases pass, plus the complete worker file. The final required gate and README supplement passed on the final source tree: 44 extension files, 1,374 Rust-reported outcomes including 94 explicit IPC skips, 51 focused recall/bar cases, and 2 accessibility fixture passes with 39 explicit browser skips. Full check summaries are in target/overnight/task-entry/final-results.json. The driver owns browser/IPC acceptance, independent review, commit SHA and publication. The two feature beads are closed; brwr-1xmv remains blocked only for its explicit real-Chromium acceptance. Future enhancement `brwr-k6rg`: show the recalled task's original page before deciding whether to reuse its instruction on the current page.
