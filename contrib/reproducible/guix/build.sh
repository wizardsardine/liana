# ==========================================================================
# The script ran within the GUIX container to build the Liana daemon or GUI.
# ==========================================================================

set -ex

# Instruct cargo to use our vendored sources
mkdir -p .cargo
cat <<EOF >.cargo/config.toml
[source.vendored_sources]
directory = "/vendor"

[source.crates-io]
replace-with = "vendored_sources"

[source."https://github.com/wizardsardine/liana"]
git = "https://github.com/wizardsardine/liana"
branch = "master"
replace-with = "vendored_sources"

[source."https://github.com/edouardparis/iced"]
git = "https://github.com/edouardparis/iced"
branch = "patch-0.12.3"
replace-with = "vendored_sources"
EOF

# We need to set RUSTC_BOOTSTRAP=1 as a workaround to be able to use unstable
# features in the GUI dependencies
RUSTC_BOOTSTRAP=1 cargo -vvv \
    --color always \
    --frozen \
    --offline \
    rustc \
    --jobs "$JOBS" \
    --release \
    --target-dir "/out"

if [ "$IS_GUI" = "1" ]; then
    BIN_NAMES="liana-gui"
else
    BIN_NAMES="lianad liana-cli"
fi

for bin_name in $BIN_NAMES; do
    # Assume 64bits. Even bitcoind doesn't ship 32bits binaries for x86.
    # FIXME: is there a cleaner way than using patchelf for this?
    patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 "/out/release/$bin_name"

    # FIXME: Find a way to use GUIX_LD_WRAPPER_DISABLE_RPATH=yes instead
    patchelf --remove-rpath "/out/release/$bin_name"
done

set +ex
