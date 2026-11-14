## 2026-11-10 — Check starter upgrade history when rules change

Change: make the starter generator check that a changed shipped match set is
retained in that type's history, comparing against the base revision.

Why: adding `doc2.aspx` requires preserving the current SharePoint match set;
otherwise saved adapters using that version receive no full starter update,
even though fresh installs recognize the new route. Old history alone does
not identify the most recently shipped version.

Where: `scripts/generate-starters.mjs`, its tests and `protocol/starters/*.json`.
- Review 2026-11-12: **Keep.** Compare changed starter match rules with the previous shipped version and require history so existing saved adapters can upgrade.
