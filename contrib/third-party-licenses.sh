#!/usr/bin/env bash
# Regenerates coincube-gui/assets/THIRD_PARTY_LICENSES.txt: every third-party
# crate shipped in COINCUBE Desktop (the gui and the Spark bridge, normal and
# build dependencies on all targets) with its declared SPDX licence.
#
# Run from the repository root after changing dependencies:
#   contrib/third-party-licenses.sh
set -euo pipefail

cd "$(dirname "$0")/.."
out=coincube-gui/assets/THIRD_PARTY_LICENSES.txt

list() {
    cargo tree "$@" -e normal,build --prefix none --target all \
        --format '{p}|{l}' 2>/dev/null
}

{
    list -p coincube-gui
    list --manifest-path coincube-spark-bridge/Cargo.toml
} |
    # Drop the "(*)" / "(proc-macro)" markers and source paths or git URLs
    # (vendored and patched forks stay listed), then this repository's own
    # crates.
    sed -E -e 's/ \(\*\)//' -e 's/ \(proc-macro\)//' -e 's/ \([^)|]*\)\|/|/' |
    grep -v -E '^coincube(d|-[a-z-]+)? v' |
    awk -F'|' 'NF == 2 { if ($2 == "") $2 = "licence not declared in its manifest"; print $1 "  —  " $2 }' |
    sort -u >"$out.tmp"

{
    echo "Third-party software in COINCUBE Desktop"
    echo
    echo "COINCUBE Desktop is built on the open-source crates below. Each is"
    echo "listed with the licence its authors declare; the full licence texts"
    echo "ship with each crate's source."
    echo
    cat "$out.tmp"
} >"$out"
rm "$out.tmp"
echo "wrote $(grep -c '  —  ' "$out") crates to $out"
