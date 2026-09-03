// Layout: one rail per policy, read left to right through the events. The left column fits
// the widest chip, the rails take the rest, and every text is checked to fit.
// Back to front: card, guides and headers, rails, markers, pills.

#import "theme.typ": *
#import "components.typ": *

#let canvas-width = 800pt
#let pad = 28pt
#let header-bottom = 82pt // headers hang from here, icon last
#let rails-top = 92pt
#let row-pitch = 80pt
#let footnote-height = 68pt
#let label-max = 124pt // a chip label wraps past this
#let column-min = 190pt
#let column-max = 280pt // the rails need the rest
#let column-gap = 32pt // column to first marker
#let name-inset = 8pt
#let name-overhang = column-gap - 12pt // a name may run into the gap, clear of the guide
#let tail-ratio = 0.7 // tail, as a fraction of the event step
#let tail-min = 140pt // room for the tail pill
#let fade-ratio = 0.35 // part of the tail that fades out
#let marker-clear = 14pt // pills keep this far from a marker

#let canvas-height(spec) = rails-top + row-pitch * spec.policies.len() + footnote-height

#let event-icons = (receipt: icon.receipt, timelock: icon.hourglass)

// Merge consecutive segments in the same state: (start event, end event or none, state).
#let runs(segments) = {
  let out = ()
  for (k, state) in segments.enumerate() {
    if out.len() > 0 and out.last().state == state {
      continue
    }
    if out.len() > 0 {
      out.last().end = k
    }
    out.push((start: k, end: none, state: state))
  }
  out
}

#let spending-timeline(spec, locale, tag) = context {
  let s(key) = {
    let own = locale.at(spec.id, default: (:))
    if key in own { own.at(key) } else { locale.common.at(key) }
  }
  let tracking = tracking-for(tag)
  let height = canvas-height(spec)
  let n-events = spec.events.len()
  let n-policies = spec.policies.len()

  // Card with the installer's hairline border, drawn inside the canvas so nothing is clipped.
  place(dx: 0.5pt, dy: 0.5pt, rect(
    width: canvas-width - 1pt,
    height: height - 1pt,
    radius: radius.card,
    fill: color.card,
    stroke: 1pt + color.line,
  ))

  // Left column: chips, and the names above them.
  let chips = spec.policies.map(p => policy-chip(p.keys, s(p.label), label-max))
  let names = spec.policies.map(p => fit-text(
    s(p.name),
    column-max + name-overhang - name-inset,
    sizes: (size.name, 12pt, 11pt),
    max-lines: 1,
    what: "policy name",
    weight: "semibold",
    fill: color.text-secondary,
  ))
  let column-width = calc.max(
    ..chips.map(c => measure(c).width),
    ..names.map(n => measure(n).width + name-inset - name-overhang),
    column-min,
  )
  assert(column-width <= column-max, message: "policy column too wide: " + repr(column-width))

  // Events evenly spaced, then an open-ended tail.
  let x-start = pad + column-width + column-gap
  let x-end = canvas-width - pad
  let span = x-end - x-start
  assert(span >= 300pt, message: "timeline too narrow: " + repr(span))
  let tail = if n-events == 1 { span } else {
    calc.max(span / (n-events - 1 + tail-ratio) * tail-ratio, tail-min)
  }
  let step = if n-events == 1 { span } else { (span - tail) / (n-events - 1) }
  let event-x = range(n-events).map(k => x-start + step * k)

  // Guides and headers. The timelock guide gets a trace of green, like its icon and the glows.
  for (k, event) in spec.events.enumerate() {
    let x = event-x.at(k)
    let hot = event == "timelock"
    place(
      dx: x,
      dy: rails-top - 4pt,
      line(
        length: row-pitch * n-policies + 8pt,
        angle: 90deg,
        stroke: 1pt + if hot { color.green.transparentize(72%) } else { color.line },
      ),
    )
    let width = calc.min(170pt, step - 16pt)
    let header = event-header(
      s("event_" + event),
      event-icons.at(event, default: icon.hourglass),
      if hot { color.green } else { color.text-secondary },
      width,
    )
    let h = measure(box(width: width, header)).height
    place(dx: x - width / 2, dy: header-bottom - h, box(width: width, header))
  }

  // Per row: rail y, runs, and which run opens with the glow (the first unlock).
  let rows = spec.policies.enumerate().map(((i, policy)) => {
    let policy-runs = runs(policy.segments)
    let glows = ()
    let glowed = false
    for (r, run) in policy-runs.enumerate() {
      let opens = r > 0 and run.state == "spend" and policy-runs.at(r - 1).state == "locked"
      let glow = opens and not glowed
      if glow { glowed = true }
      glows.push(glow)
    }
    (y: rails-top + row-pitch * i + row-pitch / 2 + 8pt, runs: policy-runs, glows: glows)
  })

  // Chips and names, then rails and markers.
  for (i, row) in rows.enumerate() {
    let chip = chips.at(i)
    let chip-h = measure(chip).height
    let name = names.at(i)
    let name-h = measure(name).height
    place(dx: pad + name-inset, dy: row.y - chip-h / 2 - 7pt - name-h, name)
    place(dx: pad, dy: row.y - chip-h / 2, chip)
    for run in row.runs {
      let x1 = event-x.at(run.start)
      let x2 = if run.end == none { x-end } else { event-x.at(run.end) }
      rail(x1, x2, row.y, run.state, fade: run.end == none)
    }
    for (r, run) in row.runs.enumerate() {
      let fill = if run.state == "spend" { color.green } else { color.line-strong }
      marker(event-x.at(run.start), row.y, fill, glow: row.glows.at(r))
    }
  }

  // Pills last, centred on the solid part of their rail, clear of markers and glows.
  for row in rows {
    for (r, run) in row.runs.enumerate() {
      let last = run.end == none
      let x1 = event-x.at(run.start)
      let x2 = if last { x-end } else { event-x.at(run.end) }
      let clear-left = if row.glows.at(r) { glow-radius + 8pt } else { marker-clear }
      let clear-right = if last { 4pt } else if row.glows.at(r + 1) { glow-radius + 4pt } else { marker-clear }
      let pill = state-pill(
        s("state_" + run.state),
        run.state,
        (x2 - x1) - clear-left - clear-right,
        tracking: tracking,
      )
      let (width: w, height: h) = measure(pill)
      let solid = if last { (x2 - x1) - tail * fade-ratio } else { x2 - x1 }
      let left = calc.max(x1 + clear-left, calc.min(x1 + solid / 2 - w / 2, x2 - clear-right - w))
      place(dx: left, dy: row.y - h / 2, pill)
    }
  }

  if "footnote" in spec {
    let note = fit-text(
      s(spec.footnote),
      canvas-width - 2 * pad,
      sizes: (size.footnote, 12pt, 11pt),
      max-lines: 2,
      what: "footnote",
      fill: color.text-muted,
    )
    place(dx: pad, dy: rails-top + row-pitch * n-policies + 16pt, note)
  }
}
