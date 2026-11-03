## 2026-11-02 — Stop committing derived queue views in every PR

Change: stop having every PR commit `BOSUN_FEATURE_BACKLOG.md`'s generated
blocks, and give the beads queue a layout GitHub's plain merge can combine.
Why: replaying the PR-branch merges of the last 25 merged PRs with plain text
merge (no drivers) gave 30 conflicting merges in 18 PRs; the queue conflicted
in 13, the backlog in 14 and this feed in 13, and 14 of the 30 conflicted only
in those three files. The backlog conflicts were all inside generated blocks
(the `new` block's insert point and the top of `shipped`, newest first), which
are pure output of the queue. Sorting the JSONL by id would fix only 3 of the
13 queue conflicts (replayed the same way): the rest are both sides editing
the same bead line (often with byte-different but JSON-equal serializations,
as with the held-PR beads), or a new bead next to an edited one. `bd`'s
auto-export also rewrites the file in its own order. Where: render the
backlog's blocks on `main` after merge (a workflow like the PR doctor) and
have PRs leave them alone, and for the queue either one file per bead or a
`bd` export the PRs do not commit.
