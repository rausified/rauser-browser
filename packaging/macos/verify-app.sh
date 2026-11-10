#!/usr/bin/env bash
# Check a built Brauser.app: bundle layout, Info.plist, the universal binary,
# its minimum macOS, that it runs, and (when asked) its signature and ticket.
#
#   packaging/macos/verify-app.sh <Brauser.app> [--signature none|adhoc|developer-id] [--notarized]
#
# --signature none (the default) checks no signature: build-app.sh output.
# --signature adhoc requires a valid ad-hoc signature, a local test signature
#   that Gatekeeper rejects; it is never a release signature.
# --signature developer-id requires a valid Developer ID Application signature
#   with the hardened runtime and a secure timestamp.
# --notarized also requires a stapled, valid notarization ticket and
#   `spctl --assess` to accept the app. Only a Developer ID signed app that
#   Apple notarized can pass it.
set -euo pipefail
# shellcheck source=packaging/macos/common.sh
source "$(dirname "$0")/common.sh"
require_macos

app="${1:-}"
[[ -d "$app" ]] || die "usage: verify-app.sh <Brauser.app> [--signature none|adhoc|developer-id] [--notarized]"
shift
signature="none"
notarized=false
while (($#)); do
  case "$1" in
    --signature) signature="${2:-}"; shift 2 ;;
    --notarized) notarized=true; shift ;;
    *) die "unknown option: $1" ;;
  esac
done
case "$signature" in none | adhoc | developer-id) ;; *) die "unknown --signature: $signature" ;; esac
if $notarized && [[ "$signature" != "developer-id" ]]; then
  die "--notarized needs --signature developer-id: only a Developer ID signed app can be notarized"
fi

version="$(brauser_version)"
contents="$app/Contents"
binary="$contents/MacOS/$BRAUSER_EXECUTABLE"
failures=0
fail() {
  printf 'FAIL: %s\n' "$*" >&2
  failures=$((failures + 1))
}
pass() {
  printf 'ok: %s\n' "$*"
}

# 1. Layout: exactly these files, plus the signature's own when signed.
expected=("Contents/Info.plist" "Contents/MacOS/$BRAUSER_EXECUTABLE" "Contents/PkgInfo")
if [[ "$signature" != "none" ]]; then
  expected+=("Contents/_CodeSignature/CodeResources")
fi
if $notarized; then
  expected+=("Contents/CodeResources")
fi
actual="$(cd "$app" && find . -type f -o -type l | sed 's|^\./||' | LC_ALL=C sort)"
wanted="$(printf '%s\n' "${expected[@]}" | LC_ALL=C sort)"
if [[ "$actual" == "$wanted" ]]; then
  pass "bundle layout: $(printf '%s ' "${expected[@]}")"
else
  fail "bundle layout differs from the expected files"
  diff <(printf '%s\n' "$wanted") <(printf '%s\n' "$actual") >&2 || true
fi

# 2. Info.plist keys.
plist="$contents/Info.plist"
if plutil -lint "$plist" > /dev/null; then pass "Info.plist is well formed"; else fail "Info.plist does not lint"; fi
check_key() {
  local key="$1" type="$2" want="$3" got
  got="$(plutil -extract "$key" raw -expect "$type" -o - "$plist" 2> /dev/null || printf '<missing>')"
  if [[ "$got" == "$want" ]]; then pass "$key = $got"; else fail "$key is '$got', want '$want'"; fi
}
check_key CFBundleExecutable string "$BRAUSER_EXECUTABLE"
check_key CFBundleIdentifier string "$BRAUSER_BUNDLE_ID"
check_key CFBundleName string "$BRAUSER_APP_NAME"
check_key CFBundlePackageType string APPL
check_key CFBundleShortVersionString string "$version"
check_key CFBundleVersion string "$version"
check_key LSMinimumSystemVersion string "$BRAUSER_MIN_MACOS"
check_key LSUIElement bool true
if [[ "$(cat "$contents/PkgInfo" 2> /dev/null)" == "APPL????" ]]; then pass "PkgInfo = APPL????"; else fail "PkgInfo is not APPL????"; fi

# 3. One universal binary with exactly the two slices.
if [[ -x "$binary" ]]; then pass "Contents/MacOS/$BRAUSER_EXECUTABLE is executable"; else fail "$binary is missing or not executable"; fi
archs="$(lipo -archs "$binary" 2> /dev/null || true)"
if [[ "$archs" == "$BRAUSER_ARCHS" ]]; then pass "lipo -archs: $archs"; else fail "lipo -archs is '$archs', want '$BRAUSER_ARCHS'"; fi

# 4. Each slice's minimum macOS (LC_BUILD_VERSION minos).
for arch in $BRAUSER_ARCHS; do
  minos="$(vtool -arch "$arch" -show-build "$binary" 2> /dev/null | awk '$1 == "minos" { print $2; exit }')"
  if [[ "$minos" == "$BRAUSER_MIN_MACOS" ]]; then pass "$arch minos $minos"; else fail "$arch minos is '$minos', want $BRAUSER_MIN_MACOS"; fi
done

# 5. It runs: `--version` writes no files (host/src/main.rs writes_no_files).
# Run each slice this Mac can run; x86_64 needs Rosetta on Apple silicon.
want_version="$BRAUSER_EXECUTABLE $version"
for arch in $BRAUSER_ARCHS; do
  if ! arch "-$arch" /usr/bin/true 2> /dev/null; then
    printf 'skip: this Mac cannot run the %s slice\n' "$arch"
    continue
  fi
  got="$(arch "-$arch" "$binary" --version 2>&1 || true)"
  if [[ "$got" == "$want_version" ]]; then pass "$arch slice runs: $got"; else fail "$arch slice --version printed '$got', want '$want_version'"; fi
done

# 6. Signature.
details="$(codesign --display --verbose=4 "$app" 2>&1 || true)"
case "$signature" in
  none)
    printf 'note: signature not checked (--signature none)\n'
    ;;
  adhoc)
    if codesign --verify --strict --verbose=2 "$app" 2> /dev/null; then pass "codesign --verify --strict (ad-hoc)"; else fail "codesign --verify --strict failed"; fi
    if grep -q '^Signature=adhoc' <<< "$details"; then
      pass "AD-HOC signature (local test only: not Developer ID, Gatekeeper rejects it)"
    else
      fail "the signature is not ad-hoc"
    fi
    ;;
  developer-id)
    if codesign --verify --deep --strict --verbose=2 "$app" 2> /dev/null; then pass "codesign --verify --deep --strict"; else fail "codesign --verify --deep --strict failed"; fi
    if grep -q '^Authority=Developer ID Application: ' <<< "$details"; then
      pass "$(grep -m1 '^Authority=Developer ID Application: ' <<< "$details")"
    else
      fail "not signed by a Developer ID Application identity"
    fi
    if grep -Eq '^CodeDirectory .*flags=.*runtime' <<< "$details"; then pass "hardened runtime"; else fail "the hardened runtime is not on"; fi
    if grep -q '^Timestamp=' <<< "$details"; then pass "secure timestamp"; else fail "no secure timestamp"; fi
    ;;
esac

# 7. Notarization ticket and Gatekeeper.
if $notarized; then
  if xcrun stapler validate "$app"; then pass "stapler validate"; else fail "stapler validate failed"; fi
  if spctl --assess --type execute --verbose=4 "$app"; then pass "spctl --assess --type execute"; else fail "spctl --assess rejected the app"; fi
fi

if ((failures)); then
  die "$failures check(s) failed for $app"
fi
note "$app passed every check (signature: $signature, notarized: $notarized)"
