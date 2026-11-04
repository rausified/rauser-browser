## 2026-11-02 — Disclose reading scope when choosing a saved summary

Change: carry validated saved-summary reading scope to the Summary line in
This page and to Recent, omitting it for legacy records (`brwr-30xv`,
discovered from `brwr-rr93`). Why: scope is now visible in run cards and Tasks
detail, but the overview shown while choosing a remembered artifact can still
look complete when the agent read only its browser viewer. Where:
`SummaryView` in `protocol/schema.json`, the host's saved-summary projection,
`extension/summary-text.ts` and the built page-summary/Recent tests. Use
recorded scope rather than inference from overview text.

- +1 2026-11-04: `brwr-divh` reproduced a run claiming whole reading and a sign-in wall while reporting success with no saved summary. Fresh-answer validation now refuses that contradiction. Carry the surviving validated scope into saved-summary choices (`brwr-30xv`) so a partial or viewer-only overview does not imply full reading after its run card is gone.
