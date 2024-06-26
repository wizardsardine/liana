# Quickly try out Liana in a test environment


*(Updated on  of 2023)*

This document is a short set of instructions for trying out Liana on Bitcoin signet, a test network using value-less bitcoins. It does not attempt to
give any nuance, details or describe alternative configurations.

This guide will make use Liana as a "hot wallet", and use the "Liana managed" `bitcoind` option.
You can also connect your Ledger or SpecterDIY hardware signer if you have some at hand (and maybe by the time you read this guide, Bitbox, Coldcard and Jade - integration is coming soon).
If you'd like to try out Liana using emulators of
hardware signing device you can use the [Specter
simulator](https://github.com/cryptoadvance/specter-diy/blob/master/docs/simulator.md) or the
[Ledger "Speculos" emulator](https://github.com/LedgerHQ/speculos).
(emulators of the other hardware signers will work too when we finish their integration)

## Step 0: preparation

### System dependencies

*If you are using Windows or MacOS, you can skip this step.*
*If you are using a somewhat recent Debian/Ubuntu, Arch/Manjaro/Endeavor, NixOS distribution or similar, you can skip this step.*

Here is a list of the system dependencies: the tools and libraries you need to have installed on
your system to follow the guide if you are running a Linux that isn't Debian- or Arch- based.

- GUI requirements, see the link to projects below to search for the name of your distribution's packages.  
    - [`fontconfig`](https://www.freedesktop.org/wiki/Software/fontconfig/) 
    - [Libudev](https://www.freedesktop.org/software/systemd/man/libudev.html) 
- Running binaries requires GLIBC >= 2.33 (Ubuntu >= 22.04 or Debian >= 12)

We'll use basic tools which should already be present on your system, such as:
- `shasum`
- `tar`

To verify binaries you will also need:
- `gpg` (On Debian/Ubuntu `apt install gpg`)

### Throwaway folder

You can follow the guide from any folder of your choice. We recommend creating a new dedicated folder you
can wipe easily after testing. 

If you are using a Linux terminal:
```
mkdir liana_quicktry
cd liana_quicktry
```


## Step 1: Liana installer

Get the Liana software for your system on the [Wizardsardine website](https://wizardsardine.com/liana).

A note for **Linux users only**: released binaries may not be working on your system if it is
running a too old glibc. In this case you may have to build from source. See the [short section
about this in the README](../README.md#a-note-on-linux-binaries-and-glibc-version).

For every file available on the website, there is an accompanying `.asc` file with the same
name on our [Github release page](https://github.com/wizardsardine/liana/releases). This is a GPG signature made with Antoine Poinsot's key:
`590B7292695AFFA5B672CBB2E13FC145CD3F4304`. This key is available elsewhere for cross-checking, such
as on [his Twitter profile](https://twitter.com/darosior) or his [personal
website](http://download.darosior.ninja/antoine_poinsot_0xE13FC145CD3F4304.txt). It is recommended
you verify your download against this key.
Example for Linux (replace the signature name with the one corresponding to your download):
```
gpg --keyserver hkps://keys.openpgp.org --receive 590B7292695AFFA5B672CBB2E13FC145CD3F4304
gpg --verify liana_2.0-1_amd64.deb.asc
```
GPG should tell you the signature is valid for Antoine's key.

If GPG told you that Antoine key has expired, you should refresh it.
Example for Linux (replace the signature name with the one corresponding to your download):
```
gpg --keyserver hkps://keys.openpgp.org --refresh-keys E13FC145CD3F4304      
```

If all is good, you can run Liana!

At startup, you will have the choice between starting Liana using an existing configuration or to
set up a new one. Choose to install Liana on a new Bitcoin network.

The next screen allows you to either configure a new wallet, participate in the configuration of a
new wallet (if you are taking part in a multisig for instance), or to recover a wallet from backup.
Choose to create a new wallet.

Choose **Bitcoin Signet** as network. Now you will need to configure the primary key(s), the recovery
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
ticking the boxes. If you are using a signing device or its simulator you'll have a step for registering
the descriptor on it.

You can then decide whether you would like to manage `bitcoind` yourself or let Liana configure
and start/stop it while the GUI is being used:
For the purpose of this guide, we will use the simpler option: to let Liana download and manage Bitcoin Core for us. It will get the software on [bitcoincore.org](https://bitcoincore.org/) and configure it in pruned mode with about 20GB of disk usage.
A full Initial Blocks Download (Bitcoin network synchronization, from the beginning of the chain) will take place, as we are using Signet it will be pretty quick.

Click on continue until we finalize the installation.


## Step 2: have fun

Once synchronized, Liana will open the wallet.
You can generate a receive address in the "Receive" menu. You can get signet coins from the signet
faucet at https://signet.bc-2.jp/.

If you want to try the timelocked recovery path, receive some coins and wait for some blocks (2 for
my own configuration, but it depends on what you configured previously). Then you can click on
"recover funds" in "Settings" > "Recovery".

Keep in mind that signet coins have no value!

Signet is a network, so you can send coins to other people on signet, receive from them, etc. Feel free to explore Liana! 


## Cleanup

You need to remove:
- The Liana binary
- its data directory

For a user Alice the default Liana data directory is:

- /Users/Alice/Library/Application Support/Liana on MacOS
- C:\Users\Alice\AppData\Roaming\Liana on Windows
- /home/Alice/.liana on Linux

Assuming you used the throwaway folder as advised in step 0 and did not use custom `bitcoind` or
Liana data directories you can wipe everything using these commands on Linux:
```
cd ..
rm -rf liana_quicktry
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
