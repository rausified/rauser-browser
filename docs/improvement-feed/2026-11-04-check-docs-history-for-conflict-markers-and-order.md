## 2026-11-04 — Check docs/history.md for conflict markers and date order

Change: Make `scripts/check_current_state.py` fail when `docs/history.md` contains merge-conflict markers (`<<<<<<<`, `=======`, `>>>>>>>` at line start) or when its dated entries are out of chronological order. Why: resolving PR #312 against main meant restoring the append-only order of two Cookie entries (04:32Z, then 09:10Z) by hand, and #308's merge had conflicts in the same kinds of logs. Nothing would have caught a wrong order or a leftover marker. Where: `scripts/check_current_state.py`, which CI already runs in `Docs and queue checks`.
- Review 2026-11-12: **Keep.** Rejects conflict markers and out-of-order dated history notes in CI, catching bad merges.
