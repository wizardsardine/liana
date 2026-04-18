# Coincube GUI

The Coincube graphical interface.

## Dependencies

You will need a few dependencies in order to run correctly this software. For Linux systems, those
are:

- [`fontconfig`](https://www.freedesktop.org/wiki/Software/fontconfig/) for access to fonts (On Debian/Ubuntu `apt install libfontconfig1-dev`)
- [`libudev-dev`](https://www.freedesktop.org/software/systemd/man/libudev.html) to communicate with devices through USB (On Debian/Ubuntu `apt install libudev-dev`)
- [`libwebkit2gtk-4.1-dev`](https://webkitgtk.org/) for web content rendering (On Debian/Ubuntu `apt install libwebkit2gtk-4.1-dev`)

In addition, if you want to build the project from source, you will need:

- [`pkg-config`](https://www.freedesktop.org/wiki/Software/pkg-config/) (On Debian/Ubuntu `apt install pkg-config`)
- [`protobuf-compiler`](https://developers.google.com/protocol-buffers/docs/reference/cpp-generated) for protobuf compilation (On Debian/Ubuntu `apt install protobuf-compiler`)

## Development

```bash
cargo run --bin coincube
```

Note: this builds and runs the gui alone. It does NOT build the
[`coincube-spark-bridge`](../coincube-spark-bridge) sibling process,
which is required for the Spark wallet to load. Without it the gui
logs "Spark bridge unavailable" and renders "Spark is not configured"
on the Spark panels. For Spark-enabled development, use the top-level
[`Makefile`](../Makefile) instead:

```bash
make run        # builds the bridge, then runs the gui
make build      # builds both without running
```

Or build the bridge manually once:

```bash
cargo build --manifest-path ../coincube-spark-bridge/Cargo.toml
```

### Spark bridge

The Spark wallet is powered by [`breez-sdk-spark`](https://github.com/breez/spark-sdk),
which cannot share a dependency graph with `breez-sdk-liquid` (incompatible
`tokio_with_wasm` and `rusqlite` requirements). The SDK therefore runs in a
sibling subprocess — [`coincube-spark-bridge`](../coincube-spark-bridge) — that
the gui spawns on Cube open and speaks to over stdin/stdout JSON-RPC. The
bridge lives in its own Cargo workspace so its dep graph stays isolated.

At runtime the gui locates the bridge binary in this order:

1. `$COINCUBE_SPARK_BRIDGE_PATH` (explicit override)
2. Next to the gui executable (packaged builds)
3. `../coincube-spark-bridge/target/{debug,release}/coincube-spark-bridge` (dev fallback)

Spark support is currently limited to Bitcoin mainnet and Regtest; on other
networks the bridge is skipped and the Spark panels stay disconnected.

## Regtest
In order to test out the "Liquid" wallet which utilizes the Breez Liquid SDK, please follow these directions [Breez SDK Regtest Setup](/docs/BREEZ_SDK_REGTEST.md)

## Usage

_For a quick guide to try out the software see [../doc/TRY.md](../doc/TRY.md)._

```bash
coincube-gui --datadir <datadir> --<network>
```

The default `datadir` is the same as for `coincubed` (`~/.coincube` for Linux). The default network is
Bitcoin mainnet, but testnet signet and regtest are supported.

If the software is started with no parameter and no data directory is detected, a Coincube installer
will be spawned that will guide you in the processing of configuring Coincube.

If the software is started and a reachable `coincubed` is running, it will plug to it via `coincubed`'s
JSONRPC interface.

The environment variable `LOG_LEVEL` with values `error`, `warn`, `info`, `debug`, `trace`, overrides the log settings from the config file.

### Troubleshooting

- If you encounter layout issue on `X11`, try to start the GUI with `WINIT_X11_SCALE_FACTOR`
  manually set to 1
