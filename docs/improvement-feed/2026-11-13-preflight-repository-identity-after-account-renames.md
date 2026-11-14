## 2026-11-13 — Preflight repository identity after account renames

Change: Resolve the repository owner and name from the configured Git remote locally and `GITHUB_REPOSITORY` in CI, then check that identity before queue, release and worker operations.
Include clone remotes, queue transport and workspace metadata in an account-rename checklist, and keep historical fixtures unchanged.

Why: An account rename left active command examples and schema identifiers pointing at the former owner.
The Orca CLI skill provided an isolated checkout successfully, but the new workspace still reported the former owner in its cached project identity; that identity needs an explicit check too.

Where: `scripts/dev/`, `scripts/work_queue.py`, release instructions in `TESTING.md`, and contributor instructions in `AGENTS.md` and `docs/agents/lane-brief-template.md`.
