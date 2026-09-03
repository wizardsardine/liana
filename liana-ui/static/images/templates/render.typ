// Entry point, run by `cargo xtask gen-images`: one spec + one locale -> one SVG.
//   typst compile --root . --ignore-system-fonts --font-path ../../fonts --font-path ../../icons \
//     --format svg --input template=<spec id> --input lang=<locale tag> render.typ out.svg

#import "lib/theme.typ": color, fonts-for
#import "lib/timeline.typ": canvas-height, canvas-width, spending-timeline

#assert(
  "template" in sys.inputs and "lang" in sys.inputs,
  message: "pass --input template=<spec id> --input lang=<locale tag>",
)
#let spec = toml("specs/" + sys.inputs.template + ".toml")
#let locale = toml("locales/" + sys.inputs.lang + ".toml")
#let tag = sys.inputs.lang.split("-")

#set page(width: canvas-width, height: canvas-height(spec), margin: 0pt, fill: none)
// Layout left to right whatever the language; fit-text re-enables `dir: auto` per string.
#set text(
  font: fonts-for(sys.inputs.lang),
  fill: color.text,
  lang: tag.at(0),
  region: if tag.len() > 1 and tag.at(1).len() == 2 { tag.at(1) } else { none },
  dir: ltr,
  hyphenate: false,
)
#set par(leading: 0.32em)

#spending-timeline(spec, locale, sys.inputs.lang)
