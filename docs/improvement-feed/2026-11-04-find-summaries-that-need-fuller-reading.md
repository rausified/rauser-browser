## 2026-11-04 — Find summaries that need fuller reading

Change: offer a reading-coverage filter in Recent, including limited reading
and unknown legacy scope (`brwr-y1q2`, discovered from `brwr-30xv`). Why:
saved-summary labels now retain the validated agent claim through replay, but
someone revisiting many artifacts still has to scan each overview to find a
viewer-only or partial summary worth another read. Where: the host's Recent
query and protocol filter, then `extension/panel.ts`. Filter before the result
cap so older limited summaries remain discoverable; never infer whole reading
from an absent field or start another summary without a user gesture.

- Done 2026-11-05: `brwr-y1q2` implements host filtering before the cap and the Recent selector; legacy unknown reading remains separate from unsummarized days.
