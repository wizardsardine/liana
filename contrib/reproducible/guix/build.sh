# ===========================================================================
# The script ran within the GUIX container to build the Liana daemon and GUI.
# ===========================================================================

set -ex

# Instruct cargo to use our vendored sources
mkdir -p .cargo
cat <<EOF >.cargo/config.toml
[source.vendored_sources]
directory = "/vendor"

[source.crates-io]
replace-with = "vendored_sources"

[source."https://github.com/edouardparis/iced"]
git = "https://github.com/edouardparis/iced"
branch = "patch-0.12.3"
replace-with = "vendored_sources"
EOF

ls -la .cargo/config.toml

export CARGO_HOME="/liana/.cargo"

# We need to set RUSTC_BOOTSTRAP=1 as a workaround to be able to use unstable
# features in the GUI dependencies
for package_name in "liana" "liana-gui"; do
    RUSTC_BOOTSTRAP=1 cargo -vvv \
        --color always \
        --frozen \
        --offline \
        rustc \
        -p "$package_name" \
        --jobs "$JOBS" \
        --release \
        --target-dir "/out"
done

for bin_name in "liana-gui" "lianad" "liana-cli"; do
    # Assume 64bits. Even bitcoind doesn't ship 32bits binaries for x86.
    # FIXME: is there a cleaner way than using patchelf for this?
    patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 "/out/release/$bin_name"

    # FIXME: Find a way to use GUIX_LD_WRAPPER_DISABLE_RPATH=yes instead
    patchelf --remove-rpath "/out/release/$bin_name"
done

set +ex
