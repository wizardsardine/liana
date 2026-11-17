# The toolchain the Linux and Windows releases are built with: a normal rustc
# `x.py` build that takes the mrustc-produced compiler as its bootstrap rustc
# instead of a tarball from static.rust-lang.org.
#
# Going through x.py rather than installing the stage 1 compiler directly is what
# gets us `std` cross-compiled from source for every target, which mrustc alone
# does not produce.
#
# Deliberately free of the Xcode SDK: the Apple targets are layered on top in
# darwin-std.nix so that the Linux and Windows chains stay buildable from source
# alone.
{ lib
, stdenv
, fetchurl
, rustc-bootstrap-stage1
, cmake
, curl
, libffi
, libxml2
, llvmPackages_20
, ninja
, perl
, pkg-config
, python3
, which
, patchelf
, zlib
, pkgsCross
, sources
}:
let
  inherit (sources) rustc;
  targets = sources.targets.free;

  # Built from source by nixpkgs, so reusing it keeps the chain closed while
  # saving x.py from compiling the LLVM bundled in the rustc tree. rustc 1.90
  # requires major >= 19, and nixpkgs builds every backend, so AArch64 is there
  # for the Apple targets layered on in darwin-std.nix.
  llvm = llvmPackages_20.llvm;

  # The binaries x.py builds and then runs during the build link against these.
  runtimeLibs = lib.makeLibraryPath [ zlib libffi libxml2 curl llvm ];

  rustcSrc = fetchurl {
    url = "https://static.rust-lang.org/dist/rustc-${rustc.version}-src.tar.gz";
    inherit (rustc) hash;
  };

  mingw = pkgsCross.mingwW64.stdenv.cc;
  mingwPrefix = pkgsCross.mingwW64.stdenv.cc.targetPrefix;

  hostTarget = stdenv.hostPlatform.rust.rustcTarget;
in
stdenv.mkDerivation {
  pname = "rustc-bootstrap";
  version = rustc.version;

  src = rustcSrc;

  strictDeps = true;
  nativeBuildInputs = [
    cmake
    ninja
    perl
    llvm.dev
    patchelf
    pkg-config
    python3
    which
  ];
  buildInputs = [
    libffi
    zlib
    libxml2
    curl
    llvm
  ];
  depsBuildBuild = [ mingw ];

  dontUseCmakeConfigure = true;

  configurePhase = ''
    runHook preConfigure

    cat > config.toml <<EOF
    change-id = "ignore"

    [build]
    rustc = "${rustc-bootstrap-stage1}/bin/rustc"
    cargo = "${rustc-bootstrap-stage1}/bin/cargo"
    host = ["${hostTarget}"]
    target = [${lib.concatMapStringsSep ", " (t: "\"${t}\"") targets}]
    docs = false
    extended = false
    vendor = true
    python = "${python3}/bin/python3"

    [install]
    prefix = "$out"
    sysconfdir = "etc"

    [llvm]
    download-ci-llvm = false

    [rust]
    channel = "stable"
    codegen-tests = false

    [target.${hostTarget}]
    llvm-config = "${llvm.dev}/bin/llvm-config"

    [target.x86_64-pc-windows-gnu]
    llvm-config = "${llvm.dev}/bin/llvm-config"
    cc = "${mingw}/bin/${mingwPrefix}cc"
    cxx = "${mingw}/bin/${mingwPrefix}c++"
    ar = "${mingw}/bin/${mingwPrefix}ar"
    ranlib = "${mingw}/bin/${mingwPrefix}ranlib"
    linker = "${mingw}/bin/${mingwPrefix}cc"
    EOF

    runHook postConfigure
  '';

  buildPhase = ''
    runHook preBuild
    # x.py executes the rustc it has just built (`rustc -vV`), and that binary
    # has no rpath for the nixpkgs libraries it links against.
    export LD_LIBRARY_PATH="${runtimeLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    python3 x.py build --stage 2 library rustc cargo -j $NIX_BUILD_CORES
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    export LD_LIBRARY_PATH="${runtimeLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    python3 x.py install --stage 2 library rustc cargo

    # Same reason as above, but for the installed toolchain, which runs outside
    # of any LD_LIBRARY_PATH we control.
    for f in $out/bin/rustc $out/bin/cargo $out/lib/*.so; do
      [ -f "$f" ] || continue
      patchelf --add-rpath "$out/lib:${runtimeLibs}" "$f"
    done
    runHook postInstall
  '';

  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck
    for t in ${lib.concatStringsSep " " targets}; do
      test -d $out/lib/rustlib/$t/lib || { echo "missing std for $t" >&2; exit 1; }
    done
    runHook postInstallCheck
  '';

  requiredSystemFeatures = [ "big-parallel" ];

  passthru = {
    inherit (rustc) version;
    inherit targets;
  };

  meta = {
    description = "rustc ${rustc.version} bootstrapped from source, with std for ${lib.concatStringsSep " and " targets}";
    homepage = "https://github.com/thepowersgang/mrustc";
    license = with lib.licenses; [ mit asl20 ];
    platforms = [ "x86_64-linux" ];
  };
}
