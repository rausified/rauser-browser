## 2026-11-09 — Highlight changed words in refreshed sharing entries

Show a text-only word comparison when reviewing a changed sharing-draft entry,
and call out a changed URL separately. Keep both complete saved and current rows
available, and retain explicit approval before a replacement enters the preview.

Evidence: `brwr-1b6o` now compares original artifact-day selections with current
evidence, but a row combines its title, measures, summary wording and URL. The
review currently shows two complete strings, so a small measure or URL change
can require rereading both long rows to identify what needs approval.

Where: `extension/sharing-draft-panel.ts`, with inert comparison helpers and
offline/keyboard cases for wording, measures, URLs and Unicode. Track the
follow-up as `brwr-cc3h`, discovered from `brwr-1b6o`.
