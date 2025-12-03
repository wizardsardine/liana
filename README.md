<div align="center">
  <br><br>
  <a href="https://coincube.io" target="_blank">
    <img src="coincube-ui/static/logos/coincube-logo.svg" width="440px" />
  </a>

_Secure Bitcoin wallet with Vault, Active spending, and inheritance_.

</div>

## About

COINCUBE is a comprehensive Bitcoin wallet solution built on a hard fork of [Liana](https://github.com/wizardsardine/liana) with significant modifications by [Coincube Technology LLC](https://coincube.io). We retain the original license and acknowledge the foundational work of the Liana project. This wallet provides:

**VAULT** - Secure multisig custody with time-locked inheritance paths. Like the original Liana, you have
a primary spending path (always accessible) and recovery paths (available after inactivity periods).
You may have multiple keys in either path. Recovery paths are enforced onchain by Bitcoin's Script
capabilities.

**ACTIVE Wallet** - Lightning-enabled spending wallet powered by Breez SDK for instant, low-fee payments.

**BUY/SELL** - Integrated Bitcoin on/off-ramp functionality.

**KEYCHAIN** - Remote keychain-based signers for enhanced flexibility.

Example VAULT configuration:

- Owner's key (can always spend)
- Any 2 keys from trusted parties (after 1 year)
- Emergency recovery key (after 1 year and 3 months)

COINCUBE is designed for **trustless inheritance**, **loss protection**, **active spending**, and
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

#### Active Wallet

Lightning-enabled spending wallet integration with Breez SDK (under development).

## Security

See [`SECURITY.md`](SECURITY.md) for details about reporting a security vulnerability or any bug
that could potentially impact the security of users' funds.

## License

COINCUBE is a hard fork of Liana, originally developed by Wizardsardine.

Released under the BSD 3-Clause Licence. See the [LICENCE](LICENCE) file.
