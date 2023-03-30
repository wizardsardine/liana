#!/usr/bin/env sh

set -ex

TARGET_DIR="${TARGET_DIR:-"$PWD/deter_build_target"}"

XCODE_PATH="${XCODE_PATH:-"$PWD/Xcode_12.2.xip"}"

# Build (only) the Liana GUI on Windows.
docker build . -t liana_cross_win -f contrib/reproducible/docker/windows.Dockerfile
docker run --rm -ti \
    -v "$TARGET_DIR/gui":/liana/target \
    -v "$PWD/contrib/reproducible/docker":/liana/docker \
    -v "$PWD/gui/Cargo.toml":/liana/Cargo.toml \
    -v "$PWD/gui/Cargo.lock":/liana/Cargo.lock \
    -v "$PWD/gui/src":/liana/src \
    -v "$PWD/gui/ui/Cargo.toml":/liana/ui/Cargo.toml \
    -v "$PWD/gui/ui/Cargo.lock":/liana/ui/Cargo.lock \
    -v "$PWD/gui/ui/src":/liana/ui/src \
    -v "$PWD/gui/ui/static":/liana/ui/static \
    liana_cross_win


# Sanity check the given MacOS SDK is the expected one.
if ! $(echo "28d352f8c14a43d9b8a082ac6338dc173cb153f964c6e8fb6ba389e5be528bd0 $(basename $XCODE_PATH)" | sha256sum -c --status); then
    echo "No or invalid Xcode SDK found. Need an Xcode_12.2.xip. You can configure the path using \$XCODE_PATH.";
    exit 1;
fi

# Build both the Liana daemon and GUI on MacOS.
docker build . -t liana_cross_mac -f contrib/reproducible/docker/macos.Dockerfile
docker run --rm -ti \
    -v "$TARGET_DIR":/liana/target \
    -v "$TARGET_DIR/gui":/liana/gui/target \
    -v "$PWD/contrib/reproducible/docker":/liana/docker \
    -v "$PWD/Cargo.toml":/liana/Cargo.toml \
    -v "$PWD/Cargo.lock":/liana/Cargo.lock \
    -v "$PWD/src":/liana/src \
    -v "$PWD/gui/Cargo.toml":/liana/gui/Cargo.toml \
    -v "$PWD/gui/Cargo.lock":/liana/gui/Cargo.lock \
    -v "$PWD/gui/src":/liana/gui/src \
    -v "$PWD/gui/ui/Cargo.toml":/liana/gui/ui/Cargo.toml \
    -v "$PWD/gui/ui/Cargo.lock":/liana/gui/ui/Cargo.lock \
    -v "$PWD/gui/ui/src":/liana/gui/ui/src \
    -v "$PWD/gui/ui/static":/liana/gui/ui/static \
    -v "$XCODE_PATH":/liana/Xcode_12.2.xip \
    liana_cross_mac

set +ex
