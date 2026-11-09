## 2026-11-05 — Save curated work-sharing drafts

Add explicit named sharing drafts (`brwr-1b6o`) to Recent: save the exact
checked preview, reopen it after panel closure, and deliberately refresh it
with visible changed or missing evidence. New work must stay unchecked.

Evidence: `brwr-p71k` now lets a person curate beyond the 200-row/96-KiB
response cap, but the existing lifecycle contract still clears that effort
when the panel closes or the host exits. A longer curated update deserves an
explicit way to keep it without writing source logs or notes.

Where: the selected-work controls in `extension/panel.ts`, an isolated local
draft store keyed to the home folder, and the panel tests. Reopening must
start no agent, change no source content, and copy nothing automatically.

- Done 2026-11-09: `brwr-1b6o` implements immutable browser-local drafts and explicit original-identity evidence review; complete local gates pass. Driver commit/review/publication pending.
