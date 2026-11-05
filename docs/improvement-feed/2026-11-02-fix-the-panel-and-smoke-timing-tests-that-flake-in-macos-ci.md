## 2026-11-02 — Fix the panel and smoke timing tests that flake in macOS CI

Change: find and fix the timing-sensitive waits in the macOS CI tests that
fail on unrelated PRs, or quarantine them with a tracked bead. Each flaky
failure costs a full 15-minute CI cycle and stops auto-merge until someone
reruns the job. Evidence from 2026-11-02: the headless smoke's "basic Capture
Site review" wait (30 s) timed out in runs 41528659106 and 41548127987. A
run-card wait after Summarize & log timed out in 41531737176.
`extension/tests/auto-summary.test.mjs` `notSetUpLeavesTheArtifactEligible`
timed out waiting for `start_auto_summary` on PR #258, which changes no
extension code; it passes 3 of 3 locally. Where: `scripts/smoke-macos.mjs`
waits, `extension/tests/auto-summary.test.mjs`, and a rerun-once-on-known-flake
step in `.github/workflows/build-os.yml` as a stopgap.

- +1 2026-11-05: PR #318's new saved-command keyboard test asserted focus immediately after an asynchronous host read, without waiting for the editor's fields. Extend the offline browser fixture with selectable per-request delays to expose similar missing readiness waits; the repair adds a delayed command-settings reply and waits for the first enabled field. Chromium is sandbox-blocked locally, so the browser result still needs the unsandboxed commit gate.
