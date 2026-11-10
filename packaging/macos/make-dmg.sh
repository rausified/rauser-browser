#!/usr/bin/env bash
# Put Brauser.app in a drag-to-Applications disk image (DESIGN.md §11): the
# image's window shows the app beside an Applications shortcut.
#
#   packaging/macos/make-dmg.sh <Brauser.app> <out.dmg>
#
# Sign the app (and, for a release, notarize and staple it) first: the image
# holds a copy of it. Sign the image afterwards with sign.sh.
set -euo pipefail
# shellcheck source=packaging/macos/common.sh
source "$(dirname "$0")/common.sh"
require_macos

app="${1:-}"
dmg="${2:-}"
[[ -d "$app" && -n "$dmg" ]] || die "usage: make-dmg.sh <Brauser.app> <out.dmg>"
[[ "$dmg" == *.dmg ]] || die "the output must end in .dmg"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
# ditto keeps the bundle's signature, extended attributes and stapled ticket.
ditto "$app" "$stage/$BRAUSER_APP_NAME.app"
ln -s /Applications "$stage/Applications"

mkdir -p "$(dirname "$dmg")"
rm -f "$dmg"
note "Creating $dmg"
# UDZO: a read-only, zlib-compressed image, which every supported macOS opens.
hdiutil create -volname "$BRAUSER_APP_NAME" -srcfolder "$stage" -fs HFS+ \
  -format UDZO -imagekey zlib-level=9 -ov "$dmg"
hdiutil verify "$dmg"
