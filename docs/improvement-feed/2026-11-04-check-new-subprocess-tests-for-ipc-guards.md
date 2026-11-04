## 2026-11-04 — Check new subprocess tests for IPC guards

Add a development-tool check that flags new subprocess-capturing integration
tests without the shared IPC probe, with an explicit exemption for injected
transports or regular-file stdio. Apply it to `scripts/tests/` and extension
test tooling, and run it with `npm run test:scripts` or the Dev tools job.

Evidence: the health gate on baseline `48f76003` caught 11 starter-generator
CLI cases and two PR-doctor integration cases missing the required guard.
They failed here with empty diagnostics or `spawnSync git EPERM`.
`brwr-xkd1` repairs these two suites; ordinary CI alone cannot identify the
next missing guard because its IPC is allowed.
Keep pure tests active, require guards before fixture/tool setup, and retain
strict failures under `BRAUSER_REQUIRE_IPC_TESTS=1`.
