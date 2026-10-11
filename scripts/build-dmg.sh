#!/bin/bash
# Build an installable DMG from the app bundle.
# Usage: scripts/build-dmg.sh  (builds the .app first if needed)
set -euo pipefail
cd "$(dirname "$0")/.."

APP="build/Smart Voices.app"
if [ ! -d "$APP" ]; then
    scripts/build-app.sh > /dev/null
fi

VERSION=$(plutil -extract CFBundleShortVersionString raw "$APP/Contents/Info.plist")
DMG="build/SmartVoices-${VERSION}.dmg"
STAGING="build/dmg-staging"

rm -rf "$STAGING" "$DMG"
mkdir -p "$STAGING"
cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"

hdiutil create \
    -volname "Smart Voices" \
    -srcfolder "$STAGING" \
    -ov -format UDZO \
    "$DMG" > /dev/null
rm -rf "$STAGING"
echo "$DMG"
