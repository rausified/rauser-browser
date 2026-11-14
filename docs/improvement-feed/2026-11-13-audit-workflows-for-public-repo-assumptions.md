## 2026-11-13 — Audit workflows for public-repository assumptions

What to change: check every workflow step that runs with no credential (`persist-credentials: false`, or a fresh clone with no header) for git or API reads that only worked while the repository was public, and add a step-scoped credential where it still needs one (as `scripts/pr-doctor-git-auth.sh` does for the PR doctor).

Why: the repository is private now (`gh repo view --json visibility` returned `PRIVATE` on 2026-11-13), but the PR keeper's checkout comment in `.github/workflows/pr-doctor.yml` still says its step "only fetches a public repo". While splitting the PR doctor (brwr-p2a7, #463), the credential-free repair step could not fetch at all, so fetching moved to a credentialed plan step. Any other step that fetches with no credential fails the same way, and only when it actually runs.

Where: `.github/workflows/*.yml` (`persist-credentials: false` steps and their `git fetch`, `git ls-remote` or `merge-tree` calls), `scripts/pr-keeper.mjs`.
