#!/usr/bin/env bash
#
# Run the #[ignore]d device_secret tests against a code-signed binary.
#
# # Why this script exists rather than a one-line cargo invocation
#
# Those tests write to the macOS data-protection keychain, which is only
# reachable from a binary carrying the `keychain-access-groups` entitlement.
# That entitlement is *restricted*: AMFI validates it against an embedded
# provisioning profile at exec time, and a profile can only be embedded in a
# bundle. `cargo test` produces a bare executable, so signing it directly is
# not enough — the kernel kills it on exec with no output at all (exit 137),
# which looks like a hung test rather than a signing problem.
#
# So the test binary is wrapped in a minimal .app whose CFBundleIdentifier
# matches the profile, that bundle is signed, and the executable is run from
# inside it.
#
# # Usage
#
# Local Mac, identity in the login keychain:
#   SIGN_IDENTITY="Developer ID Application: COINCUBE TECHNOLOGY LLC" \
#     contrib/release/macos/run-signed-device-secret-tests.sh
#
# CI, signing from a p12 with rcodesign:
#   P12_FILE=cert.p12 P12_PASSWORD_FILE=cert.pass \
#     contrib/release/macos/run-signed-device-secret-tests.sh
#
# Green here is the only proof the entitlement *took*. `codesign -d` proves
# only that it was requested.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
ENTITLEMENTS="$REPO_ROOT/contrib/release/macos/coincube.entitlements"
PROFILE="$REPO_ROOT/contrib/release/macos/embedded.provisionprofile"
WORK="${TMPDIR:-/tmp}/tenshu-signed-tests.$$"
BUNDLE="$WORK/TestRunner.app"

cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

for f in "$ENTITLEMENTS" "$PROFILE"; do
    [ -f "$f" ] || { echo "missing required file: $f" >&2; exit 1; }
done

echo "==> Building the test binary"
# `cargo test --no-run` prints the path on stderr in a human-readable line and
# in the JSON stream as "executable". Parsed without jq so this runs on a
# stock runner.
BIN=$(cd "$REPO_ROOT" && cargo test -p coincube-gui --lib --no-run --message-format=json 2>/dev/null \
    | grep -o '"executable":"[^"]*"' | grep -v ':"null"' | tail -1 | cut -d'"' -f4)
[ -n "$BIN" ] && [ -f "$BIN" ] || { echo "could not locate the test binary" >&2; exit 1; }
echo "    $BIN"

echo "==> Assembling a minimal signed bundle"
# CFBundleIdentifier MUST match the App ID the profile was issued for, or AMFI
# reports "No matching profile found" and kills the process on exec.
mkdir -p "$BUNDLE/Contents/MacOS"
cat > "$BUNDLE/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>io.coincube.tenshu</string>
    <key>CFBundleExecutable</key>
    <string>TestRunner</string>
    <key>CFBundleName</key>
    <string>TestRunner</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleVersion</key>
    <string>1.0</string>
</dict>
</plist>
PLIST
cp "$BIN" "$BUNDLE/Contents/MacOS/TestRunner"
cp "$PROFILE" "$BUNDLE/Contents/embedded.provisionprofile"

echo "==> Signing"
if [ -n "${SIGN_IDENTITY:-}" ]; then
    codesign -f -s "$SIGN_IDENTITY" --options runtime --entitlements "$ENTITLEMENTS" "$BUNDLE"
elif [ -n "${P12_FILE:-}" ] && [ -n "${P12_PASSWORD_FILE:-}" ]; then
    rcodesign sign --code-signature-flags runtime \
        --entitlements-xml-path "$ENTITLEMENTS" \
        --p12-file "$P12_FILE" --p12-password-file "$P12_PASSWORD_FILE" \
        "$BUNDLE"
else
    echo "set SIGN_IDENTITY, or both P12_FILE and P12_PASSWORD_FILE" >&2
    exit 1
fi

codesign -d --entitlements - "$BUNDLE" 2>&1 | grep -q 'keychain-access-groups' \
    || { echo "the signature does not carry keychain-access-groups" >&2; exit 1; }

echo "==> Running the ignored device_secret tests"
# --test-threads=1: these tests share one keychain service name and assert on
# provisioning races; running them concurrently would have them fight.
"$BUNDLE/Contents/MacOS/TestRunner" device_secret --ignored --test-threads=1
