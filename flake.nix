{
  description = "Dev shell to help contributing to liana";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    lipo.url = "github:edouardparis/lipo-flake";
  };

  outputs = { self, nixpkgs, flake-utils, crane, fenix, lipo, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; config = { allowUnfree = true; };};

        inherit (pkgs) lib;

        toolchain = with fenix.packages.${system};
          combine [
            minimal.rustc
            minimal.cargo
            targets.x86_64-pc-windows-gnu.latest.rust-std
            targets.aarch64-apple-darwin.latest.rust-std
            targets.x86_64-apple-darwin.latest.rust-std
          ];

        craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
        commonBuildSettings = {
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              (craneLib.fileset.commonCargoSources ./.)
              (lib.fileset.maybeMissing ./liana-ui/static)
            ];
          };
          strictDeps = true;
          doCheck = false;
        };

        x86_64-pc-windows-gnu = craneLib.buildPackage {
          inherit (commonBuildSettings) src strictDeps doCheck;

          CARGO_BUILD_TARGET = "x86_64-pc-windows-gnu";
          CARGO_BUILD_RUSTFLAGS = "-C link-arg=-Wl,--no-insert-timestamp";
          TARGET_CC = "${pkgs.pkgsCross.mingwW64.stdenv.cc}/bin/${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}cc";

          pname = "liana-gui";
          cargoExtraArgs = "-p liana-gui";
          depsBuildBuild = with pkgs; [
            pkgsCross.mingwW64.stdenv.cc
            pkgsCross.mingwW64.windows.pthreads
          ];

          installPhaseCommand = ''
            mkdir -p $out/x86_64-pc-windows-gnu
            cp target/x86_64-pc-windows-gnu/release/liana-gui.exe $out/x86_64-pc-windows-gnu
          '';
        };

        x86_64-apple-darwin = craneLib.buildPackage {
          inherit (commonBuildSettings) src strictDeps doCheck;

          CARGO_BUILD_TARGET = "x86_64-apple-darwin";
          buildPhaseCargoCommand = "cargo zigbuild --release --message-format json-render-diagnostics";

          depsBuildBuild = [
            pkgs.zig
            pkgs.cargo-zigbuild
            pkgs.darwin.xcode_12_2
          ];

          preBuild = ''
            export SDKROOT=${pkgs.darwin.xcode_12_2}/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk

            export XDG_CACHE_HOME=$TMPDIR/xdg_cache
            mkdir -p $XDG_CACHE_HOME
            export CARGO_ZIGBUILD_CACHE_DIR=$TMPDIR/cargo-zigbuild-cache
            mkdir -p $CARGO_ZIGBUILD_CACHE_DIR
            export CC=zigcc
            export CXX=zigc++

            # rcodesign needs place to sign binary
            export RUSTFLAGS="-C link-arg=-Wl,-headerpad_max_install_names"
          '';

          installPhaseCommand = ''
            mkdir -p $out/x86_64-apple-darwin
            cp target/x86_64-apple-darwin/release/liana-gui $out/x86_64-apple-darwin
            cp target/x86_64-apple-darwin/release/lianad $out/x86_64-apple-darwin
            cp target/x86_64-apple-darwin/release/liana-cli $out/x86_64-apple-darwin
          '';
        };

        aarch64-apple-darwin = craneLib.buildPackage {
          inherit (commonBuildSettings) src strictDeps doCheck;

          CARGO_BUILD_TARGET = "aarch64-apple-darwin";
          buildPhaseCargoCommand = "cargo zigbuild --release --message-format json-render-diagnostics";

          depsBuildBuild = [
            pkgs.zig
            pkgs.cargo-zigbuild
            pkgs.darwin.xcode_12_2
          ];

          preBuild = ''
            export SDKROOT=${pkgs.darwin.xcode_12_2}/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk

            export XDG_CACHE_HOME=$TMPDIR/xdg_cache
            mkdir -p $XDG_CACHE_HOME
            export CARGO_ZIGBUILD_CACHE_DIR=$TMPDIR/cargo-zigbuild-cache
            mkdir -p $CARGO_ZIGBUILD_CACHE_DIR
            export CC=zigcc
            export CXX=zigc++
          '';

          installPhaseCommand = ''
            mkdir -p $out/aarch64-apple-darwin
            cp target/aarch64-apple-darwin/release/liana-gui $out/aarch64-apple-darwin
            cp target/aarch64-apple-darwin/release/lianad $out/aarch64-apple-darwin
            cp target/aarch64-apple-darwin/release/liana-cli $out/aarch64-apple-darwin
          '';
        };

        universal2-apple-darwin = pkgs.runCommand "universal2-apple-darwin" {
          buildInputs = [ lipo.packages.${system}.lipo ];
          # Declare dependencies by referencing them in the command
          # No need to include x86_64-apple-darwin and aarch64-apple-darwin in buildInputs
          # because they are referenced directly
        } ''
          mkdir -p $out/universal2-apple-darwin

          # Combine liana-gui binaries
          lipo -output $out/universal2-apple-darwin/liana-gui -create \
            ${x86_64-apple-darwin}/x86_64-apple-darwin/liana-gui \
            ${aarch64-apple-darwin}/aarch64-apple-darwin/liana-gui
        '';

        devShell = pkgs.mkShell rec {
          buildInputs = with pkgs; [
            expat
            fontconfig
            freetype
            freetype.dev
            libGL
            pkg-config
            udev
            wayland
            libxkbcommon
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            xorg.libXrandr
            toolchain
          ];

          LD_LIBRARY_PATH =
            builtins.foldl' (a: b: "${a}:${b}/lib") "${pkgs.vulkan-loader}/lib" buildInputs;
        };

        releaseShell = pkgs.mkShell {
          buildInputs = [
            pkgs.zip
            pkgs.unzip
            pkgs.gnutar
            pkgs.dpkg
            pkgs.rcodesign
          ];
        };

      in {
        packages = {
          x86_64-pc-windows-gnu = x86_64-pc-windows-gnu;
          x86_64-apple-darwin = x86_64-apple-darwin;
          aarch64-apple-darwin = aarch64-apple-darwin;
          universal2-apple-darwin = universal2-apple-darwin;
          release = pkgs.buildEnv {
            name = "release";
            paths = [
              x86_64-pc-windows-gnu
              x86_64-apple-darwin
              aarch64-apple-darwin
              universal2-apple-darwin
            ];
          };
        };


        devShells = {
          dev = devShell;
          release = releaseShell;
          default = devShell;
        };
      }
    );
}
