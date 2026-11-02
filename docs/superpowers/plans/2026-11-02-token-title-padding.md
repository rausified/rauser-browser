# Token padding in title captures

**Goal:** Complete `brwr-tp1`: padded token IDs work in title cleanup and
tab-title aliases in both adapter engines.

**Architecture:** Keep the existing longest-alphabet-run title matchers and
whole-value canonicalization. Include `=` in the token alphabet, while the
canonical token body still excludes it after removing at most two trailing
padding characters.

**Tech Stack:** Rust, TypeScript, shared JSON vectors.

**Sources:** `adapter-templates.md` §7.1 and §9.1; `brwr-tp1` acceptance.

## Open decisions

- Resolved by the bead's acceptance and existing grammar: title captures use
  the same token padding and length rules as URL captures. No new syntax.
- Resolved by Cookie, 2026-11-02: `=` cannot bound a token title capture,
  consistently with the existing alphabet-based delimiter check.

## Design challenge

- **Challenger:** `/root/token_design_challenge`, before implementation.
- **Alternative:** A separate title-only alphabet or padding-aware scanner
  could consume padding without changing the shared alphabet.
- **Outcome:** Keep the smaller alphabet change with strict body validation.
  The existing grammar already requires longest runs followed by whole-value
  validation in both directions. This also makes the delimiter check agree;
  no new scanner or backtracking is needed. The overlap mask becomes more
  conservative, and other types do not gain padding.

## Tasks

1. Add shared vectors for prefix/suffix padding, aliases, malformed padding,
   bounds and delimiters; reproduce the failures in both engines.
2. Update both alphabets and preserve strict canonical token validation.
3. Clarify §9.1 and record the resulting behavior in `docs/history.md`.
4. Run adapter differential tests and the full local gate; review the complete
   diff, close/export the bead and append the cycle's documentation.


## Verification and outcome

Implemented all four tasks. The 26 shared vectors reproduce ten failures in
both original engines and now pass. The full local gate and README checks
pass with explicit sandbox IPC/Chromium skips; the standalone adapter
JavaScript suite passes 1,265 tests. Regular-file differential comparison
agrees on 2,024 cases. The normal differential command skips child pipes;
fuzz compilation is unavailable because `libfuzzer-sys` is uncached. CI must
supply those checks. No wire format, starter, dialog or ID-freeze change.
Compatibility: custom token title templates bounded by `=` now fail validation.
Follow-up `brwr-ys8y` asks the random generator to include padded values.
