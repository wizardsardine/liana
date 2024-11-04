#!/usr/bin/env sh

# =================================================================
# The script ran within the Docker container to build the Liana GUI.
# =================================================================

set -xe

# Build the GUI for Windows. The Windows Portable Execution (PE) format contains some timestamps.
# Instruct ld to set them to 0.
alias cargo="/liana/rust-1.71.1-x86_64-unknown-linux-gnu/cargo/bin/cargo"
RUSTFLAGS="-Clink-arg=-Wl,--no-insert-timestamp" \
    cargo rustc \
        --release \
        --target x86_64-pc-windows-gnu

# Avoid having to get root on the host to remove the target dir.
chmod -R a+rw target/

set +xe
