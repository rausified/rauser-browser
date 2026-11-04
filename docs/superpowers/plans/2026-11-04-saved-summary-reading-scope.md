# Saved summary reading scope

Bead: `brwr-30xv`. Goal: make a partial or browser-viewer-only summary
recognizable in Recent and This page before the reader opens it.

## Open decisions

- Resolved by the bead's acceptance: show the agent-reported reading scope
  from recorded evidence; absence on older summaries means unknown.
- Resolved by existing run-card wording: reuse `readScopeText`, with text-only
  rendering. Reading scope is independent of capture truncation.
- Cookie implementation decision: append an optional typed scope to new
  summary events, retain absent fields byte-for-byte, and project it through
  the existing event-body cache. No backfill or cache migration.

## Design challenge

2026-11-04 reviewer `/root/scope_design_challenge` considered deriving scope
from summary-file frontmatter instead. Rejected: the mutable file would make
replay inconsistent and add filesystem reads to history queries. Adopt the
event-backed field. Recovery must try historical payloads in schema order:
current, same overview without scope, then neither optional field. Every
attempt still compares the frozen prepared bytes; changed identity, path,
overview or already recorded scope must refuse. Return the event actually
restored. Display the page's scope even without an overview.

## Delivery

1. Add optional scope to the durable event and wire view; save only validated
   answer scope and preserve exact older prepared summary appends.
2. Project recorded scope through ordinary indexing and replay; keep old
   serialized bodies unchanged when scope is absent.
3. Show shared reading labels beside page summaries and Recent entries.
4. Verify enum rejection, finisher, incremental/cold/reindex history, recovery
   tails, literal UI rendering and legacy omission; run full gates and review
   the complete diff.

## Compatibility

Deploy the host and extension together. Older extensions reject the enriched
summary view. Older hosts reject scoped event bodies in their format-9 cache
and may fail history queries; rebuilding with an older host skips those newer
summary markers. The authoritative files remain present, but rollback does
not promise a complete history view. Forward upgrades read older events with
unknown scope. No ID migration or browser-local note recovery changes.


## Completion and validation

All four delivery steps are implemented. Complete final local gate: 1,609
workspace Rust-reported passes (+5, 2 ignored; sandbox IPC guards apply), all
47 extension test files (+2 cases), generated protocol, build, formatting,
typecheck, lint and both Clippy configurations pass. Accessibility reports two
offline fixture passes and 47 explicit sandbox browser skips. README extras
pass: 12 script-test files, 252 Python checks (92 platform/sandbox skips),
queue/current-state checks and 10,696 inert Markdown renderings.

Cookie read the complete diff fresh. The single independent in-session Codex
review (`/root/scope_design_challenge`, after the full gate) found no
actionable issues. No external harness/API or remote git action was used.
Driver review, real browser/platform gates and publication remain outside
this cycle. Follow-up `brwr-y1q2` proposes filtering Recent by recorded reading
coverage before the response cap. Assigned Re-learn work remains blocked on
its documented open decisions.
