<!--
Every PR names its owner session (AGENTS.md "PR ownership and handoff").
The PR owner check fails until the Owner line is filled in. Cookie's driver
PRs (cookie/ branches, "Cookie cycle N:" titles) are exempt.
To merge, queue it only after the final push: `node scripts/dev/queue-pr.mjs <n> --head <sha>`
(it adds the `queue` label and comments the Mergify queue command once GitHub shows
that head; AGENTS.md "Review and merge cadence"). A later push cancels the request:
run it again after every push. Never `gh pr merge` into main.
-->

Owner: <!-- the session that owns this PR, e.g. "claude session 2e52fe72" or "codex session in work/rb-foo" -->
Lane / coordinator: <!-- the lane and the coordinator session that assigned it, or "none (John asked directly)" -->

## Summary

<!-- What changes and why. Link the bead (brwr-xxx) and the plan or spec. -->

Overlap: <!-- paste the OVERLAP block from `scripts/dev/overlap-check --branch` (open PRs and in-progress beads on the same files, with their owners), or "none" -->

## Handoff state

<!-- Keep this current: if this session disappears, the coordinator reassigns the PR from here. -->

- Done:
- Next:
- How to finish if I disappear: <!-- worktree path, branch, remaining steps, review/CI state, known blockers -->

## Checks

<!-- The README.md gate commands run, the codex review pass and its P1 fixes, P2/P3 beads filed. -->
