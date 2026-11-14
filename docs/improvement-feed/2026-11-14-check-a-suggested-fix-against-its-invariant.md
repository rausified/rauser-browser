## 2026-11-14 — Check a review's suggested fix against its invariant before applying it

**What:** When a review or bead names the exact fix ("use `modified.min(now) + DRAIN_MAX`"), first write the test for the invariant the finding protects ("a marker cannot pause runs past an hour from now"), then apply the fix to that test, not the one the finding describes.

**Why:** brwr-8gdv item 2 (the #428 review) suggested capping the drain pause at `min(mtime, now) + 1h`. `now` is each check's own time, so while the mtime is still in the future the cap keeps moving forward with every check. The marker would still pause until `mtime + 1h`, the bug it was meant to fix. A test that walked `now` forward (`a_marker_dated_in_the_future_pauses_nothing`) showed it at once. The shipped fix fails open past a 5-minute skew instead (PR #488).

**Where:** bead acceptance text for review follow-ups ("each item fixed with a test"), and the "Invariant first for guard changes" rule in AGENTS.md "Before building", which could extend to any time- or bound-based guard: state the bound, test it over a moving clock, then pick the code.
