#!/usr/bin/env sh

# ==============================================================================
# Script for creating and signing the release assets. To be ran from the root of
# the repository.
# ==============================================================================

set -ex

VERSION="${VERSION:-"8.0"}"
LIANA_PREFIX="liana-$VERSION"
LINUX_DIR_NAME="$LIANA_PREFIX-x86_64-linux-gnu"
LINUX_ARCHIVE="$LINUX_DIR_NAME.tar.gz"
WINDOWS_DIR_NAME="$LIANA_PREFIX-x86_64-windows-gnu"
WINDOWS_ARCHIVE="$WINDOWS_DIR_NAME.zip"
MAC_DIR_NAME="$LIANA_PREFIX-x86_64-apple-darwin"
MAC_ARCHIVE="$MAC_DIR_NAME.tar.gz"

create_dir() {
    if [ -d "$1" ]; then
        rm -rf "$1"
    fi
    mkdir "$1"
}

# Determine the reference time used for determinism (overridable by environment)
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -c log.showSignature=false log --format=%at -1)}"
export TAR_OPTIONS="--owner=0 --group=0 --numeric-owner --sort=name"

# We'll use a folder for the builds output and another one for the final assets.
RELEASE_DIR="$PWD/release_assets"
BUILD_DIR="$PWD/release_build"
create_dir "$RELEASE_DIR"
create_dir "$BUILD_DIR"

OUT_DIR="$BUILD_DIR" ./contrib/reproducible/guix/guix-build.sh

nix build .#release
NIX_BUILD_DIR="$(nix path-info .#release)"

#Create the Linux archive and Debian binary package.
(
    cd "$BUILD_DIR"
    create_dir "$LINUX_DIR_NAME"
    cp "$BUILD_DIR/x86_64-unknown-linux-gnu/release/lianad" "$BUILD_DIR/x86_64-unknown-linux-gnu/release/liana-cli" "$BUILD_DIR/x86_64-unknown-linux-gnu/release/liana-gui" ../README.md "$LINUX_DIR_NAME"
    tar --mtime="@${SOURCE_DATE_EPOCH}" -czf "$LINUX_ARCHIVE" "$LINUX_DIR_NAME"
    mv "$LINUX_ARCHIVE" "$RELEASE_DIR"

    unzip ../contrib/release/debian/package.zip
    sed -i "s/VERSION_PLACEHOLDER/$VERSION/g" ./package/DEBIAN/control
    cp "$BUILD_DIR/x86_64-unknown-linux-gnu/release/lianad" "$BUILD_DIR/x86_64-unknown-linux-gnu/release/liana-cli" "$BUILD_DIR/x86_64-unknown-linux-gnu/release/liana-gui" ../README.md ./package/usr/bin/
    DIRNAME="liana_$VERSION-1_amd64"
    mv ./package "$DIRNAME"
    dpkg-deb -Zxz --build "$DIRNAME"
    mv "$DIRNAME.deb" "$RELEASE_DIR"
)

# Create the Windows archive and the raw executable
(
    cd "$BUILD_DIR"
    create_dir "$WINDOWS_DIR_NAME"
    cp "$NIX_BUILD_DIR/x86_64-pc-windows-gnu/liana-gui.exe" ../README.md "$WINDOWS_DIR_NAME"
    zip -r "$WINDOWS_ARCHIVE" "$WINDOWS_DIR_NAME"
    mv "$WINDOWS_ARCHIVE" "$RELEASE_DIR"
    cp "$NIX_BUILD_DIR/x86_64-pc-windows-gnu/liana-gui.exe" "$RELEASE_DIR/$LIANA_PREFIX.exe"
)

# Create the MacOS archive and a zipped application bundle of liana-gui.
(
    cd "$BUILD_DIR"
    create_dir "$MAC_DIR_NAME"
    cp "$NIX_BUILD_DIR/x86_64-apple-darwin/lianad" "$NIX_BUILD_DIR/x86_64-apple-darwin/liana-cli" "$NIX_BUILD_DIR/x86_64-apple-darwin/liana-gui" ../README.md "$MAC_DIR_NAME"
    tar --mtime="@${SOURCE_DATE_EPOCH}" -czf "$MAC_ARCHIVE" "$MAC_DIR_NAME"
    mv "$MAC_ARCHIVE" "$RELEASE_DIR"

    unzip ../contrib/release/macos/Liana.app.zip
    sed -i "s/VERSION_PLACEHOLDER/$VERSION/g" ./Liana.app/Contents/Info.plist
    cp "$NIX_BUILD_DIR/x86_64-apple-darwin/liana-gui" ./Liana.app/Contents/MacOS/Liana
    chmod u+w ./Liana.app/Contents/MacOS/Liana
    zip -ry "Liana-$VERSION-noncodesigned.zip" Liana.app
    mv "Liana-$VERSION-noncodesigned.zip" "$RELEASE_DIR/"
)

find "$RELEASE_DIR" -type f -exec sha256sum {} + | tee "$RELEASE_DIR/shasums.txt"

set +ex
