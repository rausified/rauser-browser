#!/usr/bin/env bash
# Mount a Brauser disk image read-only and check its layout, the app inside
# it, and (when asked) the image's own signature and ticket.
#
#   packaging/macos/verify-dmg.sh <file.dmg> [--signature none|adhoc|developer-id]
#                                 [--app-signature none|adhoc|developer-id] [--notarized]
#
# The options mean what they mean for verify-app.sh. --signature is the
# image's; --app-signature is the app's inside it, and defaults to the
# image's (an unsigned test image holds an ad-hoc signed app:
# `--app-signature adhoc`). --notarized needs a stapled ticket on both and
# `spctl --assess` to accept both: only a Developer ID signed, notarized
# release can pass it.
set -euo pipefail
# shellcheck source=packaging/macos/common.sh
source "$(dirname "$0")/common.sh"
require_macos

dmg="${1:-}"
[[ -f "$dmg" ]] || die "usage: verify-dmg.sh <file.dmg> [--signature ...] [--app-signature ...] [--notarized]"
shift
signature="none"
app_signature=""
notarized=false
while (($#)); do
  case "$1" in
    --signature) signature="${2:-}"; shift 2 ;;
    --app-signature) app_signature="${2:-}"; shift 2 ;;
    --notarized) notarized=true; shift ;;
    *) die "unknown option: $1" ;;
  esac
done
app_signature="${app_signature:-$signature}"
for value in "$signature" "$app_signature"; do
  case "$value" in none | adhoc | developer-id) ;; *) die "unknown signature kind: $value" ;; esac
done
if $notarized && [[ "$signature" != "developer-id" || "$app_signature" != "developer-id" ]]; then
  die "--notarized needs a Developer ID signature on both the image and the app"
fi
app_args=(--signature "$app_signature")
if $notarized; then app_args+=(--notarized); fi

failures=0
fail() {
  printf 'FAIL: %s\n' "$*" >&2
  failures=$((failures + 1))
}
pass() {
  printf 'ok: %s\n' "$*"
}

# The image's own signature.
case "$signature" in
  none) printf 'note: image signature not checked (--signature none)\n' ;;
  adhoc)
    if codesign --verify --strict "$dmg" 2> /dev/null &&
      codesign --display --verbose=2 "$dmg" 2>&1 | grep -q '^Signature=adhoc'; then
      pass "image has a valid AD-HOC signature (local test only)"
    else
      fail "image has no valid ad-hoc signature"
    fi
    ;;
  developer-id)
    if codesign --verify --strict "$dmg" 2> /dev/null; then pass "image codesign --verify --strict"; else fail "image signature does not verify"; fi
    authority="$(codesign --display --verbose=2 "$dmg" 2>&1 | grep -m1 '^Authority=Developer ID Application: ' || true)"
    if [[ -n "$authority" ]]; then pass "image $authority"; else fail "image is not Developer ID signed"; fi
    ;;
esac
if $notarized; then
  if xcrun stapler validate "$dmg"; then pass "image stapler validate"; else fail "image stapler validate failed"; fi
  if spctl --assess --type open --context context:primary-signature --verbose=4 "$dmg"; then
    pass "image spctl --assess --type open"
  else
    fail "spctl --assess rejected the image"
  fi
fi

mount="$(mktemp -d)"
cleanup() {
  hdiutil detach "$mount" -quiet 2> /dev/null || hdiutil detach "$mount" -force -quiet 2> /dev/null || true
  rmdir "$mount" 2> /dev/null || true
}
trap cleanup EXIT
hdiutil attach "$dmg" -readonly -nobrowse -noautoopen -mountpoint "$mount" -quiet
pass "image mounts read-only"

# Top level: the app and the Applications shortcut, nothing else visible.
listing="$(cd "$mount" && find . -mindepth 1 -maxdepth 1 ! -name '.*' | sed 's|^\./||' | LC_ALL=C sort | tr '\n' ' ')"
if [[ "$listing" == "Applications $BRAUSER_APP_NAME.app " ]]; then
  pass "image holds: $listing"
else
  fail "image top level is '$listing', want 'Applications $BRAUSER_APP_NAME.app'"
fi
if [[ -L "$mount/Applications" && "$(readlink "$mount/Applications")" == "/Applications" ]]; then
  pass "Applications -> /Applications"
else
  fail "Applications is not a symlink to /Applications"
fi

if "$PACKAGING_DIR/verify-app.sh" "$mount/$BRAUSER_APP_NAME.app" "${app_args[@]}"; then
  pass "the app inside the image passed verify-app.sh"
else
  fail "the app inside the image failed verify-app.sh"
fi

if ((failures)); then
  die "$failures check(s) failed for $dmg"
fi
note "$dmg passed every check (image signature: $signature, app signature: $app_signature, notarized: $notarized)"
