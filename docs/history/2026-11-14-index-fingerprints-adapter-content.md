## 2026-11-14 — The index fingerprints the adapter document's content

This record concerns DESIGN.md §6.7 (lazy regeneration and log ingestion recovery).
Until now the index recorded the adapter document it was derived under as a hash of the document's Rust `Debug` text, so every new optional adapter field needed a hand-written `Debug` to keep existing indexes from refolding (brwr-8dlu, #379 review M5).
From this change it hashes `adapters content/v1` plus the source document as canonical JSON (keys sorted at every level, no white space), kept on the validated `Document` (`Document::canonical_json`, `index::adapters_fingerprint`).
Key order and how a type prints no longer matter; any content change still does.
A golden test pins the hash of one synthetic document, and an independent `json.dumps(sort_keys=True)` reproduces it.
The changed form refolds every existing index once, on its first open after the upgrade, through the ordinary adapter-change path: only artifacts whose rendering changes are marked dirty. In this release those are artifacts whose title a summary or M1 visit on a generic page had set (`2026-11-14-summary-and-visit-titles-pass-the-generic-check.md`), and artifacts whose stored title predates the generic-title rules.
A format-8 cache still carrying the old form imports as one derived under another document, the path that already existed for that case.
No migration step is needed.
