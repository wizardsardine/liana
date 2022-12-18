FROM debian:bullseye

WORKDIR /liana

# We try to pin our dependencies to avoid potential sources of non-determinism, but we don't go
# out of our way to pin the whole tree of deps. Instead invest time in getting Guix cross-compilation.
RUN apt update && apt install -y \
                    gcc-mingw-w64-x86-64=10.2.1-6+24.2 \
                    curl=7.74.0-1.3+deb11u3 \
                    gcc=4:10.2.1-1

# Download the cargo binary and compiled stdlib from the distributed releases to make sure to build with
# the very same toolchain. We use 1.65.0 because it is unfortunately the MSRV of the GUI.
RUN curl -O "https://static.rust-lang.org/dist/rust-1.65.0-x86_64-unknown-linux-gnu.tar.gz" && \
    echo "8f754fdd5af783fe9020978c64e414cb45f3ad0a6f44d045219bbf2210ca3cb9 rust-1.65.0-x86_64-unknown-linux-gnu.tar.gz" | sha256sum -c && \
    tar -xzf rust-1.65.0-x86_64-unknown-linux-gnu.tar.gz && \
    curl -O "https://static.rust-lang.org/dist/rust-1.65.0-x86_64-pc-windows-gnu.tar.gz" && \
    echo "eaa0d89511739c16d2a6149ed3538ce10596c523c4791b4a378dde762cda77e4 rust-1.65.0-x86_64-pc-windows-gnu.tar.gz" | sha256sum -c  && \
    tar -xzf rust-1.65.0-x86_64-pc-windows-gnu.tar.gz && \
    rm -r *.tar.gz

# Copy the Cargo files to vendor the dependencies.
COPY gui/Cargo.toml gui/Cargo.lock /liana/

# We cache the dependencies sources in the image to avoid re-indexing everything from scratch
# at every run. It was useful when debugging the build, it could be removed eventually if we
# think the tradeoff vs the image size wasn't worth it anymore.
RUN /liana/rust-1.65.0-x86_64-unknown-linux-gnu/cargo/bin/cargo vendor

# Cargo configuration for using the vendored dependencies during the build.
COPY contrib/reproducible/docker/cargo_config.toml /liana/.cargo/cargo_config.toml

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
