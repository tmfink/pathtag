#!/usr/bin/env bash
# Build macOS APP bundle
#
# Requirements:
# - dylibbundler
# - librsvg
#
# Install with:
# brew install dylibbundler librsvg

set -eu

REPO_DIR="${BASH_SOURCE[0]%/*}/.."

SVG2ICNS="${REPO_DIR}/script/svg2icns.sh"
ICON_SVG="${REPO_DIR}/crates/gui/assets/pathtag-logo.svg"

if [ $# -lt 1 ] || [ $# -gt 3 ]; then
    echo "Usage: INPUT_BIN [OUTPUT_APP [OUTPUT_ZIP]]"
    exit 1
fi

BIN="$1"
shift

if [ $# -gt 0 ]; then
    APP="$1"
    shift
else
    APP="${BIN%.*}.app"
fi

if [ $# -gt 0 ]; then
    OUTPUT_ZIP="$1"
    shift
else
    OUTPUT_ZIP="${APP%.*}.zip"
fi

BIN_BASENAME="${BIN##*/}"

rm -rf "${APP}"
mkdir -p "${APP}"
mkdir -p "${APP}/Contents/MacOS"
mkdir -p "${APP}/Contents/Frameworks"
mkdir -p "${APP}/Contents/Resources"

cp "${BIN}" "${APP}/Contents/MacOS/${BIN_BASENAME}"

"${SVG2ICNS}" "${ICON_SVG}" "${APP}/Contents/Resources/AppIcon.icns"

PLIST_DATA='<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
 "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>'"${BIN_BASENAME}"'</string>
  <key>CFBundleIdentifier</key>
  <string>net.travisf.'"${BIN_BASENAME}"'</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleName</key>
  <string>'"${BIN_BASENAME}"'</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>CFBundleShortVersionString</key>
  <string>1.0</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
</dict>
</plist>
'

echo "${PLIST_DATA}" >"${APP}/Contents/Info.plist"

dylibbundler \
    -od \
    -b \
    -x "${APP}/Contents/MacOS/${BIN_BASENAME}" \
    -d "${APP}/Contents/Frameworks/" \
    -p "@executable_path/../Frameworks/"

otool -L "${APP}/Contents/MacOS/${BIN_BASENAME}"

ditto -c -k --sequesterRsrc --keepParent "${APP}" "${OUTPUT_ZIP}"

echo
echo "Created outputs:"
echo "    ${APP}"
echo "    ${OUTPUT_ZIP}"
