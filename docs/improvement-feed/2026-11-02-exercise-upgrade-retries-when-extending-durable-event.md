## 2026-11-02 — Exercise upgrade retries when extending durable event payloads

Change: keep a pre-change journal fixture and replay it with the new caller
payload whenever adding a summary event field. Why: the overview tests covered
old log reading and cache reopen, but an older interrupted append failed Retry
save because its payload hash lacked the new field. The review regression in
`host/src/capture.rs` now covers this transition; apply the same check to future
event extensions alongside the frozen single-event recovery tests.
