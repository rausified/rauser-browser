## 2026-11-10 — Check platform callers when refactoring shared helpers

When removing callers of a shared Rust helper, review the remaining callers'
`cfg` conditions and check whether the helper needs the same platform gate.
PR #392 moved shared Re-learn reads to `reopen_locked`, leaving ordinary
`ConfigStore::reopen` used only by the Unix service and tests; Linux Clippy
passed while Windows production had an unused method. Add this check to
contributor review guidance near the local Rust lint commands in `AGENTS.md`.
- Review 2026-11-12: **Keep.** Add a review prompt to check platform gates on remaining callers after a helper refactor, preventing Windows-only lint failures.
