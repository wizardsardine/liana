<div align="center">
  <br><br>
  <a href="https://coincube.io" target="_blank">
    <img src="coincube-ui/static/logos/coincube-logo.svg" width="440px" />
  </a>

_Secure Bitcoin wallet with Vault multi-sig, Liquid spending, Buy/Sell and inheritance_.

</div>

[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/coincubetech/coincube)

## About

COINCUBE is a comprehensive Bitcoin wallet solution built on a hard fork of [Liana](https://github.com/wizardsardine/liana) with significant modifications by [Coincube Technology LLC](https://coincube.io). We retain the original license and acknowledge the foundational work of the Liana project. This wallet provides:

**VAULT** - Secure multisig custody with time-locked inheritance paths. Like the original Liana, you have
a primary spending path (always accessible) and recovery paths (available after inactivity periods).
You may have multiple keys in either path. Recovery paths are enforced onchain by Bitcoin's Script
capabilities.

Example VAULT configuration:

- Owner's key (can always spend)
- Any 2 keys from trusted parties (after 1 year)
- Emergency recovery key (after 1 year and 3 months)

**LIQUID WALLET** - Lightning-enabled spending wallet powered by Breez SDK for instant, low-fee payments. [Liquid](https://liquid.net/) is a sidechain of Bitcoin that provides low cost payments and confidential transactions.

**SPARK WALLET** - Second Lightning-enabled wallet powered by the Breez Spark SDK, running alongside the Liquid wallet. Both wallets derive from the same master seed, so one mnemonic and one PIN cover everything. Spark support is currently limited to Bitcoin mainnet and Regtest.

**BUY/SELL** - Integrated Bitcoin on/off-ramp functionality.

COINCUBE is designed for **trustless inheritance**, **loss protection**, **liquid spending**, and
**user-friendly Bitcoin custody**.

## Usage

COINCUBE is available on Windows, Mac and Linux. To install and start using it see
[`doc/USAGE.md`](doc/USAGE.md).

If you just want to quickly try out COINCUBE on Bitcoin Signet or Testnet, see [`doc/TRY.md`](doc/TRY.md).

## Hacking on COINCUBE

COINCUBE is an open source project. It is [hosted at Github](https://github.com/coincubetech/coincube).
Contributions are welcome. See [here](CONTRIBUTING.md) for guidelines.

COINCUBE is separated in main components:

#### coincubed (Vault Daemon)

The daemon contains the core Vault logic. It is both a library (Rust crate) that exposes a
command interface and a standalone daemon that exposes a JSONRPC API through a Unix Domain Socket.

The code for the daemon can be found in the [`coincubed`](coincubed) folder (to be renamed to `coincubed`).

#### COINCUBE GUI

The GUI provides an installer for setting up COINCUBE and a graphical interface
built with the [`iced`](https://github.com/iced-rs/iced/) library.

The code for the GUI can be found in the [`coincube-gui`](coincube-gui) folder.

#### Liquid Wallet

Lightning-enabled spending wallet integration with Breez SDK (under development).

#### Spark Wallet

A second Lightning-enabled wallet via the Breez Spark SDK. The SDK runs in a sibling subprocess — [`coincube-spark-bridge`](coincube-spark-bridge) — that the gui spawns on startup and communicates with over stdin/stdout JSON-RPC. The bridge lives in its own Cargo workspace because `breez-sdk-spark` and `breez-sdk-liquid` cannot share a dependency graph; see the `exclude = ["coincube-spark-bridge"]` entry in the root [`Cargo.toml`](Cargo.toml) for the paired side of this setup.

The practical consequence is that `cargo run -p coincube-gui` on its own does NOT build the bridge, and without the bridge binary the gui renders "Spark is not configured for this cube" even on a freshly-created Cube. Build both halves together via the [`Makefile`](Makefile):

```bash
make build        # builds bridge + gui (debug)
make run          # builds bridge, then runs the gui
make release      # release build of both
```

Or invoke cargo directly for just the bridge:

```bash
cargo build --manifest-path coincube-spark-bridge/Cargo.toml
```

At runtime the gui looks for the bridge binary next to the gui executable, then falls back to `coincube-spark-bridge/target/{debug,release}/coincube-spark-bridge`. A custom location can be provided via the `COINCUBE_SPARK_BRIDGE_PATH` environment variable.

## Security

See [`SECURITY.md`](SECURITY.md) for details about reporting a security vulnerability or any bug
that could potentially impact the security of users' funds.

## License

COINCUBE is a hard fork of Liana, originally developed by Wizardsardine.

Released under the BSD 3-Clause Licence. See the [LICENCE](LICENCE) file.
