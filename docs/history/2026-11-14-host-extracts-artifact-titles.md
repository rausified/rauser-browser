## 2026-11-14 — The host extracts artifact titles, and summary and M1 visit titles pass the generic check

This record concerns DESIGN.md §7.2 (artifact adapters) and adapter-templates.md §9.1 and §9.4 (brwr-yf13, brwr-cgve).
The index fold now takes every recorded title from the adapter's evaluation of the event's URL and title: the §9.1 extracted or fallback title, or none when §9.4 finds it generic (`recorded_title` in `host/src/index/fold.rs`).
Before, the stored name was whatever the last writer sent, raw (`Plan - Acme`) from navigation and restore, extracted (`Plan`) from the worker's title update, and the host re-judged an already-extracted title as if it were a tab title (#379 re-review m4).
The worker now sends the raw tab title on every path.
Summary and M1 visit events go through the same check as activity events.
Ruling: a summary's title is not trusted as the artifact's name, because it is the page's own title when the summary ran (`extension/summarize.ts`), not agent text, so a summary taken on a sign-in page would otherwise rename the artifact.
The summary record itself is unchanged.
Ruling: one `generic_titles` list keeps serving both the extracted title and the whole-title fallback (#379 review M3); a second list would be a format change in both engines, no recorded title had such a name, and the adapter pages name the short entries this affects.
On the first open after the upgrade every artifact whose stored name was a raw tab title is re-derived with its extracted name, and its file is rewritten once.
