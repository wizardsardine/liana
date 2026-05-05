use crate::{
    color,
    component::text::{self, p1_regular},
    icon, image, theme,
    widget::*,
};
use bitcoin::bip32::{ChildNumber, Fingerprint};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{column, container, row, tooltip, Space},
    Alignment, Length,
};
use std::{borrow::Cow, fmt::Display};

const PADDING: u16 = 10;

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
    .padding(PADDING)
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
                .push(text::p1_regular(format!("#{fingerprint}")))
                .into(),
            Row::new()
                .spacing(5)
                .push(text::caption(kind.to_string()))
                .push_maybe(version.map(|v| text::caption(v.to_string())))
                .into(),
        ])
        .width(Length::Fill),
    )
    .padding(PADDING)
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
        write!(f, "Account #{index}")
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
    let pick_account = super::pick_list::pick_list(accounts, account.clone(), |a| {
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
            .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
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
                    .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
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
                    text::p1_regular(format!("#{fingerprint}")).into(),
                    Row::new()
                        .spacing(5)
                        .push(text::caption(kind.to_string()))
                        .push_maybe(version.map(|v| text::caption(v.to_string())))
                        .into(),
                ])
                .width(Length::Fill),
            )
            .width(Length::Fill)
            .padding(PADDING),
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
        text::p1_regular(format!("#{fingerprint}")).into(),
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
        .padding(PADDING)
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

pub fn show_qr_code<'a, M: 'a + 'static>(
    tt: Option<&'static str>,
    msg: Option<M>,
) -> Button<'a, M> {
    let mut btn = Button::new(
        Row::new()
            .push(icon::qr_icon().size(30))
            .push(p1_regular("Show QR Code"))
            .push_maybe(tt.map(super::tooltip))
            .spacing(20)
            .align_y(Alignment::Center)
            .padding(PADDING + 5),
    )
    .style(theme::button::secondary)
    .width(Length::Fill);
    if let Some(msg) = msg {
        btn = btn.on_press(msg);
    }
    btn
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
                    .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
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
            .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
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
                    .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
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
                    .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
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
    .padding(PADDING)
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
    .padding(PADDING)
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
                text::p1_regular(format!("Install version {requested_version} or later")).into(),
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
    .padding(PADDING)
}

pub fn taproot_not_supported_device<'a, T: 'static, K: Display>(kind: K) -> Container<'a, T> {
    container(
        row(vec![column(vec![
            text::p1_bold("This device doesn't support taproot miniscript").into(),
            text::caption(kind.to_string()).into(),
        ])
        .width(Length::Fill)
        .into()])
        .align_y(Alignment::Center),
    )
    .padding(PADDING)
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
                    .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
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
                    .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
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
                .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
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
                    .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
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
                    .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
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
                .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
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
                    .push(text::p1_regular(format!("#{fingerprint}")))
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
    .padding(PADDING)
}

pub fn modal_no_devices_placeholder<'a, M: 'a>() -> Element<'a, M> {
    column![
        icon::usb_icon().size(100),
        p1_regular("Plug in a hardware device ...")
    ]
    .align_x(Horizontal::Center)
    .spacing(20)
    .into()
}
