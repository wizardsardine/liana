#!/usr/bin/env bash

# ==============================================================================
# Re-attach a detached Authenticode signature to a reproducibly built .msi.
#
# This is the Windows counterpart of Bitcoin Core's `guix-codesign`: the signer
# publishes only the detached signature (see sign.sh `osslsigncode`), and anyone
# can rebuild the installer, attach the signature and compare the result with
# the published signed .msi.
#
# Usage: attach-signature.sh <noncodesigned.msi> <signature.pem> <out.msi> [ca_bundle.pem]
#
# When a CA bundle is given the output is also verified against it.
# ==============================================================================

set -euo pipefail

if [ $# -lt 3 ] || [ $# -gt 4 ]; then
    echo "Usage: $0 <noncodesigned.msi> <signature.pem> <out.msi> [ca_bundle.pem]" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

# osslsigncode comes from the release dev shell. Re-enter it through Nix rather
# than require it on the host.
if [ -z "${IN_LIANA_RELEASE_SHELL:-}" ]; then
    export IN_LIANA_RELEASE_SHELL=1
    exec nix develop "$REPO_ROOT#release" --command "$0" "$@"
fi

IN_MSI="$1"
SIG="$2"
OUT_MSI="$3"
CA_BUNDLE="${4:-}"

[ -f "$IN_MSI" ] || { echo "installer not found: $IN_MSI" >&2; exit 1; }
[ -f "$SIG" ] || { echo "signature not found: $SIG" >&2; exit 1; }

osslsigncode attach-signature -sigin "$SIG" -in "$IN_MSI" -out "$OUT_MSI"

if [ -n "$CA_BUNDLE" ]; then
    osslsigncode verify -CAfile "$CA_BUNDLE" -in "$OUT_MSI"
fi

echo "created $OUT_MSI"
