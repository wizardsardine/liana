{ craneLib, craneLibBootstrap, craneLibBootstrapApple, pkgs, lib, lipo, rootPath }:
let
  commonBuildSettings = {
    src = lib.fileset.toSource {
      root = rootPath;
      fileset = lib.fileset.unions [
        (craneLib.fileset.commonCargoSources rootPath)
        (lib.fileset.maybeMissing (rootPath + "/liana-ui/static"))
        (rootPath + "/liana-i18n/i18n")
      ];
    };
    strictDeps = true;
    doCheck = false;
    cargoLock = rootPath + "/Cargo.lock";
    cargoVendorDir = craneLib.vendorCargoDeps {
      src = rootPath;
    };
  };

  lianaInfo = craneLib.crateNameFromCargoToml { cargoToml = rootPath + "/liana-gui/Cargo.toml"; };

  mkWindowsInstaller = import ./windows-installer.nix { inherit pkgs lib rootPath; };

  normalizeElf = import ./normalize-elf.nix { inherit pkgs; };

  x86_64-pc-windows-gnu-exe = craneLibBootstrap.buildPackage {
    inherit (commonBuildSettings) src strictDeps doCheck;
    inherit (lianaInfo) pname version;

    SOURCE_DATE_EPOCH = 1;
    CARGO_BUILD_TARGET = "x86_64-pc-windows-gnu";
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-C link-arg=-Wl,--no-insert-timestamp -C link-arg=-Wl,--strip-all -C link-arg=-L${pkgs.pkgsCross.mingwW64.windows.pthreads}/lib";

    HOST_CC = "${pkgs.stdenv.cc}/bin/cc";
    TARGET_CC = "${pkgs.pkgsCross.mingwW64.stdenv.cc}/bin/${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}cc";

    AR_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}ar";
    TOOLKIT_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.bintools.bintools}/bin";
    WINDRES_x86_64_pc_windows_gnu = "${pkgs.pkgsCross.mingwW64.stdenv.cc.targetPrefix}windres";

    cargoExtraArgs = "-p liana-gui --no-default-features";
    depsBuildBuild = with pkgs; [
      pkgsCross.mingwW64.stdenv.cc
      pkgsCross.mingwW64.buildPackages.binutils
      pkgsCross.mingwW64.buildPackages.binutils-unwrapped
    ];

    installPhaseCommand = ''
      mkdir -p $out/x86_64-pc-windows-gnu
      cp target/x86_64-pc-windows-gnu/release/liana-gui.exe $out/x86_64-pc-windows-gnu
    '';
  };

  x86_64-pc-windows-gnu = mkWindowsInstaller {
    inherit (lianaInfo) pname version;
    productName = "Liana";
    exeName = "liana-gui.exe";
    exePath = "${x86_64-pc-windows-gnu-exe}/x86_64-pc-windows-gnu/liana-gui.exe";
    icon = rootPath + "/liana-ui/static/logos/liana.ico";
  };

  # Linux and Windows use the bootstrapped toolchain, whose `std` is cross-compiled
  # from source. The Apple targets use craneLibBootstrapApple: same compiler, plus
  # an Apple `std` that needs the proprietary macOS SDK to link.
  x86_64-unknown-linux-gnu = craneLibBootstrap.buildPackage {
    inherit (commonBuildSettings) src strictDeps doCheck;
    inherit (lianaInfo) pname version;

    CARGO_BUILD_TARGET = "x86_64-unknown-linux-gnu";
    # `libudev.so` itself is built against the glibc of nixpkgs: let the linker ignore the
    # symbols it needs from a newer glibc than the one we target. Only the `libudev.so.1`
    # dependency and the udev symbols we use end up in the binary.
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS = "-C link-arg=-Wl,--allow-shlib-undefined";
    # The glibc version suffix makes zig link against its own 2.31 stubs, so the binaries
    # also run on distributions shipping an older glibc than the one of the build machine.
    buildPhaseCargoCommand = ''
      cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31 -p liana-gui --no-default-features --message-format json-render-diagnostics
      cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31 -p lianad --message-format json-render-diagnostics
    '';
    doNotPostBuildInstallCargoBinaries = true;

    nativeBuildInputs = [ pkgs.pkg-config pkgs.rustPlatform.bindgenHook ];
    buildInputs = [ pkgs.udev ];
    depsBuildBuild = [ pkgs.zig pkgs.cargo-zigbuild ];

    preBuild = ''
      # hidapi compiles its own `hid.c`, which includes `libudev.h`, without asking pkg-config.
      export CFLAGS_x86_64_unknown_linux_gnu="$(pkg-config --cflags libudev)"
      export XDG_CACHE_HOME=$TMPDIR/xdg_cache
      mkdir -p $XDG_CACHE_HOME
      export CARGO_ZIGBUILD_CACHE_DIR=$TMPDIR/cargo-zigbuild-cache
      mkdir -p $CARGO_ZIGBUILD_CACHE_DIR
    '';

    installPhaseCommand = ''
      mkdir -p $out/x86_64-unknown-linux-gnu
      for bin in liana-gui lianad liana-cli; do
        install -m 755 target/x86_64-unknown-linux-gnu/release/$bin $out/x86_64-unknown-linux-gnu/$bin
        ${normalizeElf} $out/x86_64-unknown-linux-gnu/$bin
      done
    '';
  };

  x86_64-apple-darwin = craneLibBootstrapApple.buildPackage {
    inherit (commonBuildSettings) src strictDeps doCheck;
    inherit (lianaInfo) pname version;

    CARGO_BUILD_TARGET = "x86_64-apple-darwin";
    buildPhaseCargoCommand = ''
      cargo zigbuild --release -p liana-gui --no-default-features --message-format json-render-diagnostics
      cargo zigbuild --release -p lianad --message-format json-render-diagnostics
    '';
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
      # zig's MachO linker rejects relative switch tables pointing at the end of a literal section ("bad relocation" in hyper)
      # zig only ad-hoc signs arm64, so x86_64 needs header room for rcodesign to add LC_CODE_SIGNATURE
      export CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-isysroot -C link-arg=$SDKROOT -C link-arg=-F$SDKROOT/System/Library/Frameworks -C link-arg=-Wl,-dead_strip_dylibs -C link-arg=-Wl,-headerpad,0x1000 -C llvm-args=-switch-to-lookup=false"
      export XDG_CACHE_HOME=$TMPDIR/xdg_cache
      mkdir -p $XDG_CACHE_HOME
      export CARGO_ZIGBUILD_CACHE_DIR=$TMPDIR/cargo-zigbuild-cache
      mkdir -p $CARGO_ZIGBUILD_CACHE_DIR
      export CC=zigcc
      export CXX=zigc++
    '';

    installPhaseCommand = ''
      mkdir -p $out/x86_64-apple-darwin
      cp target/x86_64-apple-darwin/release/liana-gui $out/x86_64-apple-darwin
      cp target/x86_64-apple-darwin/release/lianad $out/x86_64-apple-darwin
      cp target/x86_64-apple-darwin/release/liana-cli $out/x86_64-apple-darwin
    '';
  };

  aarch64-apple-darwin = craneLibBootstrapApple.buildPackage {
    inherit (commonBuildSettings) src strictDeps doCheck;
    inherit (lianaInfo) pname version;

    CARGO_BUILD_TARGET = "aarch64-apple-darwin";
    buildPhaseCargoCommand = ''
      cargo zigbuild --release -p liana-gui --no-default-features --message-format json-render-diagnostics
      cargo zigbuild --release -p lianad --message-format json-render-diagnostics
    '';
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
      export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-isysroot -C link-arg=$SDKROOT -C link-arg=-F$SDKROOT/System/Library/Frameworks -C link-arg=-Wl,-dead_strip_dylibs"
      export XDG_CACHE_HOME=$TMPDIR/xdg_cache
      mkdir -p $XDG_CACHE_HOME
      export CARGO_ZIGBUILD_CACHE_DIR=$TMPDIR/cargo-zigbuild-cache
      mkdir -p $CARGO_ZIGBUILD_CACHE_DIR
      export CC=zigcc
      export CXX=zigc++
    '';

    installPhaseCommand = ''
      mkdir -p $out/aarch64-apple-darwin
      cp target/aarch64-apple-darwin/release/liana-gui $out/aarch64-apple-darwin
      cp target/aarch64-apple-darwin/release/lianad $out/aarch64-apple-darwin
      cp target/aarch64-apple-darwin/release/liana-cli $out/aarch64-apple-darwin
    '';
  };

  universal2-apple-darwin = pkgs.runCommand "universal2-apple-darwin" {
    buildInputs = [ lipo ];
  } ''
    mkdir -p $out/universal2-apple-darwin

    # Combine liana-gui binaries
    lipo -output $out/universal2-apple-darwin/liana-gui -create \
      ${x86_64-apple-darwin}/x86_64-apple-darwin/liana-gui \
      ${aarch64-apple-darwin}/aarch64-apple-darwin/liana-gui
  '';

in {
  inherit x86_64-unknown-linux-gnu x86_64-pc-windows-gnu x86_64-pc-windows-gnu-exe x86_64-apple-darwin aarch64-apple-darwin universal2-apple-darwin;
  release = pkgs.buildEnv {
    name = "release";
    paths = [
      x86_64-unknown-linux-gnu
      x86_64-pc-windows-gnu
      x86_64-apple-darwin
      aarch64-apple-darwin
      universal2-apple-darwin
    ];
  };
}
