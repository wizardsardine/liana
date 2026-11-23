# Liana reproducible builds

Releases of Liana are built in a reproducible manner, providing an assurance the binary a user is
going to run corresponds to the sources published. It enables the possibility for the user, or a
third party, to audit the code being ran.

Learn more about reproducible builds [here](https://reproducible-builds.org/).

All the release binaries are built with [Nix](https://nixos.org/download/#download-nix), a package
manager. We rely on nix flakes, so you may need to activate this feature by setting in your
`~/.config/nix/nix.conf`:

```
experimental-features = nix-command flakes
```

All the targets below are built from a Linux host. Replace `.#liana` with `.#liana-business` to
build Liana Business instead: the output folders are the same, but Liana Business ships a single
`liana-business` binary in place of the `lianad`, `liana-cli` and `liana-gui` trio.


## The toolchain

Every release target is compiled with a `rustc` built from source by
[`mrustc`](https://github.com/thepowersgang/mrustc), a Rust compiler written in C++. No prebuilt
Rust binary is involved: `mrustc` compiles `rustc`, and that `rustc` then cross-compiles `std` from
source for each target. The development shells are the exception and still use a prebuilt toolchain
fetched by [fenix](https://github.com/nix-community/fenix), which is a convenience for contributors
and plays no part in a release.

This does not make every target bootstrappable, and the difference matters:

- **Linux and Windows are bootstrappable.** Everything they link against is built from source too:
  `glibc` through `cargo-zigbuild`, and the Windows CRT through the `mingw-w64` cross toolchain of
  nixpkgs. Nothing in the dependency graph is a binary you have to take on trust.
- **macOS is not bootstrappable, only reproducible.** Linking against Apple's libc requires the
  macOS SDK, which is a proprietary blob extracted from Xcode and added to the Nix store by hand
  (see the MacOS section below). It has no source form and cannot be rebuilt. So the macOS binaries
  are built with a bootstrapped compiler against a trusted SDK: reproducible, auditable as to the
  Liana sources, but resting on a binary Apple ships. Calling the macOS build bootstrappable would
  overstate it.

Building the toolchain takes hours, since it compiles `mrustc`, then `rustc` twice, then `std` for
each of its targets. You can build the Linux and Windows toolchain on its own with

```
nix build .#rustc-bootstrap
```

which goes all the way down to the C++ sources of `mrustc`. The Apple `std` is not part of it: it
needs the macOS SDK, which must not become a dependency of the Linux and Windows toolchain. The
compiler carrying it is a separate output, and the SDK has to be in the store first (see
[MacOS](#macos)):

```
nix build .#toolchainApple
```


## Linux

Simply run:

```
nix build .#liana.x86_64-unknown-linux-gnu
```

The `lianad`, `liana-cli` and `liana-gui` binaries will be present in the
`./result/x86_64-unknown-linux-gnu` folder. They are linked against `glibc` version `2.31` through
[`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild), so they run on any distribution
shipping this version or a more recent one.

This target is also [bootstrappable](https://bootstrappable.org/): see
[The toolchain](#the-toolchain) above.


## Windows

Simply run:

```
nix build .#liana.x86_64-pc-windows-gnu
```

The `liana-gui-setup.exe` installer (`liana-business-setup.exe` for Liana Business) will be present
in the `./result/x86_64-pc-windows-gnu` folder. It wraps the cross-compiled executable with
[NSIS](https://nsis.sourceforge.io/); see
[`contrib/release/windows`](../release/windows/README.md) for how the installer is kept
reproducible. The bare executable is still available under `.#liana.x86_64-pc-windows-gnu-exe`.

### Verifying the codesigned installer

The codesigned `liana-<version>-setup.exe` is the reproducible
`liana-<version>-setup-noncodesigned.exe` with an Authenticode signature attached. The signature
is published on its own as `liana-<version>-setup.exe.pem` (the signing certificate, the signature
over the installer's hash and the timestamp), so you can rebuild the installer, attach the
published signature and check that the result is byte for byte the codesigned installer we
distribute:

```
./contrib/release/windows/attach-signature.sh ./result/x86_64-pc-windows-gnu/liana-gui-setup.exe liana-<version>-setup.exe.pem /tmp/liana-setup.exe
sha256sum /tmp/liana-setup.exe liana-<version>-setup.exe
```


## MacOS

This is the target that is reproducible but not bootstrappable, for the reason given in
[The toolchain](#the-toolchain): the SDK below is a binary you have to trust.

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
nix build .#liana.aarch64-apple-darwin
```
Or for legacy CPUs:
```
nix build .#liana.x86_64-apple-darwin
```

Binaries will be present in the `./result` folder.
