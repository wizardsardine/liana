//! Reusable "spending policy timeline" diagram, composed entirely from
//! in-app primitives (no image assets). Each [`PolicyRow`] renders as a
//! labelled key box on the left and a timeline of spend windows on the
//! right (a full-width green "CAN SPEND" bar, or a red "CAN'T SPEND
//! (TIMELOCKED)" segment followed by a green "CAN SPEND" segment).
//!
//! Used by every wallet-type "Introduction" screen so all templates share
//! one consistent, theme-aware diagram.

use iced::{widget::container, widget::Space, Alignment, Background, Border, Color, Length};

use coincube_ui::{
    color,
    component::text::{caption, p2_regular, Text},
    icon, theme,
    widget::*,
};

use coincube_ui::theme::{palette::ThemeMode, Theme};

use crate::installer::message::Message;

/// How a policy's spend window is timelocked in the diagram.
#[derive(Clone, Copy)]
pub enum Timelock {
    /// Always spendable — a single full-width "CAN SPEND" bar.
    None,
    /// Timelocked: the red "CAN'T SPEND" segment spans `fraction` (0.0–1.0)
    /// of the timeline before the green "CAN SPEND" segment. A larger
    /// fraction pushes the spend window further right, showing a later
    /// activation than paths with a smaller fraction.
    After(f32),
}

/// One spending policy shown as a row in the diagram.
pub struct PolicyRow<'a> {
    /// Caption above the key box, e.g. "Primary spending policy:".
    pub label: &'a str,
    /// Numbered key badges (badge number, colour) shown inside the box.
    pub keys: Vec<(u32, Color)>,
    /// Label inside the box, e.g. "2-of-3 multisig" or "Inheritance Key".
    pub box_label: &'a str,
    /// When (relative to the timeline) this policy becomes spendable.
    pub timelock: Timelock,
}

/// Fixed width of the left-hand label/key-box column so every row's
/// timeline starts at the same x-offset.
const LABEL_W: f32 = 240.0;
const BAR_H: f32 = 38.0;

/// A translucent fill derived from a solid brand colour.
fn tint(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

fn is_light(t: &Theme) -> bool {
    matches!(t.mode, ThemeMode::Light)
}

/// Background of the whole diagram card — theme-aware.
fn card_bg(t: &Theme) -> Color {
    if is_light(t) {
        color::LIGHT_CARD_BG
    } else {
        color::GREY_5
    }
}

/// Background of a key box / badge — theme-aware, distinct from the card.
fn box_bg(t: &Theme) -> Color {
    if is_light(t) {
        color::LIGHT_SURFACE
    } else {
        color::GREY_6
    }
}

/// A small numbered circle in the key's colour.
fn key_badge<'a>(n: u32, c: Color) -> Element<'a, Message> {
    Container::new(
        p2_regular(n.to_string())
            .style(theme::text::adaptive(c))
            .bold(),
    )
    .center_x(Length::Fixed(26.0))
    .center_y(Length::Fixed(26.0))
    .style(move |t| container::Style {
        background: Some(Background::Color(box_bg(t))),
        border: Border {
            color: theme::text::adapt_color(c, t),
            width: 1.5,
            radius: 13.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// The rounded box on the left of a row: key badges + a label.
fn policy_box<'a>(keys: &[(u32, Color)], label: &'a str) -> Element<'a, Message> {
    let badges = keys.iter().fold(
        Row::new().spacing(4).align_y(Alignment::Center),
        |row, (n, c)| row.push(key_badge(*n, *c)),
    );
    Container::new(
        Row::new()
            .align_y(Alignment::Center)
            .spacing(12)
            .push(badges)
            .push(p2_regular(label).bold()),
    )
    .padding([8, 14])
    .style(|t| container::Style {
        background: Some(Background::Color(box_bg(t))),
        border: Border {
            radius: 12.0.into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

/// A single spend-window pill: translucent fill + solid coloured label,
/// with an optional trailing arrow.
fn bar<'a>(label: &'a str, c: Color, arrow: bool, width: Length) -> Element<'a, Message> {
    let mut content = Row::new()
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(Space::new().width(Length::Fill))
        .push(p2_regular(label).style(theme::text::adaptive(c)).bold())
        .push(Space::new().width(Length::Fill));
    if arrow {
        content = content.push(icon::arrow_right().style(theme::text::adaptive(c)));
    }
    Container::new(content)
        .width(width)
        .center_y(Length::Fixed(BAR_H))
        .padding([0, 12])
        .style(move |t| {
            let ac = theme::text::adapt_color(c, t);
            container::Style {
                background: Some(Background::Color(tint(ac, 0.16))),
                border: Border {
                    color: tint(ac, 0.5),
                    width: 1.0,
                    radius: 8.0.into(),
                },
                ..Default::default()
            }
        })
        .into()
}

fn policy_row<'a>(r: PolicyRow<'a>, label_w: f32) -> Element<'a, Message> {
    let timeline: Element<Message> = match r.timelock {
        Timelock::None => bar("CAN SPEND", color::GREEN, true, Length::Fill),
        Timelock::After(fraction) => {
            // Split the timeline red:green by the timelock fraction. A larger
            // fraction => the green spend window starts further right.
            let red = ((fraction * 100.0).round() as u16).clamp(1, 99);
            let green = 100 - red;
            Row::new()
                .spacing(8)
                .width(Length::Fill)
                .push(bar(
                    "CAN'T SPEND (TIMELOCKED)",
                    color::RED,
                    false,
                    Length::FillPortion(red),
                ))
                .push(bar(
                    "CAN SPEND",
                    color::GREEN,
                    true,
                    Length::FillPortion(green),
                ))
                .into()
        }
    };

    // Caption on its own line, then the key box and the timeline bars in a
    // centred row so the bars line up horizontally with the key box.
    Column::new()
        .spacing(6)
        .push(caption(r.label).style(theme::text::secondary))
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .spacing(16)
                .push(
                    Container::new(policy_box(&r.keys, r.box_label)).width(Length::Fixed(label_w)),
                )
                .push(Container::new(timeline).width(Length::Fill)),
        )
        .into()
}

/// Render the full policy-timeline diagram for the given rows.
pub fn policy_timeline<'a>(rows: Vec<PolicyRow<'a>>) -> Element<'a, Message> {
    // Widen the key-box column when a box holds many badges (e.g. a 2-of-6
    // path) so it doesn't overflow into the timeline. Diagrams with ≤3 keys
    // keep the default width, so existing templates render unchanged.
    let label_w = if rows.iter().any(|r| r.keys.len() > 3) {
        360.0
    } else {
        LABEL_W
    };

    // Header: "Receipt of funds" over the timeline start, "After some time*
    // of wallet inactivity" centred over the timelock boundary.
    let header = Row::new()
        .push(Space::new().width(Length::Fixed(label_w + 16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(caption("Receipt of funds").style(theme::text::secondary))
                .push(Space::new().width(Length::Fill))
                .push(
                    caption("After some time* of wallet inactivity").style(theme::text::secondary),
                )
                .push(Space::new().width(Length::Fill)),
        );

    let body = rows
        .into_iter()
        .fold(Column::new().spacing(20).push(header), |col, r| {
            col.push(policy_row(r, label_w))
        });

    let body = body.push(
        caption(
            "*The time range (timelock) for the activation of keys can be configured in the next step.",
        )
        .style(theme::text::secondary),
    );

    Container::new(body)
        .width(Length::Fill)
        .padding(24)
        .style(|t| container::Style {
            background: Some(Background::Color(card_bg(t))),
            border: Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}
