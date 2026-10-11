#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
swift build -c release
APP="build/Smart Voices.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp .build/release/SmartVoices "$APP/Contents/MacOS/SmartVoices"
for bundle in .build/release/*.bundle; do
    if [ -e "$bundle" ]; then
        cp -R "$bundle" "$APP/Contents/Resources/"
    fi
done
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>SmartVoices</string>
    <key>CFBundleIdentifier</key>
    <string>dev.smartvoices.app</string>
    <key>CFBundleName</key>
    <string>Smart Voices</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>0.1.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>15.0</string>
    <key>LSUIElement</key>
    <true/>
</dict>
</plist>
PLIST
codesign --force --sign - "$APP"
echo "$APP"
