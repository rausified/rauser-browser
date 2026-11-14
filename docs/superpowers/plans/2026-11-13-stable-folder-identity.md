# Stable macOS selected-folder identity

**Goal:** Keep a selected macOS folder usable across device-number changes while rejecting a replacement folder or volume.

**Architecture:** New macOS selections use the filesystem's persistent volume UUID and directory inode. A safe native dependency reads the UUID; Brauser binds that observation to its already-open directory handle and device. Existing Unix device-and-inode identities remain readable only on a full exact match. No background rewrite of configuration or recovery records occurs. A native picker selection upgrades a folder once; an already-stale legacy selection needs that selection because its original volume UUID was never recorded.

**Tech Stack:** Rust, cap-std, rustix, macOS-only whichdisk (no default features), existing native picker and config revision checks.

**Sources:** `host/src/vault.rs`, `host/src/config.rs`, capture intents and service working-folder checks; live diagnosis recorded in PR #461. Native API: [whichdisk](https://docs.rs/whichdisk/0.7.0/whichdisk/); Apple's [volumeIdentifierKey](https://developer.apple.com/documentation/foundation/urlresourcekey/volumeidentifierkey) is explicitly unsuitable for persistence across restarts.

## Design challenge

- **Challenger:** Codex subagent `identity_design_challenge`, 2026-11-13, with independent `identity_impact_audit`.
- **Alternative:** Ignore the device number, put a marker in each folder, or invoke diskutil for each check. Inode-only comparison cannot distinguish volumes; markers copy with folders; measured diskutil calls took 305–710 ms each. Path checks alone also leave a volume-substitution gap.
- **Outcome:** Use a vouched UUID from whichdisk and bind its device node to the held directory's device, checking the held handle and named path again. Do not cache by device number. Keep strict legacy verification; never accept a stale legacy identity using inode alone. Preserve capture receipts and permit mixed representations only when both independently verify against the same opened directory.

## Open decisions

- **Identity:** persistent volume UUID plus inode on macOS; Windows and other Unix formats stay unchanged. **Status:** resolved by coordinator under John's fix request, 2026-11-13.
- **Legacy migration:** no silent adoption after a device mismatch; one native reselection upgrades that folder. Matching legacy identities remain supported. **Status:** resolved by coordinator to preserve the existing picker boundary.
- **Unavailable UUID:** a filesystem genuinely without a trustworthy persistent UUID retains strict legacy identity; failure reading an already-stored stable identity fails closed. **Status:** resolved by coordinator.
- **Scope:** home, knowledge base and task folders share verification; capture recovery preserves legacy receipts; caches rebuild normally when representation changes. **Status:** resolved by coordinator.

## Local delivery decision

John explicitly directed this change to stop using GitHub Actions after hosted jobs could not start. Complete the local macOS development gate and native headless smoke, run the Windows cross-build/lint with MinGW, and keep the distinction between cross-compilation and native Windows runtime evidence. Publish only completed local checks as commit statuses, with the exact revision, commands and platform limits in the PR. The transitional status names match Mergify's existing requirements; their descriptions must identify them as local results, not as a passed Actions matrix. Preserve the queue and use a fresh `[skip actions]` head, with the marker also in the PR title to suppress a push-triggered workflow on its merge commit. Do not claim repository-wide retirement: this session has repository write access but no permission to disable Actions settings, and the marker does not disable scheduled workflows.

## Tasks

1. Implement stable identities and exact legacy compatibility with simulated device-renumbering and real filesystem tests.
2. Route home, knowledge-base and task-folder checks through the shared verifier; distinguish unavailable and changed folders in diagnostics.
3. Preserve pending single-event and batch recovery across representations when the same held folder verifies both; document the migration and regression evidence.
4. Run the local development gate and one independent Codex review, record completed local results for the exact integration candidate, and merge through Mergify without Actions.
5. Deploy through `scripts/dev/deploy-live`, upgrade the local home and knowledge-base selections through the native picker, and verify live host configuration and note reads.
