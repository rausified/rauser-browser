## 2026-11-02 — Bind the physical note location into Re-learn save revisions

Change: give an alias-resolved note an internal logical key, physical ownership
key and selected file location, and bind a save's revision check to that
selection. Why: brwr-ruwj now projects one corrected artifact while retaining
both original summary paths and raw ownership keys; the migration design
challenge found that passing a corrected key to the existing note-save API
would create another file instead of updating the note the person loaded.
Where: host/src/native.rs note context, host/src/note.rs load/save and the
remaining brwr-dbp apply integration. Test stale saves across a move and a
move back, both-side notes and URL copies; preserve copy-out-only browser
recovery. This belongs to the assigned brwr-dbp work.

- Done 2026-11-03: `brwr-c1bq` integrates physical ownership and history-bound
  revisions into the shared native note path. Applied-history publication and
  panel integration remain assigned under `brwr-dbp`/`brwr-6x2`.
