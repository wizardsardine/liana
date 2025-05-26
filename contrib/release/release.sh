#!/usr/bin/env sh

# ==============================================================================
# Script for creating and signing the release assets. To be ran from the root of
# the repository.
# ==============================================================================

set -ex

VERSION="${VERSION:-"11.0"}"
LIANA_PREFIX="liana-$VERSION"
LINUX_DIR_NAME="$LIANA_PREFIX-x86_64-linux-gnu"
LINUX_ARCHIVE="$LINUX_DIR_NAME.tar.gz"

create_dir() {
    if [ -d "$1" ]; then
        rm -rf "$1"
    fi
    mkdir "$1"
}



# Determine the reference time used for determinism (overridable by environment)
export SOURCE_DATE_EPOCH="$(git -c log.showsignature=false log --format=%at -1)"
export TZ=UTC
export TAR_OPTIONS="--owner=0 --group=0 --numeric-owner --sort=name"

zip_archive () {
    local archive="$1"
    shift
    touch -d "@$SOURCE_DATE_EPOCH" "$@"
    find "$@" -type f -exec touch -d "@$SOURCE_DATE_EPOCH" {} +
    find "$@" -type f | sort | zip -oX "$archive" -@
}

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
    DIRNAME="$LIANA_PREFIX-1_amd64"
    mv ./package "$DIRNAME"
    dpkg-deb -Zxz --build --root-owner-group "$DIRNAME"
    mv "$DIRNAME.deb" "$RELEASE_DIR"
)

# Create the Windows archive and the raw executable
(
    cd "$BUILD_DIR"
    cp "$NIX_BUILD_DIR/x86_64-pc-windows-gnu/liana-gui.exe" "$RELEASE_DIR/$LIANA_PREFIX-noncodesigned.exe"
)

# Create the MacOS archive and a zipped application bundle of liana-gui.
(
    cd "$BUILD_DIR"
    create_dir "$LIANA_PREFIX-x86_64-apple-darwin"
    cp "$NIX_BUILD_DIR/x86_64-apple-darwin/lianad" "$NIX_BUILD_DIR/x86_64-apple-darwin/liana-cli" "$NIX_BUILD_DIR/x86_64-apple-darwin/liana-gui" ../README.md "$LIANA_PREFIX-x86_64-apple-darwin"
    tar --mtime="@${SOURCE_DATE_EPOCH}" -czf "$LIANA_PREFIX-x86_64-apple-darwin.tar.gz" "$LIANA_PREFIX-x86_64-apple-darwin"
    mv "$LIANA_PREFIX-x86_64-apple-darwin.tar.gz" "$RELEASE_DIR"

    create_dir "$LIANA_PREFIX-aarch64-apple-darwin"
    cp "$NIX_BUILD_DIR/aarch64-apple-darwin/lianad" "$NIX_BUILD_DIR/aarch64-apple-darwin/liana-cli" "$NIX_BUILD_DIR/aarch64-apple-darwin/liana-gui" ../README.md "$LIANA_PREFIX-aarch64-apple-darwin"
    tar --mtime="@${SOURCE_DATE_EPOCH}" -czf "$LIANA_PREFIX-aarch64-apple-darwin.tar.gz" "$LIANA_PREFIX-aarch64-apple-darwin"
    mv "$LIANA_PREFIX-aarch64-apple-darwin.tar.gz" "$RELEASE_DIR"

    unzip ../contrib/release/macos/Liana.app.zip
    sed -i "s/VERSION_PLACEHOLDER/$VERSION/g" ./Liana.app/Contents/Info.plist
    cp "$NIX_BUILD_DIR/universal2-apple-darwin/liana-gui" ./Liana.app/Contents/MacOS/Liana
    zip_archive "$LIANA_PREFIX-macos-noncodesigned.zip" Liana.app
    mv "$LIANA_PREFIX-macos-noncodesigned.zip" "$RELEASE_DIR/"
)

find "$RELEASE_DIR" -type f ! -name "$LIANA_PREFIX-shasums.txt" -exec sha256sum {} + | sed "s|$RELEASE_DIR/||" | tee "$RELEASE_DIR/$LIANA_PREFIX-shasums.txt"

set +ex
