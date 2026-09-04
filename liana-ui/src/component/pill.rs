use std::fmt::Display;

use iced::{
    widget::{container::Style, row, tooltip, Space},
    Alignment, Font, Length,
};
use iced_core::text::{LineHeight, Shaping};
use liana_i18n::t;

use crate::{
    theme::{self, Theme},
    widget::{self, *},
};

use super::text::new;

const PILL_PADDING: [u16; 2] = [6, 15];
const PILL_PADDING_COMPACT: [u16; 2] = [4, 6];
const PILL_FONT_SIZE: u32 = new::B4_MEDIUM_SPEC.size.unwrap();
const PILL_FONT_SIZE_COMPACT: u32 = new::CAPTION_SPEC.size.unwrap();
const PILL_FONT: Font = new::B4_MEDIUM_SPEC.font;
const PILL_FONT_COMPACT: Font = new::CAPTION_SPEC.font;
// Tight line box so compact pills size to the glyph, not the default 1.3 leading.
const COMPACT_LINE_HEIGHT: LineHeight = LineHeight::Relative(1.0);

fn tooltip_text<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    new::caption(content)
}

#[derive(Debug, Clone, Copy)]
#[repr(u16)]
pub enum PillWidth {
    S = 90,
    WalletStatus = 100,
    M = 150,
    ML = 175,
    L = 200,
    XL = 250,
    Shrink,
    Fill,
}

impl From<PillWidth> for Length {
    fn from(value: PillWidth) -> Self {
        match value {
            PillWidth::Shrink => return Length::Shrink,
            PillWidth::Fill => return Length::Fill,
            _ => {}
        }
        Length::Fixed(value as u16 as f32)
    }
}

pub fn pill<'a, T: 'a>(
    label: impl Display,
    tooltip: impl Display,
    width: PillWidth,
    style: fn(&Theme) -> Style,
) -> Container<'a, T> {
    let tooltip = tooltip.to_string();
    let pill = pill_body_with_font(label, width, style, PILL_FONT);
    if !tooltip.is_empty() {
        pill_with_tooltip(pill, Some(tooltip))
    } else {
        pill
    }
}

pub fn pill_with_icon<'a, T: 'a, L: Display, TT: Display>(
    icon: Option<crate::widget::Text<'static>>,
    label: L,
    tooltip: TT,
    width: PillWidth,
    style: fn(&Theme) -> Style,
    compact: bool,
) -> Container<'a, T> {
    let size = if compact {
        PILL_FONT_SIZE_COMPACT
    } else {
        PILL_FONT_SIZE
    };
    let font = if compact {
        PILL_FONT_COMPACT
    } else {
        PILL_FONT
    };
    let mut label = iced::widget::text!("{label}")
        .shaping(Shaping::Advanced)
        .font(font)
        .center()
        .size(size);
    if compact {
        label = label.line_height(COMPACT_LINE_HEIGHT);
    }
    let content = match (icon, compact) {
        (Some(icon), true) => row![
            icon.line_height(COMPACT_LINE_HEIGHT),
            Space::with_width(6),
            label
        ],
        (Some(icon), false) => row![icon, Space::with_width(15), label, Space::fill_width()],
        (None, _) => row![label],
    };
    let pill = if compact {
        Container::new(content)
            .padding(PILL_PADDING_COMPACT)
            .style(style)
            .center_x(width)
    } else {
        Container::new(content)
            .padding(PILL_PADDING)
            .style(style)
            .center_x(width)
    };
    let tooltip = tooltip.to_string();
    pill_with_tooltip(pill, (!tooltip.is_empty()).then_some(tooltip))
}

fn pill_with_tooltip<'a, T: 'a, P: Into<Container<'a, T>>, TT: Display>(
    pill: P,
    tooltip: Option<TT>,
) -> Container<'a, T> {
    if let Some(tooltip) = tooltip {
        Container::new({
            tooltip::Tooltip::new(
                pill.into(),
                Container::new(tooltip_text(tooltip))
                    .padding(PILL_PADDING)
                    .style(theme::card::simple),
                tooltip::Position::Top,
            )
        })
    } else {
        pill.into()
    }
}

fn pill_body_with_font<'a, T: 'a, L: Display>(
    label: L,
    width: PillWidth,
    style: fn(&Theme) -> Style,
    font: Font,
) -> Container<'a, T> {
    pill_body_with_text_size_and_font_and_compact(label, width, style, font, PILL_FONT_SIZE, false)
}

fn compact_pill_body_with_font<'a, T: 'a, L: Display>(
    label: L,
    width: PillWidth,
    style: fn(&Theme) -> Style,
    font: Font,
) -> Container<'a, T> {
    pill_body_with_text_size_and_font_and_compact(
        label,
        width,
        style,
        font,
        PILL_FONT_SIZE_COMPACT,
        true,
    )
}

fn pill_body_with_text_size_and_font<'a, T: 'a, L: Display>(
    label: L,
    width: PillWidth,
    style: fn(&Theme) -> Style,
    font: Font,
    size: u32,
) -> Container<'a, T> {
    pill_body_with_text_size_and_font_and_compact(label, width, style, font, size, false)
}

fn compact_pill_body_with_text_size_and_font<'a, T: 'a, L: Display>(
    label: L,
    width: PillWidth,
    style: fn(&Theme) -> Style,
    font: Font,
    size: u32,
) -> Container<'a, T> {
    pill_body_with_text_size_and_font_and_compact(label, width, style, font, size, true)
}

fn pill_body_with_text_size_and_font_and_compact<'a, T: 'a, L: Display>(
    label: L,
    width: PillWidth,
    style: fn(&Theme) -> Style,
    font: Font,
    size: u32,
    compact: bool,
) -> Container<'a, T> {
    let mut item = iced::widget::text!("{label}")
        .shaping(Shaping::Advanced)
        .font(font)
        .center()
        .size(size);
    if compact {
        item = item.line_height(COMPACT_LINE_HEIGHT);
    }
    let padding = if compact {
        PILL_PADDING_COMPACT
    } else {
        PILL_PADDING
    };
    pill_body_with_item_and_padding(item, width, style, padding)
}

fn pill_body_with_item_and_padding<'a, T: 'a, I: Into<Element<'a, T>>>(
    item: I,
    width: PillWidth,
    style: fn(&Theme) -> Style,
    padding: [u16; 2],
) -> Container<'a, T> {
    Container::new(item)
        .padding(padding)
        .width(width)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(style)
}

macro_rules! pills {
    ($($name:ident, $label:literal, $tooltip:literal, $width:ident, $style:ident);* $(;)?) => {
        $(
            pub fn $name<'a, T: 'a>() -> Container<'a, T> {
                pill(t!($label), t!($tooltip), PillWidth::$width, theme::pill::$style)
            }
        )*
    };
}

#[rustfmt::skip]
pills! {
    recovery,       "common-recovery",     "pill-recovery-tooltip",     M, simple;
    batch,          "pill-batch",        "pill-batch-tooltip",        M, simple;
    deprecated,     "pill-deprecated",   "pill-deprecated-tooltip",   M, simple;
    spent,          "pill-spent",        "pill-spent-tooltip",        M, simple;
    unsigned,       "pill-unsigned",     "pill-unsigned-tooltip",     M, soft_warning;
    signed,         "pill-signed",       "pill-signed-tooltip",       M, soft_warning;
    unconfirmed,    "pill-unconfirmed",  "pill-unconfirmed-tooltip",  M, simple_fill;
    confirmed,      "pill-confirmed",    "pill-confirmed-tooltip",    M, success;
    key_internal,   "pill-key-internal", "pill-key-internal-tooltip", M, internal;
    // Business installer only
    key_external,   "pill-key-external",   "pill-key-external-tooltip",   M, external;
    key_cosigner,   "pill-key-cosigner",   "pill-key-cosigner-tooltip",   M, safety_net;
    key_safety_net, "pill-key-safety-net", "pill-key-safety-net-tooltip", M, safety_net;
}

/// Wallet lifecycle status pills, compact (the wallet list trailing).
const WALLET_STATUS_WIDTH: PillWidth = PillWidth::WalletStatus;

pub fn register<'a, T: 'a>() -> Container<'a, T> {
    compact_pill(
        t!("common-register"),
        WALLET_STATUS_WIDTH,
        theme::pill::warning,
    )
}

pub fn draft<'a, T: 'a>() -> Container<'a, T> {
    compact_pill(t!("pill-draft"), WALLET_STATUS_WIDTH, theme::pill::simple)
}

pub fn to_approve<'a, T: 'a>() -> Container<'a, T> {
    compact_pill(
        t!("pill-to-approve"),
        WALLET_STATUS_WIDTH,
        theme::pill::warning,
    )
}

pub fn set_keys<'a, T: 'a>() -> Container<'a, T> {
    compact_pill(
        t!("btn-set-keys"),
        WALLET_STATUS_WIDTH,
        theme::pill::warning,
    )
}

pub fn active<'a, T: 'a>() -> Container<'a, T> {
    compact_pill(t!("pill-active"), WALLET_STATUS_WIDTH, theme::pill::success)
}

/// Compact key pill styled by key kind (the key-type and path key-alias pills).
pub fn key_kind<'a, T: 'a, L: Display>(
    kind: crate::component::list::EntryKeyKind,
    label: L,
) -> Container<'a, T> {
    use crate::component::list::EntryKeyKind;
    let style = match kind {
        EntryKeyKind::Internal => theme::pill::internal,
        EntryKeyKind::External => theme::pill::external,
        EntryKeyKind::SafetyNet => theme::pill::safety_net,
    };
    compact_pill_body_with_text_size_and_font(
        label,
        PillWidth::Shrink,
        style,
        PILL_FONT_COMPACT,
        PILL_FONT_SIZE_COMPACT,
    )
}

/// Spending-path availability pill: a recovery timelock with a clock.
pub fn path_timelock<'a, T: 'a, L: Display>(label: L) -> Container<'a, T> {
    pill_with_icon(
        Some(crate::icon::clock_icon()),
        label,
        "",
        PillWidth::ML,
        theme::pill::safety_net,
        true,
    )
}

/// Spending-path availability pill: always available, with a check.
pub fn path_always_available<'a, T: 'a>() -> Container<'a, T> {
    pill_with_icon(
        Some(crate::icon::check_icon()),
        t!("pill-always-available"),
        "",
        PillWidth::ML,
        theme::pill::success,
        true,
    )
}

pub fn registered<'a, T: 'a>() -> Container<'a, T> {
    pill_with_icon(
        Some(crate::icon::check_icon().style(theme::text::success)),
        t!("device-registered"),
        "",
        PillWidth::Shrink,
        theme::pill::success,
        true,
    )
}

pub fn compact_metric<'a, T: 'a, L: Display>(
    text: L,
    style: fn(&Theme) -> Style,
) -> Container<'a, T> {
    compact_pill_body_with_text_size_and_font(
        text,
        PillWidth::Shrink,
        style,
        PILL_FONT_COMPACT,
        PILL_FONT_SIZE_COMPACT,
    )
}

pub fn compact_pill<'a, T: 'a>(
    text: impl Display,
    width: PillWidth,
    style: fn(&Theme) -> Style,
) -> Container<'a, T> {
    compact_pill_body_with_text_size_and_font(
        text,
        width,
        style,
        PILL_FONT_COMPACT,
        PILL_FONT_SIZE_COMPACT,
    )
}

pub fn role_manager<'a, T: 'a>() -> Container<'a, T> {
    compact_metric(t!("business-role-manager"), theme::pill::role_manager)
}

pub fn role_participant<'a, T: 'a>() -> Container<'a, T> {
    compact_metric(
        t!("business-role-participant"),
        theme::pill::role_participant,
    )
}

pub fn ws_admin<'a, T: 'a>() -> Container<'a, T> {
    compact_metric(t!("pill-ws-admin"), theme::pill::simple)
}

/// Success pill shown for a key that is used in the template: a checkmark only, with a hover tooltip
/// stating how many spending paths use the key.
pub fn in_policy<'a, T: 'a>(usage_count: usize) -> Container<'a, T> {
    let pill = Container::new(crate::icon::check_icon().line_height(COMPACT_LINE_HEIGHT))
        .padding(PILL_PADDING_COMPACT)
        .style(theme::pill::success);
    let tip = t!("pill-used-in-spending-paths", count = usage_count);
    pill_with_tooltip(pill, Some(tip))
}

/// Light-warning pill shown for a key that is not used in any spending path yet.
pub fn not_in_policy<'a, T: 'a>() -> Container<'a, T> {
    pill_with_icon(
        Some(crate::icon::tooltip_icon()),
        t!("pill-not-in-policy"),
        "",
        PillWidth::Shrink,
        theme::pill::soft_warning,
        true,
    )
}

pub fn xpub_set<'a, T: 'a>() -> Container<'a, T> {
    compact_pill(t!("pill-xpub-set"), PillWidth::S, theme::pill::success)
}

pub fn xpub_not_set<'a, T: 'a>() -> Container<'a, T> {
    compact_pill(t!("pill-xpub-not-set"), PillWidth::S, theme::pill::warning)
}

pub fn unconfirmed_compact<'a, T: 'a>() -> Container<'a, T> {
    compact_pill_body_with_text_size_and_font(
        t!("pill-unconfirmed"),
        PillWidth::M,
        theme::pill::simple_fill,
        PILL_FONT,
        PILL_FONT_SIZE_COMPACT,
    )
}

pub fn rescan<'a, T: 'a>(progress: f64, compact: bool) -> Container<'a, T> {
    let size = if compact {
        PILL_FONT_SIZE_COMPACT
    } else {
        PILL_FONT_SIZE
    };
    let font = if compact {
        PILL_FONT_COMPACT
    } else {
        PILL_FONT
    };
    let width = if compact { PillWidth::M } else { PillWidth::L };
    let label = t!(
        "pill-rescan-progress",
        progress = format!("{:.2}", progress * 100.0)
    );
    if compact {
        compact_pill_body_with_text_size_and_font(label, width, theme::pill::simple, font, size)
    } else {
        pill_body_with_text_size_and_font(label, width, theme::pill::simple, font, size)
    }
}

pub fn fingerprint<'a, T: 'a>(
    fg: impl Into<String>,
    alias: Option<impl Into<String>>,
) -> Container<'a, T> {
    let fg = fg.into();
    match alias {
        Some(alias) => {
            let body = compact_pill_body_with_font(
                alias.into(),
                PillWidth::Shrink,
                theme::pill::fingerprint,
                PILL_FONT_COMPACT,
            );
            Container::new(tooltip::Tooltip::new(
                body,
                Container::new(tooltip_text(fg))
                    .padding(PILL_PADDING_COMPACT)
                    .style(theme::card::simple),
                tooltip::Position::Top,
            ))
        }
        None => compact_pill_body_with_font(
            fg,
            PillWidth::M,
            theme::pill::fingerprint,
            PILL_FONT_COMPACT,
        ),
    }
}

const BLOCKS_PER_DAY: u32 = 144;

#[derive(Debug, Clone, Copy)]
enum RecoveryEta {
    Available,
    Today,
    TwoDays,
    Longer,
}

fn recovery_eta(sequence: u32) -> RecoveryEta {
    if sequence == 0 {
        RecoveryEta::Available
    } else if sequence <= BLOCKS_PER_DAY {
        RecoveryEta::Today
    } else if sequence <= 2 * BLOCKS_PER_DAY {
        RecoveryEta::TwoDays
    } else {
        RecoveryEta::Longer
    }
}

impl RecoveryEta {
    fn style(self) -> fn(&Theme) -> Style {
        match self {
            Self::Available => theme::pill::warning,
            Self::Today | Self::TwoDays => theme::pill::soft_warning,
            Self::Longer => theme::pill::simple,
        }
    }

    fn clock(self) -> widget::Text<'static> {
        match self {
            Self::Available => crate::icon::clock_fill_icon(),
            _ => crate::icon::clock_icon(),
        }
    }
}

pub fn coin_sequence<'a, T: 'a>(sequence: u32) -> Container<'a, T> {
    coin_sequence_pill(sequence, false)
}

pub fn coin_sequence_compact<'a, T: 'a>(sequence: u32) -> Container<'a, T> {
    coin_sequence_pill(sequence, true)
}

fn coin_sequence_pill<'a, T: 'a>(sequence: u32, compact: bool) -> Container<'a, T> {
    let eta = recovery_eta(sequence);
    let (label, tooltip, width): (String, String, PillWidth) = match eta {
        RecoveryEta::Available => (
            if compact {
                t!("pill-available-compact")
            } else {
                t!("pill-available")
            },
            t!("pill-recovery-available-tooltip"),
            PillWidth::M,
        ),
        RecoveryEta::Today => (
            t!("pill-today"),
            t!("pill-first-recovery-today"),
            PillWidth::M,
        ),
        RecoveryEta::TwoDays => (
            if compact {
                t!("duration-days-compact-approx", count = 2)
            } else {
                t!("duration-days-approx", count = 2)
            },
            t!(
                "pill-first-recovery-in",
                units = t!("duration-days-approx", count = 2)
            ),
            PillWidth::M,
        ),
        RecoveryEta::Longer if compact => {
            let units = expire_compact_units(sequence);
            (
                units.clone(),
                t!("pill-first-recovery-in", units = units),
                PillWidth::M,
            )
        }
        RecoveryEta::Longer => {
            let mut units = expire_message_units(sequence);
            if units.len() > 2 {
                units = units[0..1].to_vec();
            }
            let width = if units.len() > 1 {
                PillWidth::XL
            } else {
                PillWidth::M
            };
            let units = format!("~{}", units.join(", "));
            (
                units.clone(),
                t!("pill-first-recovery-in", units = units),
                width,
            )
        }
    };

    let icon_size = if compact { 14 } else { 18 };
    pill_with_icon(
        Some(eta.clock().size(icon_size)),
        &label,
        tooltip,
        width,
        eta.style(),
        compact,
    )
}

fn expire_units(sequence: u32) -> Vec<(u32, ExpireUnit)> {
    const HOUR: u32 = 60/*minutes*/;
    const DAY: u32 = 60/*minutes*/ * 24/* hours */; // 1440
    const YEAR: u32 = ((365/*days*/ * 4 + 1) * DAY) / 4; // 525960
    const MONTH: u32 = YEAR / 12/*months*/; // 43830
    let mut n_minutes = sequence * 10;
    let n_years = n_minutes / YEAR;
    n_minutes -= n_years * YEAR;
    let n_months = n_minutes / MONTH;
    n_minutes -= n_months * MONTH;
    let n_days = n_minutes / DAY;

    let units = if n_years != 0 || n_months != 0 || n_days != 0 {
        vec![
            (n_years, ExpireUnit::Year),
            (n_months, ExpireUnit::Month),
            (n_days, ExpireUnit::Day),
        ]
    } else {
        n_minutes -= n_days * DAY;
        let n_hours = n_minutes / HOUR;
        n_minutes -= n_hours * HOUR;
        vec![(n_hours, ExpireUnit::Hour), (n_minutes, ExpireUnit::Minute)]
    };
    units.into_iter().filter(|(n, _)| *n != 0).collect()
}

#[derive(Debug, Clone, Copy)]
enum ExpireUnit {
    Year,
    Month,
    Day,
    Hour,
    Minute,
}

impl ExpireUnit {
    fn display(self, count: u32) -> String {
        match self {
            Self::Year => t!("duration-years", count = count),
            Self::Month => t!("duration-months", count = count),
            Self::Day => t!("duration-days", count = count),
            Self::Hour => t!("duration-hours", count = count),
            Self::Minute => t!("duration-minutes", count = count),
        }
    }

    fn compact(self, count: u32) -> String {
        match self {
            Self::Year => t!("duration-years-compact", count = count),
            Self::Month => t!("duration-months-compact", count = count),
            Self::Day => t!("duration-days-compact", count = count),
            Self::Hour => t!("duration-hours-compact", count = count),
            Self::Minute => t!("duration-minutes-compact", count = count),
        }
    }
}

fn expire_message_units(sequence: u32) -> Vec<String> {
    expire_units(sequence)
        .into_iter()
        .map(|(n, u)| u.display(n))
        .collect()
}

fn expire_compact_units(sequence: u32) -> String {
    let parts: Vec<String> = expire_units(sequence)
        .into_iter()
        .map(|(n, u)| u.compact(n))
        .collect();
    format!("~{}", parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_expire_message_units() {
        let testcases = [
            (
                61,
                vec![
                    "\u{2068}10\u{2069} hours".to_string(),
                    "\u{2068}10\u{2069} minutes".to_string(),
                ],
            ),
            (1112, vec!["\u{2068}7\u{2069} days".to_string()]),
            (52600, vec!["1 year".to_string()]),
        ];

        for (seq, result) in testcases {
            assert_eq!(expire_message_units(seq), result);
        }
    }
}
