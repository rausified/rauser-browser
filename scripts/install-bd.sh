#!/bin/sh
# Install a pinned bd (beads) release on a Linux x86-64 CI runner, checksum
# verified, into $1 (default $HOME/.local/bin). Used by the Queue check and PR
# doctor workflows, which bootstrap the work queue from refs/dolt/data
# (AGENTS.md "Work queue"). Bump VERSION and SHA256 together.
set -eu
VERSION=1.3.2
SHA256=c6ea3281f6ff82628351b2d0967c9eb85862c8a0ec0ebc24e63e645c8b02462e
dest=${1:-"$HOME/.local/bin"}
tarball="beads_${VERSION}_linux_amd64.tar.gz"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl -fsSL --retry 3 -o "$work/$tarball" \
  "https://github.com/gastownhall/beads/releases/download/v${VERSION}/${tarball}"
echo "$SHA256  $work/$tarball" | sha256sum -c -
tar -xzf "$work/$tarball" -C "$work" bd
mkdir -p "$dest"
install -m 0755 "$work/bd" "$dest/bd"
"$dest/bd" version
