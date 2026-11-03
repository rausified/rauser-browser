## 2026-11-02 — Page through work before selecting an update

Change: build the existing `brwr-p71k` follow-up with explicit next/previous
choices and stable artifact/day selections. Why: recovering selected-work
curation confirms that its 200-row/96 KiB response cap can omit work even for
a single busy day; choosing a shorter period cannot expose that remainder.
Where: `ArtifactIndex::worklog_choices`, the protocol query and the Recent
selection controls. Keep newly viewed rows unchecked and Copy equal to the
checked preview; do not silently include omitted work.
