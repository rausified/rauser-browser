## 2026-11-10 — Preview page card layouts at narrow widths

Change: include narrow-panel visual previews for unconfigured, configured and
setup-blocked pages when reviewing page-card changes.

Why: two independent capture hints and a duplicate review action pushed Save Page
and Capture Site away from the Not an artifact status in the reported panel.

Where: extend the headless fixtures in `extension/tests/a11y/` with page-card
screenshots at common sidebar widths so redundant prompts are visible in review.
