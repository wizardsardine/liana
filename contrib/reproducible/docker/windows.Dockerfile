FROM debian:bullseye

WORKDIR /liana

# We try to pin our dependencies to avoid potential sources of non-determinism, but we don't go
# out of our way to pin the whole tree of deps. Instead invest time in getting Guix cross-compilation.
RUN apt update && apt satisfy -y \
                    "gcc-mingw-w64-x86-64 (>=10.2, <=10.2)" \
                    "curl (>=7.74, <=7.74)" \
                    "gcc (>=10.2, <=10.2)"

# Download the cargo binary and compiled stdlib from the distributed releases to make sure to build with
# the very same toolchain. We use 1.65.0 because it is unfortunately the MSRV of the GUI.
RUN curl -O "https://static.rust-lang.org/dist/rust-1.65.0-x86_64-unknown-linux-gnu.tar.gz" && \
    echo "8f754fdd5af783fe9020978c64e414cb45f3ad0a6f44d045219bbf2210ca3cb9 rust-1.65.0-x86_64-unknown-linux-gnu.tar.gz" | sha256sum -c && \
    tar -xzf rust-1.65.0-x86_64-unknown-linux-gnu.tar.gz && \
    curl -O "https://static.rust-lang.org/dist/rust-1.65.0-x86_64-pc-windows-gnu.tar.gz" && \
    echo "eaa0d89511739c16d2a6149ed3538ce10596c523c4791b4a378dde762cda77e4 rust-1.65.0-x86_64-pc-windows-gnu.tar.gz" | sha256sum -c  && \
    tar -xzf rust-1.65.0-x86_64-pc-windows-gnu.tar.gz && \
    rm -r *.tar.gz

# NOTE: we were previously caching dependencies here (through `cargo vendor`). It's a tradeoff between the image size
# and not needing internet access when running the image to build the software.

# For some reason, we can't just set the RUSTFLAGS environment variable to add `-L` for compiling dependencies.
# This doesn't work: RUSTFLAGS="-L /liana/rust-1.65.0-x86_64-pc-windows-gnu/rust-std-x86_64-pc-windows-gnu/lib/rustlib/x86_64-pc-windows-gnu/lib/ -L /liana/rust-1.65.0-x86_64-unknown-linux-gnu/rust-std-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib/"
# As a workaround, we use a wrapped `rustc` binary that always links against the windows stdlib we just downloaded.
# Some issues that seem to be related:
# https://github.com/rust-lang/rust/issues/40717
# https://github.com/rust-lang/rust/issues/48409
RUN echo "#!/bin/sh" > rustc_wrapper.sh && \
    echo "/liana/rust-1.65.0-x86_64-unknown-linux-gnu/rustc/bin/rustc \"\$@\" -L /liana/rust-1.65.0-x86_64-pc-windows-gnu/rust-std-x86_64-pc-windows-gnu/lib/rustlib/x86_64-pc-windows-gnu/lib/ -L /liana/rust-1.65.0-x86_64-unknown-linux-gnu/rust-std-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib/" >> rustc_wrapper.sh && \
    chmod +x rustc_wrapper.sh
ENV RUSTC="/liana/rustc_wrapper.sh"

CMD ["./docker/windows_cmd.sh"]
