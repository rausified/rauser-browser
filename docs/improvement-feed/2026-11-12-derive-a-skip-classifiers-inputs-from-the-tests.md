## 2026-11-12 — Derive a skip classifier's inputs from the tests it skips

Change: Any rule that skips a test suite unless certain paths changed needs a guard test that reads the suite's own sources, extracts every checkout path they name, and fails when the rule would not select one. A hand-kept input list alone isn't enough. `scripts/tests/ci-changes.test.mjs` ("every checkout path the Python dev-tools tests name selects them") does this for `devToolsReasons`. Do the same for `scripts/ci-macos.mjs`'s `MACOS_PATHS` and `gate:adapter`'s step triggers wherever their tests read files by path.

Why: The first cut of `devToolsReasons` (#431, delay-fix rank 12) listed the files its author knew the Python tests read. It missed `scripts/dev/queue-pr.mjs` and `scripts/pr-constants.mjs`, which `test_worker_contract.py` reads. A change to only those files would have skipped the suite that checks their contract, both locally and in the platform builds. Codex found the gap in review. The guard test then found a third path, `scripts/register-dev-host.mjs`, on its first run.

Where: `scripts/ci-changes.mjs` (`DEV_TOOLS_INPUTS`), `scripts/tests/ci-changes.test.mjs`, and later `scripts/ci-macos.mjs` and `scripts/gate-adapter.mjs`.
