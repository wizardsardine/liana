# `dmg`: converts a raw ISO/HFS+ image into a UDIF (zlib-compressed) .dmg.
# Not in nixpkgs. Stripped-down fork maintained by fanquake (Bitcoin Core).
{ stdenv, fetchFromGitHub, cmake, zlib }:

stdenv.mkDerivation {
  pname = "libdmg-hfsplus";
  version = "unstable-2018-02-05";

  src = fetchFromGitHub {
    owner = "fanquake";
    repo = "libdmg-hfsplus";
    rev = "7ac55ec64c96f7800d9818ce64c79670e7f02b67";
    hash = "sha256-5HHb08GEPzgLQC8y9YyhGoin1Oxy2UtOCx/4Xmb4ATQ=";
  };

  nativeBuildInputs = [ cmake ];
  buildInputs = [ zlib ];

  # The fork's CMakeLists.txt predates CMake 3.5; CMake 4 needs this to configure it.
  # The binary is installed by hand (upstream has no install target), so CMake's
  # build-tree RPATH would otherwise leak a /build reference into $out.
  cmakeFlags = [
    "-DCMAKE_POLICY_VERSION_MINIMUM=3.5"
    "-DCMAKE_SKIP_BUILD_RPATH=ON"
  ];

  buildPhase = ''
    runHook preBuild
    make -C dmg -j$NIX_BUILD_CORES
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    install -Dm755 dmg/dmg $out/bin/dmg
    runHook postInstall
  '';
}
