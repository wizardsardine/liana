use std::fmt::Display;

use iced::{
    widget::{container::Style, row, tooltip, Space},
    Alignment, Font, Length,
};
use iced_core::text::Shaping;

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

fn tooltip_text<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    new::caption(content)
}

#[derive(Debug, Clone, Copy)]
#[repr(u16)]
pub enum PillWidth {
    S = 90,
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
    label: &'static str,
    tooltip: &'static str,
    width: PillWidth,
    style: fn(&Theme) -> Style,
) -> Container<'a, T> {
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
    let label = iced::widget::text!("{label}")
        .shaping(Shaping::Advanced)
        .font(font)
        .center()
        .size(size);
    let content = match (icon, compact) {
        (Some(icon), true) => row![icon, Space::with_width(6), label],
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
    pill_body_with_text_size_and_font_and_padding(
        label,
        width,
        style,
        font,
        PILL_FONT_SIZE,
        PILL_PADDING,
    )
}

fn compact_pill_body_with_font<'a, T: 'a, L: Display>(
    label: L,
    width: PillWidth,
    style: fn(&Theme) -> Style,
    font: Font,
) -> Container<'a, T> {
    pill_body_with_text_size_and_font_and_padding(
        label,
        width,
        style,
        font,
        PILL_FONT_SIZE_COMPACT,
        PILL_PADDING_COMPACT,
    )
}

fn pill_body_with_text_size_and_font<'a, T: 'a, L: Display>(
    label: L,
    width: PillWidth,
    style: fn(&Theme) -> Style,
    font: Font,
    size: u32,
) -> Container<'a, T> {
    pill_body_with_text_size_and_font_and_padding(label, width, style, font, size, PILL_PADDING)
}

fn compact_pill_body_with_text_size_and_font<'a, T: 'a, L: Display>(
    label: L,
    width: PillWidth,
    style: fn(&Theme) -> Style,
    font: Font,
    size: u32,
) -> Container<'a, T> {
    pill_body_with_text_size_and_font_and_padding(
        label,
        width,
        style,
        font,
        size,
        PILL_PADDING_COMPACT,
    )
}

fn pill_body_with_text_size_and_font_and_padding<'a, T: 'a, L: Display>(
    label: L,
    width: PillWidth,
    style: fn(&Theme) -> Style,
    font: Font,
    size: u32,
    padding: [u16; 2],
) -> Container<'a, T> {
    let item = iced::widget::text!("{label}")
        .shaping(Shaping::Advanced)
        .font(font)
        .center()
        .size(size);
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
                pill($label, $tooltip, PillWidth::$width, theme::pill::$style)
            }
        )*
    };
}

#[rustfmt::skip]
pills! {
    recovery,       "Recovery",     "This transaction is using a recovery path",                      M, simple;
    batch,          "Batch",        "This transaction contains multiple payments",                    M, simple;
    deprecated,     "Deprecated",   "This transaction cannot be included in the blockchain anymore.", M, simple;
    spent,          "Spent",        "The transaction was included in the blockchain.",                M, simple;
    unsigned,       "Unsigned",     "This transaction is missing signature(s)",                       M, soft_warning;
    signed,         "To broadcast", "This transaction is signed & ready to broadcast",                M, soft_warning;
    unconfirmed,    "Unconfirmed",  "Do not treat this as a payment until it is confirmed",           M, simple_fill;
    confirmed,      "Confirmed",    "This transaction has been included in a block",                  M, success;
    key_internal,   "Internal",     "Key held by your organization",                                  M, internal;
    // Business installer only
    key_external,   "External",     "key held by third parties",                                      M, external;
    key_cosigner,   "Cosigner",     "Professional third party co-signing key",                        M, safety_net;
    key_safety_net, "Safety Net",   "Professional third party recovery key",                          M, safety_net;
}

/// Wallet lifecycle status pills, compact (the wallet list trailing).
pub fn register<'a, T: 'a>() -> Container<'a, T> {
    compact_pill("Register", PillWidth::M, theme::pill::warning)
}

pub fn draft<'a, T: 'a>() -> Container<'a, T> {
    compact_pill("Draft", PillWidth::M, theme::pill::simple)
}

pub fn to_approve<'a, T: 'a>() -> Container<'a, T> {
    compact_pill("To approve", PillWidth::M, theme::pill::warning)
}

pub fn set_keys<'a, T: 'a>() -> Container<'a, T> {
    compact_pill("Set keys", PillWidth::M, theme::pill::warning)
}

pub fn active<'a, T: 'a>() -> Container<'a, T> {
    compact_pill("Active", PillWidth::M, theme::pill::success)
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
        "Always available",
        "",
        PillWidth::ML,
        theme::pill::success,
        true,
    )
}

pub fn registered<'a, T: 'a>() -> Container<'a, T> {
    pill_with_icon(
        Some(crate::icon::check_icon().style(theme::text::success)),
        "Registered",
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
    text: &'a str,
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
    compact_metric("Manager", theme::pill::role_manager)
}

pub fn role_participant<'a, T: 'a>() -> Container<'a, T> {
    compact_metric("Participant", theme::pill::role_participant)
}

pub fn ws_admin<'a, T: 'a>() -> Container<'a, T> {
    compact_metric("WS Admin", theme::pill::simple)
}

/// Success pill shown for a key that is used in the template: a checkmark only, with a hover tooltip
/// stating how many spending paths use the key.
pub fn in_policy<'a, T: 'a>(usage_count: usize) -> Container<'a, T> {
    let pill = Container::new(crate::icon::check_icon())
        .padding(PILL_PADDING_COMPACT)
        .style(theme::pill::success);
    let tip = format!(
        "Used in {usage_count} spending path{}",
        if usage_count == 1 { "" } else { "s" }
    );
    pill_with_tooltip(pill, Some(tip))
}

/// Light-warning pill shown for a key that is not used in any spending path yet.
pub fn not_in_policy<'a, T: 'a>() -> Container<'a, T> {
    pill_with_icon(
        Some(crate::icon::tooltip_icon()),
        "Not in policy yet",
        "",
        PillWidth::Shrink,
        theme::pill::soft_warning,
        true,
    )
}

pub fn xpub_set<'a, T: 'a>() -> Container<'a, T> {
    compact_pill("✓ Set", PillWidth::S, theme::pill::success)
}

pub fn xpub_not_set<'a, T: 'a>() -> Container<'a, T> {
    compact_pill("Not Set", PillWidth::S, theme::pill::warning)
}

pub fn unconfirmed_compact<'a, T: 'a>() -> Container<'a, T> {
    compact_pill_body_with_text_size_and_font(
        "Unconfirmed",
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
    let label = format!("Rescan… {:.2}%", progress * 100.0);
    if compact {
        compact_pill_body_with_text_size_and_font(label, width, theme::pill::simple, font, size)
    } else {
        pill_body_with_text_size_and_font(label, width, theme::pill::simple, font, size)
    }
}

pub fn fingerprint<'a, T: 'a>(fg: impl Into<String>, alias: Option<&str>) -> Container<'a, T> {
    let fg = fg.into();
    match alias {
        Some(alias) => {
            let body = compact_pill_body_with_font(
                alias.to_string(),
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
    let caption = "First recovery option available ";
    let eta = recovery_eta(sequence);
    let (label, tooltip, width): (String, String, PillWidth) = match eta {
        RecoveryEta::Available => (
            if compact { "Avail." } else { "Available" }.to_string(),
            "Recovery option(s) already available".to_string(),
            PillWidth::M,
        ),
        RecoveryEta::Today => ("Today".to_string(), format!("{caption}today"), PillWidth::M),
        RecoveryEta::TwoDays => (
            if compact { "~2d" } else { "~2 days" }.to_string(),
            format!("{caption}in ~2 days"),
            PillWidth::M,
        ),
        RecoveryEta::Longer if compact => {
            let units = expire_compact_units(sequence);
            (units.clone(), format!("{caption}in {units}"), PillWidth::M)
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
            (units.clone(), format!("{caption}in {units}"), width)
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
    fn name(self) -> &'static str {
        match self {
            Self::Year => "year",
            Self::Month => "month",
            Self::Day => "day",
            Self::Hour => "hour",
            Self::Minute => "minute",
        }
    }

    fn abbr(self) -> &'static str {
        match self {
            Self::Year => "y",
            Self::Month => "m",
            Self::Day => "d",
            Self::Hour => "h",
            Self::Minute => "min",
        }
    }
}

fn expire_message_units(sequence: u32) -> Vec<String> {
    expire_units(sequence)
        .into_iter()
        .map(|(n, u)| format!("{} {}{}", n, u.name(), if n > 1 { "s" } else { "" }))
        .collect()
}

fn expire_compact_units(sequence: u32) -> String {
    let parts: Vec<String> = expire_units(sequence)
        .into_iter()
        .map(|(n, u)| format!("{}{}", n, u.abbr()))
        .collect();
    format!("~{}", parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_expire_message_units() {
        let testcases = [
            (61, vec!["10 hours".to_string(), "10 minutes".to_string()]),
            (1112, vec!["7 days".to_string()]),
            (52600, vec!["1 year".to_string()]),
        ];

        for (seq, result) in testcases {
            assert_eq!(expire_message_units(seq), result);
        }
    }
}
