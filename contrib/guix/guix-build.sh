#!/usr/bin/env sh

set -ex

# How many cores to allocate to Guix building.
JOBS="${JOBS:-$(nproc)}"

# The binary to check the hash of downloaded archives.
SHASUM_BIN="${SHASUM_BIN:-sha256sum}"

# We do everything in a single directory. That's the root of it, configurable
# through the environment.
BUILD_ROOT="${BUILD_ROOT:-$(mktemp -d)}"

# Various folders we expose to the container. The vendor directory will contain
# the sources of all our dependencies. Because we restrict network access from
# within the container, this is pulled beforehand.
# The out directory will contain the resulting binaries. It's wired to the --target-dir
# for a cargo build.
VENDOR_DIR="$BUILD_ROOT/vendor"
OUT_DIR="${OUT_DIR:-"$BUILD_ROOT/out"}"
BIN_DIR="${BIN_DIR:-"$BUILD_ROOT/bin"}"

# Create the various folders if the root build directory is fresh.
for d in "$OUT_DIR" "$BIN_DIR"; do
    if ! [ -d "$d" ]; then
        mkdir -p "$d"
    fi
done

# That's what Guix comes with.
RUST_VERSION="1.52.0"
CARGO_BIN="$BIN_DIR/cargo"

# First off get the cargo binary to run on the host to vendor dependencies.
# We assume the host is a 64bit Linux system.
if ! [ -f "$CARGO_BIN" ]; then
    ARCHIVE_PATH="$BIN_DIR/rust-for-cargo.tar.gz"
    curl -o "$ARCHIVE_PATH" "https://static.rust-lang.org/dist/rust-$RUST_VERSION-x86_64-unknown-linux-gnu.tar.gz"
    echo "c082b5eea81206ff207407b41a10348282362dd972e93c86b054952b66ca0e2b $ARCHIVE_PATH" | $SHASUM_BIN -c
    # Path of the cargo binary within the archive
    CARGO_BIN_PATH="rust-$RUST_VERSION-x86_64-unknown-linux-gnu/cargo/bin/cargo"
    ( cd $BIN_DIR && tar -xzf $ARCHIVE_PATH $CARGO_BIN_PATH && mv $CARGO_BIN_PATH $CARGO_BIN )
fi

# Pull the sources of our dependencies before building them in the container.
if ! [ -d "$VENDOR_DIR" ]; then
    $CARGO_BIN vendor $VENDOR_DIR
fi

# Execute "$@" in a pinned, possibly older version of Guix, for reproducibility
# across time.
time_machine() {
    guix time-machine --url=https://git.savannah.gnu.org/git/guix.git \
                      --commit=059d38dc3f8b087f4a42df586daeb05761ee18d7 \
                      --cores="$JOBS" \
                      --keep-failed \
                      --fallback \
                      -- "$@"
}

# Bootstrap a reproducible environment as specified by the manifest in an isolated
# container, and build the project.
time_machine shell --no-cwd \
           --expose="$PWD/src=/liana/src" \
           --expose="$PWD/Cargo.toml=/liana/Cargo.toml" \
           --expose="$PWD/Cargo.lock=/liana/Cargo.lock" \
           --expose="$PWD/contrib/guix/build.sh=/liana/build.sh" \
           --expose="$VENDOR_DIR=$VENDOR_DIR" \
           --share="$OUT_DIR=$OUT_DIR" \
           --container \
           -m $PWD/contrib/guix/manifest.scm \
           -- env CC=clang VENDOR_DIR="$VENDOR_DIR" TARGET_DIR="$OUT_DIR" \
              /bin/sh -c "cd /liana && ./build.sh"

set +ex

echo "Build successful. Output available at $OUT_DIR"
