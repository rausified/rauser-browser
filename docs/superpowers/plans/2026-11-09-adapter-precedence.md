# Preserve recorded identity across adapter reorders

**Goal:** Close `brwr-qbub`: ordinary settings saves cannot change a recorded
page's identity by reversing overlapping live adapters.

**Architecture:** Add a host transition check beside the existing retirement
guard. Match contract occurrences across the save, then inspect reversals using
the existing conservative overlap engine. Keep configuration replacement and
revision handling in `ConfigStore`.

**Tech Stack:** Rust host, existing adapter-template engine and ConfigStore tests.

**Sources:** DESIGN.md §6.2, `brwr-qbub`, `host/src/config.rs`.

## Open decisions

- **Decision:** Does a reversal require the identity freeze? **Status:** resolved
  by Cookie, 2026-11-09, from the existing record-key freeze: refuse overlapping
  differently keyed reversals while migrations are unavailable.
- **Decision:** Does declared containment excuse reversal? **Status:** resolved
  by the existing first-match contract: no. Retiring a contained adapter into
  its container is already allowed, but moving that container ahead changes
  the primary identity of pages the contained adapter still recognizes.
- **Decision:** Check brand-new insertions and recognition expansion too?
  **Status:** resolved by Cookie: track these separately; this slice checks
  reordered existing identities, including simultaneous recognition edits.
  It does not claim to prove arbitrary template inclusion or safe handoffs.

## Design challenge

**Challenger:** `/root/reorder_design_challenge`, before implementation.
**Alternative:** Reuse the history guard or match exact contracts only.
**Outcome:** Reject those alternatives: live contracts are skipped by that
guard, its container exception is unsafe for reversal, and edits can hide an
exact-match reorder. Adopt separate precedence checking with exact matches
reserved before recognition-preserving matches and same-identity fallbacks.
Consume occurrences to avoid treating unchanged duplicate siblings as moves.
When looking for an earlier contract's retained position, prefer its first
recognition-preserving successor; this also protects consolidation such as
`[A_short, B, A_long] → [B, A_long]`. Use the complete overlap report, including
declared containers. Conservative refusals remain possible without URL sampling.

## Tasks

1. Reproduce changed first-match identity through real revision-checked saves,
   covering edits, container reversals and consolidated siblings.
2. Add occurrence matching and precedence refusal; preserve disjoint and
   same-identity reorders and unchanged duplicate configurations.
3. Run focused configuration tests and static checks; review the complete diff
   and document the resulting contract and remaining insertion/expansion gap.

## Progress

- Complete: failing-first real-save regression reproduced the identity change;
  host precedence check now rejects it before configuration replacement.
- Five new tests cover 15 type/ID/edit variants, both container directions,
  consolidated siblings, duplicate removal, disjoint and same-key reorders.
  All 138 configuration tests and four capture-site module tests pass.
- Host library/tests Clippy with warnings denied, generation check, Rust
  formatting, TypeScript, ESLint, current-state check and diff whitespace pass.
  Full platform/extension suites remain the driver's external commit gate.
- Complete fresh diff review and one independent reviewer pass: no P1/P2
  findings. No migration or ordinary ID-freeze relaxation is enabled.
- `brwr-98mo` tracks insertion/expansion without reversal; the improvement feed
  records the concrete follow-up. Assigned durable-apply decisions remain open.
