# Quickly try out Tenshu in a test environment

_(Updated July 2026)_

This document is a short set of instructions for trying out Tenshu — the Coincube desktop
app — on Bitcoin signet, a test network using value-less bitcoins. It does not attempt to
give any nuance, details or describe alternative configurations.

This guide uses Tenshu as a "hot wallet" with the "Tenshu-managed" `bitcoind` option.
You can find [here](./SIGNING_DEVICES.md) the list of supported signing devices.
If you'd like to try out Tenshu using emulators of a hardware signing device you can use the
[Specter simulator](https://github.com/cryptoadvance/specter-diy/blob/master/docs/simulator.md)
or the [Ledger "Speculos" emulator](https://github.com/LedgerHQ/speculos).
(emulators of the other hardware signers will work too when we finish their integration)

## Step 0: preparation

### System dependencies

_If you are using Windows or MacOS, you can skip this step._
_If you are using a somewhat recent Debian/Ubuntu, Arch/Manjaro/Endeavor, NixOS distribution or similar, you can skip this step._

Here is a list of the system dependencies: the tools and libraries you need to have installed on
your system to follow the guide if you are running a Linux that isn't Debian- or Arch- based.

- GUI requirements, see the link to projects below to search for the name of your distribution's packages.
  - [`fontconfig`](https://www.freedesktop.org/wiki/Software/fontconfig/)
  - [Libudev](https://www.freedesktop.org/software/systemd/man/libudev.html)
- Running binaries requires GLIBC >= 2.33 (Ubuntu >= 22.04 or Debian >= 12)

We'll use basic tools which should already be present on your system, such as:

- `sha256sum` (or `shasum` on macOS)
- `tar`

To verify binaries you will also need:

- `gpg` (On Debian/Ubuntu `apt install gpg`)

### Throwaway folder

You can follow the guide from any folder of your choice. We recommend creating a new dedicated folder you
can wipe easily after testing.

If you are using a Linux terminal:

```
mkdir tenshu_quicktry
cd tenshu_quicktry
```

## Step 1: install Tenshu

Get Tenshu for your system from the [Coincube website](https://coincube.io) or the
[GitHub releases page](https://github.com/coincubetech/coincube/releases).

A note for **Linux users only**: released binaries may not be working on your system if it is
running a too old glibc. In this case you may have to build from source. See the [short section
about this in the README](../README.md#a-note-on-linux-binaries-and-glibc-version).

### Verify your download

Every release is published on the [GitHub releases page](https://github.com/coincubetech/coincube/releases)
alongside a signed checksums manifest:

- `SHA256SUMS-<version>.txt` — SHA256 checksums of every release artifact
- `SHA256SUMS-<version>.txt.asc` — GPG signature of that manifest, made with the Coincube release
  signing key (`67F9701BF0D2DAF4`, `Coincube Release Signing <releases@coincube.io>`)

Release artifacts are named `tenshu-<version>-<target>.<ext>`, e.g.
`tenshu-1.5.0-aarch64-apple-darwin.dmg` (macOS Apple Silicon),
`tenshu-1.5.0-x86_64-pc-windows-msvc.msi` (Windows), or
`tenshu-1.5.0-x86_64-unknown-linux-gnu.tar.gz` (Linux).

To verify your download (example for version 1.5.0):

```
# Import the Coincube release key (one-time)
curl -O https://raw.githubusercontent.com/coincubetech/coincube/master/docs/security/coincube-release-public.asc
gpg --import coincube-release-public.asc

# Verify the checksums manifest, then check your artifact against it
gpg --verify SHA256SUMS-1.5.0.txt.asc
sha256sum --check SHA256SUMS-1.5.0.txt --ignore-missing
```

`gpg --verify` should report a **Good signature** from the Coincube release key, and
`sha256sum --check` should print `OK` next to the file you downloaded. See
[docs/security/VERIFY.md](./security/VERIFY.md) for full per-platform instructions and the
key fingerprint to cross-check.

If all is good, you can run Tenshu!

At startup, you will have the choice between starting Tenshu using an existing configuration or to
set up a new one. Choose to install Tenshu on a new Bitcoin network.

The next screen allows you to either configure a new wallet, participate in the configuration of a
new wallet (if you are taking part in a multisig for instance), or to recover a wallet from backup.
Choose to create a new wallet.

Choose **Bitcoin Signet** as network. Now you will need to configure the primary key(s), the recovery
key(s), and the time delay before the recovery keys become available (in # of blocks). We'll use
only one key for both the primary and recovery paths. We'll derive both keys from a "master signer", a
HD wallet whose seed is stored on the laptop.

Click on "Set" for the primary key. Click on "This computer" and set an alias for this signer. I'll
name it Alice but choose whatever. Set any timelock you want but preferably something very small if
you want to try the timelocked recovery feature! I'll go for "2" as the timelock. Click on "Set" for
the recovery key, and choose "This computer" again.

Of course, it wouldn't make sense for a real wallet to use the same signing device to derive both
the primary and recovery keys. Or even to use hot keys at all with a non-trivial amount of coins. We
only do this for convenience in testing Tenshu on Signet. If you'd like to try out signing with a
hardware wallet you can use the "testnet" mode of a Specter, the "Bitcoin testnet" app of a Ledger,
or the simulator of any of them (see the links at the top of this document).

Click on next. If you want to try restoring from wallet backup later on, make sure to backup the
mnemonic as well as the descriptor in the next two screens. Otherwise just make them happy by
ticking the boxes. If you are using a signing device or its simulator you'll have a step for registering
the descriptor on it.

You can then decide whether you would like to manage `bitcoind` yourself or let Tenshu configure
and start/stop it while the GUI is being used:
For the purpose of this guide, we will use the simpler option: to let Tenshu download and manage Bitcoin Core for us. It will get the software on [bitcoincore.org](https://bitcoincore.org/) and configure it in pruned mode with about 20GB of disk usage.
A full Initial Blocks Download (Bitcoin network synchronization, from the beginning of the chain) will take place, as we are using Signet it will be pretty quick.

Click on continue until we finalize the installation.

## Step 2: have fun

Once synchronized, Tenshu will open the wallet.
You can generate a receive address in the "Receive" menu. You can get signet coins from the signet
faucet at https://signet.bc-2.jp/.

If you want to try the timelocked recovery path, receive some coins and wait for some blocks (2 for
my own configuration, but it depends on what you configured previously). Then you can click on
"recover funds" in "Settings" > "Recovery".

Keep in mind that signet coins have no value!

Signet is a network, so you can send coins to other people on signet, receive from them, etc. Feel free to explore Tenshu!

## Cleanup

You need to remove:

- The Tenshu application (the `coincube` binary)
- its data directory

For a user Alice the default Tenshu data directory is:

- /Users/Alice/Library/Application Support/Coincube on MacOS
- C:\Users\Alice\AppData\Roaming\Coincube on Windows
- /home/Alice/.coincube on Linux

Assuming you used the throwaway folder as advised in step 0 and did not use custom `bitcoind` or
Tenshu data directories you can wipe everything using these commands on Linux:

```
cd ..
rm -rf tenshu_quicktry
rm -rf ~/.coincube/signet
```

## Tips & Tricks

### Simulating multiple wallets

You can simulate multiple wallets by using different data directories. For instance:

```
./coincube --datadir test_alice
./coincube --datadir test_bob
./coincube --datadir test_charlie
```

The directory will be created if it doesn't exist.

### Building from source with `nix develop`

If you have [nix](https://nixos.org) (the package manager) installed, you can easily
build from source as follows:

1. `git clone https://github.com/coincubetech/coincube.git && cd coincube`
2. `nix develop` which will put you into a development shell with all dependencies available
3. `cargo build --release` which will build the `coincubed` daemon, the `coincube-cli` tool, and the `coincube` GUI binary (from the `coincube-gui` crate).
4. `target/release/coincube --datadir test_alice` will load up the GUI and create/use `./test_alice` as the data directory.
