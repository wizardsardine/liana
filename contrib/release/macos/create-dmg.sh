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
#   SOURCE_DATE_EPOCH   timestamp given to every file in the image. Set it to
#                       get a reproducible build; it otherwise defaults to now
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

# xorrisofs derives the HFS+ volume name from the ISO 9660 volume id, which it
# refuses to set past 32 characters: the image would mount as "ISOIMAGE".
if [ "${#VOLNAME}" -gt 32 ]; then
    echo "volume name is ${#VOLNAME} characters, the limit is 32: $VOLNAME" >&2
    exit 1
fi

if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
    echo "warning: SOURCE_DATE_EPOCH is unset, this image will not be reproducible" >&2
fi
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(date +%s)}"
export TZ=UTC

STAGE="$(mktemp -d)"
XORRISO_LOG="$(mktemp)"
trap 'rm -rf "$STAGE" "$XORRISO_LOG"' EXIT

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
    command -v python3 >/dev/null \
        || { echo "missing tool: python3 (set DMG_NO_LAYOUT=1 to skip the Finder layout)" >&2; exit 1; }
    # Probed with the same flags as the run below, otherwise a module visible
    # only through PYTHONPATH or the user site passes here and fails there.
    python3 -I -c "import ds_store" 2>/dev/null \
        || { echo "missing python module: ds_store (set DMG_NO_LAYOUT=1 to skip the Finder layout)" >&2; exit 1; }
    python3 -I "$SCRIPT_DIR/dmg-ds-store.py" "$STAGE/.DS_Store" "$APP_NAME.app" "$VOLNAME" \
        ${DMG_BACKGROUND:+--background .background/background.png}
fi

# Normalise timestamps so the image is reproducible regardless of checkout time.
# (-t form works with both GNU and BSD touch.)
TOUCH_TS="$(date -u -d "@$SOURCE_DATE_EPOCH" +%Y%m%d%H%M.%S 2>/dev/null \
    || date -u -r "$SOURCE_DATE_EPOCH" +%Y%m%d%H%M.%S)"
find "$STAGE" -exec touch -h -t "$TOUCH_TS" {} +

ISO="$STAGE/$APP_NAME.iso"
XORRISO_STATUS=0
xorrisofs \
    -quiet \
    -D -l -R -r \
    -V "$VOLNAME" \
    -no-pad \
    -dir-mode 0755 \
    -hfsplus \
    -hfsplus-file-creator-type APPL APPL "/$APP_NAME.app" \
    -o "$ISO" \
    "$STAGE" >"$XORRISO_LOG" 2>&1 || XORRISO_STATUS=$?

# The volume name is deliberately not ISO 9660 compliant (it has spaces and
# lowercase letters), which is only a warning. Everything else is worth seeing.
grep -v 'volid text does not comply' "$XORRISO_LOG" >&2 || true

[ "$XORRISO_STATUS" -eq 0 ] || { echo "xorrisofs failed with status $XORRISO_STATUS" >&2; exit 1; }
[ -s "$ISO" ] || { echo "xorrisofs produced no output" >&2; exit 1; }

# `dmg` exits 0 on a usage error, so check the output exists rather than $?.
dmg dmg "$ISO" "$OUT_DMG" >/dev/null
[ -s "$OUT_DMG" ] || { echo "dmg produced no output" >&2; exit 1; }

# `dmg` does not check its writes either. The UDIF trailer is the last 512 bytes
# of the image and records where the data fork and the XML plist end, so a short
# write leaves it missing or inconsistent with the file size.
DMG_SIZE=$(( $(wc -c <"$OUT_DMG") ))
TRAILER="$(tail -c 512 "$OUT_DMG" | od -An -v -tx1 | tr -d ' \n')"
if [ "$DMG_SIZE" -le 512 ] || [ "${TRAILER:0:8}" != "6b6f6c79" ]; then
    echo "$OUT_DMG has no UDIF trailer, the image is truncated" >&2
    exit 1
fi
TRAILER_END=$(( $(printf '%d' "0x${TRAILER:432:16}") + $(printf '%d' "0x${TRAILER:448:16}") + 512 ))
if [ "$TRAILER_END" -ne "$DMG_SIZE" ]; then
    echo "$OUT_DMG is $DMG_SIZE bytes but its trailer describes $TRAILER_END, the image is truncated" >&2
    exit 1
fi

echo "created $OUT_DMG"
