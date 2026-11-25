#!/bin/bash
# Checks that a Bridge.app downloaded through a browser opens on macOS:
# Gatekeeper may call it unverified (Open Anyway), never damaged.
set -euo pipefail
app="${1:?pass the Bridge.app path}"
# A bundle whose seal is missing or broken is what Gatekeeper reports as
# "damaged and can't be opened", with no Open Anyway.
codesign --verify --deep --strict --verbose=2 "$app"
# Gatekeeper only assesses downloads, so copy the app and mark it the way
# Safari marks a download.
work=$(mktemp -d "${TMPDIR:-/tmp}/bridge-open-check.XXXXXX")
trap 'rm -rf "$work"' EXIT
ditto "$app" "$work/Bridge.app"
xattr -w com.apple.quarantine "0083;$(printf %x "$(date +%s)");Safari;" "$work/Bridge.app"
status=0
output=$(spctl --assess --type execute -vv "$work/Bridge.app" 2>&1) || status=$?
echo "$output"
# An ad hoc seal without notarization is "rejected" (exit 3): macOS shows
# "could not verify" and Privacy & Security offers Open Anyway. Any other
# failure, such as "code has no resources but signature indicates they must
# be present" (exit 1), is the damaged case. "accepted" would mean Gatekeeper
# is off on this machine, so the check proves nothing and fails too.
if [[ $status -eq 3 && "$output" == "$work/Bridge.app: rejected" ]]; then
    echo 'Bridge.app is sealed; a browser download opens through Open Anyway'
else
    echo "Expected an unverified, not damaged, Bridge.app; spctl exit $status" >&2
    exit 1
fi
