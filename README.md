<div align="center">
  <a href="https://wizardsardine.com/liana" target="_blank">
    <img src="gui/ui/static/logos/liana-app-icon.svg" width="140px" />
  </a>

# Liana

*The missing safety net for your bitcoins*.

</div>


## About

Liana is a simple Bitcoin wallet that features a timelocked recovery path for all your coins. That
is, your coins are spendable as with a regular wallet but a secondary key becomes available after a
configurable period of time should the primary one not be accessible anymore.

Liana can be used for inheritance, decaying multisigs or safer backups.

**[https://wizardsardine.com/liana](https://wizardsardine.com/liana)**

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

The recommended installation method for regular users is to download [an executable software
release](https://github.com/wizardsardine/liana/releases). If you prefer to build the project from
source, see [`doc/BUILD.md`](doc/BUILD.md) instead.

Head to the [release page](https://github.com/wizardsardine/liana/releases) and download the right
executable for your platform. If you are not sure what is the "right" executable for your platform,
choose:
- `liana-1.1.exe` if you are on Windows,
- `liana_1.1-1_amd64.deb` if you are running a Debian-based Linux (such as Ubuntu). Use `apt install
  ./liana_1.1-1_amd64.deb` as root (or preceded by `sudo`) to install it.
- `liana-1.1-x86_64-linux-gnu.tar.gz` if you use another Linux distribution. Note you may have to
  compile the software yourself if you are on Linux. See the [section
  below](#a-note-on-linux-binaries-and-glibc-version).

For every file available on the release page, there is an accompanying `.asc` file with the same
name. This is a GPG signature made with Antoine Poinsot's key:
`590B7292695AFFA5B672CBB2E13FC145CD3F4304`. This key is available elsewhere for cross-checking, such
as on [his Twitter profile](https://twitter.com/darosior) or his [personal
website](http://download.darosior.ninja/darosior.pub). It is recommended you verify your download
against this key.

For Arch users, a `liana-bin` is also available at the [AUR](https://aur.archlinux.org/). You can
install it using your favourite wrapper (eg `paru -S liana-bin` or `yay -S liana-bin`), or manually:
```bash
git clone https://aur.archlinux.org/liana-bin.git
cd liana-bin
cat PKGBUILD # Review the PKGBUILD script
makepkg -si
```

#### A note on Linux binaries and glibc version

*Skip this section if you are not running Linux or don't plan on using a released binary.*

Due to technical limitations in our reproducible builds system, the Linux binaries currently link
against `glibc` version `2.33`. This means you can't run a released Linux binary if your system has
an older glibc. This is the case most notably of Ubuntu 20 (Focal) and below, and Debian 11
(Bullseye) and below.

The simplest workaround is to simply build the project yourself. Fear not, it's really trivial if
you are a Linux user. Instructions [here](doc/BUILD.md).

See [this issue](https://github.com/wizardsardine/liana/issues/414) for details.


#### Apple, Windows, codesigned and notarized binaries

We distribute both a non-codesigned and a codesigned-and-notarized MacOS application
(`Liana-noncodesigned.zip` and `Liana.zip`).  To run the non-codesigned app, see [this Apple support
guide](https://support.apple.com/en-us/HT202491) (section "If you want to open an app that hasn’t
been notarized or is from an unidentified developer").

We do not yet distribute codesigned binaries for Windows at this time.


### Wallet usage tips and tricks

#### Script descriptor backup

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


#### On refreshing coins

You can use Liana just like a regular wallet. Simply be aware that if you are using a relative
timelock (the only type of timelocks supported for now), time starts ticking when you receive a
payment. That is if you want the recovery path to never be available, each coin must be spent
at least once every `N` blocks. (With `N` the configured value of the timelock.)

The GUI provide simple shortcuts to refresh one or more coin(s) if the recovery path is close to
become available. This is achieved by making a transaction to yourself (if you don't need to make a
payment.)


#### Signing devices and "hot" keys

Liana can be used as a hot wallet. Note that mnemonics would be stored in clear on your drive. We
strongly recommend using a hardware signing device for any non-trivial amount.

For now, only the Ledger and Specter DIY signing devices are supported, as Miniscript compatibility
of the signer is a must. More signing devices are expected to implement Miniscript capability in the
near future. For more information (such as minimum supported versions, please read the [signing
devices documentation](./doc/signing_devices.md).


#### Using the daemon

Liana can be run as a headless server using the `lianad` program.

You can use the `liana-cli` program to send commands to it. It will need the path to the same
configuration as the daemon. You can find a full documentation of the JSONRPC API exposed by
`lianad` at [`doc/API.md`](doc/API.md). For instance:
```
$ liana-cli --conf ./signet_config.toml getinfo
{
  "result": {
    "block_height": 3083,
    "descriptors": {
      "main": {
        "change_desc": "wsh(or_i(and_v(v:thresh(1,pkh([b883f127/48'/1'/2'/2']tpubDEP7MLK6TGe1EWhKGpMWdQQCvMmS6pRjCyN7PW24afniPJYdfeMMUb2fau3xTku6EPgA68oGuR4hSCTUpu2bqaoYrLn2UmhkytXXSzxcaqt/1/*),a:pkh([636adf3f/48'/1'/2'/2']tpubDFnPUtXZhnftEFD5vg4LfVoApf5ZVB8Nkrf8CNe9pT9j1EEPXssJnMgAjmvbTChHugnkfVfsmGafFnE6gwoifJNybSasAJ316dRpsP86EFb/1/*),a:pkh([b883f127/48'/1'/3'/2']tpubDFPMBua4idthySDayX1GxgXgPbpaEVfU7GwMc1HAfneknhqov5syrNuq4NVdSVWa2mPVP3BD6f2pGB98pMsbnVvWqrxcLgwv9PbEWyLJ6cW/1/*)),older(20)),or_i(and_v(v:pkh([636adf3f/48'/1'/1'/2']tpubDDvF2khuoBBj8vcSjQfa7iKaxsQZE7YjJ7cJL8A8eaneadMPKbHSpoSr4JD1F5LUvWD82HCxdtSppGfrMUmiNbFxrA2EHEVLnrdCFNFe75D/1/*),older(19)),or_d(multi(2,[636adf3f/48'/1'/0'/2']tpubDEE9FvWbG4kg4gxDNrALgrWLiHwNMXNs8hk6nXNPw4VHKot16xd2251vwi2M6nsyQTkak5FJNHVHkCcuzmvpSbWHdumX3DxpDm89iTfSBaL/1/*,[b883f127/48'/1'/0'/2']tpubDET11c81MZjJvsqBikGXfn1YUzXofoYQ4HkueCrH7kE94MYkdyBvGzyikBd2KrcBAFZWDB6nLmTa8sJ381rWSQj8qFvqiidxqn6aQv1wrJw/1/*),and_v(v:pkh([b883f127/48'/1'/1'/2']tpubDEA6SKh5epTZXebgZtcNxpLj6CeZ9UhgHGoGArACFE7QHCgx76vwkzJMP5wQ9yYEc6g9qSGW8EVzn4PhRxiFz1RUvAXBg7txFnvZFv62uFL/1/*),older(18))))))#056xvvp3",
        "multi_desc": "wsh(or_i(and_v(v:thresh(1,pkh([b883f127/48'/1'/2'/2']tpubDEP7MLK6TGe1EWhKGpMWdQQCvMmS6pRjCyN7PW24afniPJYdfeMMUb2fau3xTku6EPgA68oGuR4hSCTUpu2bqaoYrLn2UmhkytXXSzxcaqt/<0;1>/*),a:pkh([636adf3f/48'/1'/2'/2']tpubDFnPUtXZhnftEFD5vg4LfVoApf5ZVB8Nkrf8CNe9pT9j1EEPXssJnMgAjmvbTChHugnkfVfsmGafFnE6gwoifJNybSasAJ316dRpsP86EFb/<0;1>/*),a:pkh([b883f127/48'/1'/3'/2']tpubDFPMBua4idthySDayX1GxgXgPbpaEVfU7GwMc1HAfneknhqov5syrNuq4NVdSVWa2mPVP3BD6f2pGB98pMsbnVvWqrxcLgwv9PbEWyLJ6cW/<0;1>/*)),older(20)),or_i(and_v(v:pkh([636adf3f/48'/1'/1'/2']tpubDDvF2khuoBBj8vcSjQfa7iKaxsQZE7YjJ7cJL8A8eaneadMPKbHSpoSr4JD1F5LUvWD82HCxdtSppGfrMUmiNbFxrA2EHEVLnrdCFNFe75D/<0;1>/*),older(19)),or_d(multi(2,[636adf3f/48'/1'/0'/2']tpubDEE9FvWbG4kg4gxDNrALgrWLiHwNMXNs8hk6nXNPw4VHKot16xd2251vwi2M6nsyQTkak5FJNHVHkCcuzmvpSbWHdumX3DxpDm89iTfSBaL/<0;1>/*,[b883f127/48'/1'/0'/2']tpubDET11c81MZjJvsqBikGXfn1YUzXofoYQ4HkueCrH7kE94MYkdyBvGzyikBd2KrcBAFZWDB6nLmTa8sJ381rWSQj8qFvqiidxqn6aQv1wrJw/<0;1>/*),and_v(v:pkh([b883f127/48'/1'/1'/2']tpubDEA6SKh5epTZXebgZtcNxpLj6CeZ9UhgHGoGArACFE7QHCgx76vwkzJMP5wQ9yYEc6g9qSGW8EVzn4PhRxiFz1RUvAXBg7txFnvZFv62uFL/<0;1>/*),older(18))))))#yl5jehy9",
        "receive_desc": "wsh(or_i(and_v(v:thresh(1,pkh([b883f127/48'/1'/2'/2']tpubDEP7MLK6TGe1EWhKGpMWdQQCvMmS6pRjCyN7PW24afniPJYdfeMMUb2fau3xTku6EPgA68oGuR4hSCTUpu2bqaoYrLn2UmhkytXXSzxcaqt/0/*),a:pkh([636adf3f/48'/1'/2'/2']tpubDFnPUtXZhnftEFD5vg4LfVoApf5ZVB8Nkrf8CNe9pT9j1EEPXssJnMgAjmvbTChHugnkfVfsmGafFnE6gwoifJNybSasAJ316dRpsP86EFb/0/*),a:pkh([b883f127/48'/1'/3'/2']tpubDFPMBua4idthySDayX1GxgXgPbpaEVfU7GwMc1HAfneknhqov5syrNuq4NVdSVWa2mPVP3BD6f2pGB98pMsbnVvWqrxcLgwv9PbEWyLJ6cW/0/*)),older(20)),or_i(and_v(v:pkh([636adf3f/48'/1'/1'/2']tpubDDvF2khuoBBj8vcSjQfa7iKaxsQZE7YjJ7cJL8A8eaneadMPKbHSpoSr4JD1F5LUvWD82HCxdtSppGfrMUmiNbFxrA2EHEVLnrdCFNFe75D/0/*),older(19)),or_d(multi(2,[636adf3f/48'/1'/0'/2']tpubDEE9FvWbG4kg4gxDNrALgrWLiHwNMXNs8hk6nXNPw4VHKot16xd2251vwi2M6nsyQTkak5FJNHVHkCcuzmvpSbWHdumX3DxpDm89iTfSBaL/0/*,[b883f127/48'/1'/0'/2']tpubDET11c81MZjJvsqBikGXfn1YUzXofoYQ4HkueCrH7kE94MYkdyBvGzyikBd2KrcBAFZWDB6nLmTa8sJ381rWSQj8qFvqiidxqn6aQv1wrJw/0/*),and_v(v:pkh([b883f127/48'/1'/1'/2']tpubDEA6SKh5epTZXebgZtcNxpLj6CeZ9UhgHGoGArACFE7QHCgx76vwkzJMP5wQ9yYEc6g9qSGW8EVzn4PhRxiFz1RUvAXBg7txFnvZFv62uFL/0/*),older(18))))))#v3g9rzum"
      }
    },
    "network": "regtest",
    "rescan_progress": null,
    "sync": 1.0,
    "version": "1.0.0"
  }
}
```

Note also that you might connect the GUI to a running `lianad`. If the GUI detects a daemon is
already running, it will plug to it and communicate through the JSONRPC API.


#### Using the recovery path

You may sweep the coins whose timelocked recovery path is available. You will need to sign the
transaction using the recovery key(s), hence make sure to connect the appropriate signing device(s).

In the GUI, this option is available in the "Settings" menu at the "Recovery" section. Click on the
"Recover funds" button, enter the destination for the sweep and the feerate you want to use for the
sweep transaction. Then sign it with the recovery key and broadcast it.

For the daemon, see the [`createrecovery`](doc/API.md#createrecovery) command. It will create a
sweep PSBT to the requested address with the specified feerate, filled with all available coins.


### Reproducible builds

Releases of Liana are reproducibly built. Linux binaries are also bootstrappable. See
[`contrib/reproducible`](contrib/reproducible) for details and instructions if you want to check a
release.

All commits on master are merge commits signed using a set of trusted GPG keys. We use the
[`github-merge`](https://github.com/wizardsardine/maintainer-tools) script for this purpose. Given a
set of [trusted
keys](https://github.com/wizardsardine/maintainer-tools/blob/master/verify-commits/liana/trusted-keys)
(basically mine and Edouard Paris') and a [trusted git root
commit](https://github.com/wizardsardine/maintainer-tools/blob/master/verify-commits/liana/trusted-git-root)
you can verify the integrity of the master branch using the
[`verify-commits`](https://github.com/wizardsardine/maintainer-tools/tree/master/verify-commits)
script from our
[`maintainer-tools`](https://github.com/wizardsardine/maintainer-tools) repository. For instance:
```
$ ../maintainer-tools/verify-commits/verify-commits.py liana
...
There is a valid path from "9490159e7ca69678bb6995cd56d09b0a65a5b484" to da9149ccde5bf99cb70769b792fd003b079fc9ed where all commits are signed!
```
It's worth mentioning we didn't invent anything here: we're just reusing the [great
tooling](https://github.com/bitcoin-core/bitcoin-maintainer-tools) developed by the Bitcoin Core
contributors over the years.

Note you necessarily won't be able to reproduce codesigned binaries. We may provide detached
signatures in the future.


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

## Security

See [`SECURITY.md`](SECURITY.md) for details about reporting a security vulnerability or any bug
that could potentially impact the security of users' funds.

## License

Released under the BSD 3-Clause Licence. See the [LICENCE](LICENCE) file.
