# The bootstrapped Rust toolchain: mrustc (C++) -> rustc -> cross std, with no
# prebuilt rustc in the graph.
#
# Two toolchains come out of this:
#
#   toolchain       rustc + std for Linux and Windows. Buildable from source
#                   alone, so those two targets are bootstrappable.
#   toolchainApple  the same compiler plus std for the Apple targets. The
#                   compiler is bootstrapped, but linking needs the proprietary
#                   macOS SDK, so the macOS build is reproducible and not
#                   bootstrappable. See contrib/reproducible/README.md.
{ pkgs, lib }:
let
  sources = import ./sources.nix { inherit lib; };

  mrustc = pkgs.callPackage ./mrustc.nix { inherit sources; };
  mrustc-minicargo = pkgs.callPackage ./minicargo.nix { inherit mrustc; };

  # Split in two so the expensive compile is insulated from the install layout:
  # see rustc-stage1.nix.
  rustc-bootstrap-stage1-build = pkgs.callPackage ./rustc-stage1-build.nix {
    inherit mrustc mrustc-minicargo sources;
  };

  rustc-bootstrap-stage1 = pkgs.callPackage ./rustc-stage1.nix {
    inherit rustc-bootstrap-stage1-build sources;
  };

  rustc-bootstrap = pkgs.callPackage ./rustc.nix {
    inherit rustc-bootstrap-stage1 sources;
  };

  darwin-std = pkgs.callPackage ./darwin-std.nix {
    inherit rustc-bootstrap sources;
  };

  # rustc derives its sysroot from the realpath of its own driver library, so through
  # the join it would find rustc-bootstrap alone and miss the Apple `std` added here.
  toolchainApple = pkgs.symlinkJoin {
    name = "rustc-bootstrap-with-apple-std-${sources.rustc.version}";
    paths = [ rustc-bootstrap darwin-std ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    postBuild = ''
      for bin in rustc rustdoc; do
        rm $out/bin/$bin
        makeWrapper ${rustc-bootstrap}/bin/$bin $out/bin/$bin --add-flags "--sysroot $out"
      done
    '';
  };
in
{
  inherit
    sources
    mrustc
    mrustc-minicargo
    rustc-bootstrap-stage1-build
    rustc-bootstrap-stage1
    rustc-bootstrap
    darwin-std
    ;

  # What `crane.overrideToolchain` wants: a single package carrying cargo and rustc.
  toolchain = rustc-bootstrap;
  inherit toolchainApple;
}
