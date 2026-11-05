# Find summaries that need a fuller reading

Bead: `brwr-y1q2`. Add a Recent filter over recorded summary reading coverage
so a person can find limited readings worth revisiting, including older days
that fall outside the unfiltered response cap.

## Open decisions

- Resolved by the bead acceptance criteria: offer limited, whole and unknown
  reading, and filter on the host before the response cap. Start no agent.
- Resolved by Cookie (2026-11-05), following Recent's existing day rows:
  classify the latest renderable summary for that artifact and local day,
  exactly the summary displayed. Limited combines `partial` and `page_only`.
  Unknown means a saved summary without recorded scope; days without any
  displayed summary match only the default All activity view.
- Resolved by Cookie: add an optional protocol-v8 request field. Omit it for
  All activity, preserving older-host behavior; an older host refusing a
  filter gets rebuild guidance. No cache format or durable event changes.

## Design challenge

2026-11-05, `/root/reading_filter_challenge`: adopt streaming summarized day
rows with the existing latest-summary resolver before counting matches.
Filtering the existing capped response is simpler but loses older matches;
SQL-only latest selection risks different timestamp, same-second log-order
and unrenderable-event behavior. Retain only the bounded result, exclude days
without a renderable summary even from Unknown, and preserve the default SQL
limit. A no-match query scans summarized history under the config lock;
measure representative history before closeout. Do not cap candidate scans
and silently lose valid results. A composite summary/day index can follow if
measured history justifies a cache change.

## Delivery

1. Add the optional `summary_reading` enum to `list_artifacts`; generate both
   wire implementations and cover absent/valid/invalid values.
2. Reuse the index's latest-summary resolver to filter before limiting results.
3. Pass the typed filter through the host's existing locked Recent read.
4. Add a named Recent selector, responsive layout and stale/error handling.
5. Test mixed history, latest-event order, limits, rebuilds and panel changes.
6. Run the required and README checks, review the complete diff, document the
   feature and leave a commit message for the driver.

## Validation

All six steps are implemented. Baseline and final required commands passed
on `6ea537e6` plus this diff. Final workspace tests report 1,625 passes
(+5; 94 explicit IPC guards and two ignored tests), all 47 extension test
files pass (+2 controller cases), and both Clippy configurations pass.
Accessibility reports two fixture passes and 49 explicit sandbox browser
skips, including the new keyboard case. Real Chromium execution belongs to
the external driver gate and CI. The README's script and Markdown checks pass;
Python tooling results are recorded in the cycle log. The live queue export
and consistency check remain with the driver, as required by this run.

The 365-day single-artifact no-match probe measured 133 ms on this VM;
this is an observation, not a portable timing assertion. Its regression also
finds a limited reading beyond those 365 excluded days. Filtered no-match
queries still inspect all summarized candidate days under the config lock;
there is no arbitrary candidate cap or new persistent cache. The default
All activity query retains its SQL limit and old-host wire compatibility.

Fresh full-diff self-review and an independent read-only review by
`/root/reading_filter_challenge` found no actionable defects. Review refined
the truncation probe to preserve the default query's existing behavior.
`brwr-a30j` tracks explicit continuation beyond 32 matching rows; it is not
part of this slice. Driver owns commit fallback, external review and release.
