## 2026-11-14 — Summary and M1 visit titles pass the generic-title check

This record concerns DESIGN.md §7.2 (artifact adapters) and adapter-templates.md §9.4 (brwr-yf13, brwr-cgve).
The index fold already dropped an activity title the matching adapter finds generic; summary and M1 visit events now go through the same check (`recorded_title` in `host/src/index/fold.rs`).
Ruling: a summary's title is not trusted as the artifact's name, because it is the page's own title when the summary ran (`extension/summarize.ts`), not agent text, so a summary taken on a sign-in page would otherwise rename the artifact.
The summary record itself is unchanged.
Ruling: one `generic_titles` list keeps serving both the extracted title and the whole-title fallback (#379 review M3); a second list would be a format change in both engines, no recorded title had such a name, and the adapter pages name the short entries this affects.
Ruling: the host keeps judging an already-extracted title as if it were a tab title (#379 re-review m4), and never extracts a stored title again.
A first version of this change had the worker send raw titles and the host extract every title; review found that the refold would extract titles the worker had already extracted a second time and cut real names short (`Team plan - Q4` under Figma's `{title} - {~}`), because the journal does not record which form a title is.
Telling the forms apart needs a marker on the wire, tracked as brwr-ey3e.
