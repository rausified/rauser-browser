# Continue through Recent (`brwr-a30j`)

Goal: reach every matching artifact-day through explicit Load more, including saved-summary reading filters, with at most 32 rows per native response.

## Open decisions

Resolved within the bead's acceptance: paging is opt-in and additive in protocol v8; old clients retain the old response. Rows use the existing date/focus/title/ID order and latest displayed summary. A changed result refuses continuation and asks the reader to reload; it never silently restarts or mixes results. No content changes or agent runs.

## Design challenge

2026-11-09 reviewer `/root/recent_paging_challenge`: keep stateless full-result hashing. Keyset cursors and event-count watermarks cannot protect mutable focus/title/summary rows; persisted snapshots add lifecycle state. Reuse the worklog scope (storage, physical root identity, adapter fingerprint), bind config revision, query filters, page size, type choices and every rendered matching row within one SQLite transaction. Keep only the requested window while streaming the digest. Preserve legacy bounded queries. Paging scans all matching history and summary lookups per request; its response and memory are bounded, its work is proportional to matching history. This tradeoff follows existing worklog paging and can be optimized behind the same contract later.

## Implementation

1. Add optional request/response page objects; share the existing Recent row selection and latest-summary semantics for streaming snapshot pages.
2. Integrate native validation, scope and conflict handling; preserve legacy shapes and the 32-row cap.
3. Add explicit Load more and Reload results controls, filter/home/connection generation checks and focus transfer only when the user stayed on Load more.
4. Exercise more than 32 matches, sort ties, filters, changed summaries and scope, exhaustion, in-flight filter changes and keyboard focus; review the full diff and run focused/static gates.

Status: implemented and reviewed. Assigned Re-learn apply decisions remain outside this feature.

## Review corrections

Self-review and the independent reviewer both identified Chrome blurring a natively disabled paging button; use the existing worklog pager's guarded `aria-disabled` pattern. The reviewer also identified that a reachable host can exit normally while `connected` stays true: invalidate in the actual `onHostExit` listener, with regressions that resolve an answer and then exit its port before the async consumer resumes. Capture focus ownership before hiding a failed continuation control. The other-home test now populates byte-identical rows so it proves scope isolation rather than merely different content.


## Validation and handoff

233 index tests passed (two existing ignored); 10 Recent-focused Rust and six artifact wire tests passed. 115 extension tests pass across Recent/worklog, host refresh and agent/shape checks, including 15 new panel/metadata cases. Two new real-browser keyboard tests cover exhaustion and conflict; all three selected Recent keyboard cases explicitly skip in this sandbox because Chromium cannot start. The driver owns their unsandboxed execution and the complete suite. Host Clippy and the fast static gates pass. Independent final review reports no remaining P1/P2 findings. Follow-up `brwr-0401` preserves the reading position on returning to Recent; it does not weaken continuation validation.

## Quarantine recovery

Cycle 4 failed the external extension gate because the existing page-summary disconnect regression dereferenced a Recent button after the new host-exit invalidation removed its row. Preserve that invalidation and the page-summary disabling assertion; the repaired regression checks row removal, hidden continuation, and enabled summary buttons after an explicit offset-zero reload. TESTING.md now documents both paths. Recreated the rolled-back reading-position follow-up as `brwr-0401` and replaced its stale citations. This cycle performs repair only.

Recovery validation: protocol generation, the complete workspace build, formatting, both Clippy configurations, npm install, typecheck, lint and extension build pass. The full extension command passes all 50 test files; the explicit non-isolated page-summary/Recent run passes all 86 cases. Workspace Rust tests report 1,677 passed and two ignored, with 94 documented IPC sandbox-skip messages. Accessibility reports two passed and 55 Chromium sandbox skips. Unsandboxed browser and IPC acceptance remains with the driver. Fresh diff review found no additional corrections.
