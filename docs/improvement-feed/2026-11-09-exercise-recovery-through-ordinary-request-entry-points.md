## 2026-11-09 — Exercise recovery through ordinary request entry points

Add a table-driven activation fixture that leaves a private re-key prepare at
each publication boundary, then invokes startup, a long-lived native refresh,
and a nested review read. Check that every successful response uses one
consistent replay generation and that a refusal clears the usable snapshot
and reports the pending-recovery issue in Hello/GetConfig.

The nested-read prerequisite exposed why testing `recover` alone is insufficient:
ordinary loader recovery could reacquire a lock already held by planning, and
Hello/GetConfig ignore refresh errors while presenting `config_issue`. Test
these entry points together when the universal recovery gate and replay-aware
readers land. Locations: `host/src/config.rs`, `host/src/native.rs`,
`host/src/relearn/transaction/tests.rs`, and the staged Re-learn plan.
