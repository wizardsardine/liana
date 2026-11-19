# Mutabah's Rust Compiler: a rustc written in C++, used to build rustc itself
# without starting from a prebuilt rustc binary.
#
# Derived from the nixpkgs package, which pins mrustc 0.12.0 and is marked
# broken because the rustc versions it targeted (1.54, 1.74) need LLVM 12 and 17
# respectively, both since removed from nixpkgs. We target a mrustc new enough
# to build rustc directly, whose bundled LLVM nixpkgs still carries.
{ lib, stdenv, fetchFromGitHub, zlib, sources }:
let
  inherit (sources) mrustc;
in
stdenv.mkDerivation {
  pname = "mrustc";
  inherit (mrustc) version;

  src = fetchFromGitHub {
    owner = "thepowersgang";
    repo = "mrustc";
    rev = mrustc.tag;
    inherit (mrustc) hash;
  };

  postPatch = ''
    sed -i 's/\$(shell git show --pretty=%H -s)/${mrustc.commit}/' Makefile
    sed -i 's/\$(shell git symbolic-ref -q --short HEAD || git describe --tags --exact-match)/${mrustc.tag}/' Makefile
    sed -i 's/\$(shell git diff-index --quiet HEAD; echo $$?)/0/' Makefile
    sed '1i#include <limits>' -i src/trans/codegen_c.cpp
  '';

  strictDeps = true;
  buildInputs = [ zlib ];
  enableParallelBuilding = true;

  installPhase = ''
    runHook preInstall
    mkdir -p $out/bin
    cp bin/mrustc $out/bin
    runHook postInstall
  '';

  meta = {
    description = "Mutabah's Rust Compiler";
    mainProgram = "mrustc";
    homepage = "https://github.com/thepowersgang/mrustc";
    license = lib.licenses.mit;
    platforms = [ "x86_64-linux" ];
  };
}
