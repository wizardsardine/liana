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
# FIXME: GUIX_LD_WRAPPER_DISABLE_RPATH=yes
RUSTC_BOOTSTRAP=1 cargo -vvv \
    --color always \
    --frozen \
    --offline \
    rustc \
    --release \
    --target-dir "$TARGET_DIR"

set +ex
