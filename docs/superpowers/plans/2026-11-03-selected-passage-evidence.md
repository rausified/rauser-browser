# Selected passage evidence cards

Assigned bead: `brwr-edn4`. Preserve the browser-supplied passage behind a
decision as a local, immutable Markdown card with source attribution.

## Open decisions

Resolved by Cookie within the approved assignment: cards live in the fixed
`notes/evidence/` subfolder, outside editable page-note discovery. The native
host derives the record key, canonical source URL and first capture time. A subframe selection uses Chrome's frame URL and never
falls back to the top page when that URL is invalid. Exact UTF-8 text is stored
as hex with a SHA-256 digest; a safely rendered display accompanies it. Blank
and oversized selections refuse without truncation. Identity is frozen at
capture; repeating the same source/key/text under the current policy returns
the original card and time. This records browser-supplied text, not a claim
that Brauser verified the page or can relocate the passage later.

## Design challenge

Before implementation, reviewer `/root/passage_design_challenge` challenged
reuse of the owned-file replacement helper: its replacement path violates
immutable evidence. Adopt dedicated no-clobber hard-link publication instead,
using the existing bounded inspected-file reader and temporary cleanup. An
existing card must match the complete expected bytes using its valid original
timestamp. Identical race winners count as successful retries; unrelated or
edited occupants refuse. Hash a serialized tuple, not ambiguous concatenation.
Hex metadata avoids introducing another Markdown or YAML escaping mechanism
for exact text; the display reuses the shared Markdown neutralizer.

## Delivery

1. Add bounded protocol request/receipt and immutable capability-scoped writer.
2. Add the selection-only context menu and side-panel success/failure notice.
3. Test real native messages and mocked browser gestures, including no extractor,
   harness or network; test exact bytes, retries, changed text and conflicts.
4. Run required and README gates, review the complete diff, document the contract
   and export the queue. Driver owns publication and independent final review.

## Implementation and validation — 2026-11-03

All four delivery steps implemented. The native message accepts only source
URL and selection; host-derived attribution and first capture time are bound
to an immutable format-1 card. Exact metadata is hex encoded, while human
metadata uses JSON-compatible YAML escaping and the passage uses the shared
neutralizer. Native and browser boundary tests prohibit unrelated content or
agent invocation and cover the documented failure/retry behavior.

Required full gate passes: 1,418 Rust-reported outcomes (+9), 45 extension
files (+5 cases), both Clippy configurations, 2 accessibility fixture passes
and 42 explicit sandbox browser skips. README extras pass: 10,696 inert
Markdown renderings, 9 script files and 221 Python cases (92 platform skips).
Full manual diff review completed. The one independent in-session Codex
review found a P2: digit-only hex metadata was inferred as a YAML number.
An existing test reproduced precision/type loss before the fix; both hex
fields now use quoted YAML strings, and the focused tests pass. No other
findings. A complete post-fix gate is recorded at closeout. No real harness, live API, remote, service or deployed
path was used. Driver owns publication and external acceptance.


Final boundary self-review also reproduced canonical URL growth: a bounded raw
Unicode URL can expand beyond the receipt's limit when percent encoded. The
host now refuses that source before publication, covered by the existing
native invalid-source test. The focused regression and normal fmt/Clippy
checks pass; the final full run includes the correction. Final logs:
`target/cookie-evidence-final-reviewed/`; pre-fix reproductions and README
extras are retained under `target/cookie-evidence-final/` and that final folder.


Closeout: the complete final gate passed after the review corrections,
including the native expanded-URL regression. brwr-edn4 is closed with all
acceptance criteria met in offline tests. Queue consistency and current-state
checks pass; driver owns commit SHA, independent approval and publication.
