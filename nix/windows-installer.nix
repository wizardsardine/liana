# Wrap a cross-compiled Windows executable into an NSIS .exe installer.
# Reproducibility rules are documented in contrib/release/windows/README.md.
{ pkgs, lib, rootPath }:

{ pname
, version
, productName
, exePath
, exeName
, icon
}:
let
  # The VERSIONINFO resource needs four numeric fields; drop any pre-release suffix.
  versionMatch = builtins.match "([0-9]+\\.[0-9]+\\.[0-9]+).*" version;
  numericVersion =
    if versionMatch == null
    then throw "windows-installer: cannot derive a numeric version from '${version}'"
    else lib.head versionMatch;
  nsi = rootPath + "/contrib/release/windows/installer.nsi";
in
pkgs.runCommand "${pname}-${version}-x86_64-pc-windows-gnu-setup" {
  nativeBuildInputs = [ pkgs.nsis pkgs._7zz ];
  SOURCE_DATE_EPOCH = 1;
  inherit productName exeName version numericVersion;
} ''
  stage="$TMPDIR/stage"
  mkdir -p "$stage"
  cp ${exePath} "$stage/$exeName"
  cp ${rootPath + "/LICENCE"} "$stage/LICENCE"
  cp ${rootPath + "/README.md"} "$stage/README.md"
  cp ${icon} "$stage/app.ico"
  chmod u+w "$stage"/*
  touch --date="@$SOURCE_DATE_EPOCH" "$stage"/*

  setup="$TMPDIR/${pname}-setup.exe"
  makensis -V2 -WX -NOCD \
    -DPRODUCT_NAME="$productName" \
    -DEXE_NAME="$exeName" \
    -DVERSION="$version" \
    -DVI_VERSION="$numericVersion.0" \
    -DICON="$stage/app.ico" \
    -DSTAGE_DIR="$stage" \
    -DOUT_FILE="$setup" \
    ${nsi}

  check="$TMPDIR/check"
  7zz x -o"$check" "$setup" > /dev/null
  extracted="$(find "$check" -type f -name "$exeName")"
  [ "$(printf '%s\n' "$extracted" | grep -c .)" -eq 1 ] || {
    echo "expected exactly one $exeName in the installer, got: $extracted" >&2
    exit 1
  }
  cmp "$extracted" "$stage/$exeName"

  install -D -m 0644 "$setup" "$out/x86_64-pc-windows-gnu/${pname}-setup.exe"
''
