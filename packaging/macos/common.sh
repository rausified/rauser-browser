# Shared settings for the macOS packaging scripts (sourced, not run).
# shellcheck shell=bash
# The settings are read by the scripts that source this file.
# shellcheck disable=SC2034

# The bundle the scripts build and check (DESIGN.md §11 "macOS app").
BRAUSER_APP_NAME="Brauser"
BRAUSER_BUNDLE_ID="com.rauser.brauser"
BRAUSER_EXECUTABLE="brauser"
# macOS 15 Sequoia and later: the binary's minos and LSMinimumSystemVersion.
BRAUSER_MIN_MACOS="15.0"
# Both slices of the universal host, in the order `lipo -archs` prints them.
BRAUSER_RUST_TARGETS=(x86_64-apple-darwin aarch64-apple-darwin)
BRAUSER_ARCHS="x86_64 arm64"

PACKAGING_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$PACKAGING_DIR/../.." && pwd)"

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

note() {
  printf '==> %s\n' "$*"
}

# The host crate's version, the one source of the app's version strings.
brauser_version() {
  local version
  version="$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$REPO_ROOT/host/Cargo.toml" | head -n 1)"
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "could not read a version from host/Cargo.toml (got '$version')"
  printf '%s\n' "$version"
}

require_macos() {
  [[ "$(uname -s)" == "Darwin" ]] || die "macOS packaging runs only on macOS"
}
