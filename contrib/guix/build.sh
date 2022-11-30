set -ex

# Guix comes with Cargo 1.52 but --config was stabilized in 1.63, so we need
# to specify unstable-options.
# We use the --config to redirect cargo toward our vendored source directory
# for our dependencies.
# TODO: build in release mode
cargo -Z unstable-options -vvv \
    --color always \
    --frozen \
    --offline \
    rustc \
    --release \
    --target-dir "$TARGET_DIR" \
    --config source.vendored_sources.directory=\""$VENDOR_DIR"\" \
    --config source.crates-io.replace-with=\"vendored_sources\" \
    --config source.\"https://github.com/darosior/rust-miniscript\".replace-with=\"vendored_sources\" \
    --config source.\"https://github.com/darosior/rust-miniscript\".git=\"https://github.com/darosior/rust-miniscript\" \
    --config source.\"https://github.com/darosior/rust-miniscript\".branch=\"multipath_descriptors_on_8.0\"

set +ex
