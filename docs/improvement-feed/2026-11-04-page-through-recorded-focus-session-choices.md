## 2026-11-04 — Page through recorded focus-session choices

Add explicit paging (`brwr-t4gf`) to the recorded-session selector used when sharing work
in Recent. The first implementation (`brwr-1jeb`) returns 64 sessions with a
separate omission notice; shortening the period is its only way to find a
session beyond that catalog. Many short work blocks on one day can exceed the
catalog even though the requested date range cannot be shortened further.

Extend `worklog_focus_choices` with a stable host-controlled session cursor and
add a named More sessions control in `extension/panel.ts`. Keep entry selection
explicit, preserve the selected session independently of catalog paging, and
test equal timestamps, same-name sessions and a changing history snapshot.
