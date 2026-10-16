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

Step 6.1 (Chrome probes) and step 6.2 (the adapter-template engine, both
Rust and TypeScript) are already merged. Step 6.3 is the next unbuilt piece
of M1.5a, and it is the first time the adapter-template engine actually gets
wired into the host and extension — everything before it was pure library
code with no protocol, config, or UI surface. DESIGN.md's own text splits
step 6.3 into three dependent parts; the items below follow that split so
each is a boundable cycle (or a few) rather than one large one. Update
DESIGN.md's "M0 as built" / "Next steps" narrative (§12.2) as each lands,
the same way step 6.2 was recorded.

- [ ] **Protocol v5: `[[artifacts]]` config schema, validated by the adapter-templates crate.** Add the `[[artifacts]]` table and `[adapters] format = 1` to `protocol/schema.json` (bump protocol version 4 → 5; M2 step 1 already took v4) and to the host's config loading, validating with the already-merged `adapter-templates` crate (host/, `adapter-templates/`). Enforce `scope` and the shared-origin list, and surface overlap warnings, per adapter-templates.md §6 and §7.2. Tests (DESIGN.md §12.2 step 6.3): an unsupported `format` suspends policy without deleting any existing data; an `id` change on an adapter with recorded activity is refused (§6.2); an adapter whose origin is not enabled; an unscoped shared-origin adapter whose native confirmation carries the "every account in this profile" disclosure text.
- [ ] **Native confirmation for adapter config changes, and a policy lease that carries confirmed adapter templates.** Extend the existing native-confirmation pattern (the same shape as the M1 folder picker and M2 harness setup token flow, host/src/consent.rs) to cover adapter config changes per DESIGN.md §7.2. Extend the policy lease (§3.2) so it carries the confirmed adapter templates down to the worker, the same way it already carries the site allowlist.
- [ ] **Worker prefiltering and host re-check.** Wire the TypeScript adapter-templates module (`extension/adapter-templates.ts`) into the worker's capture path so it prefilters navigation events against the leased, confirmed adapter templates before buffering (§3.2) — this is the first real caller of that module outside its own tests. The host must still re-check every delivered event independently and return `suppressed` / `not_an_artifact` for anything that doesn't match, never trusting the worker's prefilter as authoritative. Tests: a simulated compromised worker that delivers an unrecognized URL, or a URL outside the matched adapter's rules or scope, must not be recorded; a `webNavigation` event on an origin the worker was never granted (which Chrome still delivers, DESIGN.md §5.1 Trails) must be dropped before anything is stored.

## In Progress / This Iteration

- None.

## Shipped

- None yet.
