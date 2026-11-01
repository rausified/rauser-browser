# Tasks-page review follow-ups

**Goal:** Finish the seven actionable assigned PR #188 follow-ups, preserving task selection and dismissals across overlapping work.

**Architecture:** Keep the existing page controller, shared run-card persistence and schema generator. Coalesce refresh requests using the tracker pattern; serialize dismissal storage updates with a Web Lock. Tests control timers and host replies.

**Tech Stack:** TypeScript, Node test runner, Rust protocol wire tests, Python schema generator.

**Sources:** Assigned beads `brwr-um4`, `brwr-jzs`, `brwr-90b`, `brwr-3wh`, `brwr-3pv`, `brwr-sm8`, `brwr-0at`; DESIGN.md §5.8; AGENTS.md.

## Design challenge

- **Challenger:** Read-only Codex reviewer `/root/design_challenge`, before implementation.
- **Alternative:** Use one shared refresh promise draining a boolean follow-up flag; let selection follow the latest hash instead of an earlier handler's captured id. Use `Promise.allSettled` for off-page fetches, applying results in candidate order. Wrap the existing dismissal read/write in a Web Lock rather than introducing another persistence format.
- **Outcome:** Adopted. These reuse existing mechanisms and avoid stale selection, batch-wide rejection and cross-document lost updates. Recover the local dismissal queue after a rejected lock request so later dismissals still work. Cursor length aligns with RunView (20–64); this does not introduce a new timestamp parser.

## Open decisions

- **Decision:** Refresh requests during an in-flight refresh trigger one follow-up, even after failure; latest hash wins. **Status:** Resolved by Cookie, 2026-11-01, consistent with the assigned race fix and existing tracker behavior.
- **Decision:** Keep best-effort dismissal persistence and its local-queue fallback when Web Locks are unavailable. **Status:** Resolved by Cookie, 2026-11-01, consistent with existing diagnostic persistence and supported Chrome's Web Locks.
- **Decision:** Cursor timestamps use the existing RunView length range; these schema bounds do not validate timestamp syntax. **Status:** Resolved by Cookie, 2026-11-01, per the assigned schema-alignment request.
- No open user-facing or policy decisions.

## Tasks

1. `brwr-um4`: Coalesce page refreshes and follow the latest hash; cover a deferred old response followed by a newly started run, failure, and teardown.
2. `brwr-jzs`: Use mocked polling timers; cover overlapping refreshes without real-time waits.
3. `brwr-90b`: Coordinate cross-document dismissal persistence; cover independent module instances, failures, caps and deduplication.
4. `brwr-3wh`: Assert unknown run kind rejection at the schema layer.
5. `brwr-3pv`: Verify Reuse attempts to open the source window before falling back to another panel.
6. `brwr-sm8`: Fetch missing unfinished runs concurrently, retaining partial successes and teardown checks.
7. `brwr-0at`: Align cursor timestamp length with RunView; regenerate protocol files and check boundary cases.

## Validation and completion

Initial required health gate passed with a clean worktree and no quarantine report. All seven tasks are implemented. Focused checks pass: 35 Tasks-page cases, 45 run-card cases and 13 protocol run cases. Regeneration leaves both generated type files unchanged.

One read-only Codex code-review pass found a P2 in the same refresh mechanism: with an initially empty history, an intermediate list could select a fallback and overwrite the requested hash. A failing regression reproduces it; skipping selection synchronization on intermediate passes fixes it with the existing test seam. No other actionable findings were reported. The complete product/test/doc diff was also read locally. The full required gate passed again after the review fix: 1,210 reported Rust passes, all 38 extension files, generated/build/fmt/type/lint checks and both clippy configurations. README supplemental checks passed: 85 Python tests, seven Node script files and 10,696 inert Markdown renderings. Sandbox IPC guards returned early on 92 socket cases; platform/browser certification remains outstanding. All seven assigned follow-ups are complete. Independent driver review and publication remain outstanding. No external API or remote operation was used.
