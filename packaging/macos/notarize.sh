#!/usr/bin/env bash
# Notarize a Developer ID signed Brauser.app or disk image with Apple's notary
# service, then staple the ticket to it.
#
#   packaging/macos/notarize.sh <Brauser.app | file.dmg>
#
# Authenticates with an App Store Connect API key (TESTING.md "macOS release
# packaging"), read from the environment:
#   APPLE_NOTARY_KEY_ID      the key's ID
#   APPLE_NOTARY_ISSUER_ID   the issuer ID of the key's team
#   APPLE_NOTARY_KEY_PATH    path to the key's AuthKey_<ID>.p8 file
# Fails, and staples nothing, unless Apple answers "Accepted".
set -euo pipefail
# shellcheck source=packaging/macos/common.sh
source "$(dirname "$0")/common.sh"
require_macos

target="${1:-}"
[[ -e "$target" ]] || die "usage: notarize.sh <Brauser.app | file.dmg>"
: "${APPLE_NOTARY_KEY_ID:?set APPLE_NOTARY_KEY_ID}"
: "${APPLE_NOTARY_ISSUER_ID:?set APPLE_NOTARY_ISSUER_ID}"
: "${APPLE_NOTARY_KEY_PATH:?set APPLE_NOTARY_KEY_PATH}"
[[ -f "$APPLE_NOTARY_KEY_PATH" ]] || die "APPLE_NOTARY_KEY_PATH names no file"

# Apple notarizes only Developer ID signed code; check before uploading.
codesign --verify --strict "$target"
codesign --display --verbose=2 "$target" 2>&1 | grep -q '^Authority=Developer ID Application: ' ||
  die "$target is not Developer ID signed; Apple will not notarize it"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
case "$target" in
  *.app | *.app/)
    # notarytool takes a zip, dmg or pkg; zip the bundle with its parent
    # folder so the ticket matches the bundle, as ditto preserves it.
    upload="$work/$(basename "$target" .app).zip"
    ditto -c -k --keepParent "$target" "$upload"
    ;;
  *.dmg) upload="$target" ;;
  *) die "notarize.sh takes a .app bundle or a .dmg, not $target" ;;
esac

auth=(--key "$APPLE_NOTARY_KEY_PATH" --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER_ID")
note "Submitting $target to Apple's notary service (waiting for the result)"
xcrun notarytool submit "$upload" "${auth[@]}" --wait --timeout 45m --output-format json > "$work/result.json" || true
cat "$work/result.json"
status="$(plutil -extract status raw -o - "$work/result.json" 2> /dev/null || printf 'unknown')"
id="$(plutil -extract id raw -o - "$work/result.json" 2> /dev/null || printf '')"
if [[ "$status" != "Accepted" ]]; then
  if [[ -n "$id" ]]; then
    xcrun notarytool log "$id" "${auth[@]}" >&2 || true
  fi
  die "notarization of $target ended with status '$status'; nothing was stapled"
fi

note "Stapling the ticket to $target"
xcrun stapler staple "$target"
xcrun stapler validate "$target"
