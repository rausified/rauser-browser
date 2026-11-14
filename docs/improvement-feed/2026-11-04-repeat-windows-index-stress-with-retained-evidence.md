## 2026-11-04 — Repeat Windows index stress with retained evidence

Add a manually triggered Windows stress run that executes
`many_concurrent_first_opens_never_fail` and
`mp_deliver_read_and_regenerate_share_one_cache` ten consecutive times and
retains failure output and each child result. `brwr-epl1` already owns this
acceptance; do not infer its root cause from Linux-only success. This cycle's
`brwr-bszr` diagnostics and harness capture now preserve operation, phase,
extended SQLite code, elapsed time and attempts, including a busy catch-up
whose reader serves its snapshot. The next Windows failure can therefore
distinguish acquisition from setup, statement or commit contention. Put the
repeatable invocation and artifact retention in `.github/workflows/` with
commands documented in `TESTING.md`; do not widen timeouts or weaken the
existing zero-error/zero-busy assertions.
- Review 2026-11-12: **Keep.** Runs repeated Windows concurrency tests with retained output to validate the index fix on the affected platform.
