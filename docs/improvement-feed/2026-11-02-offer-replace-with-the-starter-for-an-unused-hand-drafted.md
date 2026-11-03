## 2026-11-02 — Offer "Replace with the starter" for an unused hand-drafted rule

John's Miro capture saved a drafted `board` adapter before Brauser had a
`miro.board` starter. Switching it to the starter took a hand edit of the live
`config.toml` and `adopted-contracts.json` under the config lock, because
removing an adapter records its contract and the overlap guard then refuses a
starter on the same route under another type (rausified/rauser-browser#219).
Change: when settings or the panel sees a non-starter adapter on a starter's
origin whose routes the starter covers, and that adapter has recorded no
activity, notes or summaries, offer **Replace with the <tool> starter**. It
removes the adapter without recording its contract and adds the starter in one
revision-checked save. Where: `extension/starter-adapters.ts`, the settings
gallery, and a host check that the adapter has no records.
