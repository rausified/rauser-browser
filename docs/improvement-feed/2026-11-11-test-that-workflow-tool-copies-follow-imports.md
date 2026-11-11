## 2026-11-11 — Test that a workflow's copied tools include every import

Change: When a workflow copies scripts out of the checkout before running them (pr-doctor copies main's tools to `$RUNNER_TEMP` so no PR code runs), keep a test that walks the copied entry point's relative imports and fails when the copy step misses one. `scripts/tests/pr-keeper.test.mjs` now does this for `pr-doctor.mjs`; do the same for any other copied tool.

Why: Moving the Mergify client out of `pr-watch.mjs` into `scripts/dev/queue-pr.mjs` (the PR keeper lane) made `pr-doctor.mjs` import two new files. Every local test still passed, because the tests import the files in place, but the workflow's `cp` copied only `pr-doctor.mjs`, so the doctor would have failed on its first CI run with a module-not-found error, and a broken doctor runs no repairs and says nothing on the PRs.

Where: `.github/workflows/pr-doctor.yml` ("Keep main's tools outside the working tree"), `scripts/tests/pr-keeper.test.mjs`.
