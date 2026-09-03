// Components. Anything that measures text needs a `context` block.

#import "theme.typ": *

// Icon glyph, lowered onto the optical centre of the text next to it.
#let glyph(code, size: 12pt, fill: color.text) = box(
  baseline: 18%,
  text(font: font.icons, size: size, fill: fill, code),
)

#let cp(c) = str.to-unicode(c)

// CJK: a line may break between any two characters.
#let is-cjk(c) = {
  let n = cp(c)
  (n >= 0x2E80 and n <= 0x9FFF) or (n >= 0xAC00 and n <= 0xD7AF) or (n >= 0xF900 and n <= 0xFAFF) or (n >= 0xFF00 and n <= 0xFFEF)
}

// Latin, Greek and Cyrillic words go in boxes, so "2-of-3" never breaks at its hyphen.
// Other scripts keep Typst's line breaking, which handles CJK breaks and RTL word order.
#let is-simple(label) = label.codepoints().all(c => cp(c) < 0x0590)
#let wrappable(label) = if is-simple(label) { label.split(" ").map(w => box(w)).join(" ") } else { label }

// Pieces no line break can split: words, or single characters in CJK.
#let units(label) = label.split(" ").map(w => if w.codepoints().any(is-cjk) { w.clusters() } else { (w,) }).flatten()

// Largest size that fits: one line, else up to `max-lines` balanced lines in the narrowest
// box, else the next size down. Fails when nothing fits: clipped text must never ship.
#let fit-text(
  label,
  max-width,
  sizes: (16pt, 14pt, 13pt),
  max-lines: 2,
  what: "text",
  wrap-align: left,
  ..style,
) = {
  let body = wrappable(label)
  let result = none
  for s in sizes {
    // `dir: auto` per string; the page stays ltr.
    let styled(content) = text(size: s, dir: auto, ..style.named(), content)
    if measure(styled(body)).width <= max-width {
      result = box(styled(body))
      break
    }
    if max-lines < 2 { continue }
    let height(w) = measure(box(width: w, align(wrap-align, styled(body)))).height
    // Height of `max-lines` lines in the label's own font: CJK lines are taller.
    let probe = label.clusters().first()
    let lines = range(max-lines).map(_ => probe).join("\n")
    let limit = measure(box(width: max-width, styled(lines))).height * 1.1 + 0.5pt
    let widest = calc.max(..units(label).map(u => measure(styled(u)).width))
    if widest > max-width or height(max-width) > limit { continue }
    // Bisect down to the narrowest box with the same line count: balanced lines, no slack.
    let wrapped = height(max-width) + 0.5pt
    let (lo, hi) = (widest, max-width)
    for _ in range(10) {
      let mid = (lo + hi) / 2
      if height(mid) <= wrapped { hi = mid } else { lo = mid }
    }
    result = box(width: hi, align(wrap-align, styled(body)))
    break
  }
  assert(
    result != none,
    message: what + " \"" + label + "\" does not fit in " + repr(max-width) + " on " + str(max-lines) + " line(s)",
  )
  result
}

// Key tile: tinted square, key glyph, number badge overhanging the corner by 5 pt.
#let tile-side = 36pt
#let tile-gap = 8pt

#let key-tile(number, kind) = {
  let k = kinds.at(kind)
  box(width: tile-side, height: tile-side, {
    place(rect(width: tile-side, height: tile-side, radius: radius.tile, fill: k.chip, stroke: 1pt + k.rim))
    place(center + horizon, dy: 0.5pt, text(font: font.icons, size: 19pt, fill: k.accent, icon.key))
    place(
      bottom + right,
      dx: 5pt,
      dy: 5pt,
      circle(
        radius: 8pt,
        fill: k.accent,
        stroke: 2pt + color.surface,
        inset: 0pt,
        align(center + horizon, text(size: 10pt, weight: "bold", fill: color.ink, str(number))),
      ),
    )
  })
}

#let tiles-width(keys) = keys.len() * tile-side + (keys.len() - 1) * tile-gap

#let chip-inset = (left: 6pt, right: 10pt, top: 6pt, bottom: 6pt)
#let chip-gap = 10pt

// Policy chip: key tiles then label, styled like the GUI's form fields.
#let policy-chip(keys, label, label-width) = box(
  fill: color.surface,
  stroke: 1pt + color.line,
  radius: radius.tile,
  inset: chip-inset,
  grid(
    columns: (auto, auto),
    column-gutter: chip-gap,
    align: horizon,
    stack(dir: ltr, spacing: tile-gap, ..keys.map(k => key-tile(k.number, k.kind))),
    fit-text(label, label-width, sizes: (size.label, 13pt), what: "label", weight: "medium", fill: color.text),
  ),
)

// State pill over the rails: icon and uppercase text, no wider than its words, two lines at most.
#let state-pill(label, state, max-width, tracking: 0.06em) = {
  let st = states.at(state)
  let pad-x = 10pt
  let icon-size = 12pt
  let gap = 5pt
  let txt = fit-text(
    upper(label),
    max-width - 2 * pad-x - icon-size - gap - 1pt,
    sizes: (size.state, 10pt),
    max-lines: 2,
    what: "state",
    wrap-align: center,
    weight: "semibold",
    fill: st.ink,
    tracking: tracking,
  )
  box(
    fill: st.fill,
    stroke: 1pt + st.rim,
    radius: radius.pill,
    inset: (x: pad-x, y: 5pt),
    grid(columns: 2, column-gutter: gap, align: horizon, glyph(st.icon, size: icon-size, fill: st.ink), txt),
  )
}

// Event header: centred text, icon right above the guide.
#let event-header(label, icon-code, accent, max-width) = align(center, stack(
  dir: ttb,
  spacing: 6pt,
  fit-text(
    label,
    max-width,
    sizes: (size.header, 12pt, 11pt),
    max-lines: 3,
    what: "event header",
    wrap-align: center,
    fill: color.text-secondary,
  ),
  text(font: font.icons, size: 16pt, fill: accent, icon-code),
))

// Rail: solid green when it can spend, dashed grey when locked. The tail fades out.
#let rail(x1, x2, y, state, fade: false) = {
  if state == "spend" {
    let paint = color.green
    let fill = if fade {
      gradient.linear((paint, 0%), (paint, 55%), (paint.transparentize(100%), 100%))
    } else {
      paint
    }
    place(dx: x1, dy: y - 1pt, rect(width: x2 - x1, height: 2pt, fill: fill))
  } else {
    place(line(
      start: (x1, y),
      end: (x2, y),
      stroke: (paint: color.line-strong, thickness: 2pt, dash: (3pt, 6pt), cap: "round"),
    ))
  }
}

// Pills keep clear of the glow.
#let glow-radius = 24pt

// Marker on a rail. `glow` marks the moment the image is about: the timelock expiring.
#let marker(x, y, fill, glow: false) = {
  if glow {
    place(dx: x - glow-radius, dy: y - glow-radius, circle(
      radius: glow-radius,
      fill: gradient.radial(color.green.transparentize(55%), color.green.transparentize(100%)),
    ))
  }
  place(dx: x - 7pt, dy: y - 7pt, circle(radius: 7pt, fill: fill, stroke: 3pt + color.card))
}
