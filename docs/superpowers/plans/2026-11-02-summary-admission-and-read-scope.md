# Summary admission and visible read scope

**Goal:** Honor capture pause at automatic-summary admission (`brwr-q0i4`)
and show how much the agent read beside its summary (`brwr-rr93`).

**Architecture:** Reuse the host's locked, refreshed admission check. Project
the existing validated answer scope as an optional wire enum, and render it
through the shared text-only answer line used by cards and Tasks detail.

**Tech Stack:** Rust native host/service, generated protocol, TypeScript panel.

**Sources:** DESIGN.md §5.2 and §5.7; the two beads' acceptance criteria.

## Design challenge

- **Challenger:** Cookie reviewer session `pause_design_challenge`, 2026-11-02.
- **Alternative:** A service-wide capture gate could also catch pauses after
  forwarding, but automatic runs become ordinary summaries there, so it would
  obstruct explicit summaries too. Holding the config lock across IPC would
  obstruct the service's own config read.
- **Outcome:** Keep the single admission predicate under the existing lock,
  before IPC. It refuses requests when the current configuration is paused;
  it does not cancel queued/running summaries or promise cancellation for a
  pause after admission. Reuse the validated answer enum through an optional
  protocol projection, updating the extension's exact-key validator together.

## Open decisions

- **Pause semantics:** resolved by the existing bead and DESIGN.md: automatic
  admission honors pause; explicit summaries remain available. No cancellation
  policy is introduced.
- **Scope display:** resolved by Cookie as presentation within the bead's
  accepted scope: label all three known states, omit unknown legacy scope, and
  keep access-wall messages dominant. Labels describe what the agent reports,
  not an independent completeness guarantee.
- **Compatibility:** resolved by the existing deployment rule: rebuild host
  and extension together; older extensions reject new answer fields. No stored
  summary or run format changes.

## Tasks

1. Reproduce paused admission with fresh and stale in-memory policy, then fix
   the existing locked predicate; explicit starts still reach forwarding.
2. Add optional scope to RunAnswer, generated twins, projection and validator;
   test wire rejection and all host projections including missing legacy scope.
3. Show plain scope labels in summary cards and Tasks detail; test all states,
   legacy answers and access-wall precedence with the built extension.
4. Run the full gate, review the complete diff, and update design, testing,
   queue and cycle documentation before driver handoff.

## Progress

All four tasks complete. The paused-admission regression, scope projection
and built partial-scope card reproduced their missing behavior before the
implementation. All three Rust regressions and eight new built UI cases pass;
125 cases pass across the four focused extension files.

One local Codex reviewer pass found no P1/P2 and one P3 presentation issue:
Chrome collapsed the run card's answer newlines. A one-line `white-space:
pre-wrap` rule now matches Tasks detail. The final full gate passed again
after that correction: 1,369 workspace Rust-reported outcomes (94 explicit
IPC sandbox skips), all 41 extension files, both Clippy configurations,
2 accessibility fixtures with 36 explicit browser skips, and all README
supplementary checks. The non-sent SUMMARY.md contributor comment now names
scope/access as UI-dependent fields; the actual prompt bytes are unchanged.

Full diff reviewed, including generated twins, tests, docs and semantic
queue changes. No further code review pass was needed. Browser/IPC execution
outside the sandbox and independent publication review remain with the
driver; no real CLI harness, live service, external API or git remote was used.
