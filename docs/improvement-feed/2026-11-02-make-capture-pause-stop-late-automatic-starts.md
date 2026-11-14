## 2026-11-02 — Make capture pause stop late automatic starts

Change: check `capture_enabled` under the same native eligibility lock as
`auto_summary.enabled` (`brwr-q0i4`). Why: the `brwr-sm7` review found that a
pause during page extraction is not rechecked by `start_auto_summary`; an
already-reading page can still start a run after capture is paused. Where:
`host/src/native.rs` and its offline auto-summary regression tests. Explicit
Summarize & log should stay available while capture is paused.
- Review 2026-11-12: **Keep.** Rechecks capture pause when an automatic summary starts, preventing a run that began reading before Pause.
