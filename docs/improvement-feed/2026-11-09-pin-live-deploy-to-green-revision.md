## 2026-11-09 — Pin live deploy to a green revision

Change: Let `scripts/dev/deploy-live` accept an expected `main` SHA and refuse if the fast-forward pull resolves to another commit. Have its dry run report the remote and local SHAs explicitly, including when the local checkout is behind. A release coordinator can then watch CI for one SHA, invoke the guarded deploy with that SHA, and stop if main moved. Keep the active-run and build checks in the existing deploy path.

Why: At 05:38 UTC on 2026-11-10, CI had already passed for `0951d964b98f`, but the local checkout was 64 commits behind. The dry run said “already up to date at `7b0ce56e6b73`” because it did not pull; its actual preflight showed no active runs. The subsequent deploy safely fast-forwarded and built the intended SHA, but the operator had to compare GitHub CI, the remote ref, the local checkout and the final build manually. Main changed again shortly afterward, so an unpinned “latest main” can shift between approval and deploy. A SHA guard makes that handoff explicit without weakening any release gate.

Where: `scripts/dev/deploy-live`, `scripts/tests/test_deploy_live.py`, and the live-deploy instructions in `AGENTS.md`. Accept when a green expected SHA deploys, a changed main fails before publishing either binary, and the dry run accurately describes a behind checkout.
- Review 2026-11-12: **Keep.** Require the exact tested main commit during live deploy; main may move after CI approval and a dry run can show an old checkout.
