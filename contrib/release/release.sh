#!/usr/bin/env sh

# ==============================================================================
# Script for creating and signing the release assets. To be ran from the root of
# the repository.
# ==============================================================================

set -ex

VERSION="${VERSION:-"0.2"}"
LIANA_PREFIX="liana-$VERSION"
LINUX_DIR_NAME="$LIANA_PREFIX-x86_64-linux-gnu"
LINUX_ARCHIVE="$LINUX_DIR_NAME.tar.gz"
WINDOWS_DIR_NAME="$LIANA_PREFIX-x86_64-windows-gnu"
WINDOWS_ARCHIVE="$WINDOWS_DIR_NAME.zip"
MAC_DIR_NAME="$LIANA_PREFIX-x86_64-apple-darwin"
MAC_ARCHIVE="$MAC_DIR_NAME.tar.gz"

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

# Create the MacOS archive and the DMG
(
    cd "$BUILD_DIR"
    create_dir "$MAC_DIR_NAME"
    cp "$BUILD_DIR/x86_64-apple-darwin/release/lianad" "$BUILD_DIR/x86_64-apple-darwin/release/liana-cli" "$BUILD_DIR/gui/x86_64-apple-darwin/release/liana-gui" ../README.md "$MAC_DIR_NAME"
    tar -czf "$MAC_ARCHIVE" "$MAC_DIR_NAME"
    cp "$MAC_ARCHIVE" "$RELEASE_DIR"

    DMG_DIR="liana-$VERSION"
    cp -r ../contrib/release/macos/dmg_template "$DMG_DIR"
    sed -i "s/VERSION_PLACEHOLDER/$VERSION/g" "$DMG_DIR/Liana.app/Contents/Info.plist"
    ln -s /Applications "$DMG_DIR/Applications"
    python3 -m venv venv
    . venv/bin/activate
    pip install ds_store mac_alias
    python3 ../contrib/release/macos/gen_dstore.py
    mv .DS_Store "$DMG_DIR/"
    cp "$BUILD_DIR/gui/x86_64-apple-darwin/release/liana-gui" "$DMG_DIR/Liana.app/Contents/MacOS/Liana"
    DMG_FILE="liana-$VERSION.dmg"
    xorrisofs -D -l -V Liana -no-pad -r -dir-mode 0755 -o "$DMG_FILE" "$DMG_DIR"
    cp "$DMG_FILE" "$RELEASE_DIR/"
)

# Finally, sign all the assets
(
    cd "$RELEASE_DIR"
    for asset in $(ls); do
        gpg --detach-sign --armor "$asset"
    done
)

set +ex
