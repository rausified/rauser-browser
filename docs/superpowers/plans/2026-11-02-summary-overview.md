# Show summary overviews and retain summary history

Goal: carry the existing validated overview into the panel, Recent, and run
results, and keep retained summaries reachable after removing the harness.
Beads: `brwr-sm3`, `brwr-tsh1`. Owner: Cookie.

## Open decisions

- Resolved by John in `brwr-sm3`: show the overview and document type; old
  events without an overview retain their current wording.
- Resolved by existing contracts: agent text uses `textContent`; summary
  files, worklogs, identity and capture behavior keep their existing rules.

## Design challenge

A separate Codex reviewer challenged the representation before implementation.
Re-reading summary Markdown would avoid extending the event but introduce file
I/O and parsing into every history lookup. Keep the optional event field, omit
it when absent, and default old events and caches to absence. Reuse the page
summary validator for replay. No cache version change is needed because the
cache stores the whole serialized event; verify reopen and reindex explicitly.
Older hosts reject new event fields and older extensions reject new wire
fields: document coordinated deployment and the downgrade limitation.

## Tasks

1. Add optional bounded overview to the event and wire views; propagate it
   from the validated answer and test old/new events and cached/rebuilt views.
2. Show the overview and type in saved summary lines and Recent; lead run
   results with the overview, preserving existing labels and legacy wording.
3. Retain the history hint independently of whether a summary card shows;
   test a dismissed summary through panel restore and harness removal.
4. Run the full gate, review the complete diff, update documentation and queue.

## Validation and progress

Baseline required and README gates passed; browser and IPC sandbox skips are
explicit and remain for the external driver. The history regression failed on
the original built extension: Tasks did not list summaries without a harness.

All four tasks are implemented. Final required and README gates passed:
1,335 Rust-reported outcomes (+4; 92 explicit IPC sandbox skips), all 40
extension test files (+4 controller/card regressions), both Clippy builds,
TypeScript and lint, protocol generation, 10,696 inert Markdown renderings,
eight Node script files and Python script/queue checks. Browser suite: two
fixture tests passed, 36 browser checks explicitly skipped by the sandbox.
No real harness, live API, service or deployment was used. Complete source
diff reviewed locally; a separate final static pass is recorded at closeout.

Final static Codex review: no P1, P2 or P3 findings in the complete diff
against HEAD. Both implementation beads closed after the green gate;
brwr-khqi is the deferred search-by-overview follow-up.
