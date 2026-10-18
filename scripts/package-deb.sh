#!/bin/bash
# Package the release binary as a Debian/Ubuntu .deb (run on Debian/Ubuntu
# after `cargo build --release`). Writes target/smart-voices-linux-amd64.deb.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -1)"
BIN="$ROOT_DIR/target/release/smart-voices"
PKG="$ROOT_DIR/target/deb/smart-voices"
OUT="$ROOT_DIR/target/smart-voices-linux-amd64.deb"

rm -rf "$ROOT_DIR/target/deb"
mkdir -p "$PKG/DEBIAN" "$PKG/usr/bin" "$PKG/usr/lib/smart-voices" \
  "$PKG/usr/share/applications" "$PKG/usr/share/icons/hicolor/256x256/apps"
# The voice runtime ships as shared libraries next to the binary (the app
# finds them via $ORIGIN); keep them together under /usr/lib/smart-voices.
install -m 755 "$BIN" "$PKG/usr/lib/smart-voices/smart-voices"
install -m 644 "$ROOT_DIR"/target/release/*.so "$PKG/usr/lib/smart-voices/"
ln -sf ../lib/smart-voices/smart-voices "$PKG/usr/bin/smart-voices"
sed 's/^Icon=.*/Icon=smart-voices/' "$ROOT_DIR/packaging/linux/smart-voices.desktop" \
  > "$PKG/usr/share/applications/smart-voices.desktop"
install -m 644 "$ROOT_DIR/packaging/linux/smart-voices.png" \
  "$PKG/usr/share/icons/hicolor/256x256/apps/smart-voices.png"

# Work out shared-library dependencies from the binary itself.
SHLIBS_DIR="$ROOT_DIR/target/deb/shlibs"
mkdir -p "$SHLIBS_DIR/debian"
echo "Source: smart-voices" > "$SHLIBS_DIR/debian/control"
DEPENDS="$(cd "$SHLIBS_DIR" && dpkg-shlibdeps -O "$PKG/usr/lib/smart-voices/smart-voices" \
  | sed -n 's/^shlibs:Depends=//p')"

cat > "$PKG/DEBIAN/control" <<EOF
Package: smart-voices
Version: $VERSION
Architecture: amd64
Maintainer: saostad <16025625+saostad@users.noreply.github.com>
Depends: $DEPENDS
Section: utils
Priority: optional
Homepage: https://github.com/saostad/smart-voices
Description: Read copied text aloud with an on-device voice
 Smart Voices sits in the system tray, watches the clipboard and reads
 copied text aloud. English and Persian voices run fully offline.
EOF

dpkg-deb --root-owner-group --build "$PKG" "$OUT"
echo "Created $OUT"
