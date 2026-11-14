## 2026-11-01 — Check this feed's format in CI

The AMOS feed's writer refuses an entry without its own dated heading; this
feed is a plain file with no such guard, so a malformed or rewritten entry
would go unnoticed. Change: extend `scripts/check_current_state.py` (it already
runs in the docs-only CI job) to check that `BRAUSER_IMPROVEMENT_FEED.md` keeps
its preamble, that every entry heading matches `## YYYY-MM-DD — Title`, and
that, compared with the base branch, the change only appends.
- Done 2026-11-02: `scripts/check_improvement_feed.py`, run by `scripts/check_current_state.py` (branch `chore/adapter-speed`). It checks the entry point, entry names and headings; there is no base-branch append-only comparison, since each entry is now its own file.
- Review 2026-11-12: **Done.** Checks entry names and headings so malformed suggestions fail CI.
