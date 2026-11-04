# Wrap a cross-compiled Windows executable into an .msi installer with wixl (msitools).
# Reproducibility rules are documented in contrib/release/windows/README.md.
{ pkgs, lib, rootPath }:

{ pname
, version
, productName
, exePath
, exeName
, icon
, upgradeCode
, exeComponentGuid
, docsComponentGuid
}:
let
  # MSI ProductVersion must be numeric major.minor.build; drop any pre-release suffix.
  versionMatch = builtins.match "([0-9]+\\.[0-9]+\\.[0-9]+).*" version;
  msiVersion =
    if versionMatch == null
    then throw "windows-msi: cannot derive an MSI ProductVersion from '${version}'"
    else lib.head versionMatch;
  wxs = rootPath + "/contrib/release/windows/installer.wxs";
in
pkgs.runCommand "${pname}-${version}-x86_64-pc-windows-gnu-msi" {
  nativeBuildInputs = [ pkgs.msitools ];
  SOURCE_DATE_EPOCH = 1;
  inherit productName exeName msiVersion;
  upgradeCode = lib.toUpper upgradeCode;
  exeComponentGuid = lib.toUpper exeComponentGuid;
  docsComponentGuid = lib.toUpper docsComponentGuid;
} ''
  # Deterministic GUID: first 128 bits of sha256 of the input, laid out as a UUID.
  guid_from() {
    hash="$(printf '%s' "$1" | sha256sum | cut -c1-32)"
    printf '%s-%s-%s-%s-%s' \
      "''${hash:0:8}" "''${hash:8:4}" "''${hash:12:4}" "''${hash:16:4}" "''${hash:20:12}" \
      | tr a-f A-F
  }
  productCode="$(guid_from "$upgradeCode:$msiVersion:product")"
  packageCode="$(guid_from "$upgradeCode:$msiVersion:package")"

  stage="$TMPDIR/stage"
  mkdir -p "$stage"
  cp ${exePath} "$stage/$exeName"
  cp ${rootPath + "/LICENCE"} "$stage/LICENCE"
  cp ${rootPath + "/README.md"} "$stage/README.md"
  cp ${icon} "$stage/app.ico"
  chmod u+w "$stage"/*
  touch --date="@$SOURCE_DATE_EPOCH" "$stage"/*

  out_dir="$out/x86_64-pc-windows-gnu"
  msi="$out_dir/${pname}.msi"
  mkdir -p "$out_dir"
  wixl -v --arch x64 \
    -D ProductName="$productName" \
    -D Version="$msiVersion" \
    -D UpgradeCode="$upgradeCode" \
    -D ProductCode="$productCode" \
    -D PackageCode="$packageCode" \
    -D ExeComponentGuid="$exeComponentGuid" \
    -D DocsComponentGuid="$docsComponentGuid" \
    -D ExeName="$exeName" \
    -D StageDir="$stage" \
    -o "$msi" \
    ${wxs}

  msiinfo suminfo "$msi"
  msiinfo export "$msi" Property

  check="$TMPDIR/check"
  mkdir -p "$check"
  msiextract -C "$check" "$msi" > /dev/null
  extracted="$(find "$check" -type f -name "$exeName")"
  [ "$(printf '%s\n' "$extracted" | grep -c .)" -eq 1 ] || {
    echo "expected exactly one $exeName in the installer, got: $extracted" >&2
    exit 1
  }
  cmp "$extracted" "$stage/$exeName"
''
