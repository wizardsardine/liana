# Liana

*The missing safety net for your bitcoins*.


## About

Liana is a simple Bitcoin wallet that features a timelocked recovery path for all your coins. That
is, your coins are spendable as with a regular wallet but a secondary key becomes available after a
configurable period of time should the primary one not be accessible anymore.

Liana can be used for inheritance, where the owner of the coins is holding the primary key and the
heir the secondary one. It can also be leveraged for recovery where a single person is holding both
but different tradeoffs can be made between the backup(s) of the directly accessible and timelocked
keys.

Learn more about Liana from our [announcement blog
post](https://wizardsardine.com/blog/liana-announcement/) and about how it was enhanced with
Multisig from our [second release post](https://wizardsardine.com/blog/liana-0.2-release/).

Liana is still under heavy development. Multisig support was implemented in the second release.
Regular wallet features are also planned. In addition we intend to implement the possibility to have
multiple timelocked paths (for instance for more powerful decaying multisigs). We also intend to
switch to using Taproot as soon as possible, for enhanced privacy.

**As such please consider Liana to be beta software.**


## Usage

TL;DR: if you just want to quickly try Liana on Bitcoin Signet, check out [the guide](doc/TRY.md)!

As a Bitcoin wallet, Liana needs to be able to connect to the Bitcoin network. The software has been
developed such as multiple ways to connect to the Bitcoin network may be available. However for now
only the connection through `bitcoind` is implemented.

Therefore in order to use Liana you need to have the Bitcoin Core daemon (`bitcoind`) running on your machine for the
desired network (mainnet, signet, testnet or regtest). The `bitcoind` installation may be pruned (note this may affect block chain
rescans) up to the maximum (around 550MB of blocks).

The minimum supported version of Bitcoin Core is `24.0.1`. If you don't have Bitcoin Core installed on
your machine yet, you can download it [there](https://bitcoincore.org/en/download/).

### Installing the software

The recommended installation method for regular users is to download an executable software release. If you prefer to
build the project from source, see [`doc/BUILD.md`](doc/BUILD.md) instead.

Head to the [release page](https://github.com/wizardsardine/liana/releases) and download the right
executable for your platform. If you are not sure what is the "right" executable for your platform,
choose `liana-0.2.exe` if you are on Windows, `liana-0.2.dmg` if you are on MacOS and
`liana-0.2-x86_64-linux-gnu.tar.gz` if you are on Linux.

For every file available on the release page, there is an accompanying `.asc` file with the same
name. This is a GPG signature made with Antoine Poinsot's key:
`590B7292695AFFA5B672CBB2E13FC145CD3F4304`. This key is available elsewhere for cross-checking, such
as on [his Twitter profile](https://twitter.com/darosior) or his [personal
website](http://download.darosior.ninja/darosior.pub). It is recommended you verify your download
against this key.

Note that we do not codesign ("notarize") the released binaries for now. Windows or macOS may
prevent you from installing the software. On macOS, you would get a warning saying the developer of
this application couldn't be verified. This is because we didn't register with Apple prior to
releasing the application. Make sure you verified the GPG signature of the download, then add an
exception for Liana by following the steps from [this Apple support
guide](https://support.apple.com/en-us/HT202491) (section "If you want to open an app that hasn’t
been notarized or is from an unidentified developer").

Releases of Liana are reproducibly built. See [`contrib/reproducible`](contrib/reproducible) for
details and instructions if you want to check a release.

### Setting up a wallet

If you are using the graphical user interface (GUI), you can just start the program. It will spawn an installer that will guide
you through the process of setting up a new wallet.

If you are using the daemon, you will need to specify its configuration as a TOML file. There is a
documented example of such a configuration file in the [`contrib/` folder](contrib/lianad_config_example.toml).
Then you can start the daemon like so:
```
lianad --conf /path/to/your/conf.toml
```
#### The script descriptor

In Bitcoin, the conditions for spending a certain amount of coins are expressed using
[Script](https://en.bitcoin.it/wiki/Script). In order to be able to recover your coins, you need to
back up both:
- The Script template, in the form of a standard [Output Script
  Descriptor](https://github.com/bitcoin/bips/blob/master/bip-0380.mediawiki)
- The private key(s) corresponding to the public key(s) used in the Script

By so doing, any software that understands the Output Script Descriptor standard will be able to
retrieve your coins. By using your private key(s) you would then be able to sign a transaction spending
them.

But **without the descriptor you won't be able to recover from your backup**. Note however it is
simpler to have redundancy for your descriptor backup. A thief getting access to it would be able to
learn your balance (and transaction history), but **would not be able to steal your funds**.
Therefore you may afford a greater number of backups of your descriptor(s) and using less secure
mediums than for storing your private key(s).


### Using a wallet

You can use Liana just like a regular wallet. Just be aware that if you are using a relative
timelock (the only type of timelocks supported for now), time starts ticking when you receive a
payment. That is if you want the recovery path to never be available, each coin must be spent
at least once every `N` blocks. (With `N` the configured value of the timelock.)

For now, only the Ledger and Specter DIY signing devices are supported, as Miniscript compatibility
of the signer is a must. We expect more signing devices to implement Miniscript capability. We may
add the possibility to use Liana as a "hot" wallet in the future (i.e. with a private key directly
on the laptop). For more information, please read the
[signing devices documentation](./doc/signing_devices.md).

If you are using a Ledger device, make sure to install the currently latest version of the Bitcoin
application: `2.1.0`. This is the minimum supported version, as it's the first one to introduce
support for Miniscript.

If you are using the GUI, it should be intuitive what menu to use depending on your intention. If it
is not, bug report are very welcome so [feel free to report it](https://github.com/wizardsardine/liana/issues)! :)

If you are using the daemon, you can use the `liana-cli` binary to send commands to it. It will need
the path to the same configuration as the daemon. You can find a full documentation of the JSONRPC
API exposed by `lianad` at [`doc/API.md`](doc/API.md). For instance:
```
$ liana-cli --conf ./testnet_config.toml getinfo
{
  "result": {
    "blockheight": 2406973,
    "descriptors": {
      "main": {
        "change_desc": "wsh(or_d(pk([92162c45]tpubD6NzVbkrYhZ4WzTf9SsD6h7AH7oQEippXK2KP8qvhMMqFoNeN5YFVi7vRyeRSDGtgd2bPyMxUNmHui8t5yCgszxPPxMafu1VVzDpg9aruYW/1/*),and_v(v:pkh(tpubD6NzVbkrYhZ4Wdgu2yfdmrce5g4fiH1ZLmKhewsnNKupbi4sxjH1ZVAorkBLWSkhsjhg8kiq8C4BrBjMy3SjAKDyDdbuvUa1ToAHbiR98js/1/*),older(2))))#5rx53ql7",
        "multi_desc": "wsh(or_d(pk([92162c45]tpubD6NzVbkrYhZ4WzTf9SsD6h7AH7oQEippXK2KP8qvhMMqFoNeN5YFVi7vRyeRSDGtgd2bPyMxUNmHui8t5yCgszxPPxMafu1VVzDpg9aruYW/<0;1>/*),and_v(v:pkh(tpubD6NzVbkrYhZ4Wdgu2yfdmrce5g4fiH1ZLmKhewsnNKupbi4sxjH1ZVAorkBLWSkhsjhg8kiq8C4BrBjMy3SjAKDyDdbuvUa1ToAHbiR98js/<0;1>/*),older(2))))#uact7s3g",
        "receive_desc": "wsh(or_d(pk([92162c45]tpubD6NzVbkrYhZ4WzTf9SsD6h7AH7oQEippXK2KP8qvhMMqFoNeN5YFVi7vRyeRSDGtgd2bPyMxUNmHui8t5yCgszxPPxMafu1VVzDpg9aruYW/0/*),and_v(v:pkh(tpubD6NzVbkrYhZ4Wdgu2yfdmrce5g4fiH1ZLmKhewsnNKupbi4sxjH1ZVAorkBLWSkhsjhg8kiq8C4BrBjMy3SjAKDyDdbuvUa1ToAHbiR98js/0/*),older(2))))#d693mvvd"
      }
    },
    "network": "testnet",
    "rescan_progress": null,
    "sync": 1.0,
    "version": "0.2"
  }
}
```

Note also that you might connect the GUI to a running `lianad`. If the GUI detects a daemon is
already running, it will plug to it and communicate through the JSONRPC API.


### Using the recovery path

You may sweep the coins whose timelocked recovery path is available. You will need to sign the
transaction using the recovery key, hence make sure to connect the appropriate signing device.

In the GUI, this option is available in the "Settings" menu at the "Recovery" section. Click on the
"Recover funds" button, enter the destination for the sweep and the feerate you want to use for the
sweep transaction. Then sign it with the recovery key and broadcast it.

For the daemon, see the [`createrecovery`](doc/API.md#createrecovery) command. It will create a
sweep PSBT to the requested address with the specified feerate, filled with all available coins.


## About the software project

Liana is an open source project. It is [hosted at Github](https://github.com/wizardsardine/liana).

Contributions are very welcome. For guidelines, see [CONTRIBUTING.md](CONTRIBUTING.md).

Liana is separated in two main components: the daemon and the Graphical User Interface.

### Liana daemon

The daemon contains the core logic of the wallet. It is both a library (a Rust crate) that exposes a
command interface and a standalone UNIX daemon that exposes a JSONRPC API through a Unix Domain
Socket.

The code for the daemon can be found in the [`src/`](src/) folder at the root of this repository.

### Liana GUI

The GUI contains both an installer that guides a user through setting up a Liana wallet, as well as
a graphical interface to the daemon using the [`iced`](https://github.com/iced-rs/iced/) library.

The code for the GUI can be found in the [`gui/src/`](gui/src) folder.

## License

Released under the BSD 3-Clause Licence. See the [LICENCE](LICENCE) file.
