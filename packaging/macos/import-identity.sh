#!/usr/bin/env bash
# CI only: import the Developer ID Application certificate into a throwaway
# keychain and print the identity's name, for sign.sh.
#
#   packaging/macos/import-identity.sh <keychain-path>
#
# Reads from the environment (GitHub secrets, TESTING.md "macOS release
# packaging"):
#   MACOS_DEVELOPER_ID_P12_BASE64    the certificate and private key, as a base64 .p12
#   MACOS_DEVELOPER_ID_P12_PASSWORD  the .p12's export password
# Prints exactly one line: the "Developer ID Application: ..." identity.
# Delete the keychain afterwards with `security delete-keychain <path>`.
set -euo pipefail
# shellcheck source=packaging/macos/common.sh
source "$(dirname "$0")/common.sh"
require_macos

keychain="${1:-}"
[[ -n "$keychain" ]] || die "usage: import-identity.sh <keychain-path>"
: "${MACOS_DEVELOPER_ID_P12_BASE64:?set MACOS_DEVELOPER_ID_P12_BASE64}"
: "${MACOS_DEVELOPER_ID_P12_PASSWORD:?set MACOS_DEVELOPER_ID_P12_PASSWORD}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
printf '%s' "$MACOS_DEVELOPER_ID_P12_BASE64" | base64 --decode > "$work/identity.p12"
keychain_password="$(uuidgen)"

security create-keychain -p "$keychain_password" "$keychain" >&2
security set-keychain-settings -lut 21600 "$keychain" >&2
security unlock-keychain -p "$keychain_password" "$keychain" >&2
security import "$work/identity.p12" -k "$keychain" -P "$MACOS_DEVELOPER_ID_P12_PASSWORD" \
  -T /usr/bin/codesign -f pkcs12 >&2
# Let codesign use the key without a UI prompt.
security set-key-partition-list -S apple-tool:,apple: -s -k "$keychain_password" "$keychain" > /dev/null
# Add it to the search list, keeping the existing keychains, so codesign
# finds the certificate chain.
existing=()
while IFS= read -r line; do
  line="${line#"${line%%[![:space:]]*}"}"
  existing+=("${line//\"/}")
done < <(security list-keychains -d user)
security list-keychains -d user -s "$keychain" ${existing[@]+"${existing[@]}"}

identities="$(security find-identity -v -p codesigning "$keychain" | sed -n 's/^ *[0-9]*) [0-9A-F]* "\(Developer ID Application: .*\)"$/\1/p')"
count="$(printf '%s' "$identities" | grep -c . || true)"
[[ "$count" == "1" ]] || die "the .p12 must hold exactly one Developer ID Application identity (found $count)"
printf '%s\n' "$identities"
