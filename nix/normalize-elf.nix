# Make a cross-compiled ELF binary independent from the build machine: the standard
# interpreter path and no runpath pointing into the Nix store.
{ pkgs }:
let
  patchelf = "${pkgs.patchelf}/bin/patchelf";
  interpreter = "/lib64/ld-linux-x86-64.so.2";
in
pkgs.writeShellScript "normalize-elf" ''
  set -eu

  for bin in "$@"; do
    if [ "$(${patchelf} --print-interpreter "$bin")" != "${interpreter}" ]; then
      ${patchelf} --set-interpreter "${interpreter}" "$bin"
    fi
    if [ -n "$(${patchelf} --print-rpath "$bin")" ]; then
      ${patchelf} --remove-rpath "$bin"
    fi
  done
''
