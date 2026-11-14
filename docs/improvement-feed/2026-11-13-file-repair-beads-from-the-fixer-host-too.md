## 2026-11-13 — File repair beads from the fixer host too, not only from the keeper's Actions job

**What to change.** Let the PR fixer (`scripts/dev/pr-fixer.mjs`) file a missing repair bead itself.
On each pass it would run the keeper's pure `decideKeeper`/`repairsToFile` over the open PRs labelled `needs-merge` with pr-doctor's needs-person verdict, and file through `scripts/dev/create-bead`, as the keeper's `--file-repairs` does.

**Why.** Today a repair bead exists only if the keeper's `File repair beads` step in `.github/workflows/pr-doctor.yml` runs.
That needs the `PR_DOCTOR_TOKEN` secret and a queue bootstrapped from `refs/dolt/data` on a hosted runner.
The repository move to `webRauser` broke that bootstrap path (CI "Queue consistency" still names `rausified`), so the fixer can sit idle while a PR waits.
Building the fixer showed that the one hand-off between the two halves runs through the most fragile step.
The fixer host already has bd and a bootstrapped workspace.

**Where.** `scripts/dev/pr-fixer.mjs` (`runPass`, before deciding), `scripts/pr-keeper.mjs` (export a reusable `needsPersonRepairs(repo)`), `scripts/tests/pr-fixer.test.mjs`.
