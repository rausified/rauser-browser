# Brauser Improvement Feed

Improvement suggestions for Brauser: the app, this repository, and how agents
build it. This is the single place for them; for Brauser work it replaces the
AMOS Improvement Feed. The rules are in [AGENTS.md](AGENTS.md#brauser-improvement-feed).

Append new entries at the end, each under its own `## YYYY-MM-DD — Title`
heading. Never rewrite or reorder earlier entries. Reinforce an entry with a
`- +1 YYYY-MM-DD: <evidence>` line under it, and close one with
`- Done YYYY-MM-DD: <PR, commit or bead>`.

## 2026-11-01 — Keep concurrent sessions out of each other's checkout

Two agent sessions worked in the main checkout (`work/rauserBrowser`) at once.
One switched to `chore/panel-drop-redundant-status` and committed while the
other ran `git checkout -b`, so the new branch stacked on the first session's
commit and moved that session's HEAD; untangling it took a cherry-pick onto
`origin/main` and a manual branch restore. The main checkout is also the live
install, so a `cargo build` or `cargo test` there replaces the running
`target/debug/brauser`. Change: have `scripts/dev/worktree-hygiene` (or a new
`scripts/dev/start-branch`) create every new branch as a worktree from
`origin/main`, and refuse `checkout`/`switch` in the main checkout when its HEAD
is not `main` or another session's branch is checked out there. Where: the
"PR ownership and handoff" section of AGENTS.md and `scripts/dev/`.

- Also logged in the AMOS Improvement Feed on 2026-11-01, before this feed
  existed.

## 2026-11-01 — Check this feed's format in CI

The AMOS feed's writer refuses an entry without its own dated heading; this
feed is a plain file with no such guard, so a malformed or rewritten entry
would go unnoticed. Change: extend `scripts/check_current_state.py` (it already
runs in the docs-only CI job) to check that `BRAUSER_IMPROVEMENT_FEED.md` keeps
its preamble, that every entry heading matches `## YYYY-MM-DD — Title`, and
that, compared with the base branch, the change only appends.
