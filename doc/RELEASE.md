This document details the release process.

- First bump the version of the Liana daemon/library in master. Don't forget the release script.
  (Example: [the PR for v5](https://github.com/wizardsardine/liana/pull/1034).)
- Update the [`CHANGELOG.md`](../CHANGELOG.md) in master with the release notes for this release.
  (Example: [the PR for v5](https://github.com/wizardsardine/liana/pull/1034).)
- Bump the version of the GUI in master to get the version bump from the Liana library (this needs
  the version bump of the Liana library to have been merged in master). (Example: [the PR for
  v5](https://github.com/wizardsardine/liana/pull/1036).)
- Create a new branch forking from master dedicated to this release and the following point
  release(s): `MAJOR.x`. (For instance `5.x` for v5.)
- Update the version of the Liana daemon/library in this branch to use the `-rc1` suffix for the
  version string. (Don't forget the release script.) (Example: [the PR for
  v5](https://github.com/wizardsardine/liana/pull/1037).)
- Update the GUI to use the latest version of this branch. Don't forget to update both the
  Cargo.toml and the reproducible build. Don't forget to `cargo build` after `cargo update -p
  liana`. (Example: [the PR for v5](https://github.com/wizardsardine/liana/pull/1038).)
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
- Update the Liana version in the GUI to the latest of the release branch. (Don't forget to `cargo
  build` after having `cargo update -p liana`.)
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

In order to build the release assets:

```
nix develop .#release
./contrib/release/release.sh
```
