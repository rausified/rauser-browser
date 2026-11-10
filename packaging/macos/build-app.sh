#!/usr/bin/env bash
# Build Brauser.app: a universal (x86_64 + arm64) release host in a
# background-only bundle for macOS 15 and later (DESIGN.md §11 "macOS app").
#
#   packaging/macos/build-app.sh <out-dir>
#
# Writes <out-dir>/Brauser.app, unsigned (each slice keeps only the linker's
# own signature). Sign it with sign.sh. Needs both Rust targets installed:
#   rustup target add x86_64-apple-darwin aarch64-apple-darwin
set -euo pipefail
# shellcheck source=packaging/macos/common.sh
source "$(dirname "$0")/common.sh"
require_macos

out="${1:-}"
[[ -n "$out" ]] || die "usage: build-app.sh <out-dir>"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
version="$(brauser_version)"
app="$out/$BRAUSER_APP_NAME.app"

cd "$REPO_ROOT"
# The deployment target sets each slice's LC_BUILD_VERSION minos, and the C
# compiler (rusqlite's bundled SQLite) reads it too.
export MACOSX_DEPLOYMENT_TARGET="$BRAUSER_MIN_MACOS"
slices=()
for target in "${BRAUSER_RUST_TARGETS[@]}"; do
  note "Building the release host for $target"
  cargo build --locked --release -p brauser --bin "$BRAUSER_EXECUTABLE" --target "$target"
  slices+=("target/$target/release/$BRAUSER_EXECUTABLE")
done

note "Assembling $app (version $version)"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
lipo -create -output "$app/Contents/MacOS/$BRAUSER_EXECUTABLE" "${slices[@]}"
chmod 755 "$app/Contents/MacOS/$BRAUSER_EXECUTABLE"
sed "s/@VERSION@/$version/g" "$PACKAGING_DIR/Info.plist.in" > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"

"$PACKAGING_DIR/verify-app.sh" "$app"
