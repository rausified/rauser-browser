## 2026-11-02 — Offer a configured site's missing sibling starter in the panel

Change: when a page on a configured starter origin matches no rule but would
match a starter of the same origin family that the site does not have yet
(a Confluence page on a site with only the Jira starter, a GitHub issue on a
site with only pull requests), say so in the capture explanation and offer
**Review starter**, handing that one starter to the settings gallery's editor.
Why: Capture Site refuses any site that already has rules, and `nearMiss`
only offers rules of an adapter's own starter, so a Jira-configured Atlassian
site visiting Confluence reads "captures Jira issues; this page isn't one of
them" with no way forward from the panel; after three visits the
recurring-pages hint drafts a structural `wiki`-style adapter instead of the
shipped `confluence.page` starter, which would then need re-learning. Where:
`nearMiss` in extension/site-coverage.ts (a third `starter_type` kind built
from `starterOffers` and `hasAdapter`), the `no_rule` case of
renderCaptureExplanation in extension/panel.ts, and `recurringHint` in
extension/capture-site.ts (skip shapes a missing sibling starter matches).
