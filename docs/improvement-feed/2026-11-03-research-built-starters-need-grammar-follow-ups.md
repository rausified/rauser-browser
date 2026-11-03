## 2026-11-03 — Research-built starters need five grammar follow-ups

The blind starter test (`docs/research/blind/SCORECARD.md`) hit five things adapter format 1
cannot express. In order of real impact on the graded history:

1. Fragment matching: 125 Gmail thread views (`#inbox/<id>`, 165 visits) are unrecordable.
2. A per-mode title policy: edit views with fixed titles (`Video Edit`) and interstitials
   (`Recorded Meeting Password`, 15 of 20 Webex visits) replace good titles.
3. Nested encoded targets: ServiceNow's `nav_to.do?uri=…` and
   `/now/nav/ui/classic/params/target/<encoded>` wrap the record URL in one value.
4. Slash-containing query captures: Splunk saved searches (`s=/servicesNS/…`) have no type.
5. A dashed reference capture type: Aha! `PRJ-E-12` refs only fit `name(5,100)`, which also
   admits route words.

Change: design a format 2 (or format 1 extensions behind a version check) covering at least
1 and 2. Where: `adapter-templates.md`, `adapter-templates/`, `extension/adapter-templates.ts`.
