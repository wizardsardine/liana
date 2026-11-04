# Windows packaging and distribution

We distribute the Windows application as a `.msi` installer, built on Linux inside the Nix
reproducible build (`nix build .#liana.x86_64-pc-windows-gnu`). The approach mirrors what Bitcoin
Core does for its Windows installer: the installer is produced in the reproducible environment, the
codesigning signature is published detached from it, and anyone can re-attach the signature to a
local reproducible build to verify the signed release.

| Bitcoin Core | Liana |
|---|---|
| `share/setup.nsi.in`, compiled with `makensis` in Guix | `installer.wxs`, compiled with `wixl` in `nix/windows-msi.nix` |
| `bitcoin-<v>-win64-setup-unsigned.exe` | `liana-<v>-noncodesigned.msi` |
| `contrib/windeploy/detached-sig-create.sh` | `sign.sh <target> osslsigncode <cert> <key>` |
| `bitcoin-detached-sigs` repository | `liana-<v>.msi.pem` published next to the installer |
| `guix-codesign` (attach the detached signature) | `attach-signature.sh` |

## Building the installer

`wixl` from [GNOME msitools](https://gitlab.gnome.org/GNOME/msitools) compiles a WiX 3 `.wxs`
description into an `.msi` without needing Windows. `nix/windows-msi.nix` stages the
cross-compiled executable, the `LICENCE`, the `README.md` and the icon, runs `wixl`, then checks
the result with `msiinfo` and `msiextract` (the extracted executable must be byte-identical to the
input).

The installer is per-machine: it installs under `C:\Program Files\<Product>`, adds a Start Menu
shortcut and an "Apps & features" entry, and upgrades a previous version in place
(`MajorUpgrade`). `wixl` has no wizard pages, so running the installer shows the Windows Installer
progress dialog only. Command line usage on Windows:

```
msiexec /i liana-<v>.msi /l*v install.log   # install with a verbose log
msiexec /i liana-<v>.msi /qn                # silent install
msiexec /x liana-<v>.msi /qn                # silent uninstall
```

## Keeping the installer reproducible

An MSI is not reproducible by default. Three things are pinned:

- GUIDs: `wixl` draws random UUIDs for every `Id="*"`. The `UpgradeCode` and the component GUIDs
  are constants in `nix/liana.nix` and `nix/liana-business.nix`; never change them, they are what
  ties upgrades together. The `ProductCode` and `PackageCode` must change with every version, so
  they are derived from the `UpgradeCode` and the version with sha256.
- Timestamps: the summary information and the CAB entries take their times from
  `SOURCE_DATE_EPOCH`, and the staged files are `touch`ed to it before packaging, as Bitcoin Core
  does before `makensis`.
- Version: the MSI `ProductVersion` must be numeric `major.minor.build`. It comes from the crate
  version in `Cargo.toml` with any pre-release suffix dropped, so release candidates and the final
  release of a cycle share it; `AllowSameVersionUpgrades` lets the final release install over an
  rc.

To check determinism, rebuild the derivation and compare:

```
nix build .#liana.x86_64-pc-windows-gnu
nix build --rebuild .#liana.x86_64-pc-windows-gnu
```

Useful inspection commands from the `release` dev shell (`nix develop .#release`) or any machine
with msitools:

```
msiinfo suminfo liana-gui.msi            # summary information (codes, times, platform)
msiinfo export liana-gui.msi Property    # Property table (version, ARP entries, ...)
msiextract -C out/ liana-gui.msi         # extract the payload
```

## Codesigning

`release.sh` copies the reproducible installer to `release_assets/liana-<v>-noncodesigned.msi`.
Signing is done with [osslsigncode](https://github.com/mtrojnar/osslsigncode), provided by the
`release` dev shell:

```
./contrib/release/sign.sh liana osslsigncode <cert_path> <key_path>
```

This produces `liana-<v>.msi` (signed, RFC 3161 timestamped, `TIMESTAMP_SERVER` overrides the
timestamp authority) and `liana-<v>.msi.pem`, the detached signature, and checks that re-attaching
the detached signature to the non-codesigned installer reproduces the signed one byte for byte.
Both files are appended to the shasums. Only the installer is signed; the executable it installs
is not, which is also what Bitcoin Core does.

Anyone can then verify the published signed installer from a local reproducible build:

```
./contrib/release/windows/attach-signature.sh ./result/x86_64-pc-windows-gnu/liana-gui.msi liana-<v>.msi.pem /tmp/liana.msi
sha256sum /tmp/liana.msi liana-<v>.msi
```

`attach-signature.sh` accepts an optional CA bundle as fourth argument to also run
`osslsigncode verify` on the result.
