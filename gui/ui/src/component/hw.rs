use crate::{color, component::text::*, icon, theme, util::*, widget::*};
use iced::{
    widget::{column, container, row, tooltip},
    Alignment, Length,
};
use std::borrow::Cow;
use std::fmt::Display;

pub fn supported_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>>>,
) -> Container<'a, T> {
    Container::new(
        column(vec![
            Row::new()
                .spacing(5)
                .push_maybe(alias.map(|a| text(a).bold()))
                .push(text(format!("#{}", fingerprint)))
                .into(),
            Row::new()
                .spacing(5)
                .push(text(kind.to_string()).small())
                .push_maybe(version.map(|v| text(v.to_string()).small()))
                .into(),
        ])
        .width(Length::Fill),
    )
    .padding(10)
}

pub fn unregistered_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>>>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text(a).bold()))
                    .push(text(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text(kind.to_string()).small())
                    .push_maybe(version.map(|v| text(v.to_string()).small()))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            column(vec![tooltip::Tooltip::new(
                icon::warning_icon(),
                "The wallet descriptor is not registered on the device.\n You can register it in the settings.",
                tooltip::Position::Bottom,
            )
            .style(theme::Container::Card(theme::Card::Simple))
            .into()])
            .into(),
        ])
        .align_items(Alignment::Center),
    )
    .padding(10)
}

pub fn processing_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>>>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text(a).bold()))
                    .push(text(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text(kind.to_string()).small())
                    .push_maybe(version.map(|v| text(v.to_string()).small()))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            column(vec![
                text("Processing...").into(),
                text("Please check your device").small().into(),
            ])
            .into(),
        ])
        .align_items(Alignment::Center),
    )
    .padding(10)
}

pub fn selected_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>>>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text(a).bold()))
                    .push(text(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text(kind.to_string()).small())
                    .push_maybe(version.map(|v| text(v.to_string()).small()))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            icon::circle_check_icon()
                .style(color::legacy::SUCCESS)
                .into(),
        ])
        .align_items(Alignment::Center),
    )
    .padding(10)
}

pub fn sign_success_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>>>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text(a).bold()))
                    .push(text(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text(kind.to_string()).small())
                    .push_maybe(version.map(|v| text(v.to_string()).small()))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            row(vec![
                icon::circle_check_icon()
                    .style(color::legacy::SUCCESS)
                    .into(),
                text("Signed").style(color::legacy::SUCCESS).into(),
            ])
            .align_items(Alignment::Center)
            .spacing(5)
            .into(),
        ])
        .align_items(Alignment::Center),
    )
    .padding(10)
}

pub fn registration_success_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>>>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text(a).bold()))
                    .push(text(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text(kind.to_string()).small())
                    .push_maybe(version.map(|v| text(v.to_string()).small()))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            row(vec![
                icon::circle_check_icon()
                    .style(color::legacy::SUCCESS)
                    .into(),
                text("Registered").style(color::legacy::SUCCESS).into(),
            ])
            .align_items(Alignment::Center)
            .spacing(5)
            .into(),
        ])
        .align_items(Alignment::Center),
    )
    .padding(10)
}

pub fn unsupported_hardware_wallet<'a, T: 'a, K: Display, V: Display>(
    kind: K,
    version: Option<V>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push(text("Connection error").bold())
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text(kind.to_string()).small())
                    .push_maybe(version.map(|v| text(v.to_string()).small()))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            tooltip::Tooltip::new(
                icon::warning_icon(),
                "Make sure your device is unlocked and a supported Bitcoin application is opened.",
                tooltip::Position::Bottom,
            )
            .style(theme::Container::Card(theme::Card::Simple))
            .into(),
        ])
        .align_items(Alignment::Center),
    )
    .padding(10)
}

pub fn sign_success_hot_signer<'a, T: 'a, F: Display>(
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>>>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text(a).bold()))
                    .push(text(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text("This computer").small())
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            row(vec![
                icon::circle_check_icon()
                    .style(color::legacy::SUCCESS)
                    .into(),
                text("Signed").style(color::legacy::SUCCESS).into(),
            ])
            .align_items(Alignment::Center)
            .spacing(5)
            .into(),
        ])
        .align_items(Alignment::Center),
    )
    .padding(10)
}

pub fn selected_hot_signer<'a, T: 'a, F: Display>(
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>>>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text(a).bold()))
                    .push(text(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text("This computer").small())
                    .push(text("(A derived key from a mnemonic stored locally)").small())
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            icon::circle_check_icon()
                .style(color::legacy::SUCCESS)
                .into(),
        ])
        .align_items(Alignment::Center),
    )
    .padding(10)
}

pub fn unselected_hot_signer<'a, T: 'a, F: Display>(
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>>>,
) -> Container<'a, T> {
    Container::new(
        column(vec![
            Row::new()
                .spacing(5)
                .push_maybe(alias.map(|a| text(a).bold()))
                .push(text(format!("#{}", fingerprint)))
                .into(),
            Row::new()
                .spacing(5)
                .push(text("This computer").small())
                .push(text("(A derived key from a mnemonic stored locally)").small())
                .into(),
        ])
        .width(Length::Fill),
    )
    .padding(10)
}

pub fn hot_signer<'a, T: 'a, F: Display>(
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>>>,
) -> Container<'a, T> {
    Container::new(
        column(vec![
            Row::new()
                .spacing(5)
                .push_maybe(alias.map(|a| text(a).bold()))
                .push(text(format!("#{}", fingerprint)))
                .into(),
            Row::new()
                .spacing(5)
                .push(text("This computer").small())
                .into(),
        ])
        .width(Length::Fill),
    )
    .padding(10)
}
