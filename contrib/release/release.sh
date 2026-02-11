#!/usr/bin/env sh

# ==============================================================================
# Script for creating and signing the release assets. To be ran from the root of
# the repository.
#
# Usage: ./contrib/release/release.sh <target>
#   target: "liana" or "liana-business"
#
# Example: ./contrib/release/release.sh liana
# ==============================================================================

set -ex

if [ $# -ne 1 ]; then
    echo "Usage: $0 <target>"
    echo "  target: 'liana' or 'liana-business'"
    exit 1
fi

TARGET="$1"

if [ "$TARGET" != "liana" ] && [ "$TARGET" != "liana-business" ]; then
    echo "Error: target must be 'liana' or 'liana-business'"
    exit 1
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
        VERSION="$(echo "$LATEST_TAG" | sed "s/^${TAG_PREFIX}//")-$(git rev-parse --short HEAD)"
    else
        VERSION="0-$(git rev-parse --short HEAD)"
    fi
fi

LIANA_PREFIX="$TARGET-$VERSION"
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

if [ "$TARGET" = "liana" ]; then
    # Build liana using Guix for Linux and Nix for other platforms
    OUT_DIR="$BUILD_DIR" ./contrib/reproducible/guix/guix-build.sh

    nix build .#liana.release
    NIX_BUILD_DIR="$(nix path-info .#liana.release)"

    # Create the Linux archive and Debian binary package.
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
else
    # Build liana-business using Guix for Linux and Nix for other platforms
    OUT_DIR="$BUILD_DIR" ./contrib/reproducible/guix/guix-build.sh

    # Build liana-business using Nix only
    nix build .#liana-business.release
    NIX_BUILD_DIR="$(nix path-info .#liana-business.release)"

    (
        cd "$BUILD_DIR"
        create_dir "$LINUX_DIR_NAME"
        cp "$BUILD_DIR/x86_64-unknown-linux-gnu/release/liana-business" ../README.md "$LINUX_DIR_NAME"
        tar --mtime="@${SOURCE_DATE_EPOCH}" -czf "$LINUX_ARCHIVE" "$LINUX_DIR_NAME"
        mv "$LINUX_ARCHIVE" "$RELEASE_DIR"

        unzip ../contrib/release/debian/package.zip
        sed -i "s/VERSION_PLACEHOLDER/$VERSION/g" ./package/DEBIAN/control
        sed -i "s/Liana/LianaBusiness/g" ./package/DEBIAN/control
        sed -i "s/liana/liana-business/g" ./package/DEBIAN/control
        sed -i "s/Liana/LianaBusiness/g" ./package/usr/share/applications/Liana.desktop
        sed -i "s/liana-gui/liana-business/g" ./package/usr/share/applications/Liana.desktop
        sed -i "s/liana-icon/liana-business-icon/g" ./package/usr/share/applications/Liana.desktop
        cp ../contrib/liana-business/liana-business-icon.png ./package/usr/share/icons/liana-business-icon.png
        mv ./package/usr/share/applications/Liana.desktop ./package/usr/share/applications/LianaBusiness.desktop
        cp "$BUILD_DIR/x86_64-unknown-linux-gnu/release/liana-business" ../README.md ./package/usr/bin/
        DIRNAME="$LIANA_PREFIX-1_amd64"
        mv ./package "$DIRNAME"
        dpkg-deb -Zxz --build --root-owner-group "$DIRNAME"
        mv "$DIRNAME.deb" "$RELEASE_DIR"
    )

    # Create the Windows executable
    (
        cd "$BUILD_DIR"
        cp "$NIX_BUILD_DIR/x86_64-pc-windows-gnu/liana-business.exe" "$RELEASE_DIR/$LIANA_PREFIX-noncodesigned.exe"
    )

    # Create the MacOS archives
    (
        cd "$BUILD_DIR"
        create_dir "$LIANA_PREFIX-x86_64-apple-darwin"
        cp "$NIX_BUILD_DIR/x86_64-apple-darwin/liana-business" ../README.md "$LIANA_PREFIX-x86_64-apple-darwin"
        tar --mtime="@${SOURCE_DATE_EPOCH}" -czf "$LIANA_PREFIX-x86_64-apple-darwin.tar.gz" "$LIANA_PREFIX-x86_64-apple-darwin"
        mv "$LIANA_PREFIX-x86_64-apple-darwin.tar.gz" "$RELEASE_DIR"

        create_dir "$LIANA_PREFIX-aarch64-apple-darwin"
        cp "$NIX_BUILD_DIR/aarch64-apple-darwin/liana-business" ../README.md "$LIANA_PREFIX-aarch64-apple-darwin"
        tar --mtime="@${SOURCE_DATE_EPOCH}" -czf "$LIANA_PREFIX-aarch64-apple-darwin.tar.gz" "$LIANA_PREFIX-aarch64-apple-darwin"
        mv "$LIANA_PREFIX-aarch64-apple-darwin.tar.gz" "$RELEASE_DIR"

        unzip ../contrib/release/macos/Liana.app.zip
        cp ../contrib/liana-business/liana-business.icns ./Liana.app/Contents/Resources/LianaBusiness.icns
        sed -i "s/VERSION_PLACEHOLDER/$VERSION/g" ./Liana.app/Contents/Info.plist
        sed -i "s/Liana/LianaBusiness/g" ./Liana.app/Contents/Info.plist
        sed -i "s/liana/liana-business/g" ./Liana.app/Contents/Info.plist
        cp "$NIX_BUILD_DIR/universal2-apple-darwin/liana-business" ./Liana.app/Contents/MacOS/LianaBusiness
        mv Liana.app LianaBusiness.app
        zip_archive "$LIANA_PREFIX-macos-noncodesigned.zip" LianaBusiness.app
        mv "$LIANA_PREFIX-macos-noncodesigned.zip" "$RELEASE_DIR/"
    )
fi

find "$RELEASE_DIR" -type f ! -name "$LIANA_PREFIX-shasums.txt" -exec sha256sum {} + | sed "s|$RELEASE_DIR/||" | tee "$RELEASE_DIR/$LIANA_PREFIX-shasums.txt"

set +ex
