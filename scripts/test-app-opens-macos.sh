#!/bin/bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
check="$root/scripts/check-app-opens-macos.sh"
work=$(mktemp -d "${TMPDIR:-/tmp}/bridge-open-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
# Build the shape the release had: a linker-signed executable and resources,
# but no bundle seal.
app="$work/Bridge.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
printf 'int main(void) { return 0; }\n' >"$work/main.c"
xcrun clang -arch arm64 "$work/main.c" -o "$app/Contents/MacOS/orka-bridge"
printf 'notice\n' >"$app/Contents/Resources/notice.txt"
cat >"$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>orka-bridge</string>
<key>CFBundleIdentifier</key><string>ai.orka.bridge.open-test</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
PLIST
if bash "$check" "$app"; then
    echo 'an unsealed bundle must fail the check' >&2
    exit 1
fi
codesign --force --sign - "$app"
bash "$check" "$app"
printf 'changed\n' >"$app/Contents/Resources/notice.txt"
if bash "$check" "$app"; then
    echo 'a bundle changed after sealing must fail the check' >&2
    exit 1
fi
echo 'Open check: unsealed and changed bundles fail; a sealed bundle passes'
