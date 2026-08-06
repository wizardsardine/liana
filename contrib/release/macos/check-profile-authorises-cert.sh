#!/usr/bin/env bash
#
# Check that the embedded provisioning profile authorises the certificate we
# are about to sign with.
#
# # Why this is its own check
#
# A provisioning profile carries a `DeveloperCertificates` array, and AMFI only
# accepts the profile if the certificate that produced the signature is in it.
# Ours lists exactly one, so signing with any *other* Developer ID Application
# certificate for the same team — a rotated one, a second one issued to another
# machine, an older p12 still sitting in a CI secret — produces a bundle that
# is refused at exec:
#
#     taskgated-helper: Disallowing io.coincube.tenshu because no eligible
#                       provisioning profiles found
#     amfid: not valid: ... "No matching profile found"
#     kernel: AMFI: Code has restricted entitlements, but the validation of
#             its code signature failed.
#
# The certificate's *common name* is identical across every certificate a team
# holds, so nothing in a signing log distinguishes them — the failing run and a
# working run print the same "creating cryptographic signature with certificate
# Developer ID Application: COINCUBE TECHNOLOGY LLC (8UVR249AD5)" line.
#
# Every existing gate passes with the wrong certificate: `codesign -v` is happy,
# `spctl` passes, notarization succeeds, and the pipeline's profile check only
# asserts the profile is present, unexpired and grants the right keychain group.
# The mismatch surfaces only as SIGKILL at launch — for users, or for the signed
# device_secret tests, which is how it was found. See
# docs/MACOS_KEYCHAIN_ENTITLEMENT.md §4.
#
# # Usage
#
#   # A p12 — how CI signs. `cert.p12`/`cert.pass` exist only INSIDE a CI run
#   # (decoded there from CERTIFICATE_P12_BASE64); pass a real path locally.
#   # Omit the password file and it prompts, which is the safer way by hand:
#   # a password containing `!` breaks under zsh history expansion, and the
#   # redirection truncates the file before the shell errors, leaving an empty
#   # password file and an opaque "Error reading password from BIO".
#   check-profile-authorises-cert.sh <profile> --p12 <file> [password-file]
#
#   # A keychain identity — how you sign locally.
#   check-profile-authorises-cert.sh <profile> --identity "<common name>"
#
#   # An ALREADY-SIGNED bundle — needs no certificate and no secret, so this is
#   # the one to reach for when diagnosing a CI failure: download the signed app
#   # from the run's artifacts and ask which certificate actually signed it.
#   check-profile-authorises-cert.sh <profile> --signed-bundle <path-to-.app>
#
# Exits 0 when the certificate is authorised, 1 with a diagnosis when it is not.

set -euo pipefail

usage() {
    cat >&2 <<USAGE
usage: $0 <profile> MODE

  --p12 <file> [password-file]   the certificate CI signs with. Omit the
                                 password file to be prompted (safer by hand —
                                 nothing reaches your shell history). Note that
                                 cert.p12/cert.pass are created inside a CI run
                                 from CERTIFICATE_P12_BASE64 and do not exist in
                                 a clean checkout — pass a real path.
  --identity "<common name>"     a certificate in your login keychain.
  --signed-bundle <path-to-.app> read the certificate back out of an existing
                                 signature. Needs no secret, so this is the way
                                 to diagnose a CI failure from its artifact.
USAGE
    exit 2
}

PROFILE="${1:-}"
MODE="${2:-}"
[ -n "$PROFILE" ] || usage
[ -f "$PROFILE" ] || { echo "no such profile: $PROFILE" >&2; exit 2; }

WORK="$(mktemp -d)"
trap 'stty echo 2>/dev/null || true; rm -rf "$WORK"' EXIT INT TERM

# The certificate we would sign with, as PEM.
case "$MODE" in
    --p12)
        P12="${3:-}"
        P12_PASS="${4:-}"
        [ -n "$P12" ] || usage

        # No password file, or one that exists but is empty: prompt. The empty
        # case is worth catching by name — `>` truncates before the shell runs
        # the command, so a quoting mistake leaves a 0-byte file and openssl
        # reports only "Error reading password from BIO".
        if [ -z "$P12_PASS" ] || { [ -f "$P12_PASS" ] && [ ! -s "$P12_PASS" ]; }; then
            if [ -n "$P12_PASS" ]; then
                echo "note: $P12_PASS is empty — prompting instead." >&2
            fi
            printf 'password for %s: ' "$(basename "$P12")" >&2
            stty -echo 2>/dev/null || true
            IFS= read -r ENTERED_PASS
            stty echo 2>/dev/null || true
            printf '\n' >&2
            [ -n "$ENTERED_PASS" ] || { echo "no password entered" >&2; exit 2; }
            P12_PASS="$WORK/pass"
            PROMPTED=1
            ( umask 077; printf %s "$ENTERED_PASS" > "$P12_PASS" )
            unset ENTERED_PASS
        fi

        for f in "$P12" "$P12_PASS"; do
            [ -f "$f" ] || {
                echo "no such file: $f" >&2
                echo >&2
                echo "cert.p12 and cert.pass are written by the 'Prepare Apple Developer ID" >&2
                echo "certificate' step from repository secrets and exist only during a CI" >&2
                echo "run. To check the same thing locally, either point --p12 at a real" >&2
                echo "copy of the certificate, or use one of:" >&2
                echo >&2
                echo "  $0 $PROFILE --identity \"Developer ID Application: COINCUBE TECHNOLOGY LLC\"" >&2
                echo "  $0 $PROFILE --signed-bundle /path/to/Tenshu.app" >&2
                exit 2
            }
        done
        # Keychain Access still writes 40-bit RC2 for the certificate bag and
        # 3DES for the key. OpenSSL 3 refuses both unless the legacy provider is
        # loaded; LibreSSL (always at /usr/bin/openssl on macOS) has no provider
        # concept and reads them natively, and it prints "MAC verified OK",
        # which doubles as a password check. Try the readers in that order.
        #
        # Every failure is captured and reported: an earlier revision sent these
        # to /dev/null and the caller got "could not read the certificate" with
        # no way to tell a wrong password from an unsupported cipher.
        extracted=""
        attempts=""
        # "<binary>|<flags after the pkcs12 subcommand>" — -legacy is a pkcs12
        # flag, not a global one, so it has to go after the subcommand.
        for reader in "/usr/bin/openssl|" "openssl|-legacy" "openssl|"; do
            bin="${reader%%|*}"
            flag="${reader#*|}"
            # shellcheck disable=SC2086 # $flag is deliberately unquoted/optional
            if err=$("$bin" pkcs12 $flag -in "$P12" -passin "file:$P12_PASS" \
                        -nokeys -clcerts -out "$WORK/signer.pem" 2>&1); then
                if [ -s "$WORK/signer.pem" ]; then
                    extracted="$bin"
                    break
                fi
                err="succeeded but produced no certificate — does this p12 contain one?"
            fi
            attempts="$attempts
  [$bin ${flag:-(no flags)}] $err"
        done
        if [ -z "$extracted" ]; then
            echo "could not read a certificate out of $P12" >&2
            echo "$attempts" >&2
            echo >&2
            case "$attempts" in
                *"MAC verify"*|*"invalid password"*|*"mac verify failure"*)
                    if [ "${PROMPTED:-0}" = 1 ]; then
                        echo "That is the WRONG PASSWORD — it is not the one this p12 was" >&2
                        echo "exported with. Re-run and type it again." >&2
                    else
                        echo "That looks like a WRONG PASSWORD. $P12_PASS must hold exactly the" >&2
                        echo "password you set when exporting, with no trailing newline:" >&2
                        echo "    printf %s 'the-password' > $P12_PASS" >&2
                        echo "Or omit the password file entirely and be prompted instead." >&2
                    fi
                    ;;
                *"no certificate"*)
                    echo "The file has a private key but no certificate. In Keychain Access," >&2
                    echo "export the CERTIFICATE (which carries its key beneath it), not the key." >&2
                    ;;
            esac
            exit 2
        fi
        ;;
    --identity)
        IDENT="${3:-}"
        [ -n "$IDENT" ] || usage
        # A common name is NOT a safe selector: rotated Developer ID certificates
        # share one, and `find-certificate -c` returns whichever it hits first, so
        # it can resolve to a certificate that is not the one signing the build —
        # the exact mismatch this script exists to catch. Resolve the argument
        # against the *eligible code-signing identities* and refuse an ambiguous
        # common name; a 40-hex SHA-1 is accepted directly because it is unique.
        python3 - "$IDENT" "$WORK/signer.pem" <<'IDENT_PY'
import re, subprocess, sys

ident = sys.argv[1].strip()
out = sys.argv[2]

listing = subprocess.run(
    ["security", "find-identity", "-v", "-p", "codesigning"],
    capture_output=True, text=True,
).stdout
# lines look like:  1) <40-hex SHA-1>  "<name>"
ids = re.findall(r'^\s*\d+\)\s+([0-9A-Fa-f]{40})\s+"(.*)"\s*$', listing, re.M)

if re.fullmatch(r'[0-9A-Fa-f]{40}', ident):
    matches = [(h, n) for h, n in ids if h.lower() == ident.lower()]
else:
    matches = [(h, n) for h, n in ids if n == ident]

if not matches:
    print(f"no eligible code-signing identity matches '{ident}'", file=sys.stderr)
    if ids:
        print("known code-signing identities:", file=sys.stderr)
        for h, n in ids:
            print(f"  {h}  {n}", file=sys.stderr)
    raise SystemExit(2)
if len(matches) > 1:
    print(f"'{ident}' matches {len(matches)} code-signing identities — ambiguous.", file=sys.stderr)
    print("Re-run --identity with the exact SHA-1 of the one you mean:", file=sys.stderr)
    for h, n in matches:
        print(f"  {h}  {n}", file=sys.stderr)
    raise SystemExit(2)

sha1, name = matches[0]

# Export THAT certificate by SHA-1 (find-certificate -c would re-introduce the
# name ambiguity), walking the -Z listing which prefixes each PEM with its hash.
dump = subprocess.run(
    ["security", "find-certificate", "-a", "-Z", "-p"],
    capture_output=True, text=True,
).stdout
blocks, cur, buf = [], None, []
for line in dump.splitlines():
    m = re.match(r'SHA-1 hash:\s*([0-9A-Fa-f]{40})', line)
    if m:
        cur, buf = m.group(1), []
        continue
    if line.startswith("-----BEGIN CERTIFICATE-----"):
        buf = [line]
    elif line.startswith("-----END CERTIFICATE-----"):
        buf.append(line)
        if cur:
            blocks.append((cur, "\n".join(buf)))
        buf = []
    elif buf:
        buf.append(line)

chosen = [b for h, b in blocks if h.lower() == sha1.lower()]
if not chosen:
    print(f"resolved identity {sha1} but could not export its certificate", file=sys.stderr)
    raise SystemExit(2)
with open(out, "w") as fh:
    fh.write(chosen[0] + "\n")
print(f"selected code-signing identity {sha1}  {name}", file=sys.stderr)
IDENT_PY
        [ -s "$WORK/signer.pem" ] || exit 2
        ;;
    --signed-bundle)
        BUNDLE_PATH="${3:-}"
        [ -n "$BUNDLE_PATH" ] || usage
        [ -e "$BUNDLE_PATH" ] || { echo "no such bundle: $BUNDLE_PATH" >&2; exit 2; }
        # `--extract-certificates` writes the chain as DER, leaf first, using its
        # argument as a filename PREFIX — which may include a directory. Writing
        # straight into $WORK avoids cd'ing, which would break a RELATIVE bundle
        # path like the `Tenshu.app` the workflows pass.
        if ! err=$(codesign -d --extract-certificates="$WORK/chain" "$BUNDLE_PATH" 2>&1); then
            echo "could not read a signature from $BUNDLE_PATH" >&2
            echo "  $err" >&2
            exit 2
        fi
        [ -f "$WORK/chain0" ] || { echo "$BUNDLE_PATH carries no certificate chain (ad-hoc signed?)" >&2; exit 2; }
        openssl x509 -inform DER -in "$WORK/chain0" -out "$WORK/signer.pem" 2>/dev/null \
            || { echo "could not parse the leaf certificate from $BUNDLE_PATH" >&2; exit 2; }
        # The SIGKILL-at-launch failure hinges on the profile EMBEDDED in the
        # bundle, not on whatever --profile the caller passed. If they differ,
        # authorising $PROFILE would be a false green while the shipped bundle
        # still crashes. Require the embedded profile and assert (by UUID) that
        # it is the one we are about to check. (codesign --verify --strict is
        # intentionally NOT re-run here — the release/nightly workflows already
        # gate on codesign/spctl/notarization; this script's job is the profile
        # <-> certificate authorisation the other gates cannot see.)
        EMBEDDED=""
        for cand in \
            "$BUNDLE_PATH/Contents/embedded.provisionprofile" \
            "$BUNDLE_PATH/embedded.provisionprofile"; do
            [ -f "$cand" ] && { EMBEDDED="$cand"; break; }
        done
        [ -n "$EMBEDDED" ] || {
            echo "$BUNDLE_PATH embeds no provisioning profile" >&2
            echo "  (expected Contents/embedded.provisionprofile) — an unprovisioned" >&2
            echo "  Developer ID bundle is SIGKILLed at launch by AMFI." >&2
            exit 2
        }
        _uuid() { security cms -D -i "$1" 2>/dev/null | plutil -extract UUID raw - 2>/dev/null; }
        EMB_UUID=$(_uuid "$EMBEDDED"); PROF_UUID=$(_uuid "$PROFILE")
        if [ -z "$EMB_UUID" ] || [ -z "$PROF_UUID" ] || [ "$EMB_UUID" != "$PROF_UUID" ]; then
            echo "the bundle's embedded profile is not the one being checked" >&2
            echo "  embedded : ${EMB_UUID:-<undecodable>} ($EMBEDDED)" >&2
            echo "  --profile: ${PROF_UUID:-<undecodable>} ($PROFILE)" >&2
            echo "Pass the embedded profile as --profile, or re-sign the bundle with it." >&2
            exit 2
        fi
        ;;
    *)
        usage
        ;;
esac

SIGNER_SHA=$(openssl x509 -in "$WORK/signer.pem" -outform DER \
    | shasum -a 256 | cut -d' ' -f1)

# The certificates the profile authorises.
security cms -D -i "$PROFILE" > "$WORK/profile.plist" 2>/dev/null \
    || { echo "could not decode $PROFILE" >&2; exit 2; }

python3 - "$WORK/profile.plist" "$SIGNER_SHA" <<'PY'
import hashlib
import plistlib
import subprocess
import sys

with open(sys.argv[1], "rb") as fh:
    profile = plistlib.load(fh)
signer_sha = sys.argv[2]

certs = profile.get("DeveloperCertificates", [])
authorised = []
for der in certs:
    der = bytes(der)
    sha = hashlib.sha256(der).hexdigest()
    subject = subprocess.run(
        ["openssl", "x509", "-inform", "DER", "-noout", "-subject"],
        input=der, capture_output=True,
    ).stdout.decode().strip()
    authorised.append((sha, subject))

if any(sha == signer_sha for sha, _ in authorised):
    print(f"signing certificate {signer_sha[:16]}… is authorised by the profile")
    raise SystemExit(0)

print("the provisioning profile does NOT authorise the signing certificate", file=sys.stderr)
print("", file=sys.stderr)
print(f"  signing with : {signer_sha}", file=sys.stderr)
print(f"  profile lists: {len(authorised)} certificate(s)", file=sys.stderr)
for sha, subject in authorised:
    print(f"    {sha}", file=sys.stderr)
    print(f"      {subject}", file=sys.stderr)
print("", file=sys.stderr)
print(
    "Every other gate passes with a mismatch — codesign, spctl and notarization\n"
    "all succeed, and the common names are identical, so the signing log cannot\n"
    "tell them apart. The bundle is SIGKILLed at exec by AMFI instead.\n"
    "\n"
    "Fix: regenerate the Developer ID profile in the portal for the certificate\n"
    "that is actually in use (Profiles -> Distribution -> Developer ID), commit it\n"
    "to contrib/release/macos/embedded.provisionprofile, or point the signing\n"
    "secret back at the certificate the current profile lists.\n"
    "See docs/MACOS_KEYCHAIN_ENTITLEMENT.md section 4 and docs/APPLE_CERT_ROTATION.md.",
    file=sys.stderr,
)
raise SystemExit(1)
PY
