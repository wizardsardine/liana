# liana-icons

Additive first-party icon font for glyphs that are not present in `bootstrap-icons.ttf`.

To add a glyph:
1. Add `svg/<name>.svg`.
2. Assign a frozen PUA codepoint in `codepoints.json`.
3. Install FontForge with its Python bindings. On Debian, `apt install fontforge python3-fontforge`.
4. Run `python liana-ui/static/icons/svg_to_ttf.py liana-icons` (the shared build script; family
   and output default to the directory name).
5. Add `pub fn <name>_icon()` in `liana-ui/src/icon.rs`.
