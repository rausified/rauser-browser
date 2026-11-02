# Recorded incoming navigation in summaries

**Goal:** Complete brwr-sm1: show where an artifact was reached from in its saved summary and give the summary agent the same bounded recorded context.

**Architecture:** Snapshot the existing artifact view's incoming trails under the service's config lock at summary queue time. Keep that snapshot in the private run record and use it for both page.md and the host-rendered summary, including resume and Retry save. Reuse existing index selection and Markdown escaping.

**Tech Stack:** Rust host/service, SUMMARY.md template, offline Rust fixtures.

**Sources:** brwr-sm1 acceptance, DESIGN.md incoming navigation and summary contracts, host/src/service/{server,section,record,supervisor,finisher,prompt}.rs, host/src/summary.rs.

## Open decisions

- Resolved by the bead's acceptance (John, 2026-11-02): host-recorded navigation supplies context and a host-rendered Incoming list, never an agent-invented list.
- Resolved by Cookie, 2026-11-02: freeze the queue-time snapshot so page context, resume and save retries agree; the file labels it as recorded when the summary was requested, not current provenance.
- Resolved by Cookie: use the current adapter's host-derived artifact identity, never the socket request's record key, for optional lookup. Lookup failure logs and omits context; required configuration checks remain unchanged.
- Resolved by Cookie: at most five rows, 512-character single-line titles, existing 8192-byte URL cap, HTTP(S) without credentials or controls, recognized attributed transitions and valid local dates. Reject a bad URL whole; do not truncate links. No sources means no new section or prompt paragraph, preserving legacy behavior.
- Resolved by Cookie: the incoming section precedes Office/Miro metadata and has its own fixed ending heading. Only summaries get it. A conditional SUMMARY.md paragraph enabled by the persisted snapshot bounds that section and forbids source retrieval or inferred dependency/endorsement. Tasks keep their existing page layout.
- Resolved by existing unreleased run-record policy: the optional serialized field is backward-readable by this build but older services reject it; document rollback incompatibility.

## Design challenge

**Challenger:** /root/incoming_design_challenge (2026-11-02), before implementation.

**Alternative:** Render Incoming only in the saved file, keeping navigation out of the model prompt entirely.

**Outcome:** Keep the frozen snapshot because the bead explicitly requires assessment context. Adopt the reviewer's narrower authority: gate the prompt paragraph on a nonempty persisted snapshot, reuse safe link/title rendering, keep one fixed terminator, update summary-only Office/Miro positional wording, treat lookup failure as optional context loss, and retain the same snapshot on restart/Retry save. Navigation is an observation, not support for an artifact's claims or permission to open another document.

## Tasks

1. Define bounded incoming rendering and add hostile-title/URL and empty-list tests, including the shared Markdown corpus.
2. Derive the optional snapshot in service setup, persist it with a serde default, and place the bounded section in summary page.md. Add offline index and page-boundary tests.
3. Gate SUMMARY.md instructions from the persisted snapshot; verify legacy runs and Office/Miro retrieval boundaries.
4. Render Incoming from the frozen run snapshot on save; verify resume/retry consistency and absence for older runs.
5. Review the full diff, run the full required/README gate, update design/history/queue and close brwr-sm1 only when acceptance holds.

## State

Implemented; ten new incoming-navigation tests and all 39 prompt tests pass. Full required/README gate passes: 1,331 Rust-reported outcomes (92 IPC guard skips), 40 extension test files, both Clippy builds, 10,696 inert Markdown renderings, scripts and queue/dev-tool checks. A11y passes two fixtures and explicitly skips 36 sandbox-blocked browser cases. Complete manual diff review and the single fresh local Codex reviewer pass found no P1–P3 issues. brwr-sm1 is complete; the driver owns its external gate, independent publication review and PR. Assigned Re-learn work remains dependent on human/pr-held brwr-37ca (PR #238); no held preview code is included here.
