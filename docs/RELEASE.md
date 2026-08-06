This document details the release process.

- First bump the version of the Coincube daemon/library in master. Don't forget the release script.
  (Example: [the PR for v5](https://github.com/wizardsardine/liana/pull/1034).)
- Update the [`CHANGELOG.md`](../CHANGELOG.md) in master with the release notes for this release.
  (Example: [the PR for v5](https://github.com/wizardsardine/liana/pull/1034).)
- Bump the version of the GUI in master to get the version bump from the Coincube library (this needs
  the version bump of the Coincube library to have been merged in master). (Example: [the PR for
  v5](https://github.com/wizardsardine/liana/pull/1036).)
- Create a new branch forking from master dedicated to this release and the following point
  release(s): `MAJOR.x`. (For instance `5.x` for v5.)
- Update the version of the Coincube daemon/library in this branch to use the `-rc1` suffix for the
  version string. (Don't forget the release script.) (Example: [the PR for
  v5](https://github.com/wizardsardine/liana/pull/1037).)
- Update the GUI to use the latest version of this branch. Don't forget to update both the
  Cargo.toml and the reproducible build. Don't forget to `cargo build` after `cargo update -p
coincube-core`. (Example: [the PR for v5](https://github.com/wizardsardine/liana/pull/1038).)
- Make sure the documentation is up to date (build doc, usage doc, `TRY.md`, etc..)
- Create a `vA.Brc1` tag on this branch and push it to the Github repo.
- Make a reproducible release build on this tag using the
  [`contrib/release/release.sh`](../contrib/release/release.sh) script. Don't forget to set the
  `VERSION` and `MAC_CODESIGN` variables appropriately if they aren't already.
- Publish a pre-release for this tag on Github (https://github.com/wizardsardine/liana/releases)
  with the reproducibly built binaries.
- If bugs are discovered when testing the release candidate, fix them in master and backport them to
  the release branch. (Example: [this PR for v5](https://github.com/wizardsardine/liana/pull/1066).)
- If needed, repeat this process with new release candidates.
- Update documentation material where the former version is mentioned as being the latest.
- If applicable, update other documentation material (for instance the list of supported signing
  devices).
- Remove the "rc" suffix in the version string on the release branch. Don't forget the release
  script. (Example: [this PR for v5](https://github.com/wizardsardine/liana/pull/1067).)
- Update the Coincube version in the GUI to the latest of the release branch. (Don't forget to `cargo
build` after having `cargo update -p coincube-core`.)
- Create a new `vA.B` tag on the tip of the release branch. Don't forget to sign the tag and include
  the release notes.
- Make a reproducible release build for this tag.
- Create a Github release for this tag. Don't forget to include the release notes as well as
  instructions on what binaries a user should pick.
- If possible push the Coincube library to [crates.io](https://crates.io).
- Celebrate.

## Artifact contents check

Before publishing, confirm each artifact actually contains **both** binaries. CI asserts this on
every build, but a hand-made release does not go through those steps, and nothing else catches it:
a bundle missing `coincube-spark-bridge` signs, notarizes and passes `spctl` exactly like a good
one — it just has a Spark wallet that never starts. See
[`SPARK_WALLET.md`](./SPARK_WALLET.md#packaging).

```
# macOS — inside the mounted DMG
ls "/Volumes/Tenshu/Tenshu.app/Contents/MacOS/"        # Coincube + coincube-spark-bridge
codesign --verify --strict "/Volumes/Tenshu/Tenshu.app/Contents/MacOS/coincube-spark-bridge"

# Linux
tar tzf tenshu-<version>-<target>.tar.gz               # coincube + coincube-spark-bridge

# Windows — after installing
dir "C:\Program Files\Tenshu\bin"                      # coincube.exe + coincube-spark-bridge.exe
```

Then check the bridge runs, which needs no API key, mnemonic or network:

```
echo '{"type":"request","id":1,"method":"shutdown"}' | ./coincube-spark-bridge
# {"type":"response","id":1,"ok":{"kind":"shutdown","data":{}}}
```

Finally, open the Spark tab once on a machine **with no repository checkout**. A machine that has
one satisfies the gui's development fallback path and will report success regardless of what the
artifact contains.

In order to build the release assets:

```
nix develop .#release
./contrib/release/release.sh
```
