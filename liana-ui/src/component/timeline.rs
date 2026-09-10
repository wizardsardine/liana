//! The installer's spending timeline: one rail per policy, read left to right
//! through the events that open it.

use iced::{
    advanced::text as advanced_text,
    alignment::Vertical,
    mouse,
    widget::canvas::{self, gradient::Linear, Canvas, Frame, LineCap, Path, Stroke},
    Color, Font, Pixels, Point, Rectangle, Renderer, Size,
};
use liana_i18n::t;

use crate::{
    font,
    theme::{palette::TimelineTone, Theme},
    widget::Element,
};

/// A wallet shape the installer offers, with the policies it comes with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    Custom,
    Inheritance,
    MultisigSecurity,
}

/// The spending policies of a wallet template over time.
pub fn spending_timeline<'a, M: 'a>(template: Template) -> Element<'a, M> {
    Canvas::<Template, M, Theme, Renderer>::new(template)
        .width(CANVAS_WIDTH)
        .height(RAILS_TOP + ROW_PITCH * POLICIES as f32 + FOOTNOTE_HEIGHT)
        .into()
}

const CANVAS_WIDTH: f32 = 800.0;
const PAD: f32 = 28.0;
/// Event headers hang from here, icon last.
const HEADER_BOTTOM: f32 = 82.0;
const RAILS_TOP: f32 = 92.0;
const ROW_PITCH: f32 = 80.0;
const FOOTNOTE_HEIGHT: f32 = 68.0;
/// A chip label wraps past this.
const LABEL_MAX: f32 = 124.0;
const COLUMN_MIN: f32 = 190.0;
/// The rails need the rest.
const COLUMN_MAX: f32 = 280.0;
/// Policy column to first marker.
const COLUMN_GAP: f32 = 32.0;
const NAME_INSET: f32 = 8.0;
/// A policy name may run into the gap, clear of the guide.
const NAME_OVERHANG: f32 = COLUMN_GAP - 12.0;
const NAME_GAP: f32 = 7.0;
/// Open-ended tail, as a fraction of the event step.
const TAIL_RATIO: f32 = 0.7;
/// Room for the tail pill.
const TAIL_MIN: f32 = 140.0;
/// Part of the tail that fades out.
const FADE_RATIO: f32 = 0.35;
/// Pills keep this far from a marker.
const MARKER_CLEAR: f32 = 14.0;
/// Extra clearance a pill keeps from the glow around a marker.
const GLOW_CLEAR: f32 = 8.0;
/// Clearance a pill keeps at the open end of its rail.
const TAIL_CLEAR: f32 = 4.0;
const HEADER_WIDTH_MAX: f32 = 170.0;
/// A header keeps this much of the event step to itself.
const HEADER_STEP_INSET: f32 = 16.0;
const GUIDE_TOP: f32 = RAILS_TOP - 4.0;
const GUIDE_OVERHANG: f32 = 8.0;
/// Rails sit this far below the middle of their row.
const RAIL_DROP: f32 = 8.0;
const FOOTNOTE_GAP: f32 = 16.0;

const CARD_RADIUS: f32 = 16.0;
const TILE_RADIUS: f32 = 10.0;
const TILE_SIDE: f32 = 36.0;
const TILE_GAP: f32 = 8.0;
const CHIP_INSET_LEFT: f32 = 6.0;
const CHIP_INSET_RIGHT: f32 = 10.0;
const CHIP_INSET_Y: f32 = 6.0;
const CHIP_GAP: f32 = 10.0;
const BADGE_RADIUS: f32 = 8.0;
const BADGE_OVERHANG: f32 = 5.0;
const BADGE_RIM: f32 = 2.0;
const PILL_PAD_X: f32 = 10.0;
const PILL_PAD_Y: f32 = 5.0;
const PILL_GAP: f32 = 5.0;
const HEADER_ICON_GAP: f32 = 6.0;
const RAIL_WIDTH: f32 = 2.0;
const MARKER_RADIUS: f32 = 7.0;
const MARKER_RIM: f32 = 3.0;
/// The halo reads as part of the marker, so it scales with it.
const GLOW_RADIUS: f32 = MARKER_RADIUS * 2.0;
const GLOW_ALPHA: f32 = 0.45;
const GLOW_STEPS: usize = 16;
/// Trace of the accent left on the guide of the event the diagram is about.
const GUIDE_HOT_ALPHA: f32 = 0.28;
const HAIRLINE: f32 = 1.0;

/// Bootstrap Icons, the font liana-ui registers for [`crate::icon`].
const ICONS: Font = Font::with_name("bootstrap-icons");
const KEY_GLYPH: char = '\u{F44E}';
const LOCK_GLYPH: char = '\u{F47B}';
const UNLOCK_GLYPH: char = '\u{F600}';
const RECEIPT_GLYPH: char = '\u{F1BC}';
const HOURGLASS_GLYPH: char = '\u{F41F}';

const KEY_GLYPH_SIZE: f32 = 19.0;
const BADGE_TEXT_SIZE: f32 = 10.0;
const PILL_ICON_SIZE: f32 = 12.0;
const HEADER_ICON_SIZE: f32 = 16.0;
const LABEL_SIZES: &[f32] = &[14.0, 13.0];
const NAME_SIZES: &[f32] = &[13.0, 12.0, 11.0];
const HEADER_SIZES: &[f32] = &[13.0, 12.0, 11.0];
const STATE_SIZES: &[f32] = &[11.0, 10.0];
const FOOTNOTE_SIZES: &[f32] = &[13.0, 12.0, 11.0];
const LINE_HEIGHT: f32 = 1.0;

/// Steps of the bisection that balances a wrapped text over its lines.
const BISECT_STEPS: usize = 8;
/// Slack absorbing the rounding of a laid-out line height.
const ROUNDING: f32 = 0.5;

const POLICIES: usize = 2;
const EVENTS: [Event; 2] = [Event::Receipt, Event::Timelock];
/// A policy that spends from the first coin on.
const ALWAYS: [State; 2] = [State::Spend, State::Spend];
/// A policy the timelock opens.
const AFTER_TIMELOCK: [State; 2] = [State::Locked, State::Spend];

const SOLE_PRIMARY: [Key; 1] = [Key::new(1, KeyKind::Primary)];
const SOLE_RECOVERY: [Key; 1] = [Key::new(2, KeyKind::Recovery)];
const SOLE_INHERITANCE: [Key; 1] = [Key::new(2, KeyKind::Inheritance)];
const MULTISIG_PRIMARY: [Key; 2] = [Key::new(1, KeyKind::Primary), Key::new(2, KeyKind::Primary)];
const MULTISIG_RECOVERY: [Key; 3] = [
    Key::new(1, KeyKind::Primary),
    Key::new(2, KeyKind::Primary),
    Key::new(3, KeyKind::Recovery),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyKind {
    Primary,
    Recovery,
    Inheritance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Spend,
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    Receipt,
    Timelock,
}

#[derive(Debug, Clone, Copy)]
struct Key {
    number: u8,
    kind: KeyKind,
}

impl Key {
    const fn new(number: u8, kind: KeyKind) -> Self {
        Self { number, kind }
    }
}

struct Policy {
    name: String,
    label: String,
    keys: &'static [Key],
    segments: &'static [State],
}

impl Template {
    /// The policy that always spends, then the one the timelock opens.
    fn policies(self) -> [Policy; POLICIES] {
        let (primary_label, primary_keys, recovery_label, recovery_keys) = match self {
            Template::Custom => (
                t!("installer-template-custom-primary"),
                &SOLE_PRIMARY[..],
                t!("installer-template-custom-recovery"),
                &SOLE_RECOVERY[..],
            ),
            Template::Inheritance => (
                t!("installer-primary-key"),
                &SOLE_PRIMARY[..],
                t!("installer-inheritance-key"),
                &SOLE_INHERITANCE[..],
            ),
            Template::MultisigSecurity => (
                t!("installer-template-multisig-primary"),
                &MULTISIG_PRIMARY[..],
                t!("installer-template-multisig-recovery"),
                &MULTISIG_RECOVERY[..],
            ),
        };
        [
            Policy {
                name: t!("installer-template-policy-primary"),
                label: primary_label,
                keys: primary_keys,
                segments: &ALWAYS,
            },
            Policy {
                name: t!("installer-template-policy-recovery"),
                label: recovery_label,
                keys: recovery_keys,
                segments: &AFTER_TIMELOCK,
            },
        ]
    }
}

impl Event {
    fn label(self) -> String {
        match self {
            Event::Receipt => t!("installer-template-event-receipt"),
            Event::Timelock => t!("installer-template-event-timelock"),
        }
    }

    fn glyph(self) -> char {
        match self {
            Event::Receipt => RECEIPT_GLYPH,
            Event::Timelock => HOURGLASS_GLYPH,
        }
    }

    /// The timelock expiring is the moment the diagram is about.
    fn is_hot(self) -> bool {
        self == Event::Timelock
    }
}

impl State {
    fn label(self) -> String {
        match self {
            State::Spend => t!("installer-template-state-spend"),
            State::Locked => t!("installer-template-state-locked"),
        }
    }

    fn glyph(self) -> char {
        match self {
            State::Spend => UNLOCK_GLYPH,
            State::Locked => LOCK_GLYPH,
        }
    }
}

/// One run of consecutive events a policy spends through, or stays locked
/// through. The last run of a policy has no end: the timeline stays open.
struct Run {
    start: usize,
    end: Option<usize>,
    state: State,
    glow: bool,
}

/// A policy's rail, with the chip naming it.
struct Row {
    y: f32,
    name: Fitted,
    label: Fitted,
    keys: &'static [Key],
    chip: Size,
    runs: Vec<Run>,
}

/// A text laid out at the largest size that fits, with the bounds it takes.
struct Fitted {
    text: canvas::Text,
    bounds: Size,
}

impl<M> canvas::Program<M, Theme, Renderer> for Template {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        Painter {
            frame: &mut frame,
            theme,
        }
        .draw(*self);
        vec![frame.into_geometry()]
    }
}

struct Painter<'a> {
    frame: &'a mut Frame,
    theme: &'a Theme,
}

impl Painter<'_> {
    fn draw(&mut self, template: Template) {
        let rows = self.rows(template);
        let column = self.column_width(&rows);
        let x_start = PAD + column + COLUMN_GAP;
        let x_end = CANVAS_WIDTH - PAD;
        let (step, tail) = steps(x_end - x_start);
        let event_x: Vec<f32> = (0..EVENTS.len())
            .map(|k| x_start + step * k as f32)
            .collect();

        self.card();
        for (event, x) in EVENTS.into_iter().zip(&event_x) {
            self.guide(*x, event.is_hot());
            self.event_header(*x, event, (step - HEADER_STEP_INSET).min(HEADER_WIDTH_MAX));
        }
        for row in &rows {
            self.policy_chip(row);
            for run in &row.runs {
                let (x1, x2) = run.span(&event_x, x_end);
                self.rail(x1, x2, row.y, run.state, run.end.is_none());
            }
            for run in &row.runs {
                self.marker(event_x[run.start], row.y, run.state, run.glow);
            }
        }
        for row in &rows {
            for (r, run) in row.runs.iter().enumerate() {
                self.state_pill(row, run, row.runs.get(r + 1), &event_x, x_end, tail);
            }
        }
        self.footnote();
    }

    fn rows(&self, template: Template) -> Vec<Row> {
        template
            .policies()
            .into_iter()
            .enumerate()
            .map(|(i, policy)| {
                let chip_label = fit(
                    label(policy.label, font::MEDIUM, self.theme.colors.text.primary),
                    LABEL_SIZES,
                    LABEL_MAX,
                    2,
                );
                let policy_name = fit(
                    label(policy.name, font::BOLD, self.theme.colors.text.secondary),
                    NAME_SIZES,
                    COLUMN_MAX + NAME_OVERHANG - NAME_INSET,
                    1,
                );
                let tiles = tiles_width(policy.keys);
                let chip = Size::new(
                    CHIP_INSET_LEFT + tiles + CHIP_GAP + chip_label.bounds.width + CHIP_INSET_RIGHT,
                    2.0 * CHIP_INSET_Y + TILE_SIDE.max(chip_label.bounds.height),
                );
                Row {
                    y: RAILS_TOP + ROW_PITCH * i as f32 + ROW_PITCH / 2.0 + RAIL_DROP,
                    name: policy_name,
                    label: chip_label,
                    keys: policy.keys,
                    chip,
                    runs: runs(policy.segments),
                }
            })
            .collect()
    }

    /// The left column fits the widest chip, and the names lean into the gap.
    fn column_width(&self, rows: &[Row]) -> f32 {
        rows.iter()
            .map(|row| {
                row.chip
                    .width
                    .max(row.name.bounds.width + NAME_INSET - NAME_OVERHANG)
            })
            .fold(COLUMN_MIN, f32::max)
            .min(COLUMN_MAX)
    }

    fn card(&mut self) {
        let card = Path::rounded_rectangle(
            Point::new(HAIRLINE / 2.0, HAIRLINE / 2.0),
            Size::new(CANVAS_WIDTH - HAIRLINE, self.frame.height() - HAIRLINE),
            CARD_RADIUS.into(),
        );
        self.frame
            .fill(&card, self.theme.colors.cards.simple.background);
        self.frame.stroke(
            &card,
            Stroke::default()
                .with_color(self.theme.colors.text.border)
                .with_width(HAIRLINE),
        );
    }

    fn guide(&mut self, x: f32, hot: bool) {
        let bottom = GUIDE_TOP + ROW_PITCH * POLICIES as f32 + GUIDE_OVERHANG;
        let color = if hot {
            fade(self.theme.colors.timeline.spendable.ink, GUIDE_HOT_ALPHA)
        } else {
            self.theme.colors.text.border
        };
        self.frame.stroke(
            &Path::line(Point::new(x, GUIDE_TOP), Point::new(x, bottom)),
            Stroke::default().with_color(color).with_width(HAIRLINE),
        );
    }

    fn event_header(&mut self, x: f32, event: Event, width: f32) {
        let accent = if event.is_hot() {
            self.theme.colors.timeline.spendable.ink
        } else {
            self.theme.colors.text.secondary
        };
        let icon = glyph(event.glyph(), HEADER_ICON_SIZE, accent);
        let icon_height = measure(&icon).height;
        let header = fit(
            label(
                event.label(),
                font::REGULAR,
                self.theme.colors.text.secondary,
            ),
            HEADER_SIZES,
            width,
            3,
        );
        self.text(
            &header.text,
            Point::new(x, HEADER_BOTTOM - icon_height - HEADER_ICON_GAP),
            advanced_text::Alignment::Center,
            Vertical::Bottom,
        );
        self.text(
            &icon,
            Point::new(x, HEADER_BOTTOM - icon_height / 2.0),
            advanced_text::Alignment::Center,
            Vertical::Center,
        );
    }

    /// Key tiles then label, styled like the form fields of the installer.
    fn policy_chip(&mut self, row: &Row) {
        let top_left = Point::new(PAD, row.y - row.chip.height / 2.0);
        let chip = Path::rounded_rectangle(top_left, row.chip, TILE_RADIUS.into());
        self.frame
            .fill(&chip, self.theme.colors.general.form_field_background);
        self.frame.stroke(
            &chip,
            Stroke::default()
                .with_color(self.theme.colors.text.border)
                .with_width(HAIRLINE),
        );

        for (i, key) in row.keys.iter().enumerate() {
            let x = top_left.x + CHIP_INSET_LEFT + (TILE_SIDE + TILE_GAP) * i as f32;
            self.key_tile(Point::new(x, row.y - TILE_SIDE / 2.0), *key);
        }
        let label_x = top_left.x
            + CHIP_INSET_LEFT
            + tiles_width(row.keys)
            + CHIP_GAP
            + row.label.bounds.width / 2.0;
        self.text(
            &row.label.text,
            Point::new(label_x, row.y),
            advanced_text::Alignment::Center,
            Vertical::Center,
        );
        self.text(
            &row.name.text,
            Point::new(PAD + NAME_INSET, top_left.y - NAME_GAP),
            advanced_text::Alignment::Left,
            Vertical::Bottom,
        );
    }

    /// Tinted square, key glyph, number badge overhanging the corner.
    fn key_tile(&mut self, top_left: Point, key: Key) {
        let tone = self.tone(key.kind);
        let tile = Path::rounded_rectangle(
            top_left,
            Size::new(TILE_SIDE, TILE_SIDE),
            TILE_RADIUS.into(),
        );
        self.frame.fill(&tile, tone.fill);
        self.frame.stroke(
            &tile,
            Stroke::default().with_color(tone.rim).with_width(HAIRLINE),
        );

        let center = Point::new(top_left.x + TILE_SIDE / 2.0, top_left.y + TILE_SIDE / 2.0);
        let glyph = glyph(KEY_GLYPH, KEY_GLYPH_SIZE, tone.ink);
        self.text(
            &glyph,
            center,
            advanced_text::Alignment::Center,
            Vertical::Center,
        );

        let badge = Point::new(
            top_left.x + TILE_SIDE + BADGE_OVERHANG - BADGE_RADIUS,
            top_left.y + TILE_SIDE + BADGE_OVERHANG - BADGE_RADIUS,
        );
        let disc = Path::circle(badge, BADGE_RADIUS);
        self.frame.fill(&disc, tone.ink);
        self.frame.stroke(
            &disc,
            Stroke::default()
                .with_color(self.theme.colors.general.form_field_background)
                .with_width(BADGE_RIM),
        );
        let mut number = label(
            key.number.to_string(),
            font::BOLD,
            self.theme.colors.timeline.badge_ink,
        );
        number.size = Pixels(BADGE_TEXT_SIZE);
        self.text(
            &number,
            badge,
            advanced_text::Alignment::Center,
            Vertical::Center,
        );
    }

    /// Solid accent while the policy can spend, dashed grey while it cannot.
    /// The open tail fades out.
    fn rail(&mut self, x1: f32, x2: f32, y: f32, state: State, fades: bool) {
        let rail = match state {
            State::Spend => self.theme.colors.timeline.rail_spend,
            State::Locked => self.theme.colors.timeline.rail_locked,
        };
        match rail.dash {
            None => {
                let bar = Path::rectangle(
                    Point::new(x1, y - RAIL_WIDTH / 2.0),
                    Size::new(x2 - x1, RAIL_WIDTH),
                );
                if fades {
                    let gradient = Linear::new(Point::new(x1, y), Point::new(x2, y))
                        .add_stop(0.0, rail.color)
                        .add_stop(0.55, rail.color)
                        .add_stop(1.0, fade(rail.color, 0.0));
                    self.frame.fill(&bar, gradient);
                } else {
                    self.frame.fill(&bar, rail.color);
                }
            }
            Some(dash) => self.frame.stroke(
                &Path::line(Point::new(x1, y), Point::new(x2, y)),
                Stroke {
                    line_dash: dash,
                    ..Stroke::default()
                        .with_color(rail.color)
                        .with_width(RAIL_WIDTH)
                        .with_line_cap(LineCap::Round)
                },
            ),
        }
    }

    /// A dot where a run opens. The glow marks the first unlock.
    fn marker(&mut self, x: f32, y: f32, state: State, glow: bool) {
        let center = Point::new(x, y);
        if glow {
            self.glow(center);
        }
        let color = match state {
            State::Spend => self.theme.colors.timeline.rail_spend.color,
            State::Locked => self.theme.colors.timeline.rail_locked.color,
        };
        let dot = Path::circle(center, MARKER_RADIUS);
        self.frame.fill(&dot, color);
        self.frame.stroke(
            &dot,
            Stroke::default()
                .with_color(self.theme.colors.cards.simple.background)
                .with_width(MARKER_RIM),
        );
    }

    /// Discs from the rim inwards, standing in for the radial fade the canvas
    /// cannot fill. Each one adds just enough alpha to reach the ramp.
    fn glow(&mut self, center: Point) {
        let accent = self.theme.colors.timeline.spendable.ink;
        let mut covered = 0.0;
        for step in 0..GLOW_STEPS {
            let inward = (step + 1) as f32 / GLOW_STEPS as f32;
            let target = GLOW_ALPHA * inward;
            let alpha = (target - covered) / (1.0 - covered);
            covered = target;
            self.frame.fill(
                &Path::circle(center, GLOW_RADIUS * (1.0 - inward)),
                fade(accent, alpha),
            );
        }
    }

    /// Icon and uppercase text over the rail, clear of the markers and glows.
    fn state_pill(
        &mut self,
        row: &Row,
        run: &Run,
        next: Option<&Run>,
        event_x: &[f32],
        x_end: f32,
        tail: f32,
    ) {
        let tone = self.rail_tone(run.state);
        let (x1, x2) = run.span(event_x, x_end);
        let clear_left = if run.glow {
            GLOW_RADIUS + GLOW_CLEAR
        } else {
            MARKER_CLEAR
        };
        let clear_right = match next {
            None => TAIL_CLEAR,
            Some(next) if next.glow => GLOW_RADIUS + TAIL_CLEAR,
            Some(_) => MARKER_CLEAR,
        };

        let icon = glyph(run.state.glyph(), PILL_ICON_SIZE, tone.ink);
        let icon_bounds = measure(&icon);
        let state = fit(
            label(run.state.label().to_uppercase(), font::BOLD, tone.ink),
            STATE_SIZES,
            x2 - x1 - clear_left - clear_right - 2.0 * PILL_PAD_X - PILL_ICON_SIZE - PILL_GAP - 1.0,
            2,
        );
        let size = Size::new(
            2.0 * PILL_PAD_X + icon_bounds.width + PILL_GAP + state.bounds.width,
            2.0 * PILL_PAD_Y + icon_bounds.height.max(state.bounds.height),
        );

        // Centred on the solid part of its rail, then pushed inside the clearances.
        let solid = if run.end.is_none() {
            x2 - x1 - tail * FADE_RATIO
        } else {
            x2 - x1
        };
        let left = (x1 + clear_left)
            .max((x1 + solid / 2.0 - size.width / 2.0).min(x2 - clear_right - size.width));

        let pill = Path::rounded_rectangle(
            Point::new(left, row.y - size.height / 2.0),
            size,
            (size.height / 2.0).into(),
        );
        self.frame.fill(&pill, tone.fill);
        self.frame.stroke(
            &pill,
            Stroke::default().with_color(tone.rim).with_width(HAIRLINE),
        );
        self.text(
            &icon,
            Point::new(left + PILL_PAD_X + icon_bounds.width / 2.0, row.y),
            advanced_text::Alignment::Center,
            Vertical::Center,
        );
        self.text(
            &state.text,
            Point::new(
                left + PILL_PAD_X + icon_bounds.width + PILL_GAP + state.bounds.width / 2.0,
                row.y,
            ),
            advanced_text::Alignment::Center,
            Vertical::Center,
        );
    }

    fn footnote(&mut self) {
        let note = fit(
            label(
                t!("installer-template-footnote"),
                font::REGULAR,
                self.theme.colors.text.muted,
            ),
            FOOTNOTE_SIZES,
            CANVAS_WIDTH - 2.0 * PAD,
            2,
        );
        self.text(
            &note.text,
            Point::new(PAD, RAILS_TOP + ROW_PITCH * POLICIES as f32 + FOOTNOTE_GAP),
            advanced_text::Alignment::Left,
            Vertical::Top,
        );
    }

    fn tone(&self, kind: KeyKind) -> TimelineTone {
        let timeline = &self.theme.colors.timeline;
        match kind {
            KeyKind::Primary => timeline.primary,
            KeyKind::Recovery => timeline.recovery,
            KeyKind::Inheritance => timeline.inheritance,
        }
    }

    fn rail_tone(&self, state: State) -> TimelineTone {
        match state {
            State::Spend => self.theme.colors.timeline.spendable,
            State::Locked => self.theme.colors.timeline.locked,
        }
    }

    fn text(
        &mut self,
        text: &canvas::Text,
        position: Point,
        align_x: advanced_text::Alignment,
        align_y: Vertical,
    ) {
        self.frame.fill_text(canvas::Text {
            position,
            align_x,
            align_y,
            ..text.clone()
        });
    }
}

fn label(content: String, font: Font, color: Color) -> canvas::Text {
    canvas::Text {
        content,
        color,
        font,
        line_height: advanced_text::LineHeight::Relative(LINE_HEIGHT),
        shaping: advanced_text::Shaping::Advanced,
        ..canvas::Text::default()
    }
}

fn glyph(code: char, size: f32, color: Color) -> canvas::Text {
    let mut glyph = label(code.to_string(), ICONS, color);
    glyph.size = Pixels(size);
    glyph
}

impl Run {
    /// Where the run starts and ends on the canvas; an open run runs to the edge.
    fn span(&self, event_x: &[f32], x_end: f32) -> (f32, f32) {
        (
            event_x[self.start],
            self.end.map_or(x_end, |end| event_x[end]),
        )
    }
}

/// Merge consecutive events in the same state; the last run stays open.
fn runs(segments: &[State]) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for (k, state) in segments.iter().enumerate() {
        if out.last().is_some_and(|run| run.state == *state) {
            continue;
        }
        if let Some(last) = out.last_mut() {
            last.end = Some(k);
        }
        out.push(Run {
            start: k,
            end: None,
            state: *state,
            glow: false,
        });
    }
    // The first unlock is the moment the diagram is about.
    if let Some(run) = out.iter_mut().skip(1).find(|run| run.state == State::Spend) {
        run.glow = true;
    }
    out
}

/// Events evenly spaced over `span`, then an open-ended tail.
fn steps(span: f32) -> (f32, f32) {
    let events = EVENTS.len() as f32;
    let tail = (span / (events - 1.0 + TAIL_RATIO) * TAIL_RATIO).max(TAIL_MIN);
    ((span - tail) / (events - 1.0), tail)
}

fn tiles_width(keys: &[Key]) -> f32 {
    keys.len() as f32 * TILE_SIDE + (keys.len() as f32 - 1.0) * TILE_GAP
}

fn fade(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
}

/// Largest of `sizes` that fits on one line, else on up to `max_lines`
/// balanced lines, else the smallest size in the box it is given.
fn fit(mut text: canvas::Text, sizes: &[f32], max_width: f32, max_lines: usize) -> Fitted {
    for size in sizes {
        text.size = Pixels(*size);
        text.max_width = f32::INFINITY;
        let single = measure(&text);
        if single.width <= max_width {
            return Fitted {
                text,
                bounds: single,
            };
        }
        if max_lines > 1 {
            text.max_width = max_width;
            let wrapped = measure(&text);
            if wrapped.height <= size * LINE_HEIGHT * max_lines as f32 + ROUNDING {
                text.max_width = balanced_width(&text, wrapped.height);
                let bounds = measure(&text);
                return Fitted { text, bounds };
            }
        }
    }
    text.max_width = max_width;
    let bounds = measure(&text);
    Fitted { text, bounds }
}

/// Narrowest box that keeps the wrapped line count, so the lines come out
/// balanced rather than one long line and a stub.
fn balanced_width(text: &canvas::Text, wrapped_height: f32) -> f32 {
    let mut probe = text.clone();
    let (mut low, mut high) = (widest_word(text), text.max_width);
    if low >= high {
        return high;
    }
    for _ in 0..BISECT_STEPS {
        let mid = (low + high) / 2.0;
        probe.max_width = mid;
        if measure(&probe).height <= wrapped_height {
            high = mid;
        } else {
            low = mid;
        }
    }
    high
}

/// No box narrower than this can hold the text without breaking a word.
fn widest_word(text: &canvas::Text) -> f32 {
    let mut probe = text.clone();
    probe.max_width = f32::INFINITY;
    text.content
        .split_whitespace()
        .map(|word| {
            probe.content = word.to_owned();
            measure(&probe).width
        })
        .fold(0.0, f32::max)
}

/// The bounds `Frame::fill_text` will lay the text out in.
fn measure(text: &canvas::Text) -> Size {
    use advanced_text::Paragraph as _;

    type Layout = <Renderer as advanced_text::Renderer>::Paragraph;
    Layout::with_text(advanced_text::Text {
        content: text.content.as_str(),
        bounds: Size::new(text.max_width, f32::INFINITY),
        size: text.size,
        line_height: text.line_height,
        font: text.font,
        align_x: text.align_x,
        align_y: text.align_y,
        shaping: text.shaping,
        wrapping: advanced_text::Wrapping::default(),
    })
    .min_bounds()
}
