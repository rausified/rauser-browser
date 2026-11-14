## 2026-11-10 — Run pr-doctor when a PR branch is pushed

Change: Add a `pull_request: synchronize` trigger to `.github/workflows/pr-doctor.yml`, limited to PRs labelled `needs-merge`. Then the label clears within a minute of a pushed fix, instead of waiting for the next push to main.

Why: Mergify's `queue_conditions` exclude `needs-merge`, and only pr-doctor removes it. The `*/20` cron runs only every 4–7 hours on this personal repository: 2 scheduled runs against 41 push runs on 2026-11-09. #384 had no conflict and green checks, yet waited 62 minutes at Mergify "waiting" until the label was removed by hand. It merged 3 minutes later.

Where: `.github/workflows/pr-doctor.yml`, `scripts/pr-doctor.mjs` (its `clean` branch already clears a stale label), and the PR doctor bullet in `AGENTS.md`.
- Review 2026-11-12: **Done.** Run PR doctor after a PR push to clear stale conflict labels promptly.
- Done 2026-11-12: verified on main at 4cd617d; PR doctor runs on synchronize for needs-merge PRs.
