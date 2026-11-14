# PR fixer

`scripts/dev/pr-fixer.mjs` is the agent half of the keeper/fixer split.
The PR keeper (`scripts/pr-keeper.mjs`, GitHub Actions) stays mechanical: when pr-doctor's verdict on a queued PR is "needs a person", it files a P1 repair bead with external ref `gh-pr-<n>-<head12>`.
The fixer is what starts work on that bead with no person involved.
Plan and design challenge: [docs/superpowers/plans/2026-11-13-pr-fixer.md](../superpowers/plans/2026-11-13-pr-fixer.md).

## What one pass does

1. **Lock.**
   It takes `<work root>/pr-fixer.lock`; one fixer runs per host.
   A lock whose process is gone is stale and is replaced.
2. **Decide.**
   It runs `bd dolt pull`, then lists open beads whose external ref is a repair ref, and reads each PR from GitHub.
   - **Close:** the PR merged (the close reason cites the merge commit) or closed.
   - **Skip:**
     - another actor claimed the bead;
     - the bead is exhausted (`fixer-exhausted`, or 2 attempts);
     - the PR is a fork, does not target main, or is labelled `held` or `pr-doctor:skip`.
   - **Wait:** there is no conflict now, or GitHub has not computed mergeability yet.
   - **Repair:** the PR still conflicts with main.
     It repairs at most one PR per pass.
3. **Claim.**
   It runs `bd --actor pr-fixer update <id> --claim`, counts the attempt (`fixer_attempts` metadata) and syncs the queue.
   If the sync fails, the claim is undone and nothing else happens.
4. **Prepare.**
   It fetches main and the PR branch, and checks that the branch is at the head GitHub reports.
   It configures main's merge drivers (`setup-merge-drivers --tools <main>/scripts`), adds a detached worktree at the PR head and runs `git merge --no-ff` of main.
5. **Agent.**
   It runs `codex exec -s danger-full-access`, or `claude -p --dangerously-skip-permissions` with `--agent claude`, once, on [`scripts/dev/pr-fixer/prompt.md`](../../scripts/dev/pr-fixer/prompt.md).
   The run gets a 50-minute limit, its own process group (killed afterwards, with anything it left running) and an environment with no GitHub credential.
   The agent resolves, commits, runs focused checks under `nice -n 19` and writes a report.
   It may answer "cannot resolve", which fails the attempt honestly.
6. **Verify.**
   The runner checks the agent's work itself:
   - no unmerged paths, no unfinished merge and a clean tree;
   - a new commit that descends from both the PR head and main (no rewritten history);
   - no conflict markers in changed files;
   - a clean `git merge-tree` against main's current tip;
   - `node scripts/privacy-scan.mjs`;
   - the report says resolved, with at least one focused check, all passed;
   - `codex exec review --base <main>`, scoped to the resolution, with no `[P1]`.
7. **Publish.**
   It re-reads the PR and stops if the head moved: a live owner's push always wins.
   It checks that the PR body with the fixer's marked note (`<!-- pr-fixer -->`) still passes the PR owner check.
   The Owner line is unchanged: the fixer resolves the conflict, it does not take the PR.
   Then it pushes the merge, never forced, and runs `node scripts/dev/queue-pr.mjs <n> --head <sha>`.
8. **Record.**
   - **Success:** the bead stays `in_progress`, assigned to `pr-fixer`, with `fixer_pushed_sha` and a note.
     A later pass closes it when GitHub reports the PR merged ("Close only what is on main").
     If main moves and the PR conflicts again, the next pass repairs it again, using the same bead and attempt budget.
   - **Failure:** the claim is released (status `open`, no assignee) and the reason is appended to the notes.
     After the second failure the bead gets `fixer-exhausted` and stays open at P1 for a worker or a session.

Every pass appends a line to `<work root>/receipts.jsonl`.
Each repair keeps `runs/<bead>-<attempt>-<time>/` with `run.log` (agent and review output), `report.json` and `body.md`.
The worktree itself is always removed.

## Trust boundaries

- The runner holds the push credential and makes every GitHub write, as Cookie's driver does.
  The agent's environment has no `GH_TOKEN`, `GITHUB_TOKEN`, `PR_DOCTOR_TOKEN`, askpass, SSH agent socket or `credential.*` / `url.*.insteadOf` git config (`agentEnv`).
  The prompt also forbids pushing, `gh` and `bd`.
- Only main's tools run on the runner side.
  The unit fast-forwards its clone of main before each pass, and the queue, privacy and queue-pr commands come from that clone.
  The agent itself runs the PR's code (its tests), as any session repairing the PR would.
- The fixer never merges, never force-pushes and never runs `gh pr merge`; Mergify keeps merge authority.
- Text from the agent's report goes into bead notes and the PR body only through `clean`: one line, no `@` mentions or code-span breaks, capped.

## Running it

```sh
BRAUSER_QUEUE_WORKSPACE=<bd workspace> node scripts/dev/pr-fixer.mjs --repo webRauser/rauser-browser --dry-run
```

`--dry-run` prints each repair bead's decision and changes nothing.
Other flags: `--agent claude`, `--max-attempts`, `--timeout-minutes`, `--work-root` (default `$XDG_STATE_HOME/brauser-pr-fixer`, else `~/.local/state/brauser-pr-fixer`).
Tests: `node --test scripts/tests/pr-fixer.test.mjs`.
It runs real git on temporary repositories, with fake `bd`, `gh`, `codex`, queue sync, privacy scan and queue-pr; nothing reaches GitHub or a real agent.

## Activation

Nothing is installed or enabled anywhere.
Turning the fixer on is an activation and needs John's yes.
The intended host is a VM (the Fleet Keeper design prefers a dedicated fixer VM, so the agent never competes with a worker for CPU), never John's Mac.
On that host:

1. Clone the repository to `~/brauser-pr-fixer/main` (main checked out) and bootstrap a queue workspace: a separate clone, `scripts/dev/queue bootstrap`.
2. Install git, gh, bd, Node 24, and codex (and claude, if used) with its provider login.
   Write `~/.config/brauser-pr-fixer/env` (mode 0600).
   It names `GH_TOKEN`: a fine-grained token with Contents and Pull requests write on this repository only.
   It also names `BRAUSER_QUEUE_WORKSPACE`, `PATH`, and the agent's provider variables.
   Configure git to push with that token (a credential helper that reads `GH_TOKEN`).
3. Run one `--dry-run` pass by hand and read its decisions.
4. Copy `scripts/dev/pr-fixer/brauser-pr-fixer.{service,timer}` to `~/.config/systemd/user/`.
   Then `systemctl --user daemon-reload && systemctl --user enable --now brauser-pr-fixer.timer`, with `loginctl enable-linger` for the user.
5. To stop it: `systemctl --user disable --now brauser-pr-fixer.timer`.
   A running pass finishes its current step and the lock clears; a bead it holds is released by its next failure, or reclaimed with `bd update <id> --status open --assignee ""`.

The keeper files repair beads only when the `PR_DOCTOR_TOKEN` secret is set and its workflow can bootstrap the queue from `refs/dolt/data`.
Check the keeper job's `File repair beads` step before expecting any bead.
