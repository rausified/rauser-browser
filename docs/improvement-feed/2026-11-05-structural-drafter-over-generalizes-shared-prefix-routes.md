## 2026-11-05 — Structural drafter over-generalizes view/edit routes that share a prefix

Flip `samples_shape` to `false` by hand whenever two types on one origin
share a route word and differ only by extra literal segments, instead of
having the drafter prefer the more specific shape itself.

Evidence: `figma.branch`'s one sample
(`/design/<key>/branch/<branch key>/<slug>`) shares `figma.file`'s own
`/design/<key>/<slug>` route word, differing only by the literal `branch`
segment inserted after the key; given one example of each, the generic
structural drafter buckets both into one candidate rule and generalizes to
the more specific (branch) form, which then stops matching `figma.file`'s
plain sample. `vidcast.video`'s two samples (`/share/<id>` and
`/share/edit/<id>`) hit the same failure at two different depths under one
route word. Both pairs forced `samples_shape: false` as the only way to ship
(see "Recorded samples" in `docs/adapters/figma.branch.md` and
`docs/adapters/vidcast.video.md`), which removes the samples from
`cargo test --locked -p adapter-templates --test structural
starter_recorded_urls_draft_to_matching_adapters_one_per_kind` rather than
fixing what it drafts.

Where: `adapter-templates/src/structural.rs` and its tests. When one
candidate rule's literal segments are a strict prefix of another candidate
rule's literal segments on the same route word, prefer drafting the more
specific (longer-prefix) shape for the sample whose literal segments match
it, instead of collapsing both samples into the single more general shape.
That would let `figma.branch` and `vidcast.video`'s edit route keep
`samples_shape: true` and drop the opt-out this batch needed twice.
- Review 2026-11-12: **Keep.** Make the drafter distinguish routes that share an opening segment but diverge later. Today some real samples have to be excluded from structural tests; the proposed longer-prefix heuristic should be validated against both route families before adopting.
