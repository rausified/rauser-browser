# Named selected-work sharing drafts

**Goal:** Complete `brwr-1b6o`: preserve a deliberately selected work update after
the panel closes, then compare its original entries with current evidence.

**Architecture:** Immutable UUID records in browser-local storage, scoped by
exact home root and profile. A separate viewer retains the original preview;
refresh uses existing host queries, never restores live worklog state, and saves
reviewed versions as new records. No source content or host configuration writes.

**Tech Stack:** TypeScript, localStorage, existing native worklog requests,
built-panel Node tests and the real-browser accessibility fixture.

**Sources:** `brwr-1b6o`, DESIGN.md §5.1, `extension/note-recovery.ts`,
`extension/panel.ts`, and the protocol's worklog choice contracts.

## Open decisions

- **Persistence/lifetime:** resolved by Cookie, 2026-11-09: browser-local and
  explicitly described as such; no portability/backup promise or automatic eviction.
- **Identity:** resolved by Cookie: root/profile scope and UUID record keys;
  duplicate names are allowed, and revisions preserve their originals.
- **Refresh selection:** resolved by Cookie: compare original identities only;
  unchanged rows start selected, changed rows require explicit selection. Missing
  requires complete coverage; omitted or bounded scans yield unreached instead.
- **Preview context:** resolved by Cookie after the design challenge: preserve
  rendered session heading and gap footer with the exact original preview. Show
  changed current context separately and require its explicit approval before
  copying or saving a refreshed version. No silent footer regeneration.
- **Policy changes:** resolved by Cookie: invalidate asynchronous refresh on
  config, home, connection or selected-record changes. Ordinary notes and their
  browser recovery remain independent.

## Design challenge

**Challenger:** `/root/draft_design_challenge`, before implementation.
**Alternative:** frozen-text bookmarks rather than host-backed drafts or restoring
the live worklog's dates, sessions, pages and checkboxes.
**Outcome:** adopt browser-local immutable snapshots and a separate viewer. Add
bounded comparison only for saved identities. The reviewer identified incomplete
pagination and changing session/gap context as separate review requirements; both
are incorporated above. Host storage can follow a future portability requirement.

## Tasks

1. Add bounded snapshot storage, shared preview formatting and coverage-aware comparison.
2. Add the saved-draft viewer, explicit review actions and panel lifecycle hooks.
3. Verify storage, pagination, stale replies, home separation and keyboard use.
4. Run the full gate, review the complete diff and update docs/queue/closeout.

## Progress

- Baseline complete required gate passed: 1,641 Rust-reported passing outcomes,
  two intentionally ignored cases, 49 extension test files, both Clippy modes; accessibility
  two fixture passes and 52 explicit sandbox browser skips. No source repair needed.
- Assigned Re-learn blockers remain open policy decisions in their staged plan;
  no apply authority is inferred.

- Implementation complete: immutable scoped records, separate frozen viewer, explicit comparison/review, and unchanged live-selection lifecycle. Twenty-four new offline cases cover persistence, limits, stale context, pagination, missing/unreached evidence and changed session/gap approval.
- Independent complete-diff review found one P2 (`brwr-74bl`): a paged comparison could accept an unpaged continuation and falsely prove absence. Its regression failed before the fix; comparisons now reject that continuation while retaining first-response legacy support. The complete focused sharing/worklog run passes 86 cases. No other confirmed review findings remain.
- Supplementary script verification exposed 36 existing unguarded generator CLI callbacks (`brwr-sivz`). Added the required shared IPC guard only; strict denial remains a failure. The broader static checker proposal `brwr-f774` remains open.

Final verification after review fixes passed the complete required gate and
supplementary Markdown, script and Python tooling checks: 24 new offline cases; 86 focused sharing/worklog cases; 50 extension files; 1,641 Rust-reported passes, two intentionally ignored cases; both Clippy modes; accessibility two fixture passes/53 sandbox browser skips; 10,696 inert Markdown renderings; 13 script files; Python tooling 252 cases/92 platform skips.
Rust IPC guards may return early in this sandbox, so reported passes do not
prove real IPC execution. Real Chromium/strict IPC acceptance, queue
sync/export/consistency, independent driver review and publication remain
driver-owned. Complete feature diff and the small review/guard fixes were
read afresh; no further confirmed findings remain.
