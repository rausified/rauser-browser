# Brauser work queue (`beads` branch)

This branch holds only Brauser's work queue: `.beads/issues.jsonl` (the beads
export, authoritative) and `BOSUN_FEATURE_BACKLOG.md` (its rendered view; the
item lists between `GENERATED` markers come from the beads). It shares no
history with `main` and is never merged into it.

Sessions and Cookie push it directly, with no pull request and no build CI.
Use the tool on `main` rather than plain git, from any checkout or worktree
of the repository:

```sh
scripts/dev/queue pull                       # fetch and fast-forward the queue worktree
cd "$(scripts/dev/queue path)"               # <clone>-queue-beads, shared by all worktrees
# edit one bead's row in .beads/issues.jsonl, or the hand-written backlog text
scripts/dev/queue push -m "queue: close brwr-xyz"
```

`push` re-renders the backlog, runs `check_queue_consistency.py`, commits,
and pushes; on a race it rebases with the bead-by-bead merge driver and tries
again. It never force-pushes. `.github/workflows/queue-check.yml` re-runs the
queue check on every push here. The full rules are in `main`'s AGENTS.md,
"Work queue".
