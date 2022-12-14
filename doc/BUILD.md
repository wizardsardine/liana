# Building Liana

We use [Cargo](https://doc.rust-lang.org/stable/cargo/), the ubiquitous Rust package manager.
Cargo takes care of downloading and compiling the projects dependencies, as well as compiling the
project itself. Dependencies are specified in a [`Cargo.toml`](../Cargo.toml) file at the root of
this repository. They are pinned in a [`Cargo.lock`](../Cargo.lock) file at the same place.

We take security very seriously, and toolchain is a big part of that. We are moderatly conservative
with dependencies and aim to target reasonable compiler versions that have had time to mature (ie
that had the chance to be reviewed and distributed by third parties, as well as tested by the
community).  The minimum supported Rust version for `lianad` currently is `1.48`, that is the
version of [`rustc` shipped in Debian stable](https://packages.debian.org/stable/rustc). (It is also
inferior to the latest version of `rustc` supported by
[`mrustc`](https://github.com/thepowersgang/mrustc/) supported at the time of writing, `1.54`).


## Getting `Cargo`

### Through your system package manager

Most package managers distribute a version of `Cargo` able to build this project. For instance on
Debian-based systems (as root):
```
apt update && apt install cargo
```

### By manually downloading the latest stable version

The ["other installation
methods"](https://forge.rust-lang.org/infra/other-installation-methods.html#standalone-installers)
page of the Rust website contains a list of archives for different architectures, along with
signatures made with the ["Rust signing key"](https://static.rust-lang.org/rust-key.gpg.ascii):
```
pub   rsa4096/0x85AB96E6FA1BE5FE 2013-09-26 [SC]
      Key fingerprint = 108F 6620 5EAE B0AA A8DD  5E1C 85AB 96E6 FA1B E5FE
uid                   [ unknown] Rust Language (Tag and Release Signing Key) <rust-key@rust-lang.org>
sub   rsa4096/0x8E9AA3F7AB3F5826 2013-09-26 [E]
sub   rsa4096/0x5CB4A9347B3B09DC 2014-12-15 [S]
```

You can therefore pull the key from either the above or from a keyserver:
```
$ gpg --keyserver hkps://keys.openpgp.org --receive 108F66205EAEB0AAA8DD5E1C85AB96E6FA1BE5FE
```

And then you can download the archive corresponding to your system and CPU architecture, verify the
signature and use the `cargo` binary from this archive to build Liana. Here is an example for
`amd64`:
```
$ curl -O https://static.rust-lang.org/dist/rust-1.65.0-x86_64-unknown-linux-gnu.tar.gz
$ curl -O https://static.rust-lang.org/dist/rust-1.65.0-x86_64-unknown-linux-gnu.tar.gz.asc
$ gpg --verify rust-1.65.0-x86_64-unknown-linux-gnu.tar.gz.asc
$ tar -xzf rust-1.65.0-x86_64-unknown-linux-gnu.tar.gz
$ ./rust-1.65.0-x86_64-unknown-linux-gnu/cargo/bin/cargo build --release
```

### Through `rustup`

[`rustup`](https://rust-lang.github.io/rustup/) is a software for installing the Rust toolchain.

Some package managers distribute a version of `rustup`. Failing that, you can always follow the
"official" [installation method of `rustup`](https://www.rust-lang.org/tools/install) (that is, a
`curl`-`sh` pipe).


## Building the project

Once you've got Cargo installed, building the project is a simple `cargo` invocation away:
```
$ cargo build --release
```
Make sure not to forget the `--release`, or you would build without optimizations otherwise.

The `lianad` and `liana-cli` binaries will be in the `target/` directory at the root of the
repository:
```
$ ls target/release/
build  deps  examples  incremental  liana-cli  liana-cli.d  lianad  lianad.d  libliana.d  libliana.rlib
```

To build the GUI, do the same but in the [`gui/`](../gui/) folder present at the root of the
repository:
```
$ cd gui/
$ cargo build --release
$ ls target/release/
build  deps  examples  incremental  liana-gui  liana-gui.d  libliana_gui.d  libliana_gui.rlib
```
