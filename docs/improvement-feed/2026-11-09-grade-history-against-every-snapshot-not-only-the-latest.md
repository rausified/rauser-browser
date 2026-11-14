## 2026-11-09 — Grade history certification against every saved snapshot, not only the latest export

Change: When a starter type is certified from real browsing history, grade each type both alone and with its whole family (a type chosen alone is a supported selection), and grade it against the union of every local evidence snapshot taken so far (the latest export plus any earlier held-back sample), keyed by URL, and say in the type's History check paragraph that earlier snapshots were included. Keep each snapshot outside the repository until certification of its family is done, instead of replacing it with the newest export.

Why: Chrome drops history rows over time. On 2026-11-09 the full-history export had 1 Splunk dashboard URL, while the held-back sample taken a week earlier had 5 more real dashboard URLs and a search page that no longer exist in Chrome history (a fresh export confirmed it). Graded on the latest export alone, the search pages that Splunk dashboards wrongly record when chosen without Splunk apps, and the search-page title problem in Splunk apps, would not have shown up at all, and Splunk dashboards would have looked certifiable from a single visit. A newer export is not a larger sample: each one only holds what Chrome still keeps.

Where: `docs/superpowers/specs/2026-11-03-starter-families-design.md` "Research method", `protocol/starters/README.md` (history check paragraph), and the private history grader.
- Review 2026-11-12: **Keep.** Grade starter rules against all saved private history samples; newer Chrome history may have lost older visits.
