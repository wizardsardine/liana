# Liana GUI

The Liana graphical interface.

## Dependencies

You will need a few dependencies in order to run correctly this software. For Linux systems, those
are:
- [`fontconfig`](https://www.freedesktop.org/wiki/Software/fontconfig/) for access to fonts (On Debian/Ubuntu `apt install libfontconfig1-dev`)
- [`libudev-dev`](https://www.freedesktop.org/software/systemd/man/libudev.html) to communicate with devices through USB (On Debian/Ubuntu `apt install libudev-dev`)

In addition, if you want to build the project from source, you will need:
- [`pkg-config`](https://www.freedesktop.org/wiki/Software/pkg-config/) (On Debian/Ubuntu `apt install pkg-config`)


## Common usage

*For a quick guide to try out the software see [../doc/TRY.md](../doc/TRY.md).*

```
liana-gui --datadir <datadir> --<network>
```

The default `datadir` is the same as for `lianad` (`~/.liana` for Linux). The default network is
Bitcoin mainnet, but testnet signet and regtest are supported.

If the software is started with no parameter and no data directory is detected, a Liana installer
will be spawned that will guide you in the processing of configuring Liana.

## Connecting to an external Liana daemon

By setting in the `liana-gui` configuration file the `daemon_rpc_path`
field. The GUI will connect to the distant daemon via `lianad`'s
JSONRPC interface. In case of failure, the GUI will display an error
message.

### Troubleshooting

- If you encounter layout issue on `X11`, try to start the GUI with `WINIT_X11_SCALE_FACTOR`
  manually set to 1
