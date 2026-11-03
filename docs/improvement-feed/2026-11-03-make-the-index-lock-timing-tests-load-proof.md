## 2026-11-03 — Make the index lock-timing tests load-proof

In the last 150 PR CI runs, macOS failed while Windows passed 12 times. Four of
those were the same two timing tests in the SQLite index:
`index::db::tests::a_held_write_lock_surfaces_busy_within_about_one_busy_timeout`
and `index::tests::reads::a_read_behind_a_held_write_lock_serves_the_snapshot_within_one_timeout`
(jobs 125704264842, 125693343889, 125686885877, 125623315026), in both the
plain and the scripted-dialogs test builds. They assert wall-clock bounds
around SQLite's busy timeout, which a shared macOS runner under load misses.
Each failure costs a full macOS rerun (about 15 minutes, more when runners are
starved). Change: assert the outcome (busy reported, snapshot served) and only
a generous upper bound, or drive the timeout from an injected clock. Where:
`host/src/index/db.rs`, `host/src/index/tests/reads.rs`.
