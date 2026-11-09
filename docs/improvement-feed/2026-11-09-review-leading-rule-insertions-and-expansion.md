## 2026-11-09 — Review leading rule insertions and expansion

Change: extend identity-impact review to a new adapter inserted before a live
winner and to recognition expansion that claims a later adapter's pages.
Why: `brwr-qbub` now guards reordered existing contracts, but an insertion has
no earlier occurrence to compare, and expansion can change the first match
without changing order. Both can change the key a person sees for a previously
recorded page. Keep this separate from reorder and retirement checks, and
settle the intended policy for adding contained rules to a live container.
Where: `host/src/config.rs` transition validation and revision-checked save
regressions; tracked as `brwr-98mo`, discovered from `brwr-qbub`. Demonstrate
the handoffs with real engine evaluations before choosing the review policy.
