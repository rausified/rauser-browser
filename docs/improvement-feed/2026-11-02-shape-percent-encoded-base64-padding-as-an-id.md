## 2026-11-02 — Shape percent-encoded base64 padding as an ID

The page-pattern shaper now reads a trailing `=` or `==` as an `{id}`
(rausified/rauser-browser#215), but it never percent-decodes path segments,
so a link that encodes the padding as `%3D` (`/app/board/qK3nR8vDpZx%3D/`)
still becomes `{slug}`, and the structural drafter proposes nothing for it.
Matching already decodes `%3D` (`eval.token.miro_board_encoded_padding`).
Change: in `extension/site-patterns.ts`, treat a trailing `%3D` or `%3D%3D`
(any case) like `=` before classifying, and add a shared shapes row for it.
- Review 2026-11-12: **Keep.** Recognizes `%3D` padding as an ID in page-pattern drafting, matching the existing URL evaluator.
