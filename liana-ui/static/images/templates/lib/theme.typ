// Design tokens mirrored from liana-ui: color.rs, theme/palette/liana.rs, font.rs,
// component/text/new.rs, icon.rs. 1 pt here is 1 px in the GUI (800 pt for the 800 px column).

#let color = (
  card: rgb("#202020"), // GREY_6, cards.simple
  surface: rgb("#272727"), // GREY_5, form fields and pills
  line: rgb("#3F3F3F"), // GREY_7, hairlines
  line-strong: rgb("#555555"), // dashed rail
  text: rgb("#E6E6E6"), // GREY_1
  text-secondary: rgb("#CCCCCC"), // GREY_2
  text-muted: rgb("#8F8F8F"), // 4.6:1 on the card
  ink: rgb("#141414"), // LIGHT_BLACK, text on an accent
  green: rgb("#00FF66"),
  orange: rgb("#FFA700"),
  white: rgb("#E6E6E6"),
)

// One tinted chip per key kind, on the recipe of the GUI's fingerprint pill.
#let kinds = (
  primary: (accent: color.green, chip: rgb("#162B20"), rim: rgb("#18452B")),
  recovery: (accent: color.orange, chip: rgb("#2E2410"), rim: rgb("#4D3D14")),
  inheritance: (accent: color.white, chip: rgb("#2C2C2C"), rim: rgb("#3F3F3F")),
)

#let font = (sans: "IBM Plex Sans", icons: "bootstrap-icons")

// Bootstrap Icons, same codepoints as liana-ui/src/icon.rs.
#let icon = (
  key: "\u{F44E}", // round_key_icon
  lock: "\u{F47B}", // lock_icon
  unlock: "\u{F600}",
  receipt: "\u{F1BC}", // receive_icon
  hourglass: "\u{F41F}", // hourglass-split
)

// Rail states: `spend` on the fingerprint pill recipe, `locked` in greys (7:1).
#let states = (
  spend: (fill: rgb("#162B20"), rim: rgb("#18452B"), ink: color.green, icon: icon.unlock),
  locked: (fill: rgb("#2C2C2C"), rim: rgb("#3F3F3F"), ink: rgb("#B4B4B4"), icon: icon.lock),
)

#let size = (
  label: 14pt, // chip label
  name: 13pt, // small_caption
  header: 13pt, // small_caption
  state: 11pt, // pills, uppercase
  footnote: 13pt, // small_caption
)

#let radius = (
  card: 16pt, // CARD_RADIUS
  tile: 10pt, // key tiles and the chip around them
  pill: 999pt,
)

// Fallback fonts for the scripts IBM Plex Sans lacks; their files go to liana-ui/static/fonts.
#let fonts-for(tag) = {
  let lang = tag.split("-").at(0)
  let extra = if lang in ("ar", "fa", "ur") {
    ("IBM Plex Sans Arabic",)
  } else if lang == "ja" {
    ("IBM Plex Sans JP",)
  } else if lang == "ko" {
    ("IBM Plex Sans KR",)
  } else if lang == "zh" and (tag.contains("Hant") or tag.contains("TW") or tag.contains("HK")) {
    ("Noto Sans TC",)
  } else if lang == "zh" {
    ("Noto Sans SC",)
  } else {
    ()
  }
  (font.sans,) + extra
}

// No letter-spacing on joined scripts.
#let tracking-for(tag) = if tag.split("-").at(0) in ("ar", "fa", "ur") { 0em } else { 0.06em }
