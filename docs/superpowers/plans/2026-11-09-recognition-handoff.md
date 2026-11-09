# Refuse identity handoffs from narrowed recognition

Bead: `brwr-obqb`.

## Goal

A settings save cannot hand a live adapter's recorded routes to a different
identity by narrowing recognition while keeping its type and ID. Reuse the
existing host overlap and declared-container rules; no migration is enabled.

## Open decisions

- Resolved by the existing DESIGN.md §6.2 freeze: a recognition edit retains
  its historical routes, just like removal. Undeclared identity handoffs refuse.
- Resolved by Cookie for this fix: use conservative overlap for arbitrary
  recognition changes, as the existing retired-contract guard does. Preserve
  the clear safe case of appending rules with all old rules in their original
  order as a prefix and identical other matching settings. This is a structural
  proof, not a new language-inclusion engine. Declared containers retain their
  existing exception.

## Design challenge

2026-11-09, reviewer `/root/history_guard_challenge`: checking every superseded
shape closes the hole, but blanket checks would also refuse safe append-only
widening ahead of a catch-all. Adopt a narrow proof that a same-identity next
adapter preserves the old rule prefix and all other recognition settings.
Do not use sample URLs or overlap counts as proof of coverage. Keep `within`
out of that proof: the separate frozen-container transition check governs it.
For arbitrary rewrites, conservative refusal is preferable to inferring a
migration from incomplete evidence. No new subset parser or engine changes.

## Tasks

1. Reproduce rule removal, literal-list narrowing and scope narrowing through
   real revision-checked config saves, retaining exact bytes on refusal.
2. Check superseded current shapes alongside historical contracts, with the
   append-only proof and existing declared-container exception.
3. Exercise allowed append-only changes, same-identity fall-through, disjoint
   edits, unrelated saves and declared containers; run focused config tests and
   static checks, then read the complete diff.
4. Update current design, testing notes and cycle documentation; leave publication
   and the external full-suite gate to the driver.

## Progress — 2026-11-09

Implemented the superseded-shape check and replaced the old delta matcher
with the bounded prefix proof. The refusal regression failed before the fix;
133 config tests (three new) and all 60 Capture Site tests pass afterward.
Manual full-diff review and one independent Codex review found no issues.
The baseline full suite passed before edits; the driver owns the final full
gate. Adapter precedence changes remain separate (`brwr-qbub`), and arbitrary
scope/template rewrites deliberately retain conservative overlap behavior.

Supplemental script testing found two loop-generated CLI test groups missing
the repository's existing child-pipe guard (`brwr-vyg5`). Reusing that guard
is a test-only correction; generator validation assertions stay unchanged.
