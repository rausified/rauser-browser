#!/usr/bin/env bash
# Write a release's SBOMs and checksums beside its disk image (DESIGN.md §11
# "Release artifacts"). Provenance is not made here: the release workflow
# attests the image with actions/attest-build-provenance.
#
#   packaging/macos/release-assets.sh <dist-dir>
#
# <dist-dir> holds the .dmg. Writes:
#   brauser-<version>-host.cdx.json       CycloneDX SBOM of the Rust crates
#                                         linked into the app's binary, for
#                                         both macOS targets (cargo-cyclonedx)
#   brauser-<version>-extension.cdx.json  CycloneDX SBOM of the extension's
#                                         runtime npm packages (`npm sbom
#                                         --omit dev`); the extension ships
#                                         through the Chrome Web Store, not
#                                         in the image
#   SHA256SUMS                            sha256 of every other file
# Needs cargo-cyclonedx (cargo install --locked cargo-cyclonedx) and
# node_modules installed with `npm ci`.
set -euo pipefail
# shellcheck source=packaging/macos/common.sh
source "$(dirname "$0")/common.sh"

dist="${1:-}"
[[ -d "$dist" ]] || die "usage: release-assets.sh <dist-dir>"
dist="$(cd "$dist" && pwd)"
version="$(brauser_version)"
cd "$REPO_ROOT"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

note "Writing the host SBOM"
# One SBOM per target (the dependency graph is target-specific), merged by
# component so the file lists every crate either slice links. Dev
# dependencies are never included; build scripts' dependencies are dropped
# because none of their code is in the binary.
boms=()
other_crates=(protocol adapter-templates)
for target in "${BRAUSER_RUST_TARGETS[@]}"; do
  cargo cyclonedx --manifest-path host/Cargo.toml --format json --spec-version 1.5 \
    --no-build-deps --target "$target" \
    --override-filename "brauser-sbom-$target" > /dev/null
  bom="host/brauser-sbom-$target.json"
  [[ -f "$bom" ]] || die "cargo-cyclonedx wrote no SBOM for $target"
  mv "$bom" "$work/$target.cdx.json"
  boms+=("$work/$target.cdx.json")
  # It also writes one beside the other workspace crates' Cargo.toml; the
  # host's SBOM already lists them as components, so remove those.
  for member in "${other_crates[@]}"; do
    rm -f "$member/brauser-sbom-$target.json"
  done
done
node "$PACKAGING_DIR/merge-cdx.mjs" "${boms[@]}" > "$dist/brauser-$version-host.cdx.json"

note "Writing the extension SBOM"
npm sbom --omit dev --sbom-format cyclonedx --sbom-type application > "$work/extension.cdx.json"
# npm names the root component after the checkout's folder; use the package's name.
node -e '
  const fs = require("node:fs");
  const bom = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  bom.metadata.component.name = JSON.parse(fs.readFileSync("package.json", "utf8")).name;
  process.stdout.write(JSON.stringify(bom, null, 2) + "\n");
' "$work/extension.cdx.json" > "$dist/brauser-$version-extension.cdx.json"

note "Writing SHA256SUMS"
(
  cd "$dist"
  rm -f SHA256SUMS
  sums="$(find . -maxdepth 1 -type f ! -name SHA256SUMS | sed "s|^\./||" | LC_ALL=C sort | xargs shasum -a 256)"
  printf "%s\n" "$sums" > SHA256SUMS
  shasum -a 256 -c SHA256SUMS
)
