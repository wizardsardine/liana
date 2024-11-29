# Liana reproducible builds

## Reproducible build of linux binaries with GUIX

Releases of Liana are built in a reproducible manner, providing an assurance the binary a user is
going to run corresponds to the sources published. It enables the possibility for the user, or a
third party, to audit the code being ran.

For Linux binaries (and hopefully soon for all hosts) we go further and provide bootstrappable
builds, where the toolchain used to compile the source code reproducibly is itself built
reproducibly from source.

Learn more about reproducible builds [here](https://reproducible-builds.org/), and bootstrappable
builds [here](https://bootstrappable.org/).

For instructions on bootstrappable builds of Linux releases, see the [`guix`](./guix) folder.


## Reproducible build of windows and macos binaries with NIX

You will have to install [Nix](https://nixos.org/download/#download-nix), a package manager.
We rely on nix flakes, so you may need to activate this feature by setting in your `~/.config/nix/nix.conf`:

```
experimental-features = nix-command flakes
```

### Windows

Simply run:

```
nix build .#x86_64-pc-windows-gnu
```

Binary will be present in the `./result` folder.


### MACOS

First you need get the MacOS SDK. It is required to be able to build the MacOS binaries. The `12_2` version is
required (`Xcode_12.2.xip`). You need to download it from Apple's website. An Apple ID and cookies
enabled for the hostname are required.  You can create one for free. (Note it is illegal to
distribute the archive.) Once logged in you can use the [direct
link](https://download.developer.apple.com/Developer_Tools/Xcode_12.2/Xcode_12.2.xip) to download
the archive. Alternatively, go to 'Downloads', then 'More' and search for [`Xcode
12.2`](https://developer.apple.com/download/all/?q=Xcode%2012.2).
The `sha256sum` of the downloaded XIP archive should be
`28d352f8c14a43d9b8a082ac6338dc173cb153f964c6e8fb6ba389e5be528bd0`.

Then you have to extract the SDK and add it to the nix store:

```
nix run github:edouardparis/unxip#unxip -- Xcode_12.2.xip Xcode_12.2
cd Xcode_12.2
nix-store --add-fixed --recursive sha256 Xcode.app
```
It may take a long time.

Then to compile binaries for new apple CPUs:
```
nix build .#aarch64-apple-darwin
```
Or for legacy CPUs:
```
nix build .#x86_64-apple-darwin
```

Binaries will be present in the `./result` folder.
