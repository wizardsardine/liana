#!/usr/bin/env sh

set -ex

TARGET_DIR="${TARGET_DIR:-"$PWD/deter_build_target"}"

XCODE_PATH="${XCODE_PATH:-"$PWD/Xcode_12.2.xip"}"
XCODE_FILENAME="$(basename $XCODE_PATH)"
XCODE_SHASUM="28d352f8c14a43d9b8a082ac6338dc173cb153f964c6e8fb6ba389e5be528bd0"

# Build (only) the Liana GUI on Windows.
docker build . -t liana_cross_win -f contrib/reproducible/docker/windows.Dockerfile
docker run --rm -ti \
    -v "$TARGET_DIR":/liana/target \
    -v "$PWD/contrib/reproducible/docker":/liana/docker \
    -v "$PWD/Cargo.toml":/liana/Cargo.toml \
    -v "$PWD/Cargo.lock":/liana/Cargo.lock \
    -v "$PWD/liana/Cargo.toml":/liana/liana/Cargo.toml \
    -v "$PWD/liana/src":/liana/liana/src \
    -v "$PWD/lianad/Cargo.toml":/liana/lianad/Cargo.toml \
    -v "$PWD/lianad/src":/liana/lianad/src \
    -v "$PWD/liana-gui/Cargo.toml":/liana/liana-gui/Cargo.toml \
    -v "$PWD/liana-gui/src":/liana/liana-gui/src \
    -v "$PWD/liana-ui/Cargo.toml":/liana/liana-ui/Cargo.toml \
    -v "$PWD/liana-ui/src":/liana/liana-ui/src \
    -v "$PWD/liana-ui/static":/liana/liana-ui/static \
    -v "$PWD/fuzz/Cargo.toml":/liana/fuzz/Cargo.toml \
    liana_cross_win


# Sanity check the given MacOS SDK is the expected one.
if ! $(echo "$XCODE_SHASUM $XCODE_PATH" | sha256sum -c --status); then
    echo "No or invalid Xcode SDK found. Need an Xcode_X.Y.xip archive whose hash is $XCODE_SHASUM. You can configure the path using \$XCODE_PATH.";
    exit 1;
fi

# Build both the Liana daemon and GUI on MacOS.
docker build . -t liana_cross_mac -f contrib/reproducible/docker/macos.Dockerfile
docker run -ti \
    -v "$TARGET_DIR":/liana/target \
    -v "$PWD/contrib/reproducible/docker":/liana/docker \
    -v "$PWD/Cargo.toml":/liana/Cargo.toml \
    -v "$PWD/Cargo.lock":/liana/Cargo.lock \
    -v "$PWD/liana/Cargo.toml":/liana/liana/Cargo.toml \
    -v "$PWD/liana/src":/liana/liana/src \
    -v "$PWD/lianad/Cargo.toml":/liana/lianad/Cargo.toml \
    -v "$PWD/lianad/src":/liana/lianad/src \
    -v "$PWD/liana-gui/Cargo.toml":/liana/liana-gui/Cargo.toml \
    -v "$PWD/liana-gui/src":/liana/liana-gui/src \
    -v "$PWD/liana-ui/Cargo.toml":/liana/liana-ui/Cargo.toml \
    -v "$PWD/liana-ui/src":/liana/liana-ui/src \
    -v "$PWD/liana-ui/static":/liana/liana-ui/static \
    -v "$PWD/fuzz/Cargo.toml":/liana/fuzz/Cargo.toml \
    -v "$XCODE_PATH":"/liana/$XCODE_FILENAME" \
    -e XCODE_PATH="/liana/$XCODE_FILENAME" \
    liana_cross_mac

set +ex
