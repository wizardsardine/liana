#!/usr/bin/env bash

# ==============================================================================
# Create a macOS .dmg from an app bundle, on Linux (or macOS), without hdiutil.
#
# Pipeline:
#   1. stage <App>.app + an /Applications symlink (+ optional Finder layout)
#   2. xorrisofs  -> ISO 9660 / HFS+ hybrid image (reproducible)
#   3. dmg        -> UDIF zlib-compressed .dmg (libdmg-hfsplus)
#
# Usage: create-dmg.sh <path/to/App.app> <output.dmg> [volume name]
#
# Env:
#   SOURCE_DATE_EPOCH   honoured by xorriso for reproducible timestamps
#   DMG_BACKGROUND      optional PNG shown as Finder window background
#   DMG_NO_LAYOUT=1     skip .DS_Store generation (needs python3 + ds_store)
#
# Signing / notarization of the resulting .dmg is done separately with
# rcodesign (see sign.sh).
# ==============================================================================

set -euo pipefail

if [ $# -lt 2 ] || [ $# -gt 3 ]; then
    echo "Usage: $0 <path/to/App.app> <output.dmg> [volume name]" >&2
    exit 1
fi

APP_PATH="$(realpath "$1")"
OUT_DMG="$2"
APP_NAME="$(basename "$APP_PATH" .app)"
VOLNAME="${3:-$APP_NAME}"

for tool in xorrisofs dmg; do
    command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done
[ -d "$APP_PATH" ] || { echo "app bundle not found: $APP_PATH" >&2; exit 1; }

export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(date +%s)}"
export TZ=UTC

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

cp -R "$APP_PATH" "$STAGE/$APP_NAME.app"
ln -s /Applications "$STAGE/Applications"

if [ -n "${DMG_BACKGROUND:-}" ]; then
    mkdir -p "$STAGE/.background"
    cp "$DMG_BACKGROUND" "$STAGE/.background/background.png"
fi

# Finder window layout (icon positions, background). Purely cosmetic; the
# image is valid without it.
if [ "${DMG_NO_LAYOUT:-0}" != "1" ]; then
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    if python3 -c "import ds_store" 2>/dev/null; then
        python3 -I "$SCRIPT_DIR/dmg-ds-store.py" "$STAGE/.DS_Store" "$APP_NAME.app" "$VOLNAME" \
            ${DMG_BACKGROUND:+--background .background/background.png}
    else
        echo "note: python ds_store module not found, skipping Finder layout" >&2
    fi
fi

# Normalise timestamps so the image is reproducible regardless of checkout time.
# (-t form works with both GNU and BSD touch.)
TOUCH_TS="$(date -u -d "@$SOURCE_DATE_EPOCH" +%Y%m%d%H%M.%S 2>/dev/null \
    || date -u -r "$SOURCE_DATE_EPOCH" +%Y%m%d%H%M.%S)"
find "$STAGE" -exec touch -h -t "$TOUCH_TS" {} +

ISO="$STAGE/$APP_NAME.iso"
xorrisofs \
    -quiet \
    -D -l -R -r \
    -V "$VOLNAME" \
    -no-pad \
    -dir-mode 0755 \
    -hfsplus \
    -hfsplus-file-creator-type APPL APPL "/$APP_NAME.app" \
    -o "$ISO" \
    "$STAGE" 2>&1 | grep -v 'volid text does not comply' || true

[ -s "$ISO" ] || { echo "xorrisofs produced no output" >&2; exit 1; }

# `dmg` exits 0 on a usage error, so check the output exists rather than $?.
dmg dmg "$ISO" "$OUT_DMG" >/dev/null
[ -s "$OUT_DMG" ] || { echo "dmg produced no output" >&2; exit 1; }

echo "created $OUT_DMG"
