# Windows packaging and distribution

We distribute the Windows application as a `.exe` installer, built on Linux inside the Nix
reproducible build (`nix build .#liana.x86_64-pc-windows-gnu`). The approach mirrors what Bitcoin
Core does for its Windows installer: the installer is produced in the reproducible environment, the
codesigning signature is published detached from it, and anyone can re-attach the signature to a
local reproducible build to verify the signed release.

| Bitcoin Core | Liana |
|---|---|
| `share/setup.nsi.in`, compiled with `makensis` in Guix | `installer.nsi`, compiled with `makensis` in `nix/windows-installer.nix` |
| `bitcoin-<v>-win64-setup-unsigned.exe` | `liana-<v>-setup-noncodesigned.exe` |
| `contrib/windeploy/detached-sig-create.sh` | `sign.sh <target> osslsigncode` |
| `bitcoin-detached-sigs` repository | `liana-<v>-setup.exe.pem` published next to the installer |
| `guix-codesign` (attach the detached signature) | `attach-signature.sh` |

## Building the installer

[NSIS](https://nsis.sourceforge.io/) (`pkgs.nsis`) compiles the `installer.nsi` script into a
self-contained `setup.exe` without needing Windows: `makensis` runs natively and links the
prebuilt Windows stub shipped with NSIS. `nix/windows-installer.nix` stages the cross-compiled
executable, the `LICENCE`, the `README.md` and the icon, runs `makensis -WX` (warnings are
errors), then extracts the result with 7-Zip and checks that the extracted executable is
byte-identical to the input.

The installer is per-machine: it installs under `C:\Program Files\<Product>`, adds a Start Menu
shortcut and an "Apps & features" entry, and replaces a previous version in place. Command line
usage on Windows:

```
liana-<v>-setup.exe /S                    # silent install (no window, application not started)
liana-<v>-setup.exe /S /D=C:\Liana        # silent install elsewhere (first install only, no quotes)
"C:\Program Files\Liana\uninstall.exe" /S  # silent uninstall
```

## Installer flow

Double-clicking the installer asks nothing. The installer requests elevation in its manifest, so
Windows shows its elevation prompt with the product icon and name and Wizardsardine as verified
publisher, then a single window with the product icon shows the copy progress and closes by
itself. On success the application is started through `explorer.exe`, which runs it as the
logged-in user rather than with the installer's elevated token. Silent installs (`/S`) never
start the application. On failure the window stays open with the error.

What the script does, in order:

- `.onInit`: refuses 32-bit Windows, reads the "Apps & features" key of a previous install,
  refuses to downgrade (the installed `DisplayVersion` is newer than ours), and reuses the
  previous install location.
- Install: runs the previous version's `uninstall.exe` silently if there is one, so no stale
  file survives an upgrade, then copies the executable, `LICENCE` and `README.md`, writes the
  uninstaller and the Start Menu shortcut, and registers the "Apps & features" entry (name,
  version, icon, publisher, URL, install location, uninstall strings, size).
- Uninstall: deletes exactly the files it installed, the shortcut and the registry key. The
  install directory is removed only if empty. The wallet data directory (`%APPDATA%\Liana`, shared
  by Liana and Liana Business) is kept unless the user answers Yes to a prompt whose default is No;
  a silent uninstall never asks and never removes it.

The installer and the uninstaller are 32-bit programs (that is what NSIS produces), so the script
switches to the 64-bit registry view and `$PROGRAMFILES64` explicitly. If the application is
running during an upgrade or an uninstall, the executable cannot be replaced: the uninstaller
aborts before removing anything else, and the installer refuses to register the new version over
the old files (`AllowSkipFiles off`, so a silent install fails instead of skipping the locked
file). Close the application first.

## Keeping the installer reproducible

`makensis` writes no build timestamp: the PE header comes from the prebuilt stub and the only
dates in the archive are the modification times of the files added with `File`, which are
`touch`ed to `SOURCE_DATE_EPOCH` before packaging, as Bitcoin Core does. The `VERSIONINFO`
resource carries the crate version (`major.minor.build.0`). Two builds of the same inputs are
byte-identical. To check determinism, rebuild the derivation and compare:

```
nix build .#liana.x86_64-pc-windows-gnu
nix build --rebuild .#liana.x86_64-pc-windows-gnu
```

Useful inspection commands from the `release` dev shell (`nix develop .#release`) or any machine
with 7-Zip:

```
7zz l liana-<v>-setup.exe                # list the payload and the NSIS script
7zz x -oout/ liana-<v>-setup.exe         # extract the payload
osslsigncode verify -in liana-<v>-setup.exe   # signature, timestamp and version info
```

## Codesigning

`release.sh` copies the reproducible installer to `release_assets/liana-<v>-setup-noncodesigned.exe`.
Signing is done with [osslsigncode](https://github.com/mtrojnar/osslsigncode), provided by the
`release` dev shell, with the EV code signing key on the Certum smart card:

```
./contrib/release/sign.sh liana osslsigncode
```

osslsigncode prompts for the card PIN. The script finds the only certificate on the card and the
key behind it, reads the certificate from the card, fetches the issuing CA from the certificate's
CA Issuers URL to build the chain, and produces `liana-<v>-setup.exe`
(signed, RFC 3161 timestamped, `TIMESTAMP_SERVER` overrides the timestamp authority) and
`liana-<v>-setup.exe.pem`, the detached signature. The signed installer is verified with
`osslsigncode verify` against the CA bundle of the release shell (`TRUST_BUNDLE` overrides it),
for both the signer chain and the timestamp authority chain, and the script checks that
re-attaching the detached signature to the non-codesigned installer reproduces the signed one byte
for byte. Only then are the two files moved to `release_assets/` and their entries written to the
shasums, replacing any entry from a previous run. Only the installer is signed; the executable it
installs is not, which is also what Bitcoin Core does.

If the card holds several certificates, pass the key's PKCS#11 URI, e.g.
`'pkcs11:token=<label>;object=<key_label>;type=private'`, as the only argument. The script also
accepts `<cert_path> <key_path>` as PEM files for a key that is not on a card.

### The detached signature

`liana-<v>-setup.exe.pem` is the Authenticode signature extracted from the signed installer: a PKCS#7
structure holding the signing certificate and its issuing CA, the signature over the installer's
hash and the timestamp authority's countersignature. It is public and cannot sign anything else,
since it is bound to this installer's hash. It plays the role of Bitcoin Core's
`bitcoin-detached-sigs` repository.

Publish it on the Github release next to the installers, so that the Windows set is three files,
all listed in the gpg-signed shasums:

| File | Purpose |
|---|---|
| `liana-<v>-setup-noncodesigned.exe` | the reproducible build, what `nix build` gives anyone |
| `liana-<v>-setup.exe` | the codesigned installer Windows users download |
| `liana-<v>-setup.exe.pem` | the detached signature, for verifiers |

The signed installer is the reproducible one plus the embedded signature. Anyone can rebuild the
installer, attach the published signature and check that the result is byte for byte the signed
installer we distribute, which proves it contains exactly the reproducibly built payload. Without
the `.pem` a verifier could only compare non-codesigned builds and would have to trust that the
signed one differs solely by its signature.

```
./contrib/release/windows/attach-signature.sh ./result/x86_64-pc-windows-gnu/liana-gui-setup.exe liana-<v>-setup.exe.pem /tmp/liana-setup.exe
sha256sum /tmp/liana-setup.exe liana-<v>-setup.exe
```

`attach-signature.sh` accepts an optional CA bundle as fourth argument to also run
`osslsigncode verify` on the result, checking both the signer and the timestamp authority chains
against it.

### The Certum smart card

The EV code signing key is on a Certum cryptoCertum card in an ACS ACR40T reader. The card is
not supported by OpenSC, so the signer needs Certum's own PKCS#11 module, packaged by
`nix/certum-pkcs11.nix` from the proCertum CardManager Linux installer. Verifiers never need it.

One-time setup on the signing machine:

1. Enable the smart card daemon (`services.pcscd.enable = true;` on NixOS) and plug the reader in.
2. Download proCertum CardManager for Linux (Ubuntu/Debian x86_64 `.bin`) from
   https://support.certum.eu/en/?p=6595 and add it to the Nix store:
   `nix-store --add-fixed sha256 proCertumCardManager-2.2.15-x86_64-ubuntu.bin`. A newer version
   needs the `version` and `sha256` in `nix/certum-pkcs11.nix` updated.
3. Check the card from the `release` dev shell (`nix develop .#release`):

   ```
   export PKCS11_MODULE="$(nix build --no-link --print-out-paths .#certum-pkcs11)/lib/libcrypto3PKCS.so"
   pkcs11-tool --module "$PKCS11_MODULE" -O
   ```

   This lists the public key and the certificate. The `uri:` line of the certificate object, with
   `type=cert` replaced by `type=private`, is the key URI `sign.sh` derives (the key shares the
   certificate's id and label).

Code signing certificates are on the card's "common profile" (`libcrypto3PKCS.so`). The "secure
profile" module `libcryptoCertum3PKCS.so` from the same package is for qualified signatures and
is not used. `sign.sh` builds `.#certum-pkcs11` itself when `PKCS11_MODULE` is unset; set it to
use another module. The card locks after a few wrong PINs.
