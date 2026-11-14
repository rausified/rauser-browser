## 2026-11-14 — Measure the PR fixer by merges, not pushes

Change: Judge the PR fixer by "merged without another repair". Record each pushed repair's outcome (merged, failed CI, dequeued, conflicted again) in `receipts.jsonl`, and report that rate before tuning its attempt limit or agent timeout (`brwr-lkdz`).

Why: The fixer went live on 2026-11-14 recording only up to the push. Its receipts would show every push as a success even if the PR later failed CI or conflicted again. A later close appears only as a count (`closed: 2`), not tied to the repair. Cookie's "merged" status had the same blind spot on 2026-11-02: it meant merged on the VM, while the PRs were stuck on GitHub.

Where: `scripts/dev/pr-fixer.mjs` (`runPass` close loop and the repair receipt), `docs/agents/pr-fixer.md` "Reviewing its activity".
