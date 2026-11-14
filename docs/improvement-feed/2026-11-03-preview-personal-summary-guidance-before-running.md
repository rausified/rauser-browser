## 2026-11-03 — Preview personal summary guidance before running

What: add an explicit read-only settings action that shows the current default
or custom summary style and any host validation issue before a summary starts.
Reuse the fixed-name, bounded reader; show text only and start no harness.

Why: `brwr-sm4` makes language and emphasis editable without rebuilding, but
users currently learn whether the file is valid only by starting a summary.
The settings help explains the location and limits; a preview would also make
the host's choice of home folder and the next run's effective text reviewable.

Where: `host/src/service/summary_style.rs`, a narrow host read request,
`extension/options.html` and `extension/options.ts`, with offline host/UI tests.

Queue: `brwr-myn8`, discovered from `brwr-sm4`.
- Review 2026-11-12: **Keep.** Shows effective SUMMARY_STYLE before a run so users can fix validation errors without spending a run.
