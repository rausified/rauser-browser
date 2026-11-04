# Share one recorded focus session

**Goal:** Complete `brwr-1jeb`: select a recorded focus session in Recent and
copy an exact preview containing only its direct measured activity and context.

**Architecture:** Add a read-only protocol-v8 query over existing indexed
activity bodies. Filter by durable session ID before grouping artifact-days.
The panel reuses the selected-work checkboxes and exact-preview Copy path.

**Tech Stack:** Rust/rusqlite, generated Rust and TypeScript protocol, MV3 panel.

**Sources:** DESIGN.md §5.1/§5.7; `brwr-1jeb`; existing worklog choices and
`host/src/index/focus.rs`.

## Open decisions

- Resolved by the bead: one explicit recorded session, direct measured activity,
  no inferred work, extra capture, agent or network request.
- Resolved by Cookie as coordinator (2026-11-04): preserve unchecked-by-default
  entries and Select all shown. Session selection narrows the available choices.
- Resolved by Cookie: names use the highest recorded revision within the date
  range; renamed sessions remain one identity. Catalog cap is 64 sessions;
  entry, byte, date and capture-gap bounds reuse the existing worklog limits.
- Resolved by Cookie: absent selection returns catalog only. Unknown or
  outside-period IDs return no entries and an explicit unavailable state.
  Catalog and entry omissions are separate. Gaps remain period-wide notices.

## Design challenge

**Challenger:** `/root/focus_design_challenge`, 2026-11-04 (read-only session).

**Alternative:** Filter existing artifact-day choices in the panel, or select
artifacts by session membership and keep their whole-day totals.

**Outcome:** Reject both: whole-day rows have already combined other sessions'
time, titles and URLs. Select direct activity events before aggregation instead.
Adopt the reviewer's simpler existing-event-body query rather than a new table
or log format, and new messages rather than changing existing message shapes.
Keep the selected ID independent of catalog truncation and refuse broader
fallbacks on unsupported hosts.

## Tasks

1. Add bounded wire shapes and the revision-checked native query, sharing the
   current artifact-day formatter and excluding summaries/container copies.
2. Add scope and session selectors to Recent, invalidating preview and selections
   on date, session, mode, configuration, home or host changes.
3. Pin isolation, rename/ID behavior, empty/omitted/out-of-range cases, rebuild,
   compatibility, stale responses and exact clipboard text with offline tests.
4. Run complete gates, read the complete diff freshly, document results and
   leave the reviewed cycle for the driver to commit and publish.

## Progress

Baseline complete required health gate passed on `d4f27de1`. Tasks 1–3 are
implemented. Final complete required gate and supplemental README checks pass:
1,604 workspace Rust-reported passes (+6 new; 2 ignored; sandbox IPC guards
still apply), all 47 extension test files, both Clippy configurations, protocol
freshness, typecheck/lint/build, 12 script test files, current-state and queue
consistency, and 10,696 inert Markdown renderings. The focused built-panel
worklog file passes 45 cases (+18 new). Accessibility passes 2 offline fixtures
and explicitly skips 47 sandbox-blocked browser cases. Baseline Python tooling
also passes 252 cases (92 platform skips); no Python tooling changed.

The fresh manual complete-diff review found and corrected the debounced
config-change window, including recovery after a same-revision reload. One
independent local Codex review (`/root/focus_code_review`) completed with no
actionable findings and independently reran all 45 focused panel tests. Tasks
1–4 are complete in source; the exact commit message is ready for the driver. The driver still owns external
browser/platform gates, independent approval and publication. Assigned
Re-learn apply/panel acceptance remains incomplete and ordinary ID freezing
is unchanged. Catalog paging is recorded as follow-up `brwr-t4gf`.


Handoff: the single commit attempt failed creating `.git/index.lock` because
Git metadata is read-only in this sandbox. No retry, push or merge was made.
The complete green working tree and `.bosun_commit_msg` are ready for the
driver. `brwr-1jeb` is source-complete; external browser/platform validation
and publication remain with the driver.
