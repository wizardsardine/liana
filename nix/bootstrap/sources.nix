# Pinned sources and target list for the bootstrapped Rust toolchain.
#
# This is the only file to touch when moving the bootstrap to a new mrustc or a
# new rustc release. Fill the hashes with:
#
#   nix-prefetch-github thepowersgang mrustc --rev <tag>
#   nix-prefetch-url https://static.rust-lang.org/dist/rustc-<version>-src.tar.gz
#
# `mrustc.commit` is the commit the tag points at. mrustc's Makefile embeds it
# through `git show`, which is not available in the sandbox, so it is pinned
# here and substituted in.
{ lib }:
{
  # v0.12.0 ships `rustc-1.90.0-src.patch`, `rustc-1.90.0-overrides.toml` and the
  # matching `RUSTC_VERSION = 1.90.0` branches in `minicargo.mk`. Note nixpkgs
  # pins this very tree yet still marks its `mrustc-bootstrap` broken over a
  # 1.74 ceiling, which no longer applies.
  mrustc = {
    version = "0.12.0";
    tag = "v0.12.0";
    commit = "2d14b09a7e75166bec4413f48f61e3b3cd4de8ca";
    hash = "sha256-wqHTTnk9c1khLsN6e0v703tUoTlpncMwZPXTKEVZ33s=";
  };

  rustc = {
    version = "1.90.0";
    # Matches `MRUSTC_TARGET_VER`, which mrustc keys its per-release patches on.
    targetVersion = "1.90";
    hash = "sha256-eZqfnLpO1TUeBxBIvPa1VgdV2QCWSN7zOkB91JYfm34=";
  };

  # Targets whose `std` is cross-compiled from source by the bootstrapped rustc.
  # The Apple targets are built separately, in darwin-std.nix: they need the
  # Xcode SDK, which must not become a dependency of the Linux and Windows
  # toolchain.
  targets = {
    free = [
      "x86_64-unknown-linux-gnu"
      "x86_64-pc-windows-gnu"
    ];
    apple = [
      "x86_64-apple-darwin"
      "aarch64-apple-darwin"
    ];
  };
}
