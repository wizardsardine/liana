This document details the release process.

The repository releases two targets from the same tree:

- `liana` (the daemon, the CLI and liana-gui), tagged `vMAJOR.MINOR`, whose release notes live in
  [`CHANGELOG.md`](../CHANGELOG.md).
- `liana-business` (liana-business and its installer), tagged `business-vMAJOR.MINOR`, whose
  release notes live in [`liana-business/CHANGELOG.md`](../liana-business/CHANGELOG.md).

Both targets share a release branch: `MAJOR.x` is named after the `liana` major, and the
`liana-business` tags of that cycle are created on the same branch. Their version numbers are
independent of each other, so the same branch can carry, say, `v16.0` and `business-v2.0`.

## Releasing `liana`

- First bump the version of the Liana crates in master: `liana`, `lianad`, `liana-gui`, `liana-ui`
  and `liana-connect` all move to the new major in their `Cargo.toml`. Run `cargo build` to update
  the `Cargo.lock`, and update the version asserted by `tests/test_rpc.py::test_getinfo`. The
  crates are path dependencies of a single workspace, so they are all bumped in the same commit.
  (Example: [the PR for v15](https://github.com/wizardsardine/liana/pull/2221).)
- Update the [`CHANGELOG.md`](../CHANGELOG.md) in master with the release notes for this release.
  (Example: [the PR for v15](https://github.com/wizardsardine/liana/pull/2221).)
- Create a new branch forking from master dedicated to this release and the following point
  release(s): `MAJOR.x`. (For instance `15.x` for v15.)
- Update the version string in this branch to use the `-rc1` suffix: the `VERSION` constants in
  `lianad/src/lib.rs` and `liana-gui/src/lib.rs`, and the version asserted by
  `tests/test_rpc.py::test_getinfo`. (Example: [the PR for
  v15](https://github.com/wizardsardine/liana/pull/2224).)
- Make sure the documentation is up to date (build doc, usage doc, `TRY.md`, etc..)
- Create a `vA.Brc1` tag on this branch and push it to the Github repo.
- Make a reproducible release build on this tag using the
  [`contrib/release/release.sh`](../contrib/release/release.sh) script: `./contrib/release/release.sh liana`.
  The script derives the version from the tag and re-enters the `release` dev shell by itself, so
  there is nothing to configure. Sign the assets with
  [`contrib/release/sign.sh`](../contrib/release/sign.sh): `./contrib/release/sign.sh liana gpg`.
- Publish a pre-release for this tag on Github (https://github.com/wizardsardine/liana/releases)
  with the reproducibly built binaries.
- If bugs are discovered when testing the release candidate, fix them in master and backport them to
  the release branch. (Example: [this PR for v5](https://github.com/wizardsardine/liana/pull/1066).)
- If needed, repeat this process with new release candidates.
- Update documentation material where the former version is mentioned as being the latest.
- If applicable, update other documentation material (for instance the list of supported signing
  devices).
- Remove the "rc" suffix from the `VERSION` constants and from `tests/test_rpc.py` on the release
  branch. (Example: [this PR for v15](https://github.com/wizardsardine/liana/pull/2235).)
- Create a new `vA.B` tag on the tip of the release branch. Don't forget to sign the tag and include
  the release notes.
- Make a reproducible release build for this tag.
- Create a Github release for this tag. Don't forget to include the release notes as well as
  instructions on what binaries a user should pick.
- If necessary, write a companion blog post on [the blog](https://wizardsardine.com/blog/), update
  the link to the binaries on [the website](https://wizardsardine.com/liana/), brag on social
  medias.
- If possible push the Liana library to [crates.io](https://crates.io).
- Update the package managers with the new version. As of this writing we only update the [AUR
  package](https://aur.archlinux.org/packages/liana-bin) ourselves.
- Celebrate.

## Releasing `liana-business`

The flow is the same, with its own version numbering, on the release branch of the cycle:

- Bump `liana-business/Cargo.toml` and `liana-business/business-installer/Cargo.toml` in master,
  and run `cargo build` to update the `Cargo.lock`.
- Update [`liana-business/CHANGELOG.md`](../liana-business/CHANGELOG.md) in master.
- On the release branch, set the `-rc1` suffix in the `VERSION` constants of
  `liana-business/src/lib.rs` and `liana-business/business-installer/src/lib.rs`. (Example: [the PR
  for v1](https://github.com/wizardsardine/liana/pull/2225).)
- Create a `business-vA.Brc1` tag on this branch and push it.
- Build and sign with the `liana-business` target: `./contrib/release/release.sh liana-business`,
  then `./contrib/release/sign.sh liana-business rcodesign <cert_path> <key_path> <apikey_json_path>`.
- Publish the pre-release, iterate on release candidates, then drop the "rc" suffix and tag
  `business-vA.B` the same way.

## Building the release assets

```
./contrib/release/release.sh <liana|liana-business>
```

The script requires `guix` on the host for the Linux binaries, and re-enters `nix develop .#release`
on its own for the packaging tools. It derives the version from `git describe`: an exact tag match
gives `vA.B` -> `A.B`, otherwise it falls back to the latest matching tag suffixed with the short
commit hash.
