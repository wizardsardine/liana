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

# Create the directory if it doesn't exist already
maybe_create_dir() {
    if ! [ -d "$@" ]; then
        mkdir -p "$@"
    fi
}
maybe_create_dir "$BIN_DIR"

# That's what Guix comes with.
RUST_VERSION="1.60.0"
CARGO_BIN="$BIN_DIR/cargo"

# First off get the cargo binary to run on the host to vendor dependencies.
# We assume the host is a 64bit Linux system.
if ! [ -f "$CARGO_BIN" ]; then
    ARCHIVE_PATH="$BIN_DIR/rust-for-cargo.tar.gz"
    curl -o "$ARCHIVE_PATH" "https://static.rust-lang.org/dist/rust-$RUST_VERSION-x86_64-unknown-linux-gnu.tar.gz"
    echo "b8a4c3959367d053825e31f90a5eb86418eb0d80cacda52bfa80b078e18150d5 $ARCHIVE_PATH" | $SHASUM_BIN -c
    # Path of the cargo binary within the archive
    CARGO_BIN_PATH="rust-$RUST_VERSION-x86_64-unknown-linux-gnu/cargo/bin/cargo"
    ( cd $BIN_DIR && tar -xzf $ARCHIVE_PATH $CARGO_BIN_PATH && mv $CARGO_BIN_PATH $CARGO_BIN )
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

# Build both the daemon (at the root of the repository) and the GUI (in gui/)
for project_folder in "" "gui"; do
    PROJECT_ROOT="$PWD/$project_folder"
    PROJECT_VENDOR_DIR="$VENDOR_DIR/$project_folder"
    PROJECT_OUT_DIR="$OUT_DIR/$project_folder"
    PROJECT_PATCHES_ROOT="$PWD/contrib/reproducible/guix/liana-patches/$project_folder"

    project_needs_patches() {
        test "$(ls -A1q "$PROJECT_PATCHES_ROOT" |grep patch)"
    }

    maybe_create_dir "$PROJECT_OUT_DIR"

    # Pull the sources of our dependencies before building them in the container.
    if ! [ -d "$PROJECT_VENDOR_DIR" ]; then
        # Download the dependencies
        ( cd "$project_folder" && $CARGO_BIN vendor "$PROJECT_VENDOR_DIR" )

        # Patch some dependencies sources if needed for this project
        if project_needs_patches; then
            (
                cd "$PROJECT_VENDOR_DIR"
                for patch_file in $(ls "$PROJECT_PATCHES_ROOT"); do
                    patch -p1 < "$PROJECT_PATCHES_ROOT/$patch_file"
                done
            )

            # Some of the checksums will be incorrect. Instead of cherry-picking remove them
            # altogether, since they aren't useful anyways (see comment below).
            for dep in $(ls "$PROJECT_VENDOR_DIR"); do
                echo "{\"files\":{}}" > "$PROJECT_VENDOR_DIR/$dep/.cargo-checksum.json"
            done
        fi
    fi

    cp "$PROJECT_ROOT/Cargo.lock" "$BUILD_ROOT/Cargo.lock"
    if project_needs_patches; then
        # Remove the checksums from the Cargo.lock. In the container `cargo rustc` would compare
        # them against the .cargo-checksum.json to make sure they weren't tampered with since they
        # where vendored. But we just removed the checksums from the .cargo-checksum.json.
        # There is little point in checking integrity between the above vendor step and now anyways.
        # What matters is checking integrity after downloading the crates from the internet and
        # `cargo vendor` does that already.
        sed -i '/checksum/d' "$BUILD_ROOT/Cargo.lock"
    fi

    # FIXME: find a cleaner way to get the binary name, or get rid of patchelf entirely
    # Note: we also rely on it in manifest.scm
    if [ "$project_folder" = "" ]; then
        IS_GUI=0
    elif [ "$project_folder" = "gui" ]; then
        IS_GUI=1
    else
        echo "Can't determine binary name"
        exit 1
    fi

    # Bootstrap a reproducible environment as specified by the manifest in an isolated
    # container, and build the project.
    # NOTE: it looks like "--rebuild-cache" is necessary for the IS_GUI variable to
    # be taken into account when building the container (otherwise the GUI container could
    # miss some dependencies).
    # TODO: only expose the "ui" folder for GUI builds
    IS_GUI="$IS_GUI" time_machine shell --no-cwd \
               --expose="$PROJECT_ROOT/src=/liana/src" \
               --expose="$PROJECT_ROOT/Cargo.toml=/liana/Cargo.toml" \
               --expose="$BUILD_ROOT/Cargo.lock=/liana/Cargo.lock" \
               --expose="$PWD/gui/ui/src=/liana/ui/src" \
               --expose="$PWD/gui/ui/Cargo.toml=/liana/ui/Cargo.toml" \
               --expose="$PWD/gui/ui/static=/liana/ui/static" \
               --expose="$PWD/contrib/reproducible/guix/build.sh=/liana/build.sh" \
               --expose="$PROJECT_VENDOR_DIR=/vendor" \
               --share="$PROJECT_OUT_DIR=/out" \
               --cores="$JOBS" \
               --container \
               --pure \
               --fallback \
               --rebuild-cache \
               -m $PWD/contrib/reproducible/guix/manifest.scm \
               -- env CC=gcc VENDOR_DIR="$PROJECT_VENDOR_DIR" TARGET_DIR="$PROJECT_OUT_DIR" IS_GUI="$IS_GUI" JOBS="$JOBS" \
                  /bin/sh -c "cd /liana && ./build.sh"
done

set +ex

echo "Build successful. Output available at $OUT_DIR"
