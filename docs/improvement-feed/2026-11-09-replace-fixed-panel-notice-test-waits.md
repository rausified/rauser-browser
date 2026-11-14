## 2026-11-09 — Replace fixed waits in panel notice tests

Change: Make the panel test harness expose when startup has finished reading stored notices, then have `extension/tests/panel/summarize.test.mjs` await that signal instead of sleeping for 20 ms before sending a new notice.

Why: The Windows build for Mergify queue PR #347 failed `a worker notice for a tab the panel has not followed yet shows once that tab is followed` after the test's fixed wait; 2,840 other tests passed, and the queue retry passed. A startup signal would make this ordering explicit and keep runner speed from deciding the assertion.

Where: `extension/tests/panel/harness.mjs` and `extension/tests/panel/summarize.test.mjs`.
- Review 2026-11-12: **Keep.** Await an explicit notice-read completion signal instead of sleeping 20 ms; Windows had one ordering flake.
