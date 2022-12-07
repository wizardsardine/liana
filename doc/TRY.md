# Quickly try out Liana

> *Just give me the TL;DR!*

This document is a short set of instructions for trying out Liana on signet. It does not attempt to
give any nuance, details or describe alternative configurations.

This guide mostly assumes you are running a 64-bit Linux.

TODO: adapt the guide to Windows and MacOS.


## Step 0: preparation

### System dependencies

Here is a list of the system dependencies: the tools and libraries you need to have installed on
your system to follow the guide.

We'll use basic tools which should already be present on your system, such as:
- `shasum`
- `tar`

To run the Specter signing device simulator you will need:
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

Liana needs `bitcoind` to communicate with the Bitcoin network. Minimum supported version is 24.0.

### Download

TODO: update to 24.0.1 when it's released.

Download the `bitcoind` binary from [the official website of the Bitcoin Core
project](https://bitcoincore.org/bin/bitcoin-core-24.0/) according to your platform (in the context
of this guide, it is most likely `bitcoin-24.0-x86_64-linux-gnu.tar.gz`).

Then verify the signature against a key you trust. The Bitcoin Core Github repo contains [a
list](https://github.com/bitcoin/bitcoin/blob/master/contrib/builder-keys/keys.txt) of frequent
signers. Mine is `590B7292695AFFA5B672CBB2E13FC145CD3F4304`.

Finally, uncompress the archive to get access to the `bitcoind` binary.

```
curl -O https://bitcoincore.org/bin/bitcoin-core-24.0/bitcoin-24.0-x86_64-linux-gnu.tar.gz -O https://bitcoincore.org/bin/bitcoin-core-24.0/SHA256SUMS -O https://bitcoincore.org/bin/bitcoin-core-24.0/SHA256SUMS.asc
sha256sum --ignore-missing --check SHA256SUMS
gpg --keyserver hkps://keys.openpgp.org --receive 590B7292695AFFA5B672CBB2E13FC145CD3F4304
gpg --verify SHA256SUMS.asc
tar -xzf bitcoin-24.0-x86_64-linux-gnu.tar.gz
```

For details on verifying your download, or for verifying the download on a non-Linux machine refer
to
[https://bitcoincore.org/en/download/#verify-your-download](https://bitcoincore.org/en/download/#verify-your-download).

### Start `bitcoind` on signet

Run `bitcoind` in the background on the public signet network.
```
./bitcoin-24.0/bin/bitcoind -signet -daemon
```

If it is the first time you start a signet Bitcoin on this machine it will take a few minutes to
synchronize (depends on your connection and hardware of course, but it shouldn't take longer than a
handful of minutes). You can track the progress using the `getblockchaininfo` command:
```
./bitcoin-24.0/bin/bitcoin-cli -signet getblockchaininfo
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

A window will pop up. Choose a dummy pin code and generate a new key. Then go to settings, switch
network to signet.

Keep the Specter simulator open and move on to the next step.


## Step 3: start Liana

Get the latest Liana software release and start it.

TODO: download and sig verification details

Since you presumably never installed Liana, this will start the installer. Create a new wallet.

Choose network Signet. For the primary key use the one from your dummy signing device, the Specter
simulator you just started. You can do that simply by clicking on the "import" button next to the
text input in the installer. For the number of blocks before the recovery key becomes active, you
can choose anything valid. But prefer something small to test the case where coins are soon to
become accessible on the recovery branch.

For the recovery key you could use another simulator but in this guide i'll just use a random xpub:
`tpubDDU2vzv4Rk2kU8VjDDQBWYTb7tmSd9ddV4ERmm5VesfoaxBJQm3CyNc4fjcYAzEqXn3YQ8dxpzkhQjpxT3Nqp7yQh1UMczL1MMfTSKXNv3n/<0;1>/*`.
You can also generate one from, for instance https://iancoleman.io/bip39/. But make sure to append
`/<0;1>/*` to it, this is to be able to derive both change and receive addresses from the same
descriptor.

TODO: have the installer input xpubs without the derivation part. https://github.com/revault/liana/issues/155

Make the next step happy by ticking the "i backed up my descriptor" checkbox.

Import the descriptor to your Specter by clicking on the `specter-simulator` tile.

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
