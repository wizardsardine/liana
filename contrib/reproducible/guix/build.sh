# ===========================================================================
# The script ran within the GUIX container to build the Liana daemon and GUI.
# ===========================================================================

set -ex

# Tell Cargo to look in /tmp for its configuration
export CARGO_HOME="/tmp/.cargo"
# Instruct cargo to use our vendored sources
mkdir -p $CARGO_HOME

cat <<EOF >"$CARGO_HOME/config.toml"
[source.vendored_sources]
directory = "/vendor"

[source.crates-io]
replace-with = "vendored_sources"

[source."git+https://github.com/wizardsardine/iced?rev=ccf011541eff660ed2bc6a01f9f00e150b501453"]
git = "https://github.com/wizardsardine/iced"
rev = "ccf011541eff660ed2bc6a01f9f00e150b501453"
replace-with = "vendored_sources"

[source."git+https://github.com/wizardsardine/iced_aw?rev=488248db097769cd2269af75b5f93d5c65f45a38"]
git = "https://github.com/wizardsardine/iced_aw"
rev = "488248db097769cd2269af75b5f93d5c65f45a38"
replace-with = "vendored_sources"

[source."git+https://github.com/wizardsardine/bdk?branch=release/1.0.0-alpha.13"]
git = "https://github.com/wizardsardine/bdk"
branch = "release/1.0.0-alpha.13"
replace-with = "vendored_sources"
EOF

# We need to set RUSTC_BOOTSTRAP=1 as a workaround to be able to use unstable
# features in the GUI dependencies
for package_name in "lianad" "liana-gui" "liana-business"; do
    # Disable default features for liana-gui and liana-business so the
    # `debugger` feature (default-on for development) is not compiled into
    # release artifacts.
    case "$package_name" in
        liana-gui|liana-business) extra_args="--no-default-features" ;;
        *) extra_args="" ;;
    esac
    RUSTC_BOOTSTRAP=1 cargo zigbuild -vvv \
        --color always \
        --frozen \
        --offline \
        -p "$package_name" \
        $extra_args \
        --jobs "$JOBS" \
	--target x86_64-unknown-linux-gnu.2.31 \
        --release \
        --target-dir "/out"
done

for bin_name in "liana-gui" "lianad" "liana-cli" "liana-business"; do
    # Assume 64bits. Even bitcoind doesn't ship 32bits binaries for x86.
    # FIXME: is there a cleaner way than using patchelf for this?
    patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 "/out/x86_64-unknown-linux-gnu/release/$bin_name"

    # FIXME: Find a way to use GUIX_LD_WRAPPER_DISABLE_RPATH=yes instead
    patchelf --remove-rpath "/out/x86_64-unknown-linux-gnu/release/$bin_name"
done

set +ex
