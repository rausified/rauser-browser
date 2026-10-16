# Brauser Feature Backlog

This file is read by Cookie's overnight loop. DESIGN.md is the authoritative
spec: read it before starting any item below, follow its decisions (§14) and
security rules (AGENTS.md), and update it yourself when an implementation
choice changes a contract it describes. Do not treat this backlog as a
substitute for DESIGN.md — it exists to keep each cycle's scope bounded, not
to repeat the spec.

## Known Issues (Blocking Deploy)

- None.

## Proposed

### M1.5a step 6.3 — Adapter config and lease (DESIGN.md §12.2 step 6.3)

Step 6.1 (Chrome probes), step 6.2 (the adapter-template engine), and the
protocol/config slice of step 6.3 are merged. The remaining slices turn the
now-host-validated config into active artifact recognition. Keep the worker
prefilter and host re-check close together: the host check is the trust
boundary, while the worker check prevents unrecognized browsing from ever
entering durable extension storage.

- [ ] **Complete adapter review confirmation and carry confirmed templates in the policy lease.** The config slice now puts every addition, edit, or precedence-changing reorder behind the existing revision-bound native token and includes the unscoped shared-origin disclosure. Expand that review to show match rules, scope, Tier 1 fields, sync caution, and a match preview per DESIGN.md §7.2, then extend the policy lease (§3.2) with the exact confirmed templates alongside the site allowlist.
- [ ] **Worker prefiltering and host re-check.** Wire the TypeScript adapter-templates module (`extension/adapter-templates.ts`) into the worker's capture path so it prefilters navigation events against the leased, confirmed adapter templates before buffering (§3.2) — this is the first real caller of that module outside its own tests. The host must still re-check every delivered event independently and return `suppressed` / `not_an_artifact` for anything that doesn't match, never trusting the worker's prefilter as authoritative. Tests: a simulated compromised worker that delivers an unrecognized URL, or a URL outside the matched adapter's rules or scope, must not be recorded; a `webNavigation` event on an origin the worker was never granted (which Chrome still delivers, DESIGN.md §5.1 Trails) must be dropped before anything is stored.

## In Progress / This Iteration

- None.

## Shipped

- **Category:** New Capability. **Make adapter templates real, host-validated protocol v5 configuration (2026-10-16).** Brauser now round-trips `[adapters] format = 1` and `[[artifacts]]`, validates every definition with the Rust adapter-template authority and enabled-origin policy, surfaces bounded overlap warnings, and puts adapter changes behind native consent. Unsupported future formats remain intact on disk while every extension entry point and the autonomous worker suspend capture; unscoped shared origins disclose every-account capture, existing IDs stay frozen until migrations exist, and page notes and AI settings remain usable during adapter repair. **Metrics:** 12 protocol tests, 8 focused host regressions, and 5 extension suites passing; full build, format, clippy, typecheck, lint, and extension checks green.

## Notes / Ideas for Later

- The configuration foundation is deliberately useful without pretending artifact logging works yet. The highest-value next move is the end-to-end lease/prefilter/host-recheck slice, not another config-only refinement: it is the point where users first get “Brauser remembers the tickets and docs I work on.”
- Protocol v5 currently freezes an existing adapter's `id` template before any alias migration exists. When activity events land, replace that conservative guard with the designed activity-aware migration and explicit alias events rather than relaxing it in place.
- A future adapter editor should explain overlap pairs with adapter names and example URLs, not raw indices. The indices are stable protocol data for the next UI, while settings currently gives a bounded “earlier rule wins” warning.
