set -ex

# Instruct cargo to use our vendored sources
mkdir -p ~/.cargo
cat <<EOF >~/.cargo/config.toml
[source.vendored_sources]
directory = "$VENDOR_DIR"

[source.crates-io]
replace-with = "vendored_sources"

[source."https://github.com/darosior/rust-miniscript"]
git = "https://github.com/darosior/rust-miniscript"
branch = "multipath_descriptors_on_8.0"
replace-with = "vendored_sources"

[source."https://github.com/revault/liana"]
git = "https://github.com/revault/liana"
branch = "master"
replace-with = "vendored_sources"
EOF

# We need to set RUSTC_BOOTSTRAP=1 as a workaround to be able to use unstable
# features in the GUI dependencies
RUSTC_BOOTSTRAP=1 cargo -vvv \
    --color always \
    --frozen \
    --offline \
    rustc \
    --release \
    --target-dir "$TARGET_DIR"

# Assume 64bits. Even bitcoind doesn't ship 32bits binaries for x86.
# FIXME: is there a cleaner way than using patchelf for this?
patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 "$TARGET_DIR/release/$BINARY_NAME"

# FIXME: Find a way to use GUIX_LD_WRAPPER_DISABLE_RPATH=yes instead
patchelf --remove-rpath "$TARGET_DIR/release/$BINARY_NAME"

set +ex
