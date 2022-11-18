# liana GUI

Liana GUI is an user graphical interface written in rust for the 
[Liana daemon](https://github.com/revault/liana).

## Dependencies

- `fontconfig` (On Debian/Ubuntu `apt install libfontconfig1-dev`)
- [`pkg-config`](https://www.freedesktop.org/wiki/Software/pkg-config/) (On Debian/Ubuntu `apt install pkg-config`)
- [`libxkbcommon`](https://xkbcommon.org/) for the dummy signer (On Debian/Ubuntu `apt install libxkbcommon-dev`)
- Vulkan drivers (On Debian/Ubuntu `apt install mesa-vulkan-drivers libvulkan-dev`)
- `libudev-dev` (On Debian/Ubuntu `apt install libudev-dev`)

We are striving to remove dependencies, especially the 3D ones.

## Usage

`liana-gui --datadir <datadir> --<network>`

The default `datadir` is the default `lianad` `datadir` (`~/.liana`
for linux) and the default `network` is the bitcoin mainnet.

If no argument is provided, the GUI checks in the default `datadir` 
the configuration file for the bitcoin mainnet.

If the provided `datadir` is empty or does not have the configuration
file for the targeted `network`, the GUI starts with the installer mode.

Instead of using `--datadir` and `--<network>`, a direct path to
the GUI configuration file can be provided with `--conf`.

After start up, The GUI will connect to the running lianad.
A command starting lianad is launched if no connection is made.

## Troubleshooting

- If you encounter layout issue on `X11`, try to start the GUI with
  `WINIT_X11_SCALE_FACTOR` manually set to 1
