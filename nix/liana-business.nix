{ craneLib, pkgs, lib, lipo, rootPath }:
let
  commonBuildSettings = {
    src = lib.fileset.toSource {
      root = rootPath;
      fileset = lib.fileset.unions [
        (craneLib.fileset.commonCargoSources rootPath)
        (lib.fileset.maybeMissing (rootPath + "/liana-ui/static"))
      ];
    };
    strictDeps = true;
    doCheck = false;
    cargoLock = rootPath + "/Cargo.lock";
    cargoVendorDir = craneLib.vendorCargoDeps {
      src = rootPath;
    };
  };

  lianaBusinessInfo = craneLib.crateNameFromCargoToml { cargoToml = rootPath + "/liana-business/Cargo.toml"; };

  x86_64-pc-windows-gnu = craneLib.buildPackage {
    inherit (commonBuildSettings) src strictDeps doCheck;
    inherit (lianaBusinessInfo) pname version;

    SOURCE_DATE_EPOCH = 1;
    CARGO_BUILD_TARGET = "x86_64-pc-windows-gnu";
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-C link-arg=-Wl,--no-insert-timestamp -C link-arg=-L${pkgs.pkgsCross.mingwW64.windows.pthreads}/lib";

    HOST_CC = "${pkgs.stdenv.cc}/bin/cc";
    TARGET_CC = "${pkgs.pkgsCross.mingwW64.stdenv.cc}/bin/${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}cc";

    AR_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}ar";
    TOOLKIT_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.bintools.bintools}/bin";
    WINDRES_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}windres";

    cargoExtraArgs = "-p liana-business";
    depsBuildBuild = with pkgs; [
      pkgsCross.mingwW64.stdenv.cc
      pkgsCross.mingwW64.buildPackages.binutils
      pkgsCross.mingwW64.buildPackages.binutils-unwrapped
    ];

    installPhaseCommand = ''
      mkdir -p $out/x86_64-pc-windows-gnu
      cp target/x86_64-pc-windows-gnu/release/liana-business.exe $out/x86_64-pc-windows-gnu
    '';
  };

  x86_64-apple-darwin = craneLib.buildPackage {
    inherit (commonBuildSettings) src strictDeps doCheck;
    inherit (lianaBusinessInfo) pname version;

    CARGO_BUILD_TARGET = "x86_64-apple-darwin";
    buildPhaseCargoCommand = "cargo zigbuild --release --message-format json-render-diagnostics";
    doNotPostBuildInstallCargoBinaries = true;

    depsBuildBuild = [
      pkgs.zig
      pkgs.cargo-zigbuild
      pkgs.darwin.xcode_12_2
    ];

    preBuild = ''
      export SDKROOT=${pkgs.darwin.xcode_12_2}/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk
      export MACOSX_DEPLOYMENT_TARGET=11.0
      export CFLAGS_x86_64_apple_darwin="-isysroot $SDKROOT -iframework $SDKROOT/System/Library/Frameworks"
      export CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-isysroot -C link-arg=$SDKROOT -C link-arg=-F$SDKROOT/System/Library/Frameworks"
      export XDG_CACHE_HOME=$TMPDIR/xdg_cache
      mkdir -p $XDG_CACHE_HOME
      export CARGO_ZIGBUILD_CACHE_DIR=$TMPDIR/cargo-zigbuild-cache
      mkdir -p $CARGO_ZIGBUILD_CACHE_DIR
      export CC=zigcc
      export CXX=zigc++
    '';

    installPhaseCommand = ''
      mkdir -p $out/x86_64-apple-darwin
      cp target/x86_64-apple-darwin/release/liana-business $out/x86_64-apple-darwin
    '';
  };

  aarch64-apple-darwin = craneLib.buildPackage {
    inherit (commonBuildSettings) src strictDeps doCheck;
    inherit (lianaBusinessInfo) pname version;

    CARGO_BUILD_TARGET = "aarch64-apple-darwin";
    buildPhaseCargoCommand = "cargo zigbuild --release --message-format json-render-diagnostics";
    doNotPostBuildInstallCargoBinaries = true;

    depsBuildBuild = [
      pkgs.zig
      pkgs.cargo-zigbuild
      pkgs.darwin.xcode_12_2
    ];

    preBuild = ''
      export SDKROOT=${pkgs.darwin.xcode_12_2}/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk
      export MACOSX_DEPLOYMENT_TARGET=11.0
      export CFLAGS_aarch64_apple_darwin="-isysroot $SDKROOT -iframework $SDKROOT/System/Library/Frameworks"
      export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-isysroot -C link-arg=$SDKROOT -C link-arg=-F$SDKROOT/System/Library/Frameworks"
      export XDG_CACHE_HOME=$TMPDIR/xdg_cache
      mkdir -p $XDG_CACHE_HOME
      export CARGO_ZIGBUILD_CACHE_DIR=$TMPDIR/cargo-zigbuild-cache
      mkdir -p $CARGO_ZIGBUILD_CACHE_DIR
      export CC=zigcc
      export CXX=zigc++
    '';

    installPhaseCommand = ''
      mkdir -p $out/aarch64-apple-darwin
      cp target/aarch64-apple-darwin/release/liana-business $out/aarch64-apple-darwin
    '';
  };

  universal2-apple-darwin = pkgs.runCommand "universal2-apple-darwin" {
    buildInputs = [ lipo ];
  } ''
    mkdir -p $out/universal2-apple-darwin

    # Combine liana-business binaries
    lipo -output $out/universal2-apple-darwin/liana-business -create \
      ${x86_64-apple-darwin}/x86_64-apple-darwin/liana-business \
      ${aarch64-apple-darwin}/aarch64-apple-darwin/liana-business
  '';

in {
  inherit x86_64-pc-windows-gnu x86_64-apple-darwin aarch64-apple-darwin universal2-apple-darwin;
  release = pkgs.buildEnv {
    name = "release";
    paths = [
      x86_64-pc-windows-gnu
      x86_64-apple-darwin
      aarch64-apple-darwin
      universal2-apple-darwin
    ];
  };
}
