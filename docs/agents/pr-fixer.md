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
     - the PR is a fork, does not target main, or is labelled `held` or `pr-doctor:skip`;
     - the PR has no `queue` label: its owner took it out of the queue, and the fixer never puts it back.
   - **Queue:** the fixer pushed the current head but `queue-pr.mjs` did not succeed then (`fixer_queue_status` is not 0), so it removes a leftover `needs-merge` and asks again, even after the attempt budget is spent.
   - A PR merged into a branch other than main is skipped, not closed.
   - **Wait:** there is no conflict now, or GitHub has not computed mergeability yet.
   - **Repair:** the PR still conflicts with main.
     It repairs at most one PR per pass.
3. **Sandbox probe.**
   Before claiming, it runs a probe inside the agent sandbox and refuses the pass (`refused`) unless every secret path, and the runner's own `/proc/<pid>/environ`, is hidden there.
   The sandbox is bubblewrap, and the secret paths are `~/.config/brauser-pr-fixer`, `~/.config/gh`, `~/.git-credentials`, `~/.config/git/credentials`, `~/.ssh` and `~/.netrc`.
4. **Claim.**
   It runs `bd --actor pr-fixer update <id> --claim`, counts the attempt (`fixer_attempts` metadata) and syncs the queue.
   If the sync fails, the claim is undone and nothing else happens.
5. **Prepare.**
   It fetches main and the PR branch, and checks that the branch is at the head GitHub reports.
   It configures main's merge drivers (`setup-merge-drivers --tools <main>/scripts`), adds a detached worktree at the PR head and runs `git merge --no-ff` of main.
6. **Agent.**
   It runs `codex exec -s danger-full-access`, or `claude -p --dangerously-skip-permissions` with `--agent claude`, once, on [`scripts/dev/pr-fixer/prompt.md`](../../scripts/dev/pr-fixer/prompt.md).
   The run gets a 50-minute limit, its own process group (killed afterwards, with anything it left running) and an environment with no GitHub credential.
   The agent resolves, commits, runs focused checks under `nice -n 19` and writes a report.
   It may answer "cannot resolve", which fails the attempt honestly.
7. **Verify.**
   The runner checks the agent's work itself:
   - no unmerged paths, no unfinished merge and a clean tree;
   - a new commit that descends from both the PR head and main (no rewritten history);
   - no conflict markers in changed files;
   - a clean `git merge-tree` against main's current tip;
   - `node scripts/privacy-scan.mjs`;
   - the report says resolved, with at least one focused check, all passed;
   - `codex exec review --base <main>`, scoped to the resolution, with no `[P1]`.
8. **Publish.**
   It re-reads the PR and stops if the head moved, the `queue` label is gone, or `held` or `pr-doctor:skip` was added: a live owner's push or withdrawal always wins.
   It checks that the PR body with the fixer's marked note (`<!-- pr-fixer -->`) still passes the PR owner check.
   The Owner line is unchanged: the fixer resolves the conflict, it does not take the PR.
   Then it pushes the merge, never forced, removes `needs-merge` (it has just verified the merge against main's tip) and runs `node scripts/dev/queue-pr.mjs <n> --head <sha>`.
9. **Record.**
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
  The agent runs as the same user, so an environment without the token is not enough.
  The agent and the review therefore run under `bwrap --dev-bind / / --unshare-pid --proc /proc` (`SANDBOX_ARGS`), with an empty tmpfs or an empty file over each secret path (`maskArgs`).
  Their own PID namespace and procfs keep the runner's `/proc/<pid>/environ`, which holds `GH_TOKEN`, out of reach.
  The probe proves those paths are unreadable there before every repair, so a host without a working bubblewrap gets `refused` and nothing else happens.
  Their environment also has no `GH_TOKEN`, `GITHUB_TOKEN`, `PR_DOCTOR_TOKEN`, askpass, SSH agent socket or `credential.*` / `url.*.insteadOf` git config (`agentEnv`).
  Keep every credential the runner uses under one of the secret paths.
  The prompt also forbids pushing, `gh` and `bd`.
- Only main's tools run on the runner side.
  The unit fast-forwards its clone of main before each pass, and the queue, privacy and queue-pr commands come from that clone.
  The agent itself runs the PR's code (its tests), as any session repairing the PR would.
- The fixer never merges, never force-pushes and never runs `gh pr merge`; Mergify keeps merge authority.
- Text from the agent's report goes into bead notes and the PR body only through `clean`: one line, no `@` mentions or code-span breaks, capped.

## Reviewing its activity

- `~/.local/state/brauser-pr-fixer/receipts.jsonl`: one JSON object per line, format v2 (older lines without `v` are v1 pass lines, which readers still accept).
  Every v2 line has `v: 2`, `kind` and `at` (ISO-8601 UTC).
  - `kind: "pass"`: one per pass, with `outcome` (locked, dry-run, idle, closed, queued, refused, claim-refused, pushed, failed), `closed`, `queued`, `durationMs`, and `code` for refused or claim-refused.
  - `kind: "repair"`: one per repair attempt, written before its pass line.
    It has `bead`, `pr`, `attempt`, `maxAttempts`, `outcome` (pushed or failed), `code` (a failure code, or `ok`), `reason` (at most 300 characters), `files` (the conflicted paths, at most 50), `changedFiles`, `agent`, `agentVersion`, `tokens` (or null), `durationsMs` {`agent`, `review`, `total`} (null for a step that did not run), `review` {`p1`, `p2`, `p3`} (or null), `exhausted`, and for a push `sha` and `queueStatus`.
  - `kind: "outcome"`: what became of a bead the fixer pushed, written when a later pass sees it, once per bead, pushed sha and result.
    It has `bead`, `pr`, `result` (merged; closed-unmerged; reconflicted, when the pushed PR conflicts with main again; requeue-failed, when a queue-pr retry failed), `pushedSha`, `mergeCommit` for a merge, `attemptsUsed` and `hoursSincePush`.
  - The failure codes are fixed: sandbox-refused, claim-refused, unsafe-branch, head-moved, merge-failed, agent-timeout, agent-no-report, agent-unresolved, checks-failed, unmerged-paths, uncommitted-merge, dirty-tree, no-commit, history-rewritten, conflict-markers, main-moved, privacy-failed, review-incomplete, review-p1, pr-moved, pr-labelled, pr-left-queue, owner-check, push-failed, exception and ok.
- `~/.local/state/brauser-pr-fixer/runs/<bead>-<attempt>-<time>/`: `run.log` (agent and review output), `report.json` (the agent's focused checks), `body.md`.
  Each pass that is not a dry run first deletes run folders older than `--keep-days` (default 30) with `pruneRuns`, and logs how many it removed.
  It never touches `receipts.jsonl`, and refuses symlinks and anything that resolves outside `runs/`.
  `--keep-days` must be at least 1, because pruning runs before the lock and a shorter window could remove a pass that is still running.
- The bead's notes and `fixer_*` metadata (`bd show <id>`).
- `journalctl --user -u brauser-pr-fixer.service` on the host.

To summarize the receipts, run this on the host:

```sh
node scripts/dev/pr-fixer-report.mjs [--work-root DIR] [--since 30d|12h|all|<ISO date>] [--json]
```

It prints passes by outcome; repairs attempted, pushed and failed; failure codes by count; the success rate; median and p90 agent, review and total durations; total tokens; the most conflicted files; and the re-conflict and requeue-failed counts.
`--json` prints the same as JSON; unreadable lines are counted and skipped.
`--since` (default 30 days) limits passes, repairs and outcomes to that window, while a pushed repair's fate is read from every outcome in the file.

**The success measure is "merged without another repair", not "pushed".**
A pushed repair succeeds when its bead later gets a `merged` outcome with no `reconflicted` outcome between the push and the merge.
The rate is successes over decided pushes (merged, re-conflicted or closed unmerged); pushes with no outcome yet are reported as pending, outside the rate.
Read it before tuning the attempt limit or the agent's timeout.

## Running it

```sh
BRAUSER_QUEUE_WORKSPACE=<bd workspace> node scripts/dev/pr-fixer.mjs --repo webRauser/rauser-browser --dry-run
```

`--dry-run` prints each repair bead's decision and changes nothing.
Other flags: `--agent claude`, `--max-attempts`, `--timeout-minutes`, `--keep-days` (default 30), `--work-root` (default `$XDG_STATE_HOME/brauser-pr-fixer`, else `~/.local/state/brauser-pr-fixer`).
Tests: `node --test scripts/tests/pr-fixer.test.mjs scripts/tests/pr-fixer-report.test.mjs`.
It runs real git on temporary repositories, with fake `bd`, `gh`, `codex`, queue sync, privacy scan and queue-pr; nothing reaches GitHub or a real agent.

## Activation

**Active since 2026-11-14 on the `constitution` VM** (John's yes, 2026-11-13 23:14 MST).
It runs as the `ubuntu` user, with the timer `brauser-pr-fixer.timer` enabled and linger on.
Its clones are `~/brauser-pr-fixer/main` (tools) and `~/brauser-pr-fixer/queue` (bd workspace), and its env file is `~/.config/brauser-pr-fixer/env`, which holds `GH_TOKEN`, `BRAUSER_QUEUE_WORKSPACE` and `PATH`. codex uses the host's existing login.
The first real pass (2026-11-14T06:55Z) closed two stale repair beads, `brwr-5d5n` (#428) and `brwr-zjwi` (#431), whose PRs had already merged; the next pass was `idle`.
No conflict has been repaired yet.
`constitution` was chosen over a new fixer VM because it is Brauser's dev box with no production service, already has bubblewrap's user-namespace exception, and Cookie is paused.
It has only 2 CPUs, so when Cookie resumes the two share them (the unit caps the fixer at `CPUQuota=150%` and `Nice=19`); move the fixer to its own VM if Cookie's cycles slow down.

The token is a webRauser token for this repository.
The runbook asks for Contents and Pull requests write only; the installed token also has Administration, which John accepted on 2026-11-14.
Replacing it with a narrower one needs only a new env file and no other change.

To set it up again on another host:

1. Clone the repository to `~/brauser-pr-fixer/main` (main checked out) and bootstrap a queue workspace: a separate clone, `scripts/dev/queue bootstrap`.
2. Install git, gh, bd, bubblewrap (`bwrap`; Ubuntu also needs its AppArmor profile for unprivileged user namespaces), Node 24, and codex (and claude, if used) with its provider login.
   Create the directory first (`install -d -m 700 ~/.config/brauser-pr-fixer`): nothing else makes it, and writing the file into a missing directory fails.
   Write `~/.config/brauser-pr-fixer/env` (mode 0600).
   It names `GH_TOKEN`: a fine-grained token, resource owner `webRauser`, with Contents and Pull requests write on this repository only.
   Enter the token at a hidden prompt on the host, never in a chat or a command line: `ssh -t <host> 'read -rs -p "GH_TOKEN: " t; echo; printf "GH_TOKEN=%s\n" "$t" > ~/.config/brauser-pr-fixer/env; chmod 600 ~/.config/brauser-pr-fixer/env'` (without `-t` the prompt is invisible and the command only seems to hang).
   It also names `BRAUSER_QUEUE_WORKSPACE`, `PATH` (include `~/.cargo/bin` when codex lives there), and the agent's provider variables.
   Configure git to push with that token in **each clone's own config**, not globally.
   First reset the inherited helpers with an empty `credential.helper` entry, then add one that prints `username=x-access-token` and `password=$GH_TOKEN` for `get`.
   A worker host already has a global GitHub helper for its own token, which would otherwise answer first.
   Cookie's, for example, cannot read this repository since it moved from `rausified`.
3. Run one `--dry-run` pass by hand and read its decisions.
4. Copy `scripts/dev/pr-fixer/brauser-pr-fixer.{service,timer}` to `~/.config/systemd/user/`.
   Then `systemctl --user daemon-reload && systemctl --user enable --now brauser-pr-fixer.timer`, with `loginctl enable-linger` for the user.
5. To stop it: `systemctl --user disable --now brauser-pr-fixer.timer`.
   A running pass finishes its current step and the lock clears; a bead it holds is released by its next failure, or reclaimed with `bd update <id> --status open --assignee ""`.

The keeper files repair beads only when the `PR_DOCTOR_TOKEN` secret is set and its workflow can bootstrap the queue from `refs/dolt/data`.
Check the keeper job's `File repair beads` step before expecting any bead.
