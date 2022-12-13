#!/usr/bin/env sh

set -ex

TARGET_DIR="$PWD/deter_build_target"

docker build . -t liana_cross_compile -f contrib/docker/Dockerfile
docker run --rm -ti \
    -v "$TARGET_DIR":/liana/target \
    -v "$PWD/contrib/docker":/liana/docker \
    -v "$PWD/gui/src":/liana/src \
    -v "$PWD/gui/static":/liana/static \
    liana_cross_compile

set +ex
