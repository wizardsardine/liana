# Start using Liana

This documents how to install and start using Liana. If you just want to quickly try Liana on
Bitcoin Signet, check out [this guide](TRY.md) instead.


### Installing the software

The recommended installation method for regular users is to download [an executable software release
from our website](https://wizardsardine.com/liana/). If you prefer to build the project from source,
see [`BUILD.md`](BUILD.md) instead.

We recommend you verify the software you downloaded against a PGP signature made by Edouard Paris
using his key `5B63F3B97699C7EEF3B040B19B7F629A53E77B83`. For now the PGP signatures for the
binaries downloaded on our website are only available on the [Github release
page](https://github.com/wizardsardine/liana/releases). Find the `.asc` file in the list
corresponding to the binary you downloaded. Edouard's key is available elsewhere for cross-checking,
such as on [his personal website](https://edouard.paris).

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
you are a Linux user. Instructions [here](BUILD.md).

See [this issue](https://github.com/wizardsardine/liana/issues/414) for details.

#### Apple, Windows, codesigned and notarized binaries

We distribute both a non-codesigned and a codesigned-and-notarized MacOS application
(`Liana-noncodesigned.zip` and `Liana.zip`).  To run the non-codesigned app, see [this Apple support
guide](https://support.apple.com/en-us/HT202491) (section "If you want to open an app that hasn’t
been notarized or is from an unidentified developer").

We do not yet distribute codesigned binaries for Windows at this time.


### Starting the software

You will most likely want to use the graphical user interface. Start the software you just installed
in the previous section. This will start the "installer": a configuration wizard for your Liana
wallet.

The installer will guide you through a few steps:
- Configuring the policy for your wallet:
  - Set the key(s) which should be able to spend immediately
  - Set how many recovery path(s) you want, and set the lockup period
- Making sure your backup your descriptor and register it on your signing device
- Configuring the connection to the Bitcoin network

Once you've been through these steps, your Liana wallet will open.

You might have to wait for Bitcoin Core to perform its initial block download. When using Liana, the
connection to the Bitcoin network is established by using a full node. This means you are fully
sovereign: you are not trusting a third party to get your onchain data. This does come with a
drawback: you have to wait for Bitcoin Core to download and validate the historical block chain. But
fear not! This is just a one time cost. Also, the full node is pruned so it will not use more than
20GB of disk space.

#### Using the daemon

Liana can be run as a headless server using the `lianad` program.

As a Bitcoin wallet, Liana needs to be able to connect to the Bitcoin network,
which is currently possible through the Bitcoin Core daemon (`bitcoind`) or an Electrum server.

The chosen Bitcoin backend must be available while Liana is running.

If using `bitcoind`, it must be running on your machine for the desired network (mainnet, signet, testnet or regtest)
and may be pruned (note this may affect block chain rescans) up to the maximum (around 550MB of blocks).

The minimum supported version of Bitcoin Core is `24.0.1` (if you want to use Taproot it's `26.0`).
If you don't have Bitcoin Core installed on your machine yet, you can download it
[here](https://bitcoincore.org/en/download/).

You can use the `liana-cli` program to send commands to it. It will need the path to the same
configuration as the daemon. You can find a full documentation of the JSONRPC API exposed by
`lianad` at [`API.md`](API.md). For instance:
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

A sample configuration file is available [here](../contrib/lianad_config_example.toml). Notably you
will need to generate an output descriptor. The easiest way to achieve it is to use the Liana GUI's
installer (see above).

Note also that you might connect the GUI to a running `lianad`. If the GUI detects a daemon is
already running, it will plug to it and communicate through the JSONRPC API.


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

The list of supported devices can be found [here](./SIGNING_DEVICES.md).

#### Using the recovery path

You can sweep the coins whose timelocked recovery path is available. You will need to sign the
transaction using the recovery key(s), hence make sure to connect the appropriate signing device(s).

In the GUI, this option is available in the "Settings" menu at the "Recovery" section. Click on the
"Recover funds" button, enter the destination for the sweep and the feerate you want to use for the
sweep transaction. Then sign it with the recovery key and broadcast it.

For the daemon, see the [`createrecovery`](API.md#createrecovery) command. It will create a
sweep PSBT to the requested address with the specified feerate, filled with all available coins.

#### Recovering a Liana wallet backup on another wallet

You can always restore a Liana wallet backup using the Liana software. In the extremely unlikely
scenario that you lose access to a Liana software (all copies of the binaries and source code are
entirely wiped from the surface of the planet) or are otherwise unable to use it, we've got [a guide
on how to recover a Liana wallet backup with Bitcoin Core 25.0 and above](RECOVER.md).


### Reproducible builds

Releases of Liana are reproducibly built. Linux binaries are also bootstrappable. See
[`contrib/reproducible`](../contrib/reproducible) for details and instructions if you want to check a
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
