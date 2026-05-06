use iced::{
    widget::{container::Style, tooltip},
    Alignment, Font, Length,
};
use iced_core::text::Shaping;

use crate::{
    component::text,
    font, icon,
    theme::{self, Theme},
    widget::*,
};

use super::text::H5_SIZE;

const PILL_PADDING: [u16; 2] = [6, 20];

#[derive(Debug, Clone, Copy)]
#[repr(u16)]
pub enum PillWidth {
    S = 90,
    M = 150,
    L = 220,
    Auto,
}

impl From<PillWidth> for Length {
    fn from(value: PillWidth) -> Self {
        if matches!(value, PillWidth::Auto) {
            return Length::Shrink;
        }
        Length::Fixed(value as u16 as f32)
    }
}

pub fn pill<'a, T: 'a>(
    label: &'a str,
    tooltip: &'a str,
    width: PillWidth,
    style: fn(&Theme) -> Style,
) -> Container<'a, T> {
    let pill = Container::new(text::h5_medium(label))
        .padding(PILL_PADDING)
        .center_x(width)
        .style(style);
    if !tooltip.is_empty() {
        Container::new({
            tooltip::Tooltip::new(
                pill,
                Container::new(text::p1_regular(tooltip))
                    .padding(PILL_PADDING)
                    .style(theme::card::simple),
                tooltip::Position::Top,
            )
        })
    } else {
        pill
    }
}

fn pill_body<'a, T: 'a>(
    label: String,
    width: PillWidth,
    style: fn(&Theme) -> Style,
) -> Container<'a, T> {
    pill_body_with_font(label, width, style, font::MEDIUM)
}

fn pill_body_with_font<'a, T: 'a>(
    label: String,
    width: PillWidth,
    style: fn(&Theme) -> Style,
    font: Font,
) -> Container<'a, T> {
    let txt = iced::widget::text!("{label}")
        .shaping(Shaping::Advanced)
        .font(font)
        .size(H5_SIZE);
    Container::new(txt)
        .padding(PILL_PADDING)
        .center_x(width)
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
    batch,          "Batch",        "This transaction contains multiple payments",                    S, simple;
    deprecated,     "Deprecated",   "This transaction cannot be included in the blockchain anymore.", M, simple;
    spent,          "Spent",        "The transaction was included in the blockchain.",                M, simple;
    unsigned,       "Unsigned",     "This transaction is missing signature(s)",                       M, soft_warning;
    signed,         "To broadcast", "This transaction is signed & ready to broadcast",                M, soft_warning;
    unconfirmed,    "Unconfirmed",  "Do not treat this as a payment until it is confirmed",           M, warning;
    confirmed,      "Confirmed",    "This transaction has been included in a block",                  M, success;
    key_internal,   "Internal",     "Key held by your organization",                                  M, internal;
    // Business installer only
    key_external,   "External",     "key held by third parties",                                      M, external;
    key_cosigner,   "Cosigner",     "Professional third party co-signing key",                        M, safety_net;
    key_safety_net, "Safety Net",   "Professional third party recovery key",                          M, safety_net;
    to_approve,     "To approve",   "",                                                               M, warning;
    draft,          "Draft",        "",                                                               M, simple;
    set_keys,       "Set keys",     "",                                                               M, warning;
    active,         "Active",       "",                                                               M, success;
    ws_admin,       "WS Admin",     "",                                                               M, simple;
    register,       "Register",     "",                                                               M, warning;
    xpub_set,       "✓ Set",        "",                                                               M, success;
    xpub_not_set,   "Not Set",      "",                                                               M, warning;
}

pub fn rescan<'a, T: 'a>(progress: f64) -> Container<'a, T> {
    pill_body(
        format!("Rescan…{:.2}%", progress * 100.0),
        PillWidth::L,
        theme::pill::simple,
    )
}

pub fn fingerprint<'a, T: 'a>(fg: impl Into<String>, alias: Option<&str>) -> Container<'a, T> {
    let fg = fg.into();
    let font = font::REGULAR;
    let height = 32;
    let padding = [0, 10];
    match alias {
        Some(alias) => {
            let body = pill_body_with_font(
                alias.to_string(),
                PillWidth::Auto,
                theme::pill::fingerprint,
                font,
            )
            .padding(padding)
            .center_y(height);
            Container::new(tooltip::Tooltip::new(
                body,
                Container::new(text::h5_regular(fg))
                    .padding(padding)
                    .style(theme::card::simple),
                tooltip::Position::Top,
            ))
            .center_y(height)
        }
        None => pill_body_with_font(fg, PillWidth::M, theme::pill::fingerprint, font)
            .padding(padding)
            .center_y(height),
    }
    .center_y(height)
}

pub fn coin_sequence<'a, T: 'a>(seq: u32, timelock: u32) -> Container<'a, T> {
    let (label, style, secondary): (String, fn(&Theme) -> Style, bool) = if seq == 0 {
        ("Expired".to_string(), theme::pill::warning, false)
    } else if seq < timelock * 10 / 100 {
        (expire_message(seq), theme::pill::simple, false)
    } else {
        (expire_message(seq), theme::pill::simple, true)
    };

    let mut label_text = text::h5_medium(label);
    if secondary {
        label_text = label_text.style(theme::text::secondary);
    }

    Container::new(
        Row::new()
            .spacing(5)
            .push(icon::clock_icon().width(Length::Fixed(25.0)))
            .push(label_text)
            .align_y(Alignment::Center),
    )
    .padding(10)
    .style(style)
}

fn expire_message(sequence: u32) -> String {
    if sequence <= 144 {
        "Expires today".to_string()
    } else if sequence <= 2 * 144 {
        "Expires in ≈ 2 days".to_string()
    } else {
        format!("Expires in {}", expire_message_units(sequence).join(","))
    }
}

/// returns y,m,d
fn expire_message_units(sequence: u32) -> Vec<String> {
    let mut n_minutes = sequence * 10;
    let n_years = n_minutes / 525960;
    n_minutes -= n_years * 525960;
    let n_months = n_minutes / 43830;
    n_minutes -= n_months * 43830;
    let n_days = n_minutes / 1440;

    #[allow(clippy::nonminimal_bool)]
    if n_years != 0 || n_months != 0 || n_days != 0 {
        [(n_years, "year"), (n_months, "month"), (n_days, "day")]
            .iter()
            .filter_map(|(n, u)| {
                if *n != 0 {
                    Some(format!("{} {}{}", n, u, if *n > 1 { "s" } else { "" }))
                } else {
                    None
                }
            })
            .collect()
    } else {
        n_minutes -= n_days * 1440;
        let n_hours = n_minutes / 60;
        n_minutes -= n_hours * 60;
        [(n_hours, "hour"), (n_minutes, "minute")]
            .iter()
            .filter_map(|(n, u)| {
                if *n != 0 {
                    Some(format!("{} {}{}", n, u, if *n > 1 { "s" } else { "" }))
                } else {
                    None
                }
            })
            .collect()
    }
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
