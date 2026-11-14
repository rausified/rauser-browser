## 2026-11-02 — Check that DESIGN.md's cache-file table matches the code

Change: add a phrase check in `scripts/check_current_state.py` that every file
DESIGN.md's cache-directory table lists matches a cache file name constant in
`host/src/index` (`DB_FILE`, `REGEN_LOCK`, `OPEN_LOCK`), and that every such
constant appears in the table. Why: `index-open.lock` reached the code
(8ad206a) without reaching DESIGN.md, and only a manual docs pass caught it.
Where: `scripts/check_current_state.py`, `DESIGN.md` cache-directory table.
- Review 2026-11-12: **Revise.** Add a narrow check of cache filenames to catch documentation drift; avoid brittle prose matching.
