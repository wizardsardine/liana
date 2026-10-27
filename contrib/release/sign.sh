#!/usr/bin/env sh

# ==============================================================================
# Script for signing the release assets.
#
# Usage: ./contrib/release/sign.sh <target> <gpg|rcodesign|osslsigncode> [args...]
#   target: "liana" or "liana-business"
#
# Examples:
#   ./contrib/release/sign.sh liana gpg
#   ./contrib/release/sign.sh liana-business rcodesign <cert_path> <key_path> <apikey_json_path>
#   ./contrib/release/sign.sh liana osslsigncode
#   ./contrib/release/sign.sh liana osslsigncode 'pkcs11:token=<label>;object=<key_label>;type=private'
#   ./contrib/release/sign.sh liana osslsigncode <cert_path> <key_path>
# ==============================================================================

set -e  # Exit immediately if a command exits with a non-zero status
set -x  # Print commands and their arguments as they are executed

if [ $# -lt 2 ]; then
    echo "Usage: $0 <target> <gpg|rcodesign|osslsigncode> [args...]"
    echo "  target: 'liana' or 'liana-business'"
    exit 1
fi

TARGET="$1"
shift 1

if [ "$TARGET" != "liana" ] && [ "$TARGET" != "liana-business" ]; then
    echo "Error: target must be 'liana' or 'liana-business'"
    exit 1
fi

# The signing and disk image tools (rcodesign, osslsigncode, xorrisofs, dmg) come
# from the release dev shell. Re-enter it through Nix rather than require them on the host.
if [ -z "${IN_LIANA_RELEASE_SHELL:-}" ]; then
    export IN_LIANA_RELEASE_SHELL=1
    exec nix develop "$PWD#release" --command "$0" "$TARGET" "$@"
fi

# Auto-detect version from git:
# - If HEAD is exactly at a matching tag, use the tag version (e.g., v13.1 -> 13.1)
# - Otherwise, use the latest matching tag (by version sort) combined with the short
#   commit hash (e.g., v13.1 + 6601d205 -> 13.1-6601d205). This works even if the tag
#   is not an ancestor of HEAD (e.g., on feature branches).
# - If no matching tag exists, use 0-<hash> (e.g., 0-6601d205)
# Tags are filtered by target: "v*" for liana, "business-v*" for liana-business
if [ "$TARGET" = "liana" ]; then
    TAG_PATTERN="v[0-9]*"
    TAG_PREFIX="v"
else
    TAG_PATTERN="business-v*"
    TAG_PREFIX="business-v"
fi
if git describe --tags --exact-match --match "$TAG_PATTERN" >/dev/null 2>&1; then
    VERSION="$(git describe --tags --exact-match --match "$TAG_PATTERN" | sed "s/^${TAG_PREFIX}//")"
else
    # Find latest matching tag by version sorting (works even if tag is not an ancestor of HEAD)
    LATEST_TAG="$(git tag --sort=-v:refname --list "$TAG_PATTERN" | head -1)"
    if [ -n "$LATEST_TAG" ]; then
        VERSION="$(echo "$LATEST_TAG" | sed "s/^${TAG_PREFIX}//")-$(git rev-parse --short=8 HEAD)"
    else
        VERSION="0-$(git rev-parse --short=8 HEAD)"
    fi
fi

LIANA_PREFIX="$TARGET-$VERSION"
# Define the release directory
RELEASE_DIR="$PWD/release_assets"
RELEASE_BUILD_DIR="$PWD/release_build"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Function to perform GPG signing
sign_with_gpg() {
    (
        cd "$RELEASE_DIR"
        gpg --detach-sign --armor "$LIANA_PREFIX-shasums.txt"
    )
}

# Function to convert a path to an absolute path
absolute_path() {
    local path="$1"
    if [[ "$path" = /* ]]; then
        echo "$path"
    else
        echo "$PWD/$path"
    fi
}

# Determine the reference time used for determinism (overridable by environment)
export SOURCE_DATE_EPOCH="$(git -c log.showsignature=false log --format=%at -1)"
export TZ=UTC

# Function to perform rcodesign signing
sign_with_rcodesign() {
    # Ensure the correct number of arguments are provided
    if [ "$#" -ne 3 ]; then
        echo "Usage: $0 <target> <version> rcodesign <cert_path> <key_path> <apikey_json_path>"
        exit 1
    fi

    # Assign arguments to variables
    CODESIGN_CERT="$(absolute_path $1)"
    CODESIGN_KEY="$(absolute_path $2)"
    NOTARY_API_CREDS_FILE="$(absolute_path $3)"

    # Verify that the provided files exist
    if [ ! -f "$CODESIGN_CERT" ]; then
        echo "Certificate file not found: $CODESIGN_CERT"
        exit 1
    fi

    if [ ! -f "$CODESIGN_KEY" ]; then
        echo "Key file not found: $CODESIGN_KEY"
        exit 1
    fi

    if [ ! -f "$NOTARY_API_CREDS_FILE" ]; then
        echo "API credentials file not found: $NOTARY_API_CREDS_FILE"
        exit 1
    fi

    cd "$RELEASE_BUILD_DIR"
    if [ "$TARGET" = "liana" ]; then
        APP_BUNDLE="Liana.app"
        chmod u+w "./$APP_BUNDLE/Contents/MacOS/Liana"
    else
        APP_BUNDLE="LianaBusiness.app"
        chmod u+w "./$APP_BUNDLE/Contents/MacOS/LianaBusiness"
    fi

    rcodesign sign \
        --digest sha256 \
        --code-signature-flags runtime \
        --pem-source "$CODESIGN_KEY" \
        --der-source "$CODESIGN_CERT" \
        "$APP_BUNDLE/"

    rcodesign notary-submit \
        --max-wait-seconds 600 \
        --api-key-path "$NOTARY_API_CREDS_FILE" \
        --staple "$APP_BUNDLE"

    # Disk image: built from the signed+stapled bundle, then signed and
    # notarized itself so Gatekeeper accepts both the .dmg and the app.
    DMG="$LIANA_PREFIX-macos.dmg"
    "$SCRIPT_DIR/macos/create-dmg.sh" "$APP_BUNDLE" "$DMG" "${APP_BUNDLE%.app} $VERSION"
    rcodesign sign \
        --digest sha256 \
        --pem-source "$CODESIGN_KEY" \
        --der-source "$CODESIGN_CERT" \
        "$DMG"
    rcodesign notary-submit \
        --max-wait-seconds 600 \
        --api-key-path "$NOTARY_API_CREDS_FILE" \
        --staple "$DMG"
    mv "$DMG" "$RELEASE_DIR/"
    sha256sum "$RELEASE_DIR/$DMG" | sed "s|$RELEASE_DIR/||" | tee -a "$RELEASE_DIR/$LIANA_PREFIX-shasums.txt"
}

# Decode one percent-encoded attribute of a PKCS#11 URI (RFC 7512).
pkcs11_uri_attr() {
    local value
    value="$(printf '%s' "$1" | sed 's/^pkcs11://' | tr ';' '\n' | sed -n "s/^$2=//p" | head -1)"
    printf '%b' "$(printf '%s' "$value" | sed 's/%/\\x/g')"
}

# Print the URI of the key matching the only certificate on the card.
card_key_uri() {
    local uris count
    uris="$(pkcs11-tool --module "$PKCS11_MODULE" -O \
        | awk '/^Certificate Object/{c=1; next} /^[^ ]/{c=0} c && /^ *uri:/{print $2}')"
    count="$(printf '%s\n' "$uris" | grep -c .)" || true
    if [ "$count" -ne 1 ]; then
        echo "Expected exactly one certificate on the card, found $count:" >&2
        printf '%s\n' "$uris" >&2
        echo "Pass the key URI explicitly: $0 <target> osslsigncode <pkcs11_uri>" >&2
        exit 1
    fi
    printf '%s\n' "$uris" | sed 's/;type=cert$/;type=private/'
}

# Read the certificate matching a key URI from the card into <dir>/leaf.pem, fetch
# its issuing CA into <dir>/ca.pem and write <dir>/chain.pem.
read_card_certificate_chain() {
    local key_uri="$1" dir="$2" token id label ca_uri
    token="$(pkcs11_uri_attr "$key_uri" token)"
    id="$(printf '%s' "$key_uri" | tr ';' '\n' | sed -n 's/^id=//p' | tr -d %)"
    label="$(pkcs11_uri_attr "$key_uri" object)"

    local selector=()
    [ -n "$token" ] && selector+=(--token-label "$token")
    if [ -n "$id" ]; then
        selector+=(--id "$id")
    elif [ -n "$label" ]; then
        selector+=(--label "$label")
    else
        echo "PKCS#11 URI needs an id or object attribute to find the certificate: $key_uri"
        exit 1
    fi

    pkcs11-tool --module "$PKCS11_MODULE" "${selector[@]}" --read-object --type cert -o "$dir/leaf.der"
    openssl x509 -inform der -in "$dir/leaf.der" -out "$dir/leaf.pem"
    openssl x509 -in "$dir/leaf.pem" -noout -subject -issuer -enddate

    ca_uri="$(openssl x509 -in "$dir/leaf.pem" -noout -ext authorityInfoAccess | sed -n 's/.*CA Issuers - URI://p' | head -1)"
    if [ -z "$ca_uri" ]; then
        echo "Certificate has no CA Issuers URL, cannot build the chain"
        exit 1
    fi
    curl -fsSL -o "$dir/ca.bin" "$ca_uri"
    openssl x509 -inform der -in "$dir/ca.bin" -out "$dir/ca.pem" 2>/dev/null \
        || openssl x509 -in "$dir/ca.bin" -out "$dir/ca.pem"
    openssl x509 -in "$dir/ca.pem" -noout -subject
    cat "$dir/leaf.pem" "$dir/ca.pem" > "$dir/chain.pem"
}

# Function to sign the Windows installer with osslsigncode and extract the
# detached signature (see contrib/release/windows/README.md).
# Without arguments the key is the one on the smart card, found from its only
# certificate; a PKCS#11 URI selects it explicitly instead. In both cases the
# certificate is read from the card and its issuing CA fetched from the
# certificate's CA Issuers URL. The card is reached through the module in
# PKCS11_MODULE (default: Certum common profile, built from the flake).
# Alternatively <cert_path> <key_path> as PEM files.
sign_with_osslsigncode() {
    TIMESTAMP_SERVER="${TIMESTAMP_SERVER:-http://timestamp.digicert.com}"

    case "$#:${1:-}" in
        0:|1:pkcs11:*)
            if [ -z "${PKCS11_MODULE:-}" ]; then
                PKCS11_MODULE="$(nix build --no-link --print-out-paths "$PWD#certum-pkcs11")/lib/libcrypto3PKCS.so"
            fi
            if [ ! -f "$PKCS11_MODULE" ]; then
                echo "PKCS#11 module not found: $PKCS11_MODULE"
                exit 1
            fi
            export PKCS11_PROVIDER_MODULE="$PKCS11_MODULE"

            if [ "$#" -eq 1 ]; then
                KEY_URI="$1"
            else
                KEY_URI="$(card_key_uri)"
            fi

            CERT_DIR="$(mktemp -d)"
            trap 'rm -rf "$CERT_DIR"' EXIT
            read_card_certificate_chain "$KEY_URI" "$CERT_DIR"
            CODESIGN_CERT="$CERT_DIR/chain.pem"
            KEY_ARGS=(-provider pkcs11 -pkcs11module "$PKCS11_MODULE" -key "$KEY_URI" -askpass)
            ;;
        2:*)
            CODESIGN_CERT="$(absolute_path $1)"
            CODESIGN_KEY="$(absolute_path $2)"
            if [ ! -f "$CODESIGN_CERT" ]; then
                echo "Certificate file not found: $CODESIGN_CERT"
                exit 1
            fi
            if [ ! -f "$CODESIGN_KEY" ]; then
                echo "Key file not found: $CODESIGN_KEY"
                exit 1
            fi
            KEY_ARGS=(-key "$CODESIGN_KEY")
            ;;
        *)
            echo "Usage: $0 <target> osslsigncode [<pkcs11_uri> | <cert_path> <key_path>]"
            exit 1
            ;;
    esac

    if [ "$TARGET" = "liana" ]; then
        PRODUCT_NAME="Liana"
    else
        PRODUCT_NAME="Liana Business"
    fi

    UNSIGNED_SETUP="$RELEASE_DIR/$LIANA_PREFIX-setup-noncodesigned.exe"
    SIGNED_SETUP="$RELEASE_DIR/$LIANA_PREFIX-setup.exe"
    DETACHED_SIG="$RELEASE_DIR/$LIANA_PREFIX-setup.exe.pem"

    if [ ! -f "$UNSIGNED_SETUP" ]; then
        echo "Installer not found: $UNSIGNED_SETUP (run release.sh first)"
        exit 1
    fi

    # osslsigncode refuses to overwrite its outputs.
    rm -f "$SIGNED_SETUP" "$DETACHED_SIG"

    osslsigncode sign \
        -certs "$CODESIGN_CERT" \
        "${KEY_ARGS[@]}" \
        -n "$PRODUCT_NAME" \
        -i "https://wizardsardine.com/liana/" \
        -h sha256 \
        -ts "$TIMESTAMP_SERVER" \
        -in "$UNSIGNED_SETUP" \
        -out "$SIGNED_SETUP"

    osslsigncode extract-signature -pem -in "$SIGNED_SETUP" -out "$DETACHED_SIG"

    # The published signature must reproduce the published signed installer.
    REATTACHED_DIR="$(mktemp -d)"
    "$SCRIPT_DIR/windows/attach-signature.sh" "$UNSIGNED_SETUP" "$DETACHED_SIG" "$REATTACHED_DIR/reattached.exe"
    cmp "$REATTACHED_DIR/reattached.exe" "$SIGNED_SETUP"
    rm -rf "$REATTACHED_DIR"

    sha256sum "$SIGNED_SETUP" "$DETACHED_SIG" | sed "s|$RELEASE_DIR/||" | tee -a "$RELEASE_DIR/$LIANA_PREFIX-shasums.txt"
}

COMMAND="$1"
shift  # Shift the arguments to access any additional parameters

case "$COMMAND" in
    gpg)
        sign_with_gpg
        ;;
    rcodesign)
        sign_with_rcodesign "$@"
        ;;
    osslsigncode)
        sign_with_osslsigncode "$@"
        ;;
    *)
        echo "Invalid command: $COMMAND"
        echo "Usage: $0 <target> <gpg|rcodesign|osslsigncode> [args...]"
        exit 1
        ;;
esac

# Disable debugging and exit on success
set +ex
