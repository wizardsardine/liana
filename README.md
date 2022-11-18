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

Liana is still under heavy development. Apart from the regular wallet features that are planned, we
intend to implement the possibility to have multiple keys per spending path (multisig) as well as
multiple timelocked paths (for instance for decaying multisigs). We also intend to switch to using
Taproot as soon as possible, for enhanced privacy.

**As such please consider Liana to be beta software.**


## Usage

As a Bitcoin wallet, Liana needs to be able to connect to the Bitcoin network. The software has been
developed such as multiple ways to connect to the Bitcoin network may be available. However for now
only the connection through `bitcoind` is implemented.

Therefore in order to use Liana you need to have Bitcoin Core running on your machine for the
desired network. The `bitcoind` installation may be pruned (note this may affect block chain
rescans) up to the maximum (around 550MB of blocks).

The minimum supported version of Bitcoin Core is `24.0`. If you don't have Bitcoin Core installed on
your machine yet, you can download it [there](https://bitcoincore.org/en/download/).

### Installing the software

TODO: download links

TODO: link to a longer-form document in the doc/ folder containing the different build methods.

### Setting up a wallet

If you are using the GUI, you can just start the program. It will spawn an installer that will guide
you through the process of setting up a new wallet.

If you are using the daemon, you will need to specify its configuration as a TOML file. There is a
documented example of such a configuration file in the [`contrib/` folder](contrib/lianad_config_example.toml).
Then you can start the daemon like so:
```
lianad --conf /path/to/your/conf.toml
```

#### The script descriptor

**MAKE SURE TO BACK UP YOUR DESCRIPTOR**

In Bitcoin, the conditions for spending a certain amount of coins are expressed using a
[Script](https://en.bitcoin.it/wiki/Script). In order to be able to recover your coins, you need to
back up both:
- The Script template, in the form of a standard [Output Script
  Descriptor](https://github.com/bitcoin/bips/blob/master/bip-0380.mediawiki)
- The private key corresponding to the public key used in the Script

By so doing, any software that understands the Output Script Descriptor standard will be able to
retrieve your coins. By using your private key you would then be able to sign a transaction spending
them.

But **without the descriptor you won't be able to recover from your backup**. Note however it is
simpler to have redundancy for your descriptor backup. A thief getting access to it will be able to
learn your balance (and transaction history), but **will not be able to steal your funds**.
Therefore you may afford a greater number of backups of your descriptor(s) and using less secure
mediums than for storing your private key.

### Using a wallet

You can use Liana just like a regular wallet. Just be aware that if you are using a relative
timelock (the only type of timelocks supported for now), time starts ticking when you receive a
payment. That is if you want the recovery path to never be available, all coins must be spent
at least once every `N` blocks. (With `N` the configured value of the timelock.)

For now, only the Ledger and Specter DIY signing devices are supported. We may add the possibility
to use Liana as a "hot" wallet in the future (i.e. with a private key directly on the laptop).

If you are using the GUI, it should be intuitive what menu to use depending on your intention. If it
is not, bug report are very welcome so [feel free to report it](https://github.com/revault/liana/issues)! :)

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
    "version": "0.1"
  }
}
```

Note also that you might connect the GUI to a running `lianad`. If the GUI detects a daemon is
already running, it will plug to it and communicate through the JSONRPC API.


### Wallet recovery

TODO: have a longer form document on recovery through the recovery path.


## About the software project

Liana is an open source project. It is [hosted at Github](https://github.com/revault/liana).

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
