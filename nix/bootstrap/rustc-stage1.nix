# Assembles a usable toolchain out of the raw prefix tree produced by
# rustc-stage1-build.nix.
#
# Split out on purpose: this derivation is seconds to build, so the install
# layout, the rpath fixup and the smoke tests can be iterated on without
# re-running the 90 minute mrustc build.
{ lib
, stdenv
, runCommand
, patchelf
, rustc-bootstrap-stage1-build
, sources
}:
let
  inherit (sources) rustc;
  target = stdenv.targetPlatform.rust.rustcTarget;
  prefix = "${rustc-bootstrap-stage1-build}/prefix";
in
runCommand "rustc-bootstrap-stage1-${rustc.version}"
{
  nativeBuildInputs = [ patchelf ];

  passthru = {
    inherit (rustc) version;
    build = rustc-bootstrap-stage1-build;
  };

  meta = {
    description = "rustc ${rustc.version} built from source by mrustc (bootstrap stage 1)";
    homepage = "https://github.com/thepowersgang/mrustc";
    license = with lib.licenses; [ mit asl20 ];
    platforms = [ "x86_64-linux" ];
  };
}
  ''
    mkdir -p $out/bin $out/lib

    cp ${prefix}/bin/rustc_binary $out/bin/rustc
    cp ${prefix}/bin/cargo $out/bin/cargo
    cp -r ${prefix}/lib/* $out/lib/
    chmod -R u+w $out/bin $out/lib

    cp $out/lib/rustlib/${target}/lib/*.so $out/lib/

    # Upstream ships bin/rustc as a wrapper setting LD_LIBRARY_PATH to build-dir
    # paths. We install the bare binary, so it needs an rpath of its own to find
    # libstd. Added, not set, to keep the rpath entries the build already put in.
    for bin in rustc cargo; do
      patchelf --add-rpath $out/lib $out/bin/$bin
    done

    # The sample prints "Hello, world!"; match case-insensitively so a change of
    # capitalisation upstream does not fail the build.
    LD_LIBRARY_PATH=$out/lib ${prefix}/bin/hello_world | grep -i "hello, world"
    $out/bin/rustc --version
    $out/bin/cargo --version
  ''
