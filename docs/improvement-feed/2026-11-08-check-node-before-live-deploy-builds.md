## 2026-11-08 — Check Node before live deploy builds

Change: Make `scripts/dev/deploy-live` check the active Node version and its ability to import TypeScript before staging the host build. If it is unsupported, print the required version and how to select it, then stop before changing either live artifact.

Why: The 2026-11-08 live refresh stopped at `npm run build:extension` under Node 22.15.0 with `ERR_UNKNOWN_FILE_EXTENSION` for `extension/brand.ts`. The same deploy succeeded under the repository CI version, Node 24.14.0. A preflight would make this failure immediate and actionable.

Where: `scripts/dev/deploy-live`, its tests in `scripts/tests/test_deploy_live.py`, and the local setup guidance in `README.md`.

- Done 2026-11-09: PR #345, deployed at 75a00a714fc6.
