## 2026-11-14 — Test fold changes against pre-upgrade events

**What to change.** A change to how `host/src/index/fold.rs` derives a row (titles, totals, roll-ups) needs a test that folds events shaped the way earlier builds wrote them, not only events the current extension sends. Add a short note to the fold's module comment and a `replayed_*` test beside each such change in `host/src/index/tests/`.

**Why.** Any adapter-document change, including a change to how the index fingerprints it, refolds the whole journal under the new fold. The journal is never rewritten, so old event shapes stay forever. In the brwr-yf13/brwr-cgve lane the first version made the host extract every artifact title; its tests sent only raw titles and passed, but stored activity events also hold titles the old worker had already extracted, and the refold would have extracted those again and cut real names short (`Team plan - Q4` under Figma's `{title} - {~}`). Only the Codex review caught it.

**Where.** `host/src/index/fold.rs` (module comment), `host/src/index/tests/titles.rs` (`a_replayed_title_is_never_extracted_again` is the pattern).
