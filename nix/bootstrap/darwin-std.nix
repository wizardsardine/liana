# `std` for the Apple targets, cross-compiled from source by the bootstrapped
# rustc.
#
# This is the one part of the toolchain that is not buildable from source alone:
# linking against Apple's libc needs the macOS SDK, which is a proprietary blob
# extracted from Xcode. Kept out of rustc.nix on purpose so the Linux and
# Windows toolchain does not depend on it.
{ lib
, stdenv
, fetchurl
, rustc-bootstrap
, cmake
, libffi
, libxml2
, llvmPackages_20
, ninja
, perl
, pkg-config
, python3
, which
, writeShellScriptBin
, zlib
, zig
, darwin
, sources
}:
let
  inherit (sources) rustc;
  targets = sources.targets.apple;

  llvm = llvmPackages_20.llvm;
  runtimeLibs = lib.makeLibraryPath [ zlib libffi libxml2 llvm ];

  rustcSrc = fetchurl {
    url = "https://static.rust-lang.org/dist/rustc-${rustc.version}-src.tar.gz";
    inherit (rustc) hash;
  };

  sdk = "${darwin.xcode_12_2}/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk";

  # cargo-zigbuild does this for the cargo builds; x.py needs its own shims.
  zigTargets = {
    "x86_64-apple-darwin" = "x86_64-macos.11.0";
    "aarch64-apple-darwin" = "aarch64-macos.11.0";
  };

  # The `cc` crate adds `--target=x86_64-apple-macosx11.0`, a spelling zig's triple
  # parser rejects. The shim already pins the target, so drop the caller's.
  mkZigCc = rustTarget: writeShellScriptBin "zigcc-${rustTarget}" ''
    args=()
    skip=
    for arg in "$@"; do
      if [ -n "$skip" ]; then skip=; continue; fi
      case "$arg" in
        --target=*) ;;
        -target) skip=1 ;;
        *) args+=("$arg") ;;
      esac
    done

    exec ${zig}/bin/zig cc -target ${zigTargets.${rustTarget}} \
      -isysroot ${sdk} -iframework ${sdk}/System/Library/Frameworks \
      -F${sdk}/System/Library/Frameworks ''${args[@]+"''${args[@]}"}
  '';

  zigCcs = map mkZigCc targets;

  targetConfig = lib.concatMapStrings (t: ''

    [target.${t}]
    llvm-config = "${llvm.dev}/bin/llvm-config"
    cc = "${mkZigCc t}/bin/zigcc-${t}"
    linker = "${mkZigCc t}/bin/zigcc-${t}"
  '') targets;
in
stdenv.mkDerivation {
  pname = "rustc-bootstrap-darwin-std";
  version = rustc.version;

  src = rustcSrc;

  strictDeps = true;
  nativeBuildInputs = [
    cmake
    ninja
    perl
    llvm.dev
    pkg-config
    python3
    which
  ] ++ zigCcs;
  buildInputs = [
    libffi
    zlib
    libxml2
    llvm
  ];
  depsBuildBuild = [ zig darwin.xcode_12_2 ];

  dontUseCmakeConfigure = true;

  configurePhase = ''
    runHook preConfigure

    export SDKROOT=${sdk}
    export MACOSX_DEPLOYMENT_TARGET=11.0
    export XDG_CACHE_HOME=$TMPDIR/xdg_cache
    mkdir -p $XDG_CACHE_HOME

    cat > config.toml <<EOF
    change-id = "ignore"

    [build]
    rustc = "${rustc-bootstrap}/bin/rustc"
    cargo = "${rustc-bootstrap}/bin/cargo"
    host = ["${stdenv.hostPlatform.rust.rustcTarget}"]
    target = [${lib.concatMapStringsSep ", " (t: "\"${t}\"") targets}]
    docs = false
    extended = false
    vendor = true
    python = "${python3}/bin/python3"

    [llvm]
    download-ci-llvm = false

    [target.${stdenv.hostPlatform.rust.rustcTarget}]
    llvm-config = "${llvm.dev}/bin/llvm-config"

    [rust]
    channel = "stable"
    ${targetConfig}
    EOF

    runHook postConfigure
  '';

  buildPhase = ''
    runHook preBuild
    export LD_LIBRARY_PATH="${runtimeLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    python3 x.py build --stage 2 library -j $NIX_BUILD_CORES
    runHook postBuild
  '';

  # Only the target libraries: the compiler itself comes from rustc-bootstrap.
  installPhase = ''
    runHook preInstall
    mkdir -p $out/lib/rustlib
    for t in ${lib.concatStringsSep " " targets}; do
      src=$(find build -type d -path "*/stage2/lib/rustlib/$t" | head -1)
      if [ -z "$src" ]; then
        echo "no std built for $t" >&2
        exit 1
      fi
      cp -r "$src" $out/lib/rustlib/$t
    done
    runHook postInstall
  '';

  # binutils `strip` rewrites the Mach-O rlibs as GNU archives, which zig's linker
  # then reads as garbage ("unknown cpu architecture").
  dontStrip = true;

  requiredSystemFeatures = [ "big-parallel" ];

  passthru = { inherit targets; };

  meta = {
    description = "Apple-target std for the bootstrapped rustc (needs the macOS SDK)";
    license = with lib.licenses; [ mit asl20 ];
    platforms = [ "x86_64-linux" ];
  };
}
