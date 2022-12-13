#!/usr/bin/env sh

set -ex

TARGET_DIR="$PWD/deter_build_target"

docker build . -t liana_cross_compile -f contrib/docker/Dockerfile
docker run --rm -ti -v "$TARGET_DIR":/liana/target liana_cross_compile

set +ex
