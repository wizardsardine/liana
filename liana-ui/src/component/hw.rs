use crate::{color, component::text, icon, image, theme, widget::*};
use bitcoin::bip32::{ChildNumber, Fingerprint};
use iced::{
    alignment::Vertical,
    widget::{column, container, pick_list, row, tooltip, Space},
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

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub index: ChildNumber,
    pub fingerprint: Fingerprint,
}

impl Account {
    pub fn new(index: ChildNumber, fingerprint: Fingerprint) -> Self {
        Self { index, fingerprint }
    }
}

impl Display for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let index = self.index.to_string();
        let index = index.replace("'", "");
        write!(f, "Account #{}", index)
    }
}

pub fn supported_hardware_wallet_with_account<
    'a,
    M: 'static + From<(Fingerprint, ChildNumber)> + Clone,
    K: Display,
    V: Display,
>(
    kind: K,
    version: Option<V>,
    fingerprint: Fingerprint,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    account: Option<ChildNumber>,
    edit_account: bool,
) -> Container<'a, M> {
    let accounts: Vec<_> = (0..10)
        .map(|i| {
            Account::new(
                ChildNumber::from_hardened_idx(i).expect("hardcoded"),
                fingerprint,
            )
        })
        .collect();
    let account = Some(account.unwrap_or(ChildNumber::Hardened { index: 0 }));
    let account = account.map(|i| Account::new(i, fingerprint));
    let pick_account = pick_list(accounts, account.clone(), |a| {
        (a.fingerprint, a.index).into()
    });
    let pick_account = if edit_account {
        Some(pick_account)
    } else {
        None
    };
    let display_account = account.and_then(|a| {
        if !edit_account {
            Some(text::p1_bold(a))
        } else {
            None
        }
    });
    let key = column(vec![
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
    .width(Length::Fill);
    Container::new(
        Row::new()
            .push(key)
            .push(Space::with_width(Length::Fill))
            .push_maybe(pick_account)
            .push_maybe(display_account.map(|a| column![Space::with_height(8), a])),
    )
    .align_y(Alignment::Center)
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

pub fn disabled_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    label: &'static str,
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
                .push(Space::with_width(15))
                .push(Space::with_width(Length::Fill))
                .push(text::text(label))
                .push(Space::with_width(Length::Fill))
                .align_y(Vertical::Center),
        )
        .width(Length::Fill)
        .padding(10)
        .style(theme::card::simple),
    )
    .width(Length::Fill)
}

pub fn unrelated_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
) -> Container<'a, T> {
    disabled_hardware_wallet(
        kind,
        version,
        fingerprint,
        "This signing device is not related to this Liana wallet.",
    )
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
    account: Option<ChildNumber>,
    display_account: bool,
) -> Container<'a, T> {
    let account = account.unwrap_or(ChildNumber::from_hardened_idx(0).expect("hardcoded"));
    let index = match account {
        ChildNumber::Hardened { index } => index,
        ChildNumber::Normal { .. } => unreachable!(),
    };
    let account = if display_account {
        Some(format!("Account #{index}"))
    } else {
        None
    };

    let key = column(vec![
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
    ]);
    container(
        Row::new()
            .push(key)
            .push(Space::with_width(Length::Fill))
            .push_maybe(account.map(|a| column![Space::with_height(8), text::p1_bold(a)]))
            .push(Space::with_width(10))
            .push_maybe(warning.map(|w| {
                tooltip::Tooltip::new(
                    icon::warning_icon(),
                    iced::widget::text!("{}", w),
                    tooltip::Position::Bottom,
                )
                .style(theme::card::simple)
            }))
            .push(image::success_mark_icon().width(Length::Fixed(50.0))),
    )
    .padding(10)
    .align_y(Alignment::Center)
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
    can_sign: bool,
) -> Container<'a, T> {
    Container::new(
        Row::new()
            .push(column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text::p1_bold(a)))
                    .push(text::p1_regular(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption("This computer"))
                    .into(),
            ]))
            .push(Space::with_width(Length::Fixed(20.0)))
            .push_maybe(if !can_sign {
                Some(text::text(
                    "This hot signer is not part of this spending path.",
                ))
            } else {
                None
            })
            .push(Space::with_width(Length::Fill))
            .align_y(Vertical::Center),
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
