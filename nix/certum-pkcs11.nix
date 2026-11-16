# PKCS#11 modules for Certum cryptoCertum smart cards, extracted from Certum's proCertum
# CardManager for Linux. Usage is documented in contrib/release/windows/README.md.
{ lib, stdenv, requireFile, autoPatchelfHook, pcsclite }:

stdenv.mkDerivation rec {
  pname = "certum-pkcs11";
  version = "2.2.15";

  src = requireFile {
    name = "proCertumCardManager-${version}-x86_64-ubuntu.bin";
    sha256 = "7953df7095a8e662bb59e30008b41c6f7852df05b9cd7ac3e2fcf221309e6f9c";
    message = ''
      Download proCertum CardManager ${version} for Linux (Ubuntu/Debian x86_64)
      from https://support.certum.eu/en/?p=6595 and run:

        nix-store --add-fixed sha256 proCertumCardManager-${version}-x86_64-ubuntu.bin
    '';
  };

  nativeBuildInputs = [ autoPatchelfHook ];
  buildInputs = [ (lib.getLib pcsclite) (lib.getLib stdenv.cc.cc) ];

  # makeself: the gzipped tarball starts after the `skip` lines of shell.
  unpackPhase = ''
    skip="$(sed -n 's/^skip="\([0-9]*\)"$/\1/p' "$src")"
    [ -n "$skip" ] || { echo "not a makeself archive: $src" >&2; exit 1; }
    tail -n "+$((skip + 1))" "$src" | tar xz
  '';

  installPhase = ''
    mkdir -p "$out/lib" "$out/share/certum-pkcs11"
    install -m 0644 sc30pkcs11-*.so cryptoCertum3PKCS-*.so "$out/lib/"
    ln -s sc30pkcs11-*.so "$out/lib/libcrypto3PKCS.so"
    ln -s cryptoCertum3PKCS-*.so "$out/lib/libcryptoCertum3PKCS.so"
    cp -r CACerts "$out/share/certum-pkcs11/"
    install -m 0644 proCertumCardManager_en_licence.rtf "$out/share/certum-pkcs11/"
  '';

  meta = {
    description = "PKCS#11 modules for Certum cryptoCertum smart cards (common and secure profiles)";
    homepage = "https://support.certum.eu/en/?p=6595";
    license = lib.licenses.unfree;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
    platforms = [ "x86_64-linux" ];
  };
}
