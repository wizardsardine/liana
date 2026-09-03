# Installer template images

The installer illustrates each wallet template with a diagram of its spending policies over time (`liana-gui/src/installer/view/editor/template/`). This directory holds the sources of those diagrams. The rendered SVGs, one per template and per locale, are committed to `../generated/<locale>/<template>.svg`, so building Liana never runs the generator.

```
templates/
├── render.typ            entry point: one spec + one locale -> one SVG
├── lib/                  design tokens (mirrored from liana-ui), components, layout
├── specs/<id>.toml       structure of one image: events, policies, keys, segments; no text
└── locales/<tag>.toml    strings, one file per locale; `en.toml` is the source of truth
```

The renderer is [Typst](https://typst.app): one static binary, deterministic for a given version, that reads TOML natively and outlines the text in its SVG output. The GUI renders SVGs with `resvg` and only knows the fonts installed on the user's machine, so an SVG with `<text>` would not come out in IBM Plex Sans there.

## Regenerate

1. Install the pinned Typst release (`TYPST_VERSION` in `xtask/src/main.rs`). The Nix dev shell provides it. With another version, pass `--allow-typst-version-mismatch` for a preview and let CI produce the final files.
2. Run `cargo xtask gen-images`. `cargo xtask help` lists the options; `--typst PATH` or `$TYPST` points at a binary.
3. Commit `../generated/` and `liana-ui/src/image/template_images.rs`, which the task rewrites.

CI runs `cargo xtask gen-images --check` and fails when the committed files differ from what the sources render.

## Add a locale

Copy `locales/en.toml` to `locales/<tag>.toml`, with the BCP 47 tag the runtime will ask for (`de`, `pt-BR`, ...), translate every value and regenerate. The task refuses a locale with a missing, unknown or empty key: an image never mixes languages. A translation too long to fit fails the render with the offending string. Shorten the string, not the layout.

The accessors in `template_images.rs` take the locale tag and fall back to English for any other value. IBM Plex Sans covers Latin, Greek and Cyrillic; for Arabic, Japanese, Korean and Chinese, `lib/theme.typ` names a fallback family, whose font files go to `liana-ui/static/fonts` since the GUI needs them too. Arabic text is shaped correctly, but the diagram itself still reads left to right.

## Add a template

Copy a spec to `specs/<id>.toml`, add an `[<id>]` section to every locale and regenerate: `<id>_template_description(locale)` appears in `template_images.rs`.

A spec lists the timeline `events` (each needs a `common.event_<name>` string; `timelock` gets the highlight), then its `policies`: the keys shown in the chip (`kind` is `primary`, `recovery` or `inheritance` and sets the colour) and one `segments` state per event, `spend` or `locked`. The image grows with the number of policies. A template section may override a `common` string for that template only.

## Layout

`lib/theme.typ` mirrors the tokens of `liana-ui`: the colours of `color.rs`, IBM Plex Sans, the text sizes of `component/text/new.rs`, the card radius and the Bootstrap Icons glyphs of `icon.rs`. The image is 800 pt wide for the installer's 800 px column, so a size here is a size on screen. The green is spent on one thing, the timelock expiring: the icon of that column, a trace on its guide and the glow on the marker where a policy unlocks.

Every string goes through `fit-text` (`lib/components.typ`): one line when it fits, else the allowed number of lines, balanced in the narrowest box that holds them, and only then one size down. In practice, a pill or a chip is never wider than its words, the state pills carry the same wording in every image, and a Latin word is never broken at a hyphen. Chip labels wrap past `label-max` and a policy name may run a little past its chip (`name-overhang`), both in `lib/timeline.typ`.
