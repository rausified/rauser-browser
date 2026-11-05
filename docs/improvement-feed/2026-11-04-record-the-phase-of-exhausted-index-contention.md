## 2026-11-04 — Record the phase of exhausted index contention

Add phase, extended SQLite error code, elapsed wait and attempt count to the
index's exhausted-contention diagnostics, keeping its clear caller message.
`brwr-epl1` records Windows failures only as a caller-visible busy error and a
global counter. Ten repeated Linux first-open runs passed before today's
`brwr-rx0s` fix, while a separate real SQLite regression proves that three
immediate retries could exhaust early. That evidence does not identify the
Windows failure's phase. Recording the boundary would let its next stress run
distinguish opening/recovery, BEGIN, statement and commit failures.

Where: `host/src/index/db.rs`, its database tests and
`host/src/index/tests/mp_harness.rs`. Follow-up: `brwr-bszr`.
