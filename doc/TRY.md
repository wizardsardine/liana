# Quickly try out Liana

> *Just give me the TL;DR!*

*(Updated on February the 28th of 2023)*

This document is a short set of instructions for trying out Liana on Bitcoin signet. It does not attempt to
give any nuance, details or describe alternative configurations.

This guide will make use Liana as a "hot wallet". If you'd like to try out Liana using dummy
hardware signing device you can use the [Specter
simulator](https://github.com/cryptoadvance/specter-diy/blob/master/docs/simulator.md) or the
[Ledger "Speculos" emulator](https://github.com/LedgerHQ/speculos).


## Step 0: preparation

### System dependencies

Here is a list of the system dependencies: the tools and libraries you need to have installed on
your system to follow the guide if you are running Linux.

TL;DR:
- Debian/Ubuntu: `apt install curl gpg udev libfontconfig1-dev libudev-dev`
- Arch Linux: check if you have all the required packages: `pacman -Q coreutils tar curl gnupg fontconfig systemd-libs`.
If any is listed as "was not found", get it with `pacman -S [missing package name]`
- Other distribution: see the link to projects below to search for the name of your distribution's packages.
- Running binaries requires GLIBC >= 2.33 (Ubuntu >= 22.04 or Debian >= 12)

We'll use basic tools which should already be present on your system, such as:
- `shasum`
- `tar`

To download and verify binaries you will also need:
- `curl` (On Debian/Ubuntu `apt install curl`)
- `gpg` (On Debian/Ubuntu `apt install gpg`)

To run the GUI you will need some additional libraries:
- [`fontconfig`](https://www.freedesktop.org/wiki/Software/fontconfig/) (On Debian/Ubuntu `apt install libfontconfig1-dev`)
- [Libudev](https://www.freedesktop.org/software/systemd/man/libudev.html) (On Debian/Ubuntu `apt install udev libudev-dev`)

### Throwaway folder

You can follow the guide from any folder of your choice. We recommend using a dedicated folder you
can wipe easily. On Linux:
```
mkdir liana_quicktry
cd liana_quicktry
```


## Step 1: setup `bitcoind`

Liana needs `bitcoind` to communicate with the Bitcoin network. Minimum supported version is 24.0.1.

### Download

The following instructions are specific to Linux (they may work on MacOS but i'm not sure). For
other platforms refer to
[https://bitcoincore.org/en/download/#verify-your-download](https://bitcoincore.org/en/download).

1. Download the `bitcoind` binary from [the official website of the Bitcoin Core
project](https://bitcoincore.org/bin/bitcoin-core-25.0/) according to your platform (in the context
of this guide, it is most likely `bitcoin-25.0-x86_64-linux-gnu.tar.gz`), and associated SHA256SUMS and SHA256SUMS.asc verification files.
```
curl -O https://bitcoincore.org/bin/bitcoin-core-25.0/bitcoin-25.0-x86_64-linux-gnu.tar.gz -O https://bitcoincore.org/bin/bitcoin-core-25.0/SHA256SUMS -O https://bitcoincore.org/bin/bitcoin-core-25.0/SHA256SUMS.asc
```

2. Verify the hash of the downloaded archive.
```
sha256sum --ignore-missing --check SHA256SUMS
```

3. Verify the signature against a key you trust. The Bitcoin Core Guix Attestations Github repo contains [a
folder](https://github.com/bitcoin-core/guix.sigs) of signers for each release and a folder of their keys.
Mine is `590B7292695AFFA5B672CBB2E13FC145CD3F4304`.
```
gpg --keyserver hkps://keys.openpgp.org --receive 590B7292695AFFA5B672CBB2E13FC145CD3F4304
gpg --verify SHA256SUMS.asc
```

4. Finally, uncompress the archive to get access to the `bitcoind` binary.
```
tar -xzf bitcoin-25.0-x86_64-linux-gnu.tar.gz
```

### Start `bitcoind` on signet

Run `bitcoind` in the background on the public signet network. On Linux:
```
./bitcoin-25.0/bin/bitcoind -signet -daemon
```

If it is the first time you start a signet Bitcoin on this machine it will take a few minutes to
synchronize (depends on your connection and hardware of course, but it shouldn't take longer than a
handful of minutes). You can track the progress using the `getblockchaininfo` command. On Linux:
```
./bitcoin-25.0/bin/bitcoin-cli -signet getblockchaininfo
```

**You do not need to wait for full synchronisation before moving on to the next step.**


## Step 2: start Liana

Head to the [release page](https://github.com/wizardsardine/liana/releases) and download the right
executable for your platform. If you are not sure what is the "right" executable for your platform,
choose `liana-0.3.exe` if you are on Windows, `liana-0.3.dmg` if you are on MacOS and
`liana-0.3-x86_64-linux-gnu.tar.gz` if you are on Linux.

For every file available on the release page, there is an accompanying `.asc` file with the same
name. This is a GPG signature made with Antoine Poinsot's key:
`590B7292695AFFA5B672CBB2E13FC145CD3F4304`. This key is available elsewhere for cross-checking, such
as on [his Twitter profile](https://twitter.com/darosior) or his [personal
website](http://download.darosior.ninja/antoine_poinsot_0xE13FC145CD3F4304.txt). It is recommended
you verify your download against this key.

At startup, you will have the choice between starting Liana using an existing configuration or to
set up a new one. Choose to install Liana on a new Bitcoin network.

The next screen allows you to either configure a new wallet, participate in the configuration of a
new wallet (if you are taking part in a multisig for instance), or to recover a wallet from backup.
Choose to create a new wallet.

Choose Bitcoin Signet as network. Now you will need to configure the primary key(s), the recovery
key(s), and the time delay before the recovery keys become available (in # of blocks). We'll use
only one key for both the primary and recovery paths. We'll derive both keys from a "hot signer", a
HD wallet whose seed is stored on the laptop.

Click on "Set" for the primary key. Click on "This computer" and set an alias for this signer. I'll
name it Alice but choose whatever. Set any timelock you want but preferably something very small if
you want to try the timelocked recovery feature! I'll go for "2" as the timelock. Click on "Set" for
the recovery key, and choose "This computer" again.

Of course, it wouldn't make sense for a real wallet to use the same signing device to derive both
the primary and recovery keys. Or even to use hot keys at all with a non-trivial amount of coins. We
only do this for convenience in testing Liana on Signet. If you'd like to try out signing with a
hardware wallet you can use the "testnet" mode of a Specter, the "Bitcoin testnet" app of a Ledger,
or the simulator of any of them (see the links at the top of this document).

Click on next. If you want to try restoring from wallet backup later on, make sure to backup the
mnemonic as well as the descriptor in the next two screens. Otherwise just make them happy by
ticking the boxes. If you are using a signing device simulator you'll have a step for registering
the descriptor on it.

Finally, configure the connection to `bitcoind`. The default should work for what we did in this
guide. Click on continue and finalize the installation.


## Step 3: have fun

You can generate a receive address in the "Receive" menu. You can get signet coins from the signet
faucet at https://signet.bc-2.jp/.

If you want to try the timelocked recovery path, receive some coins and wait for some blocks (2 for
my own configuration, but it depends on what you configured previously). Then you can click on
"recover funds" in "Settings" > "Recovery".

Keep in mind that signet coins have no value!


## Cleanup

You need to remove:
- The Bitcoin Core archive, binary and data directory
- The Liana binary and data directory

Assuming you used the throwaway folder as advised in step 0 and did not use custom `bitcoind` or
Liana data directories you can wipe everything using these commands:
```
cd ..
rm -rf liana_quicktry
rm -rf ~/.bitcoin/signet
rm -rf ~/.liana/signet
```

## Tips & Tricks 

### Simulating multiple wallets

You can simulate multiple wallets by using different data directories. For instance:

```
./liana-gui --datadir test_alice
./liana-gui --datadir test_bob
./liana-gui --datadir test_charlie
 ```
The directory will be created if it doesn't exist.