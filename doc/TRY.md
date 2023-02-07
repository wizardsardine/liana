# Quickly try out Liana

> *Just give me the TL;DR!*

This document is a short set of instructions for trying out Liana on Bitcoin signet. It does not attempt to
give any nuance, details or describe alternative configurations.
This guide uses an emulator of the Specter hardware signer.

This guide mostly assumes you are running a 64-bit Linux.

TODO: adapt the guide to Windows and MacOS.


## Step 0: preparation

### System dependencies

Here is a list of the system dependencies: the tools and libraries you need to have installed on
your system to follow the guide.

TL;DR:
- Debian/Ubuntu: `apt install git libsdl2-dev curl gpg libfontconfig1-dev libudev-dev gcc make`
- Arch Linux: check if you have all the required packages: `pacman -Q coreutils tar git sdl2 curl gnupg fontconfig systemd-libs`.
If any is listed as "was not found", get it with `pacman -Sy [missing package name]`
- Other distribution: see the link to projects below to search for the name of your distribution's packages.

We'll use basic tools which should already be present on your system, such as:
- `shasum`
- `tar`

To run the Specter signing device simulator you will need:
- `gcc` and `make` (On Debian/Ubuntu `apt install gcc make`)
- `git` (On Debian/Ubuntu `apt install gpg`)
- [SDL2](https://wiki.libsdl.org/SDL2/FrontPage) (On Debian/Ubuntu `apt install libsdl2-dev`)

To download and verify binaries you will also need:
- `curl` (On Debian/Ubuntu `apt install curl`)
- `gpg` (On Debian/Ubuntu `apt install gpg`)

To run the GUI you will need some additional libraries:
- [`fontconfig`](https://www.freedesktop.org/wiki/Software/fontconfig/) (On Debian/Ubuntu `apt install libfontconfig1-dev`)
- [Libudev](https://www.freedesktop.org/software/systemd/man/libudev.html) (On Debian/Ubuntu `apt install libudev-dev`)

### Throwaway folder

You can follow the guide from any folder of your choice. We recommend using a dedicated folder you
can wipe easily:
```
mkdir liana_quicktry
cd liana_quicktry
```


## Step 1: setup `bitcoind`

Liana needs `bitcoind` to communicate with the Bitcoin network. Minimum supported version is 24.0.1.

### Download

1. Download the `bitcoind` binary from [the official website of the Bitcoin Core
project](https://bitcoincore.org/bin/bitcoin-core-24.0.1/) according to your platform (in the context
of this guide, it is most likely `bitcoin-24.0.1-x86_64-linux-gnu.tar.gz`), and associated SHA256SUMS and SHA256SUMS.asc verification files.
```
curl -O https://bitcoincore.org/bin/bitcoin-core-24.0.1/bitcoin-24.0.1-x86_64-linux-gnu.tar.gz -O https://bitcoincore.org/bin/bitcoin-core-24.0.1/SHA256SUMS -O https://bitcoincore.org/bin/bitcoin-core-24.0.1/SHA256SUMS.asc
```

2. Verify the hash of the downloaded archive.
```
sha256sum --ignore-missing --check SHA256SUMS
```

3. Verify the signature against a key you trust. The Bitcoin Core Github repo contains [a
list](https://github.com/bitcoin/bitcoin/blob/master/contrib/builder-keys/keys.txt) of frequent
signers. Mine is `590B7292695AFFA5B672CBB2E13FC145CD3F4304`.
```
gpg --keyserver hkps://keys.openpgp.org --receive 590B7292695AFFA5B672CBB2E13FC145CD3F4304
gpg --verify SHA256SUMS.asc
```

4. Finally, uncompress the archive to get access to the `bitcoind` binary.
```
tar -xzf bitcoin-24.0.1-x86_64-linux-gnu.tar.gz
```

For details on verifying your download, or for verifying the download on a non-Linux machine refer
to
[https://bitcoincore.org/en/download/#verify-your-download](https://bitcoincore.org/en/download/#verify-your-download).

### Start `bitcoind` on signet

Run `bitcoind` in the background on the public signet network.
```
./bitcoin-24.0.1/bin/bitcoind -signet -daemon
```

If it is the first time you start a signet Bitcoin on this machine it will take a few minutes to
synchronize (depends on your connection and hardware of course, but it shouldn't take longer than a
handful of minutes). You can track the progress using the `getblockchaininfo` command:
```
./bitcoin-24.0.1/bin/bitcoin-cli -signet getblockchaininfo
```

You do not need to wait for full synchronisation before moving on to the next step.


## Step 2: setup a dummy signing device

Liana does not support "hot keys" at the moment. It needs a connection to a signing device for
signing transactions.

We will be using a [Specter](https://github.com/cryptoadvance/specter-diy) simulator.

```
git clone https://github.com/cryptoadvance/specter-diy
cd specter-diy
git checkout 6a6983e15e4d3c8c03937f8bee040de350ce722f
make simulate
```

This step might take a few minutes at the first launch of the Specter emulator.
A window will pop up. Choose a dummy pin code and generate a new key. Then go to settings, switch
network to signet.

Keep the Specter simulator open and move on to the next step.


## Step 3: start Liana

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

At startup, you will have the choice between using an existing wallet or setting up a new one. Since
you presumably never installed Liana, choose to set up a new one.

Choose network Signet. For the primary key we will use the one from the dummy signing device you just started. Do this by simply clicking on the "import" button next to the text input in the Liana installer. For the number of blocks before the recovery key becomes active, you
can choose anything valid. Preferably something small to test the case where coins are soon to
become accessible on the recovery branch.

For the recovery key you could use another simulator but in this guide i'll just use a key generated online at https://iancoleman.io/bip39/. Click the **GENERATE** at the top of the website, then make sure to select **Coin: BTC-Bitcoin Testnet** to generate a tpub. You can then copy the **Account Extended Public Key** and paste it in the recovery path of Liana.

Make the next step happy by ticking the "I backed up my descriptor" checkbox.

Import the descriptor to your Specter by clicking on the `specter-simulator` tile. Accept it on the Specter emulator.

Finally, configure the connection to `bitcoind`. The default should work for what we did in this
guide. Click sur continue and finalize the installation.


## Step 4: have fun

You can generate a receive address in the "Receive" menu. You can get signet coins from the signet
faucet at https://signet.bc-2.jp/.

Keep in mind that signet coins have no value!


## Cleanup

You need to remove:
- The Bitcoin Core archive and `bitcoind` binary
- The Specter directory
- The `bitcoind` data directory
- The Liana data directory

Assuming you used the throwaway folder as advised in step 0 and did not use custom `bitcoind` or
Liana data directories you can wipe everything using these commands:
```
cd ..
rm -rf liana_quicktry
rm -rf ~/.bitcoin/signet
rm -rf ~/.liana/signet
```
