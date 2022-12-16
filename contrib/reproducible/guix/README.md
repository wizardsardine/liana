## Bootstrappable Liana builds

This repository contains the scripts to perform [reproducible](https://reproducible-builds.org/) and
[bootstrappable](https://bootstrappable.org/) builds of Liana using [Guix](https://guix.gnu.org/), a
functional package manager.

For a short high-level introduction to the purpose of bootstrappable builds, see [this
talk](https://www.youtube.com/watch?v=I2iShmUTEl8) by Carl Dong in the context of the Bitcoin Core
project.

For now, only `x86_64` Linux binaries are supported. We aim to extend bootstrappable builds to more
targets in the future.


### Installation

See the very [detailed document about GUIX installation in the Bitcoin Core
project](https://github.com/bitcoin/bitcoin/blob/master/contrib/guix/INSTALL.md). Almost all of it
is directly applicable to Liana as well.


### Usage

First of all, you need to decide on the amount of trust in the building process. Of course this
decision needs to fall within a broader threat model, but it is still interesting to consider.

*(Note: this section was taken from the Bitcoin Core documentation and initially written by Carl
Dong.)*

#### Choosing your security model

No matter how you installed Guix, you need to decide on your security model for
building packages with Guix.

Guix allows us to achieve better binary security by using our CPU time to build
everything from scratch. However, it doesn't sacrifice user choice in pursuit of
this: users can decide whether or not to use **substitutes** (pre-built
packages).

##### Option 1: Building with substitutes

###### Step 1: Authorize the signing keys

Depending on the installation procedure you followed, you may have already
authorized the Guix build farm key. In particular, the official shell installer
script asks you if you want the key installed, and the debian distribution
package authorized the key during installation.

You can check the current list of authorized keys at `/etc/guix/acl`.

At the time of writing, a `/etc/guix/acl` with just the Guix build farm key
authorized looks something like:

```lisp
(acl
 (entry
  (public-key
   (ecc
    (curve Ed25519)
    (q #8D156F295D24B0D9A86FA5741A840FF2D24F60F7B6C4134814AD55625971B394#)
    )
   )
  (tag
   (guix import)
   )
  )
 )
```

If you've determined that the official Guix build farm key hasn't been
authorized, and you would like to authorize it, run the following as root:

```
guix archive --authorize < /var/guix/profiles/per-user/root/current-guix/share/guix/ci.guix.gnu.org.pub
```

If
`/var/guix/profiles/per-user/root/current-guix/share/guix/ci.guix.gnu.org.pub`
doesn't exist, try:

```sh
guix archive --authorize < <PREFIX>/share/guix/ci.guix.gnu.org.pub
```

Where `<PREFIX>` is likely:
- `/usr` if you installed from a distribution package
- `/usr/local` if you installed Guix from source and didn't supply any
  prefix-modifying flags to Guix's `./configure`

For dongcarl's substitute server at https://guix.carldong.io, run as root:

```sh
wget -qO- 'https://guix.carldong.io/signing-key.pub' | guix archive --authorize
```

To remove previously authorized keys, simply edit `/etc/guix/acl` and remove the
`(entry (public-key ...))` entry.

###### Step 2: Specify the substitute servers

Once its key is authorized, the official Guix build farm at
https://ci.guix.gnu.org is automatically used unless the `--no-substitutes` flag
is supplied. This default list of substitute servers is overridable both on a
`guix-daemon` level and when you invoke `guix` commands. See examples below for
the various ways of adding dongcarl's substitute server after having [authorized
his signing key](#step-1-authorize-the-signing-keys).

Change the **default list** of substitute servers by starting `guix-daemon` with
the `--substitute-urls` option (you will likely need to edit your init script):

```sh
guix-daemon <cmd> --substitute-urls='https://guix.carldong.io https://ci.guix.gnu.org'
```

Override the default list of substitute servers by passing the
`--substitute-urls` option for invocations of `guix` commands:

```sh
guix <cmd> --substitute-urls='https://guix.carldong.io https://ci.guix.gnu.org'
```

For scripts under `./contrib/reproducible/guix`, set the `SUBSTITUTE_URLS` environment
variable:

```sh
export SUBSTITUTE_URLS='https://guix.carldong.io https://ci.guix.gnu.org'
```

##### Option 2: Disabling substitutes on an ad-hoc basis

If you prefer not to use any substitutes, make sure to supply `--no-substitutes`
like in the following snippet. The first build will take a while, but the
resulting packages will be cached for future builds.

For direct invocations of `guix`:
```sh
guix <cmd> --no-substitutes
```

The build script doesn't yet allow you to provide this via an environment variable, you'd
have to modify it yourself (or contribute a patch :p).

##### Option 3: Disabling substitutes by default

`guix-daemon` accepts a `--no-substitutes` flag, which will make sure that,
unless otherwise overridden by a command line invocation, no substitutes will be
used.

If you start `guix-daemon` using an init script, you can edit said script to
supply this flag.


#### Building Liana

For both the daemon (`lianad`) and the GUI (`liana-gui`), the [`guix-build.sh`](./guix-build.sh)
script will vendor the dependencies of the project (as pinned in the `Cargo.lock`) and start a [GUIX
container](https://guix.gnu.org/manual/devel/en/html_node/Invoking-guix-shell.html) that will run
the `build.sh` script that will take care of building the dependencies and the project using `cargo`.

To start a build, simply run the `guix-build.sh` script from the root of the repository:
```
$ ./contrib/reproducible/guix/guix-build.sh
```

The script shouldn't contain any bash-ism, so it should work with other shells as well.


#### Customization

Environment variables are available to configure the build.

`BUILD_ROOT` allows you to specify the root folder that will contain the vendored dependencies'
source, the build cache as well as the resulting binaries (in `$BUILD_ROOT/out`).

`JOBS` allows you to specify the number of cores to dedicate to the build both for bootstrapping the
toolchain with GUIX and building the project using `cargo`.
