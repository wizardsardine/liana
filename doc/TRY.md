## Quickly try out Liana

*Just give me the TL;DR!*

This document is a short set of instructions for trying out Liana on signet. It does not attempt to
give any nuance, details or describe alternative configurations.

This guide mostly assumes you are running a 64-bit Linux.

TODO: adapt the guide to Windows and MacOS.


### Step 0: dependencies

We'll use basic tools such as:
- `curl`
- `tar`

To run the simulator you will need:
- `git` (On Debian/Ubuntu `apt install gpg`)
- SDL2. See
  [here](https://github.com/cryptoadvance/specter-diy/blob/6a6983e15e4d3c8c03937f8bee040de350ce722f/docs/build.md#prerequisities-simulator)
  for the instructions depending on your system. (On Debian/Ubuntu `apt install libsdl2-dev`)

To verify the downloads you will need:
- `shasum`
- `gpg` (On Debian/Ubuntu `apt install gpg`)

To run the GUI you will need some libraries:
- `fontconfig` (On Debian/Ubuntu `apt install libfontconfig1-dev`)
- [`pkg-config`](https://www.freedesktop.org/wiki/Software/pkg-config/) (On Debian/Ubuntu `apt install pkg-config`)
- [Libudev](https://www.freedesktop.org/software/systemd/man/libudev.html) (On Debian/Ubuntu `apt install libudev-dev`)


### Step 1: setup `bitcoind`

Liana needs `bitcoind` to communicate with the Bitcoin network. Minimum supported version is 24.0.

#### Download

TODO: update to 24.0.1 when it's released.

Download the `bitcoind` binary from [the official website of the Bitcoin Core
project](https://bitcoincore.org/bin/bitcoin-core-24.0/) according to your platform.

Then verify the signature against a key you trust. The Bitcoin Core Github repo contains [a
list](https://github.com/bitcoin/bitcoin/blob/master/contrib/builder-keys/keys.txt) of frequent
signers. Mine is `590B7292695AFFA5B672CBB2E13FC145CD3F4304`.

Finally, uncompress the archive to get access to the `bitcoind` binary.

```
curl -O https://bitcoincore.org/bin/bitcoin-core-24.0/bitcoin-24.0-x86_64-linux-gnu.tar.gz -O https://bitcoincore.org/bin/bitcoin-core-24.0/SHA256SUMS -O https://bitcoincore.org/bin/bitcoin-core-24.0/SHA256SUMS.asc
sha256sum --ignore-missing --check SHA256SUMS
gpg --keyserver hkps://keys.openpgp.org --receive 590B7292695AFFA5B672CBB2E13FC145CD3F4304
gpg --verify SHASUMS.asc
tar -xzf bitcoin-24.0-x86_64-linux-gnu.tar.gz bitcoin-24.0/bin/bitcoind
```

For details on verifying your download, or for verifying the download on another platform please
refer to
[https://bitcoincore.org/en/download/#verify-your-download](https://bitcoincore.org/en/download/#verify-your-download).

#### Start `bitcoind` on signet

Run `bitcoind` in the background on the public signet network. If it is the first time you start a
signet Bitcoin on this machine it will take a few minutes to synchronize (depends on your connection
and hardware of course, but it shouldn't take longer than a handful of minutes).

```
./bitcoin-24.0/bin/bitcoind -signet -daemon
```


### Step 2: setup a dummy signing device

Liana does not support "hot keys" at the moment. It needs a connection to a signing device for
signing transactions.

Let's start a Specter simulator.

```
git clone https://github.com/cryptoadvance/specter-diy
cd specter-diy
git checkout 6a6983e15e4d3c8c03937f8bee040de350ce722f
make simulate
```

A window will pop up. Choose a dummy pin code and generate a new key. Then go to settings, switch
network to signet. Then go to the next section. (Keep it running obviously, you'll need it.)


### Step 3: start Liana

Get the latest Liana software release and start it.

TODO: download and sig verification details

Since you presumably never installed Liana, this will start the installer. Create a new wallet.

Choose network Signet. For the primary key use the one from your dummy signing device, the Specter
simulator you just started. You can do that simply by clicking on "import". For the number of blocks
before the recovery key becomes active, you can choose anything valid. But prefer something small to
test the case where coins are soon to become accessible on the recovery branch.

For the recovery key you could use another simulator but in this guide i'll just use a random xpub:
`tpubDDU2vzv4Rk2kU8VjDDQBWYTb7tmSd9ddV4ERmm5VesfoaxBJQm3CyNc4fjcYAzEqXn3YQ8dxpzkhQjpxT3Nqp7yQh1UMczL1MMfTSKXNv3n/<0;1>/*`.
You can generate one from, for instance https://iancoleman.io/bip39/. But make sure to append
`/<0;1>/*` to it, this is to be able to derive both change and receive addresses from the same
descriptor.

Make the next step happy by ticking the "i backed up my descriptor" checkbox.

Import the descriptor to your Specter by clicking on the `specter-simulator` tile.

Finally, configure the connection to `bitcoind`. The default should work for what we did in this
guide. Click sur continue and finalize the installation.


### Step 4: have fun

You can generate a receive address in the "Receive" menu. You can get signet coins from the signet
faucet at https://signet.bc-2.jp/.

Keep in mind that signet coins have no value!
