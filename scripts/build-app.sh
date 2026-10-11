#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
swift build -c release
APP="build/Smart Voices.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp .build/release/SmartVoices "$APP/Contents/MacOS/SmartVoices"
if [ ! -e ".build/release/FluidAudio_FluidAudio.bundle" ]; then
    echo "build-app.sh: missing FluidAudio_FluidAudio.bundle in .build/release/ (the Kokoro engine needs it in Contents/Resources/)" >&2
    exit 1
fi
for bundle in .build/release/*.bundle; do
    if [ -e "$bundle" ]; then
        cp -R "$bundle" "$APP/Contents/Resources/"
    fi
done
if [ ! -f "Resources/AppIcon.icns" ]; then
    echo "build-app.sh: missing Resources/AppIcon.icns (run: swift scripts/make-icon.swift Resources/AppIcon.icns)" >&2
    exit 1
fi
cp "Resources/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
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
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
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
    <key>NSServices</key>
    <array>
        <dict>
            <key>NSMenuItem</key>
            <dict>
                <key>default</key>
                <string>Read Aloud with Smart Voices</string>
            </dict>
            <key>NSMessage</key>
            <string>readSelection</string>
            <key>NSPortName</key>
            <string>Smart Voices</string>
            <key>NSSendTypes</key>
            <array>
                <string>NSStringPboardType</string>
                <string>public.utf8-plain-text</string>
            </array>
        </dict>
    </array>
</dict>
</plist>
PLIST
codesign --force --sign - "$APP"
echo "$APP"
