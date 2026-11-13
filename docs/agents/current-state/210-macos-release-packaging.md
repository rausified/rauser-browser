# macOS release packaging

Built, unsigned path only (brwr-3ppp): `packaging/macos/` builds `Brauser.app` (universal host, `LSUIElement`, macOS 15+) in a drag-to-Applications `.dmg` with SBOMs and checksums;
`.github/workflows/release-macos.yml` (tag or dispatch) adds Developer ID signing, notarization, stapling, SLSA provenance and a draft release only when the signing secrets exist, and none are set yet, so no build has been signed or notarized

Pointer: DESIGN.md §11, TESTING.md "macOS release packaging"
