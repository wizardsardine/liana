#!/usr/bin/env bash
# Exercise the real imported-inventory Keychain item across process restarts.
# Requires a signing identity authorised by embedded.provisionprofile.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
ENTITLEMENTS="$REPO_ROOT/contrib/release/macos/coincube.entitlements"
PROFILE="$REPO_ROOT/contrib/release/macos/embedded.provisionprofile"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/imported-anchor-native.XXXXXX")"
BUNDLE="$WORK/Probe.app"
RUN_ID="$(uuidgen | tr '[:upper:]' '[:lower:]')"
REPOSITORY="$WORK/repository"
MAY_HAVE_ITEM=0
cleanup() {
    local phase_status=$?
    trap - EXIT
    if [[ "$MAY_HAVE_ITEM" -eq 1 ]]; then
        if ! "$BUNDLE/Contents/MacOS/Probe" delete "$RUN_ID" "$REPOSITORY"; then
            echo "failed to remove the synthetic imported-anchor Keychain item" >&2
            if [[ "$phase_status" -eq 0 ]]; then
                phase_status=1
            fi
        fi
    fi
    if ! rm -rf "$WORK"; then
        echo "failed to remove imported-anchor probe bundle" >&2
        if [[ "$phase_status" -eq 0 ]]; then
            phase_status=1
        fi
    fi
    exit "$phase_status"
}
trap cleanup EXIT

[[ -f "$PROFILE" ]] || { echo "missing embedded profile" >&2; exit 1; }
[[ -f "$ENTITLEMENTS" ]] || { echo "missing release entitlements" >&2; exit 1; }
[[ -n "${SIGN_IDENTITY:-}" ]] || { echo "set SIGN_IDENTITY to the profile-authorised signing identity" >&2; exit 1; }

"$REPO_ROOT/contrib/release/macos/check-profile-authorises-cert.sh" \
    "$PROFILE" --identity "$SIGN_IDENTITY"

cd "$REPO_ROOT"
cargo build -p coincube-gui --example imported_anchor_native --features native-anchor-probe --locked
mkdir -p "$BUNDLE/Contents/MacOS"
BUILD_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
cp "$BUILD_TARGET_DIR/debug/examples/imported_anchor_native" "$BUNDLE/Contents/MacOS/Probe"
cp "$PROFILE" "$BUNDLE/Contents/embedded.provisionprofile"
cat > "$BUNDLE/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>io.coincube.desktop</string>
  <key>CFBundleExecutable</key><string>Probe</string>
  <key>CFBundleName</key><string>Imported Anchor Probe</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1.0</string>
</dict></plist>
PLIST
# CODESIGN_KEYCHAIN picks one keychain when the identity is installed in
# more than one (codesign calls that ambiguous), e.g. login.keychain-db.
codesign -f -s "$SIGN_IDENTITY" ${CODESIGN_KEYCHAIN:+--keychain "$CODESIGN_KEYCHAIN"} \
    --options runtime --entitlements "$ENTITLEMENTS" "$BUNDLE"
codesign --verify --strict "$BUNDLE"
"$REPO_ROOT/contrib/release/macos/check-profile-authorises-cert.sh" \
    "$PROFILE" --signed-bundle "$BUNDLE"
codesign -d --entitlements - "$BUNDLE" 2>&1 | grep -q '8UVR249AD5.io.coincube.desktop'

# Each phase is a fresh signed process against one disposable empty repository.
# The fault seam changes only synthetic Security.framework queries in this
# debug-only build. It cannot demonstrate sudden-power-loss durability.
for phase in setup update-fails verify-old update-error verify-committed readback-fails verify-pending-closed recover verify; do
    MAY_HAVE_ITEM=1
    "$BUNDLE/Contents/MacOS/Probe" "$phase" "$RUN_ID" "$REPOSITORY"
done
