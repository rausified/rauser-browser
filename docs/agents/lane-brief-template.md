# Lane brief template

Use this when a coordinator splits Brauser work into parallel lanes, one agent per lane.
Copy the "Brief" section into a shared `COMMON.md` for the batch, then write one short spec per lane that names its bead, its files and its done condition.
The rules come from the 2026-11-09 and 2026-11-10 lane batches, where lanes that ran inside one session, polled in the background or stopped at "PR opened" cost hours each.

## Coordinator setup

- **Durable workers for long batches.** A batch expected to take more than an hour runs each lane as a durable worker (an Orca worker, or another worker that survives the coordinator's session), never as subagents of one session.
  Subagents die with their parent, and their work is invisible to anyone else.
- **A lane ledger.** Keep `<state dir>/<batch>/lanes.json` outside the repository, written only by the coordinator.
  Each lane records its id, worker, worktree path, branch, bead ids, PR number, head SHA and done condition, and its state (`briefed`, `working`, `pr-open`, `reviewed`, `queued`, `merged`, `closed`).
- **Claim the bead before briefing.** Find or create the lane's bead (`scripts/dev/create-bead`), set it `in_progress`, and run `scripts/dev/queue sync` before the worker starts, so no other lane or worker picks it up.
- **One task per agent.** A lane has one bead or one tightly related group of beads.
  A follow-up found during the lane becomes a new bead and, if it is to be built now, a new worker with its own brief.
- **Freeze shared files.** When a lane rewrites a shared file, tell the other lanes it is frozen until that PR merges.
- **Finisher sweep after any restart.** After the coordinator's session, a worker or the machine restarts, list every open PR the batch owns that is green but unmerged, every PR that left the queue, and every worktree with no live worker.
  Requeue, reassign or clean each one (`scripts/dev/pr-watch.mjs <n> --once`, `scripts/dev/worktree-hygiene`) before starting new work.

## Brief

Paste this into the batch's `COMMON.md` and fill in the placeholders.

- **Your lane.** You are lane `<id>`, coordinated by `<coordinator session>`.
  Read this file, then your spec `<spec path>`, and follow both.
  Do only your lane's task, yourself: no subagents unless the spec asks for them.
- **Your worktree only.** Work in the worktree created for you.
  Never run checkout, switch, reset or stash in the shared checkout, and never touch another lane's worktree.
- **Read first.** `AGENTS.md` ("Local checks", "Review and merge cadence", "PR ownership and handoff", "Worker contract", "Work queue") and `TESTING.md`.
- **Toolchains.** Node 24.14.0 and the Rust version in `rust-toolchain.toml`, with your worktree's own `./target`.
- **Claim, then sync.** Confirm your bead is `in_progress`; if the coordinator has not claimed it, claim it and run `scripts/dev/queue sync`.
  If sync reports a conflict, stop and report it.
- **Check overlap before editing.** List the open PRs and `in_progress` beads that touch the files you will change (`scripts/dev/overlap-check` once it lands).
  Until then, check both: open PRs with `gh pr list --repo webRauser/rauser-browser --json number,files`, and claimed work that has no PR yet with `bd list --status in_progress` and the batch's `lanes.json`.
  Name every overlap in your report and PR body.
- **Foreground, bounded waits only.** Run each command in the foreground with an explicit timeout.
  Never arm a monitor or background job and go idle: completion notices are not delivered reliably.
  Split a suite that needs longer than one call allows.
- **TDD and history.** Write a failing test first for any behaviour change.
  Never `git stash`, amend, rebase or force-push; integrate main with `git merge origin/main`.
- **Load-aware gates.** Run focused checks locally and leave full suites to CI when the machine is loaded (AGENTS.md "Review and merge cadence").
- **Synthetic data only.** No real tenant, company or person names, and no private paths, in the repository, commits or PR bodies.
- **PRs.** Never open a draft.
  Fill in `.github/pull_request_template.md`, with `Owner:` naming the session that will finish the PR (your worker session and `lane <id>`) and `Lane / coordinator:` naming the coordinator, and check it with `node scripts/pr-owner.mjs --body-file <file>`.
  If the batch's `COMMON.md` names a different owner, follow it.
- **Do not queue before review.** The coordinator reviews, then queues.
  After the coordinator says "reviewed", do not push again unless asked: a push cancels the queue request.
- **One Codex pass.** Run `codex exec review --base origin/main` once and fix every P1; file P2 and P3 findings as beads.
- **Done means merged and closed.** A lane is done when its PR is merged on `origin/main` and its bead is closed with the PR named in the close reason.
  "PR opened" or "checks green" is a handoff point, not done.
- **Handoff receipt.** When you stop, for any reason, report: PR number and head SHA, the time you read that state, each gate run with pass or fail, the Codex result, design decisions (one line each), beads claimed or filed, overlaps found, the worktree path, and the next step.
  Write the same report to `<state dir>/<batch>/<lane>-report.md`.
- **Improvement feed.** Add one entry under `docs/improvement-feed/` only for a genuinely new lesson.

## Lane spec

A lane spec is short:

- **Lane and ranks.** The lane id and the backlog items or beads it owns.
- **Deliverables.** Each file or behaviour, with the files it may touch.
- **Out of scope.** Files another lane owns or has frozen.
- **Evidence.** The gates and checks that prove it, and the done condition.
