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

        toolchain = fenix.packages.${system}.fromToolchainFile {
          file = ./rust-toolchain.toml;
          sha256 = "sha256-Hn2uaQzRLidAWpfmRwSRdImifGUCAb9HeAqTYFXWeQk=";
        };

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
          cargoLock = ./Cargo.lock;
          cargoVendorDir = craneLib.vendorCargoDeps {
            src = ./.;
          };
        };

        lianaInfo = craneLib.crateNameFromCargoToml { cargoToml = ./liana-gui/Cargo.toml; };

        x86_64-pc-windows-gnu = craneLib.buildPackage {
          inherit (commonBuildSettings) src strictDeps doCheck;
          inherit (lianaInfo) pname version;

          SOURCE_DATE_EPOCH = 1;
          CARGO_BUILD_TARGET = "x86_64-pc-windows-gnu";
          # CARGO_BUILD_RUSTFLAGS = "-C link-arg=-Wl,--no-insert-timestamp -C link-arg=-L${pkgs.pkgsCross.mingwW64.windows.pthreads}/lib";
          # CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-C link-arg=-Wl,--no-insert-timestamp -C link-arg=-L${pkgs.pkgsCross.mingwW64.windows.pthreads}/lib";
          # CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-C link-arg=-Wl,--no-insert-timestamp -C link-arg=-Wl,--image-base,0x10000 -C link-arg=-L${pkgs.pkgsCross.mingwW64.windows.pthreads}/lib";
          CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-C link-arg=-Wl,--no-insert-timestamp -C link-arg=-L${pkgs.pkgsCross.mingwW64.windows.pthreads}/lib";

          HOST_CC = "${pkgs.stdenv.cc}/bin/cc";
          TARGET_CC = "${pkgs.pkgsCross.mingwW64.stdenv.cc}/bin/${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}cc";

          AR_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}ar";
          TOOLKIT_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.bintools.bintools}/bin";
          WINDRES_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}windres";

          cargoExtraArgs = "-p liana-gui";
          depsBuildBuild = with pkgs; [
            pkgsCross.mingwW64.stdenv.cc
            pkgsCross.mingwW64.buildPackages.binutils
            pkgsCross.mingwW64.buildPackages.binutils-unwrapped
          ];

          installPhaseCommand = ''
            mkdir -p $out/x86_64-pc-windows-gnu
            cp target/x86_64-pc-windows-gnu/release/liana-gui.exe $out/x86_64-pc-windows-gnu
          '';
        };

        x86_64-apple-darwin = craneLib.buildPackage {
          inherit (commonBuildSettings) src strictDeps doCheck;
          inherit (lianaInfo) pname version;

          CARGO_BUILD_TARGET = "x86_64-apple-darwin";
          buildPhaseCargoCommand = "cargo zigbuild --release --message-format json-render-diagnostics";
          doNotPostBuildInstallCargoBinaries = true;

          depsBuildBuild = [
            pkgs.zig
            pkgs.cargo-zigbuild
            pkgs.darwin.xcode_12_2
          ];

          preBuild = ''
            export SDKROOT=${pkgs.darwin.xcode_12_2}/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk
            export MACOSX_DEPLOYMENT_TARGET=11.0
            export CFLAGS_x86_64_apple_darwin="-isysroot $SDKROOT -iframework $SDKROOT/System/Library/Frameworks"
            export CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-isysroot -C link-arg=$SDKROOT -C link-arg=-F$SDKROOT/System/Library/Frameworks"
            export XDG_CACHE_HOME=$TMPDIR/xdg_cache
            mkdir -p $XDG_CACHE_HOME
            export CARGO_ZIGBUILD_CACHE_DIR=$TMPDIR/cargo-zigbuild-cache
            mkdir -p $CARGO_ZIGBUILD_CACHE_DIR
            export CC=zigcc
            export CXX=zigc++
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
          inherit (lianaInfo) pname version;

          CARGO_BUILD_TARGET = "aarch64-apple-darwin";
          buildPhaseCargoCommand = "cargo zigbuild --release --message-format json-render-diagnostics";
          doNotPostBuildInstallCargoBinaries = true;

          depsBuildBuild = [
            pkgs.zig
            pkgs.cargo-zigbuild
            pkgs.darwin.xcode_12_2
          ];

          preBuild = ''
            export SDKROOT=${pkgs.darwin.xcode_12_2}/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk
            export MACOSX_DEPLOYMENT_TARGET=11.0
            export CFLAGS_aarch64_apple_darwin="-isysroot $SDKROOT -iframework $SDKROOT/System/Library/Frameworks"
            export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-isysroot -C link-arg=$SDKROOT -C link-arg=-F$SDKROOT/System/Library/Frameworks"
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

        # Common build inputs for all shells
        commonBuildInputs = with pkgs; [
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
        ];

        # Minimal shell without Rust toolchain
        minimalShell = pkgs.mkShell rec {
          buildInputs = commonBuildInputs;

          LD_LIBRARY_PATH =
            builtins.foldl' (a: b: "${a}:${b}/lib") "${pkgs.vulkan-loader}/lib" buildInputs;
        };

        # Full development shell with Rust toolchain
        devShell = pkgs.mkShell rec {
          buildInputs = commonBuildInputs ++ [ toolchain ];

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
          ] ++ [ toolchain ];
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
          minimal = minimalShell;
          release = releaseShell;
          default = devShell;
        };
      }
    );
}
