# Minimal cargo written in C++, shipped in the mrustc tree. Drives the rustc
# build before a real cargo exists.
{ lib, stdenv, makeWrapper, mrustc }:
stdenv.mkDerivation {
  pname = "mrustc-minicargo";
  inherit (mrustc) src version;

  strictDeps = true;
  nativeBuildInputs = [ makeWrapper ];

  enableParallelBuilding = true;
  makefile = "minicargo.mk";
  makeFlags = [ "bin/minicargo" ];

  installPhase = ''
    runHook preInstall
    mkdir -p $out/bin
    cp bin/minicargo $out/bin
    wrapProgram "$out/bin/minicargo" --set MRUSTC_PATH ${mrustc}/bin/mrustc
    runHook postInstall
  '';

  meta = {
    description = "Minimalist builder for Rust, shipped with mrustc";
    mainProgram = "minicargo";
    homepage = "https://github.com/thepowersgang/mrustc";
    license = lib.licenses.mit;
    platforms = [ "x86_64-linux" ];
  };
}
