## 2026-11-13 — One file per entry for the shared logs

Development tooling (`brwr-jtd8`, delay-fix rank 6).
In the 40 main commits before this change, every one that touched docs/history.md edited it, and 29 of its 41 hunks appended at the end of the file, where two concurrent PRs always conflict.
New records now go one per file in docs/history/, the improvement feed's layout, and docs/history.md is frozen with its text unchanged, so DESIGN.md's links to its section anchors keep working.
No combined view is committed: a committed aggregate would conflict the same way, because GitHub's mergeability check ignores merge drivers.
`npm run docs:history` prints one, and `scripts/check_split_docs.py` (run by `scripts/check_current_state.py`) checks the names and headings and refuses a record in docs/history.md dated after 2026-11-13.
TESTING.md's Learning log moved to docs/testing-log/ the same way.
AGENTS.md's Current state table (13 of AGENTS.md's 76 hunks in the same window, all in-place row edits) is generated from one file per row in docs/agents/current-state/, and the `brauser-generated` merge driver regenerates it, so two PRs that each change or add a row merge cleanly through the driver and the PR doctor.
AGENTS.md is now one sentence per line (`scripts/sentence-lines.mjs`); the reflow changed only whitespace, which `--compare` proves.
