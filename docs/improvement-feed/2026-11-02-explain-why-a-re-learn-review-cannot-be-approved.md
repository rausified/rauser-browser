## 2026-11-02 — Explain why a Re-learn review cannot be approved

Change: add the conflicting origins and record keys to the host's bounded,
revision-bound conflict evidence, and translate those into plain-language
review messages. For damaged owned markers, show the existing path/line error
with guidance to inspect a backup and the local log before retrying; never
silently repair or choose a winning file. Why: the assigned brwr-eo7/brwr-r62
regressions reproduced an empty report from a torn why marker and a hidden
HTTP/HTTPS merge. Approval now refuses both, but issue codes alone do not tell
a person how to proceed. Where: host/src/relearn.rs, its strict index scan,
and the future panel Re-learn review under brwr-6x2. Tracked as brwr-pgxz
(discovered from both fixes); preserve read-only review and separate recovery.

- +1 2026-11-03: `brwr-c1bq` proves that A→B→C with notes at A and B
  must refuse an editor while C has no note. Include the original note keys
  and read-only viewing in `brwr-pgxz`/panel Re-learn conflict evidence so a
  person can resolve that case without guessing which file is being edited.
- +1 2026-11-03: `brwr-d586` reproduces a misleading mention when re-learning
  changes which URL segment supplies an ID. Extend `brwr-pgxz`'s future review
  with a bounded before/after example of the affected links, alongside key and
  note changes. Show both artifact titles/addresses so a person can verify
  that the saved mention follows the intended artifact, even when an old ID
  resembles a different artifact's new ID. Use the host-derived projection in
  `host/src/relearn.rs` and the future panel review; never infer a new mapping
  from display text.
