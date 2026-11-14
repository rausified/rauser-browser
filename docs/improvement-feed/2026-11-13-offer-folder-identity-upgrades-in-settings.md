## 2026-11-13 — Offer folder identity upgrades in settings

Change: Show which configured folders still use a legacy device-and-inode identity, and offer a guided native-picker reselection for each, preserving its purpose-bound token and revision checks.

Why: The persistent macOS identity fix prevents device-number changes from invalidating new selections, but existing selections cannot supply their original volume UUID. The home folder, separate knowledge base and configured task folders must each be upgraded once, and an ordinary home-folder reselection intentionally leaves the others unchanged. Clear settings guidance would make that one-time step discoverable.

Where: `extension/options.ts`, `extension/options.html`, the host's read-only configuration view, and the selected-folder compatibility tests. Do not infer a volume from an inode or bypass native selection.
