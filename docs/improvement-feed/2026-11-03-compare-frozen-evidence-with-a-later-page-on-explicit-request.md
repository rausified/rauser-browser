## 2026-11-03 — Compare frozen evidence with a later page on explicit request

Add a user-requested agent review that compares an immutable selected-passage
card with separately supplied current page context, cites both and explains
changed or missing wording without rewriting the snapshot. This cycle's
`brwr-edn4` preserves the exact passage and source, but a person must still
compare it manually when a decision is revisited. Build this in the panel
and the existing fake-harness task seams; never schedule a background fetch.
Tracked as `brwr-fzya`, discovered from `brwr-edn4`.
- Review 2026-11-12: **Revise.** Compare an immutable passage with a newly supplied page only on request; useful but lower priority until a user workflow needs it.
