# iconex

First-party icon font, built with the shared `../svg_to_ttf.py`.

It keeps the legacy family name `Untitled1` (referenced by `liana-ui/src/icon.rs`), so the family
and output are passed explicitly:

```
python liana-ui/static/icons/svg_to_ttf.py iconex --family Untitled1 --output iconex-icons.ttf
```

`codepoints.json` lists the glyphs `icon.rs` actually uses, each frozen at its existing codepoint.
To add or change one, edit `svg/` + `codepoints.json` and rerun the command above.
