import sys
import os
import fontforge


def generate_glyph_code(filename):
    glyph_code = hash(filename) % 0xFFFF

    # Avoid surrogate range (0xD800-0xDFFF)
    if 0xD800 <= glyph_code <= 0xDFFF:
        glyph_code += 0x800

    return glyph_code


input_folder = sys.argv[1]
output_font_file = sys.argv[2]

font = fontforge.font()
font.encoding = 'UnicodeFull'

for svg_file in os.listdir(input_folder):
    if not svg_file.endswith('.svg'):
        continue

    glyph_name = os.path.splitext(svg_file)[0]
    glyph_code = generate_glyph_code(glyph_name)

    glyph = font.createChar(glyph_code)
    glyph.importOutlines(os.path.join(input_folder, svg_file))
    glyph.simplify()

font.generate(output_font_file)
