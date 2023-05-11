#!/usr/bin/env sh

# ==============================================================================
# Script for creating and signing the release assets. To be ran from the root of
# the repository.
# ==============================================================================

set -ex

VERSION="${VERSION:-"0.4"}"
LIANA_PREFIX="liana-$VERSION"
LINUX_DIR_NAME="$LIANA_PREFIX-x86_64-linux-gnu"
LINUX_ARCHIVE="$LINUX_DIR_NAME.tar.gz"
WINDOWS_DIR_NAME="$LIANA_PREFIX-x86_64-windows-gnu"
WINDOWS_ARCHIVE="$WINDOWS_DIR_NAME.zip"
MAC_DIR_NAME="$LIANA_PREFIX-x86_64-apple-darwin"
MAC_ARCHIVE="$MAC_DIR_NAME.tar.gz"
MAC_CODESIGN="${MAC_CODESIGN:-"0"}"
RCODESIGN_BIN="${RCODESIGN_BIN:-"$PWD/../../macos_codesigning/apple-codesign-0.22.0-x86_64-unknown-linux-musl/rcodesign"}"
CODESIGN_KEY="${CODESIGN_KEY:-"$PWD/../../macos_codesigning/wizardsardine_liana.key"}"
CODESIGN_CERT="${CODESIGN_CERT:-"$PWD/../../macos_codesigning/antoine_devid_liana_codesigning.cer"}"
NOTARY_API_CREDS_FILE="${NOTARY_API_CREDS_FILE:-"$PWD/../../macos_codesigning/encoded_appstore_api_key.json"}"

create_dir() {
    test -d "$1" || mkdir "$1"
}

# We'll use a folder for the builds output and another one for the final assets.
RELEASE_DIR="$PWD/release_assets"
BUILD_DIR="$PWD/release_build"
create_dir "$RELEASE_DIR"
create_dir "$BUILD_DIR"

OUT_DIR="$BUILD_DIR" ./contrib/reproducible/guix/guix-build.sh
TARGET_DIR="$BUILD_DIR" ./contrib/reproducible/docker/docker-build.sh

# Create the Linux archive
(
    cd "$BUILD_DIR"
    create_dir "$LINUX_DIR_NAME"
    cp "$BUILD_DIR/release/lianad" "$BUILD_DIR/release/liana-cli" "$BUILD_DIR/gui/release/liana-gui" ../README.md "$LINUX_DIR_NAME"
    tar -czf "$LINUX_ARCHIVE" "$LINUX_DIR_NAME"
    cp "$LINUX_ARCHIVE" "$RELEASE_DIR"
)

# Create the Windows archive and the raw executable
(
    cd "$BUILD_DIR"
    create_dir "$WINDOWS_DIR_NAME"
    cp "$BUILD_DIR/gui/x86_64-pc-windows-gnu/release/liana-gui.exe" ../README.md "$WINDOWS_DIR_NAME"
    zip -r "$WINDOWS_ARCHIVE" "$WINDOWS_DIR_NAME"
    cp "$WINDOWS_ARCHIVE" "$RELEASE_DIR"
    cp "$BUILD_DIR/gui/x86_64-pc-windows-gnu/release/liana-gui.exe" "$RELEASE_DIR/$LIANA_PREFIX.exe"
)

# Create the MacOS archive and a zipped application bundle of liana-gui.
(
    cd "$BUILD_DIR"
    create_dir "$MAC_DIR_NAME"
    cp "$BUILD_DIR/x86_64-apple-darwin/release/lianad" "$BUILD_DIR/x86_64-apple-darwin/release/liana-cli" "$BUILD_DIR/gui/x86_64-apple-darwin/release/liana-gui" ../README.md "$MAC_DIR_NAME"
    tar -czf "$MAC_ARCHIVE" "$MAC_DIR_NAME"
    cp "$MAC_ARCHIVE" "$RELEASE_DIR"

    cp -r ../contrib/release/macos/Liana.app ./
    sed -i "s/VERSION_PLACEHOLDER/$VERSION/g" ./Liana.app/Contents/Info.plist
    cp "$BUILD_DIR/gui/x86_64-apple-darwin/release/liana-gui" ./Liana.app/Contents/MacOS/Liana
    zip -ry Liana-noncodesigned.zip Liana.app
    cp ./Liana-noncodesigned.zip "$RELEASE_DIR/"

    if [ "$MAC_CODESIGN" = "1" ]; then
        $RCODESIGN_BIN sign --digest sha256 --code-signature-flags runtime --pem-source "$CODESIGN_KEY" --der-source "$CODESIGN_CERT" Liana.app/
        $RCODESIGN_BIN notary-submit --max-wait-seconds 600 --api-key-path "$NOTARY_API_CREDS_FILE" --staple Liana.app
        zip -ry Liana.zip Liana.app
        cp ./Liana.zip "$RELEASE_DIR/"
    fi
)

# Finally, sign all the assets
(
    cd "$RELEASE_DIR"
    for asset in $(ls); do
        gpg --detach-sign --armor "$asset"
    done
)

set +ex
