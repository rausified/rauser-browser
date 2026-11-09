# Guard live route identity during insertion and expansion

Items: `brwr-98mo`, `brwr-nhwb`.

Invariant: an ordinary save cannot silently hand a still-live recorded contract's
routes to a different identity. Existing explicitly declared child/container
addition and narrowing allowances remain; the retirement guard owns removals.

## Open decisions

None for this slice. DESIGN.md §6.2 already requires ordinary saves to retain the
ID freeze and conservative matching checks. Existing declared-child insertion and
scope narrowing remain supported. Re-learn recovery/history/downgrade decisions
remain open in the separate capture-relearning plan; no apply is enabled here.

## Design challenge

The AGENTS.md reviewer `/root/design_challenge` recommended retaining the existing
occurrence matching rather than replacing it with a new route algebra. Adopted:
inspect every preceding next contract and prove a complete earlier rule prefix,
including matching settings; individual rule membership cannot preserve capture
bindings. Appended suffixes may pass only when the existing overlap checker proves
them disjoint from the protected contract. Scope disjointness is not inferred.
The review also identified live containers as requiring protection; widening one
must not take its child's direct records. Existing declared-child additions are
a separate established compatibility rule and cannot authorize a live reversal.

## Delivery

1. Complete: real ConfigStore failing-first insertion regression, plus append,
   prepend, replacement, case/scope and generic/Splunk container coverage.
2. Complete: extend the positional precedence guard to every earlier candidate,
   preserving exact duplicate/sibling matching and declared-child compatibility.
3. Complete: model actual exact URL rules and coverage-changing edits, checking
   the order and history guards together across 2,000 × 12 generated steps.
   Assert acceptance of safe narrowing/widening and rejection of unsafe edits.
   Unrecognized URLs may stop capture; recognized routes retain their identity.
4. Validation and review: 149 configuration tests pass (+6); host lib/tests
   Clippy passes. Final static checks, capture-site tests and independent review
   are recorded in the cycle log. Driver owns the complete external gate.

No file migration, protocol change, live service action or new credentials.

Independent review found one P2 in the initial declared-child exception: it also
permitted an existing child to widen over direct container pages. Corrected before
closeout with the existing test helper: require a newly added child identity or a
provable rule-prefix/scope narrowing, retaining every old scope constraint with
subset values. A concrete direct-container-page regression passes. No new test
seam was needed. Final checks: 149 configuration + 6 capture-site tests pass,
host lib/tests Clippy with warnings denied, generation/format/type/lint/current
state checks pass. Full external gate and publication remain driver-owned.

Queue reflection: the last three cycles fixed distinct ordinary-save identity
gaps. The shared guard now checks additions, widening, narrowing and reorders;
the next cycle should recheck assigned decisions and fresh ready priorities,
then choose a visible product task rather than invent another guard layer. The
seeded model intentionally uses exact paths; scope, case and containment policy
remain explicit real-save regressions.
