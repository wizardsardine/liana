# Nixos 23.11 comes with libc 2.38, this version of libc may not be compatible
# with some drivers. For now the hack found is to user a community wrapper that
# detects the requirements and do the link (https://github.com/guibou/nixGL).
# usage:
# nixGL cargo run

let
   nixgl = import (fetchTarball "https://github.com/guibou/nixGL/archive/master.tar.gz") {};
   pkgs = import <nixpkgs> {};
in
pkgs.mkShell rec {
  buildInputs = with pkgs; [
    pkgs.expat
    pkgs.fontconfig
    pkgs.freetype
    pkgs.freetype.dev
    pkgs.libGL
    pkgs.pkg-config
    pkgs.udev
    pkgs.wayland
    pkgs.libxkbcommon
    pkgs.xorg.libX11
    pkgs.xorg.libXcursor
    pkgs.xorg.libXi
    pkgs.xorg.libXrandr
    nixgl.auto.nixGLDefault
  ];

  LD_LIBRARY_PATH =
    builtins.foldl' (a: b: "${a}:${b}/lib") "${pkgs.vulkan-loader}/lib" buildInputs;
}

