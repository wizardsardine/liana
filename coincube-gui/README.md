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
