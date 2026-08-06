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

# The profile authorises a specific CERTIFICATE, not just an App ID, and every
# other gate passes when that is wrong — the failure is a bare SIGKILL below.
# Check it before spending a signing round-trip on it.
echo "==> Checking the profile authorises the signing certificate"
if [ -n "${SIGN_IDENTITY:-}" ]; then
    "$REPO_ROOT/contrib/release/macos/check-profile-authorises-cert.sh" \
        "$PROFILE" --identity "$SIGN_IDENTITY"
elif [ -n "${P12_FILE:-}" ] && [ -n "${P12_PASSWORD_FILE:-}" ]; then
    "$REPO_ROOT/contrib/release/macos/check-profile-authorises-cert.sh" \
        "$PROFILE" --p12 "$P12_FILE" "$P12_PASSWORD_FILE"
fi

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

# Read the certificate back OUT of the finished signature and check the profile
# authorises it. The pre-sign check above tests what we meant to sign with; this
# tests what actually landed, which is the thing AMFI will judge.
"$REPO_ROOT/contrib/release/macos/check-profile-authorises-cert.sh" \
    "$PROFILE" --signed-bundle "$BUNDLE"

# The profile has to survive signing and be sealed by it, or AMFI never sees it.
test -f "$BUNDLE/Contents/embedded.provisionprofile" \
    || { echo "the signer dropped Contents/embedded.provisionprofile" >&2; exit 1; }
# CodeResources is a plist (often binary), so grepping the raw bytes can miss a
# real seal or match for the wrong reason. Parse it and require the profile to be
# sealed under files2 (modern) or files (legacy). PlistBuddy splits key paths on
# ':', so the dotted key name is matched literally.
_cr="$BUNDLE/Contents/_CodeSignature/CodeResources"
/usr/libexec/PlistBuddy -c 'Print :files2:embedded.provisionprofile' "$_cr" >/dev/null 2>&1 \
    || /usr/libexec/PlistBuddy -c 'Print :files:embedded.provisionprofile' "$_cr" >/dev/null 2>&1 \
    || { echo "the profile is present but not sealed into CodeResources" >&2; exit 1; }

echo "==> Running the ignored device_secret tests"
# --test-threads=1: these tests share one keychain service name and assert on
# provisioning races; running them concurrently would have them fight.
#
# A SIGKILL here (137) is AMFI refusing the signature at exec, and it arrives
# with no output at all — which reads as a hung or crashed test rather than a
# signing problem. The only evidence is in the unified log, so fetch it rather
# than leaving the next person to rediscover the predicate.
STARTED_AT=$(date '+%Y-%m-%d %H:%M:%S')
set +e
"$BUNDLE/Contents/MacOS/TestRunner" device_secret --ignored --test-threads=1
STATUS=$?
set -e
if [ "$STATUS" -eq 137 ]; then
    echo >&2
    echo "==> Killed at exec (137). This is AMFI rejecting the signature," >&2
    echo "    not a test failure." >&2
    echo >&2
    echo "    Bundle state at the moment it was refused:" >&2
    codesign -dvvv --entitlements - "$BUNDLE" 2>&1 | sed 's/^/      /' >&2 || true
    echo >&2
    echo "    Unified log since $STARTED_AT:" >&2
    log show --start "$STARTED_AT" --info --predicate \
        'eventMessage CONTAINS[c] "coincube" OR eventMessage CONTAINS[c] "provisioning profile" OR eventMessage CONTAINS[c] "AMFI"' \
        2>/dev/null | grep -iE "amfi|provisioning|taskgated|coincube" | tail -20 >&2 \
        || echo "    (no matching log entries — run: log show --last 5m --info)" >&2
    echo >&2
    echo "    See docs/MACOS_KEYCHAIN_ENTITLEMENT.md section 4." >&2
fi
exit "$STATUS"
