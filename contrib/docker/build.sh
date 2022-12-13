#!/usr/bin/env sh

# =================================================================
# The script ran within the Docker container to build the Liana GUI.
# =================================================================


# Build the GUI for Windows. The Windows Portable Execution (PE) format contains some timestamps.
# Instruct ld to set them to 0.
RUSTFLAGS="-Clink-arg=-Wl,--no-insert-timestamp" /liana/rust-1.65.0-x86_64-unknown-linux-gnu/cargo/bin/cargo rustc --release --target x86_64-pc-windows-gnu

# Avoid having to get root on the host to remove the target dir.
chmod -R a+rw target/
