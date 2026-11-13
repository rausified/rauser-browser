## 2026-11-11 — Check PR text only in a workflow that reruns on edit

Change: Any check that reads a PR's title or body belongs in `.github/workflows/pr-hygiene.yml`, whose `pull_request` trigger includes `edited`, not in `ci.yml`. Say so in the header comment of `ci.yml` and in AGENTS.md's `ci.yml` bullet, so the next PR-text check does not land in the build workflow.

Why: The first draft of the privacy scan (brwr-la7i) read the PR body from `ci.yml`. The Codex review caught it: `ci.yml` does not run on `edited`, so a body edited after a green run is never rescanned, and the required gate stays green. Adding `edited` to `ci.yml` would restart the whole platform build (about 40 minutes on macOS) for every body edit. The `PR owner` job already reruns cheaply on edits, so the scan moved there.

Where: `.github/workflows/ci.yml` (header comment), `.github/workflows/pr-hygiene.yml`, AGENTS.md (the `ci.yml` and `pr-hygiene.yml` bullets).
