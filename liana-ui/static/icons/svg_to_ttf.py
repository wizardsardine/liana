#!/usr/bin/env python3

"""Build an icon TTF from a font directory's ``svg/`` + ``codepoints.json``.

Shared by every first-party icon font in this folder (e.g. liana-icons, iconex): each font is a
sibling directory holding ``svg/<name>.svg`` and a ``codepoints.json`` mapping ``<name>`` to a hex
codepoint. Run once per font, e.g.:

    python svg_to_ttf.py liana-icons
    python svg_to_ttf.py iconex --family Untitled1 --output iconex-icons.ttf
"""

import argparse
import json
import struct
from pathlib import Path

UNITS_PER_EM = 1000
ASCENT = 1000
DESCENT = 0


def load_codepoints(codepoints_path: Path) -> dict[str, int]:
    raw = json.loads(codepoints_path.read_text())
    return {name: int(codepoint, 16) for name, codepoint in raw.items()}


def iter_glyphs(svg_dir: Path, codepoints_path: Path) -> list[tuple[str, int, Path]]:
    codepoints = load_codepoints(codepoints_path)
    glyphs = []
    for glyph_name, codepoint in sorted(codepoints.items(), key=lambda item: item[1]):
        svg_path = svg_dir / f"{glyph_name}.svg"
        if not svg_path.exists():
            raise FileNotFoundError(f"missing source SVG for {glyph_name}: {svg_path}")
        glyphs.append((glyph_name, codepoint, svg_path))
    return glyphs


def build_with_fontforge(
    glyphs: list[tuple[str, int, Path]], output_font_file: Path, family_name: str
) -> None:
    try:
        import fontforge
    except ModuleNotFoundError as err:
        raise SystemExit(
            "fontforge is required to build the icon font. "
            "Install FontForge and its Python bindings, then rerun this script."
        ) from err

    font = fontforge.font()
    font.encoding = "UnicodeFull"
    font.familyname = family_name
    font.fontname = family_name
    font.fullname = family_name
    font.weight = "Regular"
    font.em = UNITS_PER_EM
    font.ascent = ASCENT
    font.descent = DESCENT
    font.appendSFNTName("English (US)", "Family", family_name)
    font.appendSFNTName("English (US)", "SubFamily", "Regular")
    font.appendSFNTName("English (US)", "UniqueID", family_name)
    font.appendSFNTName("English (US)", "Fullname", family_name)
    font.appendSFNTName("English (US)", "Version", "Version 001.000")
    font.appendSFNTName("English (US)", "PostScriptName", family_name)

    for glyph_name, codepoint, svg_path in glyphs:
        glyph = font.createChar(codepoint, glyph_name)
        glyph.importOutlines(str(svg_path))
        glyph.simplify()
        glyph.round()

    font.generate(str(output_font_file))
    normalize_font_file(output_font_file)


def normalize_font_file(output_font_file: Path) -> None:
    data = bytearray(output_font_file.read_bytes())
    table_count = struct.unpack_from(">H", data, 4)[0]

    table_records: dict[bytes, tuple[int, int, int]] = {}
    head_record = None
    for index in range(table_count):
        record_offset = 12 + (index * 16)
        tag, _, offset, length = struct.unpack_from(">4sIII", data, record_offset)
        table_records[tag] = (record_offset, offset, length)
        if tag == b"head":
            head_record = (record_offset, offset, length)

    if head_record is None:
        raise RuntimeError("generated font is missing the head table")
    head_record_offset, head_offset, head_length = head_record

    fftm_record = table_records.get(b"FFTM")
    if fftm_record is not None:
        fftm_record_offset, fftm_offset, fftm_length = fftm_record
        if fftm_length >= 28:
            struct.pack_into(">IQQQ", data, fftm_offset, 1, 0, 0, 0)
        else:
            raise RuntimeError("generated font has an unexpected FFTM table")
        struct.pack_into(">I", data, fftm_record_offset + 4, table_checksum(data, fftm_offset, fftm_length))

    struct.pack_into(">Q", data, head_offset + 20, 0)
    struct.pack_into(">Q", data, head_offset + 28, 0)
    struct.pack_into(">I", data, head_offset + 8, 0)

    checksum = font_checksum(data)
    checksum_adjustment = (0xB1B0AFBA - checksum) & 0xFFFFFFFF
    struct.pack_into(">I", data, head_offset + 8, checksum_adjustment)
    struct.pack_into(">I", data, head_record_offset + 4, table_checksum(data, head_offset, head_length))
    output_font_file.write_bytes(data)


def table_checksum(data: bytearray, offset: int, length: int) -> int:
    table_data = bytes(data[offset : offset + length])
    padding = (-len(table_data)) % 4
    if padding:
        table_data += b"\0" * padding
    return sum(
        struct.unpack_from(">I", table_data, index)[0]
        for index in range(0, len(table_data), 4)
    ) & 0xFFFFFFFF


def font_checksum(data: bytearray) -> int:
    checksum_data = bytes(data)
    padding = (-len(checksum_data)) % 4
    if padding:
        checksum_data += b"\0" * padding
    return sum(
        struct.unpack_from(">I", checksum_data, offset)[0]
        for offset in range(0, len(checksum_data), 4)
    ) & 0xFFFFFFFF


def main() -> None:
    parser = argparse.ArgumentParser(description="Build an icon TTF from a font directory.")
    parser.add_argument("font_dir", type=Path, help="font directory holding svg/ and codepoints.json")
    parser.add_argument("--family", help="font family name (default: the font directory name)")
    parser.add_argument(
        "--output",
        help="output TTF filename, written inside the font directory "
        "(default: <font directory name>.ttf)",
    )
    args = parser.parse_args()

    base_dir = args.font_dir.resolve()
    family_name = args.family or base_dir.name
    output_font_file = base_dir / (args.output or f"{base_dir.name}.ttf")
    glyphs = iter_glyphs(base_dir / "svg", base_dir / "codepoints.json")
    build_with_fontforge(glyphs, output_font_file, family_name)


if __name__ == "__main__":
    main()
