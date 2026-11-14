## 2026-11-10 — Exercise policy transitions after re-keying

Extend the eventual Re-learn activation acceptance tests to remove, narrow and
restore capture rules after real applies, checking note identity, older-note
access and physical ownership at each step. Independent review found that a
successful apply fixture alone missed raw retired-contract ambiguity after a
normal site removal. The new note regression covers that removal; the broader
transition matrix belongs beside multi-apply/rebuild acceptance in
`host/src/relearn/transaction/tests/` and
`docs/superpowers/plans/2026-10-28-capture-relearning.md`.
- Review 2026-11-12: **Keep.** Test removal, narrowing and restoration of capture rules after Re-learn applies so old notes keep the right physical owner.
