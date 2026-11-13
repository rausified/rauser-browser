## 2026-11-11 — Post overlap-check's result on every PR

**What to change.** Run `scripts/dev/overlap-check` in `pr-hygiene.yml` for each opened or pushed PR, and post its OVERLAP block as one sticky comment that mentions nobody (the way PR doctor posts its comment). Keep the job advisory: it never fails, the same way the tool always exits 0.

**Why.** The template's `Overlap:` line and the AGENTS.md rule only help when a session remembers to run the tool. On its first live run (lane L5, PR #426) it found two other lanes' open PRs editing `AGENTS.md`, plus six in-progress beads on the same files. Nobody had named either PR. In CI the bead half would need a bootstrapped queue (as `queue-check.yml` already does), and the PR half only needs `gh`.

**Where.** `.github/workflows/pr-hygiene.yml`, `scripts/ownership.py` (`overlaps`, which is pure and takes the PR and bead lists), `scripts/pr-doctor.mjs` (its sticky-comment pattern).
