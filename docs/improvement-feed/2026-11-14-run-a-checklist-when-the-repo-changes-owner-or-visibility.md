## 2026-11-14 — Run a checklist when the repository changes owner or visibility

Change: Add `docs/repo-move-checklist.md`, and link it from AGENTS.md. List
everything that depends on the repository's owner or visibility, and check
each item the day the repository is transferred or made private:
- Actions minutes and the spending limit
- features that need a paid plan on private repositories, such as dependency
  review
- fine-grained tokens scoped to the old owner (the PR doctor, the keeper,
  Backstaff and the workers)
- remotes stored outside the code, such as the Dolt queue remote in the bead
  database
- branch protection
- Mergify's installation

Why: on 2026-11-13 the move to private, then to webRauser/rauser-browser,
broke three things that no code review would have caught:
- dependency review started failing on every PR (fixed in #471);
- CI's queue-consistency check could no longer authenticate against the old
  Dolt remote;
- free Actions minutes ran out, and GitHub refused every job (CI, hygiene,
  doctor and keeper) for about an hour, until billing was fixed.
Each problem was found separately and late. One checklist would have caught
all three on the day.

Where: `docs/repo-move-checklist.md` (new) and AGENTS.md. Optionally, the
keeper can flag a job GitHub refused to start ("job was not started", with
no steps), which is how the billing block first appeared.
