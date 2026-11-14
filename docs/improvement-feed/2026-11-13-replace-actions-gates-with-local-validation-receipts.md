## 2026-11-13 — Replace Actions gates with local validation receipts

Change: Define a distinctly named local-validation gate backed by an inspectable receipt containing commit and tree IDs, commands, outcomes and platform coverage. Update Mergify and its batch workflow to consume it, and retire the Actions triggers through an owner-authorized repository change.

Why: The folder-identity fix encountered unavailable hosted jobs and an explicit direction to stop using GitHub Actions. A local replacement must preserve meaningful checks and the merge queue without implying that a native Windows matrix passed when only cross-compilation was possible. Reusing the old check names transparently can deliver the current fix, but a distinct durable contract will avoid ambiguous status provenance and batches waiting for workflows that will not run.

Where: `.mergify.yml`, `.github/workflows/`, `scripts/dev/`, `AGENTS.md`, `README.md` and the PR handoff/evidence contract. Keep local checks bound to the actual integration revision, and state native platform gaps explicitly.
