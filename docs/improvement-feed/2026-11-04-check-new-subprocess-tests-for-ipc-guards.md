## 2026-11-04 — Check new subprocess tests for IPC guards

Add a focused development check for tests that capture child-process output
without the shared IPC guard, with explicit exemptions for injected transports
and regular-file stdio. The repair for `brwr-xkd1` found 19 generator CLI
fixtures and two PR-doctor integrations that depend on child pipes;
normal CI with IPC available cannot reveal that missing boundary. Keep pure
unit tests active and require denial to fail in strict CI mode.

Implement the check alongside `scripts/tests/` and run it through the existing
Dev tools or script gate. Track it as `brwr-f774`; this repair only reuses
`scripts/tests/ipc-guard.mjs` and does not add a checker.
