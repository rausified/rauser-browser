# Disclose capped page capture (brwr-sm5)

**Goal:** let summary and task agents distinguish a bounded browser excerpt
from a capture with no reported cut, including automatic summaries.

**Architecture:** retain the extractor's existing combined page/selection
flag through the optional wire field `capture_truncated`. Persist it in run
status, use it when writing page.md, and select fixed prompt text from that
status. Never infer provenance from page text.

**Tech Stack:** TypeScript extension, generated protocol types, Rust service,
and the compiled SUMMARY.md template.

**Sources:** brwr-sm5 acceptance; DESIGN.md §4.5 and §5.2; archived
implementation 2cf67c46 and Office/Miro task correction 5fa32bac.

## Design challenge

**Challenger:** the read-only `truncation_design` Codex reviewer session.
**Alternative:** use one unconditional instruction to honor a completeness
header in page.md, without another stored field or conditional paragraph.
**Outcome:** retain explicit stored provenance: an ordinary page can forge
that header, while the existing Office-section pattern already preserves
trusted prompt selection over queue recovery. Adopt the reviewer's additional
findings: carry the flag through automatic summaries and correct the Office
and Miro *summary* paragraphs as well as their task paragraphs. The archived
patch omitted both. Missing/false means only that no cut was reported.

## Open decisions

- **Decision:** reuse the existing combined text/selection flag, with wording
  that names both. **Status:** resolved by the bead's existing implementation
  plan and Cookie's recovery review, 2026-11-02. It conservatively warns when
  a borrowed highlight's frame was capped; it does not claim which part was cut.
- **Decision:** retrieval and read_scope. **Status:** resolved by brwr-sm5's
  acceptance: retrieve the full artifact or report partial/page_only; whole
  requires actually reading the full artifact. Tasks retain the user's request
  as their authority. No new retrieval permission.
- **Decision:** apply the same signal to automatic summaries. **Status:**
  resolved by the existing shared extractor/summary contract; an automatic
  start uses the same service run and prompt as a manual summary.

## Tasks

1. Recover the archived regression tests and demonstrate manual/task and
   automatic truncation failures against this checkout.
2. Recover code selectively without archived queue/docs; extend automatic
   requests, native forwarding, wire tests and Office/Miro summary wording.
3. Verify chosen versus discarded frames, borrowed highlights, wire defaults,
   page-file boundaries and stored/queued provenance; run the full local gate.
4. Read the complete diff, update current docs and queue, and leave the cycle
   ready for the driver's independent review and publication.

## Verification progress

The manual summary, borrowed-highlight and task regressions failed before
recovery. A new automatic-summary regression also failed against the recovered
implementation before its missing path was added. Focused extension tests pass.
The initial and final full required gates passed, as did all README
supplementary checks. Final results: 1,374 Rust-reported outcomes (94 explicit
IPC sandbox skips), all 41 extension test files, both Clippy builds, 2
accessibility fixture passes and 36 explicit browser sandbox skips. Twelve
regressions were added (eight Rust, four extension); 10,696 inert Markdown
renderings, eight script files and 221 Python tests (92 platform skips) pass.
Fresh full code/test/generated/doc diff and semantic queue review are complete.
All four tasks are done; brwr-sm5 is closed for driver review/publication.
Platform smoke steps and dialog counts do not change. The single local commit
attempt was refused by the read-only .git mount; the driver supplies the SHA.

Queue audit retained the driver's local held-PR record brwr-tcfc and the
existing saved-summary scope proposal brwr-30xv. brwr-q0i4/brwr-rr93 are absent
from this checkout but implemented in held PR #249; they are blocked on that
integration rather than exported as shipped. Assigned work was unchanged.
