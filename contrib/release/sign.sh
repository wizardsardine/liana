#!/usr/bin/env sh

set -e  # Exit immediately if a command exits with a non-zero status
set -x  # Print commands and their arguments as they are executed

VERSION="${VERSION:-"11.0"}"
LIANA_PREFIX="liana-$VERSION"
# Define the release directory
RELEASE_DIR="$PWD/release_assets"
RELEASE_BUILD_DIR="$PWD/release_build"

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

zip_archive () {
    local archive="$1"
    shift
    touch -d "@$SOURCE_DATE_EPOCH" "$@"
    find "$@" -type f -exec touch -d "@$SOURCE_DATE_EPOCH" {} +
    zip -r -oX - "$@" > "$archive"
}



# Function to perform rcodesign signing
sign_with_rcodesign() {
    # Ensure the correct number of arguments are provided
    if [ "$#" -ne 3 ]; then
        echo "Usage: $0 rcodesign <cert_path> <key_path> <apikey_json_path>"
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
    chmod u+w ./Liana.app/Contents/MacOS/Liana

    rcodesign sign \
        --digest sha256 \
        --code-signature-flags runtime \
        --pem-source "$CODESIGN_KEY" \
        --der-source "$CODESIGN_CERT" \
        Liana.app/

    rcodesign notary-submit \
        --max-wait-seconds 600 \
        --api-key-path "$NOTARY_API_CREDS_FILE" \
        --staple Liana.app

    zip_archive "$LIANA_PREFIX-macos.zip" Liana.app
    mv "$LIANA_PREFIX-macos.zip" "$RELEASE_DIR/"
    sha256sum "$RELEASE_DIR/$LIANA_PREFIX-macos.zip" | sed "s|$RELEASE_DIR/||" | tee -a "$RELEASE_DIR/$LIANA_PREFIX-shasums.txt"
}

if [ "$#" -lt 1 ]; then
    echo "Usage: $0 <gpg|rcodesign> [args...]"
    exit 1
fi

COMMAND="$1"
shift  # Shift the arguments to access any additional parameters

case "$COMMAND" in
    gpg)
        sign_with_gpg
        ;;
    rcodesign)
        sign_with_rcodesign "$@"
        ;;
    *)
        echo "Invalid command: $COMMAND"
        echo "Usage: $0 <gpg|rcodesign> [args...]"
        exit 1
        ;;
esac

# Disable debugging and exit on success
set +ex
