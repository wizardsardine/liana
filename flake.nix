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
          sha256 = "sha256-Qxt8XAuaUR2OMdKbN4u8dBJOhSHxS+uS06Wl9+flVEk=";
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;

        lianaPackages = import ./nix/liana.nix {
          inherit craneLib pkgs lib;
          lipo = lipo.packages.${system}.lipo;
          rootPath = ./.;
        };

        lianaBusinessPackages = import ./nix/liana-business.nix {
          inherit craneLib pkgs lib;
          lipo = lipo.packages.${system}.lipo;
          rootPath = ./.;
        };

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
          libx11
          libxcursor
          libxi
          libxrandr
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

        guiRuntimeInputs = commonBuildInputs ++ (with pkgs; [
          expat
          mesa
          vulkan-loader
        ]);

        guiTestPython = pkgs.python3.withPackages (ps:
          let
            bip380 = ps.buildPythonPackage rec {
              pname = "bip380";
              version = "0.2.0-fb61971";
              pyproject = true;

              src = pkgs.fetchzip {
                url = "https://github.com/darosior/python-bip380/archive/fb61971d9128e663f110ea2734c1d023e7e0266b.zip";
                sha256 = "0qhnczv7ndvgw18s6mds892l4kmgj3grvk5zj7xbpmq1p264f9mi";
              };

              pythonRelaxDeps = [
                "bip32"
                "coincurve"
              ];

              nativeBuildInputs = with ps; [
                setuptools
              ];

              propagatedBuildInputs = with ps; [
                bip32
                coincurve
              ];

              doCheck = false;
              pythonImportsCheck = [ "bip380" ];
            };
          in
          with ps; [
            bip32
            bip380
            ephemeral-port-reserve
            numpy
            opencv4
            pillow
            pytest
            pytest-timeout
            pytest-xdist
          ]);

        guiTestShell = pkgs.mkShell {
          packages = guiRuntimeInputs ++ (with pkgs; [
            bitcoin
            dbus
            electrs
            imagemagick
            openbox
            tesseract
            tigervnc
            x11vnc
            xdg-desktop-portal
            xdg-desktop-portal-gtk
            xdotool
            xauth
            xdpyinfo
            xev
            xvfb
            xwininfo
            zenity
          ]) ++ [
            guiTestPython
          ];

          BITCOIND_PATH = "${pkgs.bitcoin}/bin/bitcoind";
          ELECTRS_PATH = "${pkgs.electrs}/bin/electrs";
          GUI_TEST_RUNTIME_LIBRARY_PATH = lib.makeLibraryPath guiRuntimeInputs;

          shellHook = ''
            export LD_LIBRARY_PATH="$GUI_TEST_RUNTIME_LIBRARY_PATH''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
            export WINIT_X11_SCALE_FACTOR=1
          '';
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
          liana = lianaPackages;
          liana-business = lianaBusinessPackages;
        };

        devShells = {
          gui-tests = guiTestShell;
          minimal = minimalShell;
          release = releaseShell;
          default = devShell;
        };
      }
    );
}
