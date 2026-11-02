#!/bin/sh
# Lays out "MonoCode CEF.app" for development: the host binary, its helper
# app (the same binary) and the Chromium Embedded Framework, symlinked from
# the copy cef-dll-sys downloaded.
#   scripts/cef-host-bundle.sh <host binary> <framework dir> <out dir>
set -eu
binary=$1
framework=$2
out=$3
app="$out/MonoCode CEF.app"
helper="$app/Contents/Frameworks/MonoCode CEF Helper.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$helper/Contents/MacOS"
cp "$binary" "$app/Contents/MacOS/monocode-cef-host"
cp "$binary" "$helper/Contents/MacOS/MonoCode CEF Helper"
ln -s "$framework" "$app/Contents/Frameworks/Chromium Embedded Framework.framework"
plist() {
  cat > "$1" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>$2</string>
<key>CFBundleIdentifier</key><string>$3</string>
<key>CFBundleName</key><string>$2</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSUIElement</key><true/>
<key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict></plist>
PLIST
}
plist "$app/Contents/Info.plist" monocode-cef-host dev.monocode.cef
plist "$helper/Contents/Info.plist" "MonoCode CEF Helper" dev.monocode.cef.helper
echo "$app"
