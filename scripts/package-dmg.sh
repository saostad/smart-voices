#!/bin/bash
# Wrap target/Smart Voices.app (from bundle-macos.sh) in a drag-to-install
# disk image. Writes target/smart-voices-macos-arm64.dmg.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
APP="$ROOT_DIR/target/Smart Voices.app"
STAGE="$ROOT_DIR/target/dmg"
OUT="$ROOT_DIR/target/smart-voices-macos-arm64.dmg"

rm -rf "$STAGE" "$OUT"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "Smart Voices" -srcfolder "$STAGE" -ov -format UDZO "$OUT"
echo "Created $OUT"
