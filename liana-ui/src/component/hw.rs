use crate::{color, component::text, icon, image, theme, widget::*};
use iced::{
    alignment::Vertical,
    widget::{column, container, row, tooltip, Space},
    Alignment, Length,
};
use std::borrow::Cow;
use std::fmt::Display;

pub fn locked_hardware_wallet<'a, T: 'a, K: Display>(
    kind: K,
    pairing_code: Option<impl Into<Cow<'a, str>> + Display>,
) -> Container<'a, T> {
    Container::new(
        column(vec![
            Row::new()
                .spacing(5)
                .push(text::p1_bold(format!(
                    "Locked{}",
                    if pairing_code.is_some() {
                        ", check code:"
                    } else {
                        ""
                    }
                )))
                .push_maybe(pairing_code.map(|a| text::p1_bold(a)))
                .into(),
            Row::new()
                .spacing(5)
                .push(text::caption(kind.to_string()))
                .into(),
        ])
        .width(Length::Fill),
    )
    .padding(10)
}

pub fn supported_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
) -> Container<'a, T> {
    Container::new(
        column(vec![
            Row::new()
                .spacing(5)
                .push_maybe(alias.map(|a| text::p1_bold(a)))
                .push(text::p1_regular(format!("#{}", fingerprint)))
                .into(),
            Row::new()
                .spacing(5)
                .push(text::caption(kind.to_string()))
                .push_maybe(version.map(|v| text::caption(v.to_string())))
                .into(),
        ])
        .width(Length::Fill),
    )
    .padding(10)
}

pub fn warning_hardware_wallet<'a, T: 'static, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    warning: &'a str,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text::p1_bold(a)))
                    .push(text::p1_regular(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption(kind.to_string()))
                    .push_maybe(version.map(|v| text::caption(v.to_string())))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            column(vec![tooltip::Tooltip::new(
                icon::warning_icon(),
                iced::widget::text!("{}", warning),
                tooltip::Position::Bottom,
            )
            .style(theme::card::simple)
            .into()])
            .into(),
        ])
        .align_y(Alignment::Center),
    )
    .padding(10)
}

pub fn unimplemented_method_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    message: &'static str,
) -> Container<'a, T> {
    container(
        tooltip::Tooltip::new(
            container(
                column(vec![
                    text::p1_regular(format!("#{}", fingerprint)).into(),
                    Row::new()
                        .spacing(5)
                        .push(text::caption(kind.to_string()))
                        .push_maybe(version.map(|v| text::caption(v.to_string())))
                        .into(),
                ])
                .width(Length::Fill),
            )
            .width(Length::Fill)
            .padding(10),
            message,
            tooltip::Position::Bottom,
        )
        .style(theme::card::simple),
    )
    .width(Length::Fill)
}

pub fn unrelated_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
) -> Container<'a, T> {
    let key = column(vec![
        text::p1_regular(format!("#{}", fingerprint)).into(),
        Row::new()
            .spacing(5)
            .push(text::caption(kind.to_string()))
            .push_maybe(version.map(|v| text::caption(v.to_string())))
            .into(),
    ]);
    container(
        container(
            Row::new()
                .push(key)
                .push(Space::with_width(Length::Fill))
                .push(text::text(
                    "This device key is not part of the wallet descriptor.",
                ))
                .push(Space::with_width(Length::Fill))
                .align_y(Vertical::Center),
        )
        .width(Length::Fill)
        .padding(10)
        .style(theme::card::simple),
    )
    .width(Length::Fill)
}

pub fn processing_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text::p1_bold(a)))
                    .push(text::p1_regular(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption(kind.to_string()))
                    .push_maybe(version.map(|v| text::caption(v.to_string())))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            column(vec![
                text::p1_regular("Processing...").into(),
                text::p1_regular("Please check your device").into(),
            ])
            .into(),
        ])
        .align_y(Alignment::Center),
    )
    .padding(10)
}

pub fn selected_hardware_wallet<'a, T: 'static, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    warning: Option<&'static str>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(text::p1_bold))
                    .push(text::p1_regular(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption(kind.to_string()))
                    .push_maybe(version.map(|v| text::caption(v)))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            if let Some(w) = warning {
                tooltip::Tooltip::new(
                    icon::warning_icon(),
                    iced::widget::text!("{}", w),
                    tooltip::Position::Bottom,
                )
                .style(theme::card::simple)
                .into()
            } else {
                Row::new().into()
            },
            image::success_mark_icon().width(Length::Fixed(50.0)).into(),
        ])
        .align_y(Alignment::Center),
    )
    .padding(10)
}

pub fn sign_success_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text::p1_bold(a)))
                    .push(text::p1_regular(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption(kind.to_string()))
                    .push_maybe(version.map(|v| text::caption(v.to_string())))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            row(vec![
                text::p1_regular("Signed").color(color::GREEN).into(),
                image::success_mark_icon().width(Length::Fixed(50.0)).into(),
            ])
            .align_y(Alignment::Center)
            .spacing(5)
            .into(),
        ])
        .align_y(Alignment::Center),
    )
    .padding(10)
}

pub fn registration_success_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text::p1_bold(a)))
                    .push(text::p1_regular(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption(kind.to_string()))
                    .push_maybe(version.map(|v| text::caption(v.to_string())))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            row(vec![
                text::p1_regular("Registered").color(color::GREEN).into(),
                image::success_mark_icon().width(Length::Fixed(50.0)).into(),
            ])
            .align_y(Alignment::Center)
            .spacing(5)
            .into(),
        ])
        .align_y(Alignment::Center),
    )
    .padding(10)
}

pub fn wrong_network_hardware_wallet<'a, T: 'static, K: Display, V: Display>(
    kind: K,
    version: Option<V>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push(text::p1_bold("Wrong network in the device settings"))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption(kind.to_string()))
                    .push_maybe(version.map(|v| text::caption(v.to_string())))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            tooltip::Tooltip::new(
                icon::warning_icon(),
                "The wrong bitcoin application is open or the device was initialized with the wrong network",
                tooltip::Position::Bottom,
            )
            .style(theme::card::simple)
            .into(),
        ])
        .align_y(Alignment::Center),
    )
    .padding(10)
}

pub fn unsupported_hardware_wallet<'a, T: 'static, K: Display, V: Display>(
    kind: K,
    version: Option<V>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push(text::p1_bold("Connection error"))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption(kind.to_string()))
                    .push_maybe(version.map(|v| text::caption(v.to_string())))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            tooltip::Tooltip::new(
                icon::warning_icon(),
                "Make sure your device is unlocked and a supported Bitcoin application is opened.",
                tooltip::Position::Bottom,
            )
            .style(theme::card::simple)
            .into(),
        ])
        .align_y(Alignment::Center),
    )
    .padding(10)
}

pub fn unsupported_version_hardware_wallet<'a, T: 'static, K: Display, V: Display, S: Display>(
    kind: K,
    version: Option<V>,
    requested_version: S,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                text::p1_bold("Unsupported firmware version").into(),
                text::p1_regular(format!("Install version {} or later", requested_version)).into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption(kind.to_string()))
                    .push_maybe(version.map(|v| text::caption(v.to_string())))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            tooltip::Tooltip::new(
                icon::warning_icon(),
                "Please upgrade firmware",
                tooltip::Position::Bottom,
            )
            .style(theme::card::simple)
            .into(),
        ])
        .align_y(Alignment::Center),
    )
    .padding(10)
}

pub fn sign_success_hot_signer<'a, T: 'a, F: Display>(
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text::p1_bold(a)))
                    .push(text::p1_regular(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption("This computer"))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            row(vec![
                text::p1_regular("Signed").color(color::GREEN).into(),
                image::success_mark_icon().width(Length::Fixed(50.0)).into(),
            ])
            .align_y(Alignment::Center)
            .spacing(5)
            .into(),
        ])
        .align_y(Alignment::Center),
    )
    .padding(10)
}

pub fn selected_hot_signer<'a, T: 'a, F: Display>(
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text::p1_bold(a)))
                    .push(text::p1_regular(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption("This computer"))
                    .push(text::caption(
                        "(A derived key from a mnemonic stored locally)",
                    ))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            image::success_mark_icon().width(Length::Fixed(50.0)).into(),
        ])
        .align_y(Alignment::Center),
    )
    .padding(10)
}

pub fn unselected_hot_signer<'a, T: 'a, F: Display>(
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
) -> Container<'a, T> {
    Container::new(
        column(vec![
            Row::new()
                .spacing(5)
                .push_maybe(alias.map(|a| text::p1_bold(a)))
                .push(text::p1_regular(format!("#{}", fingerprint)))
                .into(),
            Row::new()
                .spacing(5)
                .push(text::caption("This computer"))
                .push(text::caption(
                    "(A derived key from a mnemonic stored locally)",
                ))
                .into(),
        ])
        .width(Length::Fill),
    )
    .padding(10)
}

pub fn hot_signer<'a, T: 'a, F: Display>(
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
) -> Container<'a, T> {
    Container::new(
        column(vec![
            Row::new()
                .spacing(5)
                .push_maybe(alias.map(|a| text::p1_bold(a)))
                .push(text::p1_regular(format!("#{}", fingerprint)))
                .into(),
            Row::new()
                .spacing(5)
                .push(text::caption("This computer"))
                .into(),
        ])
        .width(Length::Fill),
    )
    .padding(10)
}

pub fn selected_provider_key<'a, T: 'a, F: Display>(
    fingerprint: F,
    alias: impl Into<Cow<'a, str>> + Display,
    key_kind: impl Into<Cow<'a, str>> + Display,
    token: impl Into<Cow<'a, str>> + Display,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push(text::p1_bold(alias))
                    .push(text::p1_regular(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption(format!("{key_kind} ({token})")))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            image::success_mark_icon().width(Length::Fixed(50.0)).into(),
        ])
        .align_y(Alignment::Center),
    )
    .padding(10)
}

pub fn unselected_provider_key<'a, T: 'a, F: Display>(
    fingerprint: F,
    alias: impl Into<Cow<'a, str>> + Display,
    key_kind: impl Into<Cow<'a, str>> + Display,
    token: impl Into<Cow<'a, str>> + Display,
) -> Container<'a, T> {
    container(
        row(vec![column(vec![
            Row::new()
                .spacing(5)
                .push(text::p1_bold(alias))
                .push(text::p1_regular(format!("#{}", fingerprint)))
                .into(),
            Row::new()
                .spacing(5)
                .push(text::caption(format!("{key_kind} ({token})")))
                .into(),
        ])
        .width(Length::Fill)
        .into()])
        .align_y(Alignment::Center),
    )
    .padding(10)
}

pub fn unsaved_provider_key<'a, T: 'a, F: Display>(
    fingerprint: F,
    key_kind: impl Into<Cow<'a, str>> + Display,
    token: impl Into<Cow<'a, str>> + Display,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push(text::p1_regular(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption(format!("{key_kind} ({token})")))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            image::success_mark_icon().width(Length::Fixed(50.0)).into(), // it must be selected if unsaved
        ])
        .align_y(Alignment::Center),
    )
    .padding(10)
}
