# Stage 1: rustc + cargo built from source by mrustc, with no prebuilt rustc
# anywhere in the dependency graph. Host only, no cross `std`.
#
# This is not the toolchain the release is built with: it only exists to
# bootstrap the real `x.py` build in rustc.nix, which is where the cross targets
# come from.
#
# LLVM is the one bundled in the rustc source tarball and is built from source
# too, so the toolchain does not depend on which LLVM versions nixpkgs happens
# to still carry.
{ lib
, stdenv
, fetchurl
, mrustc
, mrustc-minicargo
, cmake
, curl
, libffi
, libxml2
, llvmPackages_20
, perl
, pkg-config
, python3
, which
, time
, zlib
, sources
}:
let
  inherit (sources) rustc;

  rustcSrc = fetchurl {
    url = "https://static.rust-lang.org/dist/rustc-${rustc.version}-src.tar.gz";
    inherit (rustc) hash;
  };

  rustcDir = "rustc-${rustc.version}-src";
  outputDir = "output-${rustc.version}";

  # minicargo.mk defaults LLVM_CONFIG to an LLVM it builds itself out of the rustc
  # source tree, which costs hours and tens of gigabytes of build space. It is a
  # plain `:=`, so overriding it on the command line wins and the build rule for
  # the bundled LLVM never fires. nixpkgs builds LLVM from source as well, so
  # reusing it does not put a binary in the chain. rustc 1.90 wants major >= 19.
  llvm = llvmPackages_20.llvm;

  mrustcMakeFlags = [
    "MRUSTC=${mrustc}/bin/mrustc"
    "RUSTC_TARGET=${stdenv.targetPlatform.rust.rustcTarget}"
    "LLVM_CONFIG=${llvm.dev}/bin/llvm-config"
  ];
in
stdenv.mkDerivation {
  pname = "rustc-bootstrap-stage1-build";
  version = "${mrustc.version}_${rustc.version}";

  inherit (mrustc) src;
  postUnpack = "tar -xf ${rustcSrc} -C source/";

  dontFixLibtool = true;

  patches = [ ./patches/0001-dont-download-rustc.patch ];

  postPatch = ''
    patch -p0 -d ${rustcDir}/ < rustc-${rustc.version}-src.patch
  '';

  # Kept for the parts of the rustc build that still shell out to cmake; the
  # top level has no CMakeLists of its own.
  dontUseCmakeConfigure = true;

  strictDeps = true;
  nativeBuildInputs = [
    cmake
    mrustc
    mrustc-minicargo
    llvm.dev
    perl
    pkg-config
    python3
    time
    which
  ];
  buildInputs = [
    libffi
    zlib
    libxml2
    curl
    llvm
  ];

  makeFlags = mrustcMakeFlags;

  buildPhase = ''
    runHook preBuild

    local flagsArray=(
      PARLEVEL=$NIX_BUILD_CORES
      ${toString mrustcMakeFlags}
    )

    touch ${rustcDir}/dl-version
    export OUTDIR_SUF=-${rustc.version}
    export RUSTC_VERSION=${rustc.version}
    export MRUSTC_TARGET_VER=${rustc.targetVersion}
    export MRUSTC_PATH=${mrustc}/bin/mrustc

    make -f minicargo.mk "''${flagsArray[@]}" LIBS
    make "''${flagsArray[@]}" test
    make -f minicargo.mk "''${flagsArray[@]}" ${outputDir}/rustc
    make -f minicargo.mk "''${flagsArray[@]}" ${outputDir}/cargo
    make -C run_rustc "''${flagsArray[@]}"

    unset flagsArray

    runHook postBuild
  '';

  # Deliberately dumb: copy out the whole prefix tree and nothing else. Every
  # decision about what to install, patchelf or smoke-test lives in
  # rustc-stage1.nix, so iterating on those does not invalidate this 90 minute
  # derivation.
  doCheck = false;
  installPhase = ''
    runHook preInstall
    mkdir -p $out
    cp -r run_rustc/${outputDir}/prefix $out/prefix
    runHook postInstall
  '';

  # Builds LLVM and several rustc stages: not something to schedule on a small builder.
  requiredSystemFeatures = [ "big-parallel" ];

  passthru = { inherit (rustc) version; };

  meta = {
    description = "rustc ${rustc.version} built from source by mrustc (raw stage 1 prefix tree)";
    homepage = "https://github.com/thepowersgang/mrustc";
    license = with lib.licenses; [ mit asl20 ];
    platforms = [ "x86_64-linux" ];
  };
}
