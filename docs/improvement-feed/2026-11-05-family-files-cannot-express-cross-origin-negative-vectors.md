## 2026-11-05 — Family files cannot express cross-origin negative vectors

Add a way for a family file to carry a negative vector whose URL is
deliberately not on the family's own origin, instead of forcing those cases
to be dropped on the floor.

Evidence: `scripts/generate-starters.mjs`'s `validateVectors` rejects any
vector whose URL's origin isn't claimed by the family's own `origin`
(`${vectorWhere}.url is not on this family's origin`), because every vector
runs through the family's own synthesized fixture. That dropped real
research coverage during this batch: Figma's research included
`embed_origin`, `apex_origin` and `http_scheme` negatives (confirming
`embed.figma.com`, bare `figma.com`, and plain `http://` never match), and
Vidcast's research included `http://app.vidcast.io` negatives — all expect
`no_adapter`/`{match: false}`, and all had to be left out of
`figma.json`/`vidcast.json` because the generator has no origin-agnostic
vector slot.

Where: `protocol/adapter-templates/vectors.json`'s shape and
`scripts/generate-starters.mjs`'s `validateVectors`. Either let a vector opt
out of the family-origin check when its `expect` is `{match: false}` (a
pure negative needs no family fixture to prove non-match), or add a
family-level `foreign_negatives` list of `{url, why}` entries that the
generator checks only for `no_adapter` outside the family's own fixture.
Document the chosen shape in `protocol/starters/README.md` next to the
existing `sample_origin` rule.

- +1 2026-11-05: positives on a second tenant host can't be family vectors
  either, not just negatives. Webex's research kept the
  `site4-example.webex.com` case as a positive on its own host (documented in
  `docs/adapters/webex.recording.md`), but it had to stay excluded from
  coverage rather than ride as a vector in `webex.json`, because synthesized
  fixtures pin every vector to the family's own `sample_origin`
  (`https://example.webex.com`) and have no slot for a positive confirmed on
  a different, real tenant host.
