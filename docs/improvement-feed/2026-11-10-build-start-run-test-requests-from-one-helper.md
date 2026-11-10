## 2026-11-10 — Build start_run test requests from one helper

Change: give host tests one constructor for a summary `StartRunRequest`, a
`StartRun` and a `RunStatusFile` (for example `test_support::start_run(url)`
returning a value whose optional fields are all `None` or empty), and have
the existing literals use it with struct-update syntax. Why: adding the one
additive `open_tabs` field (brwr-sm2) needed a scripted edit of 25 struct
literals across `native.rs`, `capture_site.rs`, `service/server.rs`,
`supervisor.rs`, `finisher.rs`, `maintenance.rs`, `open.rs` and `record.rs`,
none of which cared about the field; `capture_truncated` and `no_page` paid
the same cost before it. Where: a `#[cfg(test)]` module in `host/src/service/`
used by those test modules.
