# Blind starter scorecard (2026-11-03)

Eight starters were researched **blind**: public sources only, no browsing history and no
accounts. Their output was committed unchanged (`f0f9f85`) before grading. They were then
graded against a held-back answer key: 398 real URLs, with their tab titles, from one Brauser
user's Chrome history over the last year. The key is not in the repository. Grading ran each
`adapters.toml` through Brauser's TypeScript engine (`extension/adapter-templates.ts`) and
judged every URL shape by hand: should it be recorded, and as what?

Each site folder keeps `adapters.toml`, `vectors.json` and `research.md`. The throwaway
validation helpers the researchers wrote (`validate.sh`, small Rust crates and Node runners)
are in commit `f0f9f85` only; the research notes still name them.

## Summary

| Site | Should match | Matched correctly | Missed | Wrongly matched | ID stable across URL forms | Title template right |
|---|---|---|---|---|---|---|
| Figma | 34 | 34 | 0 | 0 | yes (`/file/` and `/design/` give one ID) | files yes; prototypes no (no `▶` prefix in real tabs) |
| Box | 5 | 5 | 0 | 0 | yes (`/s/…/file/<id>` and `/file/<id>` give one ID) | yes (`{title} \| Powered by Box`) |
| Vidcast | 11 | 11 | 0 | 0 | yes (share and edit give one ID) | yes (`Vidcast - {title}`) |
| Webex | 20 | 20 | 0 | 0 | yes (two player forms, 11 recordings) | yes, once loaded |
| Splunk Cloud | 5 | 5 | 0 | 0 | yes; rolled up into its app | yes (`{title} \| Splunk {~}`, rated low confidence but right) |
| Aha! | 2 | 2 | 0 | 0 | yes (reference number) | not judgeable (only login titles seen) |
| Google Workspace | 17 | 15 | 2 (Gmail message permalinks) | 0 | yes | Docs and Forms yes; Drive folders miss the en-dash variant |
| ServiceNow | 4 | 0 | 4 (KB articles on custom portal paths) | 0 | n/a | n/a (no templates, as researched) |
| **Total** | **98** | **92 (94%)** | **6** | **0** | | |

Of the other 300 URLs:

- **175 should not be recorded, and none were:** logins, SSO hops, search, home pages, lists,
  settings, the Box "All Files" root, the Webex legacy `ldr.php` forwards, Google Calendar
  booking pages, and ServiceNow's search, tiles and dashboards.
- **125 are Gmail thread views (`#inbox/<id>`, 165 visits)** that the adapter format cannot
  express: format 1 ignores the fragment. The researcher named this limit and left Gmail out.
  It is the largest single gap, and it is in the format, not the research. Recording Gmail
  threads needs fragment matching in a later format.

## Misses, and what the research method learns from them

1. **ServiceNow: customer-named portal paths.** The instance serves its Service Portal under
   its own suffixes (`/helpzone`, `/policyhub`), not the documented defaults (`/sp`, `/esc`,
   `/kb`). The KB rules keyed on the path. Fix: match any single-segment portal path that has
   `id=kb_article` or `id=kb_article_view` and `sysparm_article=KB…`; the query is specific
   enough on its own. **Fixed (2026-11-09):** the adapter format first refused that rule
   (`no_literal_segment`: a query never anchored a rule). `adapter-templates.md` §4.1 now lets a
   literal query condition plus an identifier capture anchor one, and `servicenow.kb`'s portal
   rules are `/{_}` with `id=kb_article` or `id=kb_article_view` and `sysparm_article`, with
   `sys_id` and `sys_kb_id` absent. Synthetic `/helpzone?id=kb_article_view&sysparm_article=KB0012345`
   and `/policyhub?id=kb_article&sysparm_article=KB0012345` match in both engines (the family's
   vectors). **Lesson:** when a product lets each customer rename a route, key the
   rule on the query or the record ID and leave the configurable part as `{_}`.
2. **Google: Gmail message permalinks.** `mail/u/<n>/?ik=…&view=pt&permmsgid=msg-f:…`
   (print view and permalink) uses the query, not the fragment. The researcher ruled out
   Gmail because thread views are fragment-routed, and missed this form. **Lesson:** when a
   product is mostly fragment-routed, look for print, share and permalink forms before
   excluding it.
3. **Google Drive: en dash.** One Drive folder tab ends in ` – Google Drive` (en dash), not
   ` - Google Drive`. **Lesson:** list both dashes for every suffix template; Figma uses the en
   dash too.
4. **Figma prototypes: assumed `▶` prefix.** Real prototype tabs are `<file> - <page>`. The
   extracted title keeps the page name, which is acceptable but not what the research
   predicted. **Lesson:** a title format read from client code can be out of date; mark it
   medium at most until seen.

## Predicted limits that showed up as predicted

- Loading or interstitial titles recorded as the title: Figma (`Figma`), Box (the enterprise
  name), Webex (`Recorded Meeting Password`, `Replay Recorded Meeting`, 15 of 20 visits),
  Vidcast (`Video Edit`), Google (`Service Not Allowed`, `Login`), Aha! (the login page). Every
  researcher named this risk. It is a Brauser-level problem (see the improvement feed), not
  a per-starter one.
- Box `<company>.box.com/s/…` redirects to `<company>.app.box.com/s/…`. Only the latter needs
  a rule, as researched.

## Verdict

The method is good enough to build starters for sites nobody on the team can open. On the
eight sites that could be checked, it recorded 94% of the pages that should be recorded,
recorded nothing it shouldn't, and kept IDs stable across URL variants. Every miss traces to
one of four specific, fixable research habits (above), now part of the research checklist.
Starters built this way still ship as development drafts until someone with an account runs
the per-site checklist in its `research.md`.
