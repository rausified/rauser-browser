#!/usr/bin/env bash
# Sign Brauser.app or its disk image.
#
#   packaging/macos/sign.sh --identity "Developer ID Application: <Name> (<TEAMID>)" [--keychain <path>] <Brauser.app | file.dmg>
#   packaging/macos/sign.sh --adhoc <Brauser.app | file.dmg>
#
# A Developer ID signature is the release signature (DESIGN.md §11): the app
# gets the hardened runtime and a secure timestamp, both of which notarization
# requires. No entitlements are needed: the host loads no third-party code,
# uses no JIT, and its harness children (codex, claude) are separate programs.
#
# --adhoc makes an AD-HOC signature for local testing only. It is not a
# Developer ID signature, Gatekeeper rejects it, and Apple will not notarize
# it; never publish an ad-hoc signed build.
set -euo pipefail
# shellcheck source=packaging/macos/common.sh
source "$(dirname "$0")/common.sh"
require_macos

identity=""
keychain=""
adhoc=false
target=""
while (($#)); do
  case "$1" in
    --identity) identity="${2:-}"; shift 2 ;;
    --keychain) keychain="${2:-}"; shift 2 ;;
    --adhoc) adhoc=true; shift ;;
    -*) die "unknown option: $1" ;;
    *) target="$1"; shift ;;
  esac
done
[[ -e "$target" ]] || die "usage: sign.sh (--identity <Developer ID Application: ...> | --adhoc) <Brauser.app | file.dmg>"

args=(--force)
if $adhoc; then
  [[ -z "$identity" ]] || die "--adhoc and --identity are exclusive"
  args+=(--sign -)
  label="AD-HOC (local test only, not Developer ID)"
else
  # Refuse anything but a Developer ID Application identity, so a stray Apple
  # Development or Installer identity never signs a release by mistake.
  [[ "$identity" == "Developer ID Application: "* ]] ||
    die "--identity must be a 'Developer ID Application: ...' identity (got '$identity'); use --adhoc for a local test signature"
  args+=(--sign "$identity" --timestamp)
  [[ -z "$keychain" ]] || args+=(--keychain "$keychain")
  label="$identity"
fi

case "$target" in
  *.app | *.app/)
    # The hardened runtime applies to the app's code; a disk image has none.
    args+=(--options runtime)
    ;;
  *.dmg) ;;
  *) die "sign.sh signs a .app bundle or a .dmg, not $target" ;;
esac

note "Signing $target with $label"
codesign "${args[@]}" "$target"
codesign --verify --strict --verbose=2 "$target"
if $adhoc; then
  printf 'WARNING: %s carries an AD-HOC signature. It is for local testing only and will not pass Gatekeeper.\n' "$target" >&2
fi
