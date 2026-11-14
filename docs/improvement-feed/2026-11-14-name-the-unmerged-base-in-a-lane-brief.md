## 2026-11-14 — Name the unmerged base in a lane brief

Change: When a lane brief asks for edits to doc sections that exist only in an open PR, name that PR as the base (or say to stack on it) instead of "branch from origin/main".

Why: The brwr-lkdz lane B brief said to branch from origin/main and edit `docs/agents/pr-fixer.md` "Reviewing its activity" and the plan's Status "Left" section.
Both existed only in the unmerged #475, so the lane had to find that PR and stack on its commit, and its PR carries #475's diff until #475 merges.

Where: the coordinator's lane briefs; AGENTS.md "PR ownership and handoff" (`scripts/dev/overlap-check --branch` shows the overlap, but only after the edit).
