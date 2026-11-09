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

- +1 2026-11-09: The supplementary gate found another 36 unguarded captured-pipe callbacks in `scripts/tests/generate-starters.test.mjs`. `brwr-sivz` adds the existing guards without changing assertions; real/simulated denial gives 13 pure passes and 66 explicit skips, and strict simulated denial fails. This reinforces the still-open static checker in `brwr-f774`.
- +1 2026-11-09: `brwr-vyg5` found two parameterized callback groups missed by the previous generator guard pass: 14 origin.exclude and multi-origin CLI cases still asserted against empty denied-pipe output. Include template-literal `test(...)` names and loop-generated cases in the guard audit; normal sandbox, simulated denial and strict denial should all be checked.
