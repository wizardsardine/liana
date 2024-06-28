#!/usr/bin/env sh

# ==========================================================
# The script ran within the Docker container to build Liana.
# ==========================================================

set -xe

test -f "$XCODE_PATH" || exit 1

# Build the SDK and the toolchain using osxcross. It is expected to be located at $XCODE_PATH
# It's not part of the image to be able to share the $XCODE_PATH instead of copying it in the Docker context
# and then to the image.
git clone https://github.com/darosior/osxcross -b dependencies_pinning
cd osxcross
git checkout 50e86ebca7d14372febd0af8cd098705049161b9
DARLING_DMG_REVISION=241238313a47d3cf6427ac5a75b7a0311a3a4cb4 \
    P7ZIP_REVISION=2f60a51ac3aa2507d36df3c4f58f71a3716b1357 \
    PBZX_REVISION=2a4d7c3300c826d918def713a24d25c237c8ed53 \
    XAR_REVISION=c2111a9a9cabc50d2b9c604aff41a481ae3f1989 ./tools/gen_sdk_package_pbzx.sh "$XCODE_PATH"
mv MacOSX* tarballs/
DARLING_DMG_REVISION=241238313a47d3cf6427ac5a75b7a0311a3a4cb4 \
    P7ZIP_REVISION=2f60a51ac3aa2507d36df3c4f58f71a3716b1357 \
    PBZX_REVISION=2a4d7c3300c826d918def713a24d25c237c8ed53 \
    XAR_REVISION=c2111a9a9cabc50d2b9c604aff41a481ae3f1989 \
    UNATTENDED=1 ./build.sh
cd ..

# Finally build the projects using the toolchain just created.
alias cargo="/liana/rust-1.70.0-x86_64-unknown-linux-gnu/cargo/bin/cargo"

PATH="$PATH:$PWD/osxcross/target/bin/" \
    CC=o64-clang \
    CXX=o64-clang++ \
    RUSTFLAGS="$RUSTFLAGS -Clinker=o64-clang" \
    cargo rustc \
        --target x86_64-apple-darwin \
        --release

cd gui/
PATH="$PATH:$PWD/../osxcross/target/bin/" \
    CC=o64-clang \
    CXX=o64-clang++ \
    RUSTFLAGS="$RUSTFLAGS -Clinker=o64-clang" \
    cargo rustc \
        --target x86_64-apple-darwin \
        --release
cd ..

# Avoid having to get root on the host to remove the target dir.
chmod -R a+rw target/ gui/target

set +xe
