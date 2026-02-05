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
          liana = lianaPackages;
          liana-business = lianaBusinessPackages;
        };

        devShells = {
          minimal = minimalShell;
          release = releaseShell;
          default = devShell;
        };
      }
    );
}
