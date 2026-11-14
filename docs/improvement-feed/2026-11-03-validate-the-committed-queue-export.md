## 2026-11-03 — Validate the committed queue export

What: run the queue consistency check against the final committed queue and
backlog immediately before publication, including after Beads auto-export.

Why: this review reproduced six violations because the export omitted
`brwr-9eio` and `brwr-fzya` while their generated entries and a dependency
remained. Both records were present in the local database; its auto-export
restored exactly those two rows without changing any existing issue.

Where: the overnight publication driver and
`scripts/check_queue_consistency.py --issues <committed-export> --backlog <committed-backlog>`.
- Review 2026-11-12: **Merge.** Same final queue validation as the other 11-03 item.
- Superseded 2026-11-12: consolidated into `2026-11-03-check-the-final-exported-queue-before-publication.md`; validate the final published Dolt queue state there.
