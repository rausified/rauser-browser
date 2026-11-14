## 2026-11-13 — Replace Actions gates with local validation receipts

Change: Define a distinctly named local-validation gate backed by an inspectable receipt containing commit and tree IDs, commands, outcomes and platform coverage. Update Mergify and its batch workflow to consume it, and retire the Actions triggers through an owner-authorized repository change.

Why: The folder-identity fix encountered unavailable hosted jobs and an explicit direction to stop using GitHub Actions. A local replacement must preserve meaningful checks and the merge queue without implying that a native Windows matrix passed when only cross-compilation was possible. Reusing the old check names transparently can deliver the current fix, but a distinct durable contract will avoid ambiguous status provenance and batches waiting for workflows that will not run.

Where: `.mergify.yml`, `.github/workflows/`, `scripts/dev/`, `AGENTS.md`, `README.md` and the PR handoff/evidence contract. Keep local checks bound to the actual integration revision, and state native platform gaps explicitly.

- +1 2026-11-14: The full seven-workflow inventory includes PR repair/keeper, Dolt checks and release packaging as well as tests. The user now requires complete removal of Actions. The [replacement proposal](../superpowers/plans/2026-11-14-replace-github-actions.md) recommends independent Buildkite orchestration with native Mac/Windows workers, exact-integration receipts, preserved Mergify and separate trusted maintenance/release paths; worker capacity and service choice remain open.
