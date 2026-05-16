use crate::{
    component::{self, button, text},
    icon,
    theme::{self},
    widget::*,
};
use bitcoin::bip32::{ChildNumber, Fingerprint};
use iced::{
    alignment::Vertical,
    widget::{column, tooltip, Space},
    Alignment, Length,
};
use std::{borrow::Cow, fmt::Display};

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

/// Two-row identity for a hardware device: "alias  #fingerprint" + "kind version".
fn device_identity<'a, T: 'a, K: Display, V: Display, F: Display>(
    alias: Option<String>,
    fingerprint: F,
    kind: K,
    version: Option<V>,
) -> Column<'a, T> {
    column(vec![
        Row::new()
            .spacing(5)
            .push_maybe(alias.map(text::p1_bold))
            .push(text::p1_regular(format!("#{fingerprint}")))
            .into(),
        Row::new()
            .spacing(5)
            .push(text::caption(kind.to_string()))
            .push_maybe(version.map(|v| text::caption(v.to_string())))
            .into(),
    ])
    .width(Length::Fill)
}

/// Two-row identity for a hot signer: "alias  #fingerprint" + "This computer".
fn hot_identity<'a, T: 'a, F: Display>(
    alias: Option<String>,
    fingerprint: F,
    detail: Option<&'static str>,
) -> Column<'a, T> {
    column(vec![
        Row::new()
            .spacing(5)
            .push_maybe(alias.map(text::p1_bold))
            .push(text::p1_regular(format!("#{fingerprint}")))
            .into(),
        Row::new()
            .spacing(5)
            .push(text::caption("This computer"))
            .push_maybe(detail.map(text::caption))
            .into(),
    ])
    .width(Length::Fill)
}

/// Two-row identity for a provider key: "alias  #fingerprint" + "{key_kind} ({token})".
fn provider_identity<'a, T: 'a, F: Display>(
    fingerprint: F,
    alias: Option<String>,
    key_kind: String,
    token: String,
) -> Column<'a, T> {
    column(vec![
        Row::new()
            .spacing(5)
            .push_maybe(alias.map(text::p1_bold))
            .push(text::p1_regular(format!("#{fingerprint}")))
            .into(),
        Row::new()
            .spacing(5)
            .push(text::caption(format!("{key_kind} ({token})")))
            .into(),
    ])
    .width(Length::Fill)
}

/// Trailing success block: optional GREEN label followed by the 50px success-mark icon.
fn success_marker<T: 'static>(label: Option<&'static str>) -> Row<'static, T> {
    let mut r = Row::new().align_y(Alignment::Center).spacing(5);
    if let Some(l) = label {
        r = r.push(text::p1_regular(l));
    }
    r.push(component::badge::success())
}

/// Trailing warning icon with a bottom tooltip styled by `theme::card::simple`.
fn warning_tooltip<'a, M: 'static + Clone>(message: &'a str) -> Element<'a, M> {
    tooltip::Tooltip::new(
        icon::warning_icon(),
        iced::widget::text!("{}", message),
        tooltip::Position::Bottom,
    )
    .style(theme::card::simple)
    .into()
}

pub fn locked_device<'a, T: 'a + Clone, K: Display>(
    kind: K,
    pairing_code: Option<impl Into<Cow<'a, str>> + Display>,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = column(vec![
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
            .push_maybe(pairing_code.map(text::p1_bold))
            .into(),
        Row::new()
            .spacing(5)
            .push(text::caption(kind.to_string()))
            .into(),
    ])
    .width(Length::Fill);
    button::device(content, msg)
}

pub fn supported_device<'a, T: 'a + Clone, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = device_identity(alias.map(|a| a.to_string()), fingerprint, kind, version);
    button::device(content, msg)
}

pub fn supported_device_with_account<
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
    msg: Option<M>,
) -> Element<'a, M> {
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
    let pick_account = crate::component::pick_list::pick_list(accounts, account.clone(), |a| {
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
    let key = device_identity(alias.map(|a| a.to_string()), fingerprint, kind, version);
    let content = Row::new()
        .push(key)
        .push(Space::with_width(Length::Fill))
        .push_maybe(pick_account)
        .push_maybe(display_account.map(|a| column![Space::with_height(8), a]))
        .align_y(Alignment::Center);
    button::device(content, msg)
}

pub fn warning_device<'a, T: 'static + Clone, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    warning: &'a str,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(device_identity(
            alias.map(|a| a.to_string()),
            fingerprint,
            kind,
            version,
        ))
        .push(warning_tooltip(warning))
        .align_y(Alignment::Center);
    button::device(content, msg)
}

pub fn unimplemented_method_device<'a, T: 'a + Clone, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    message: &'static str,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = tooltip::Tooltip::new(
        device_identity::<T, _, _, _>(None, fingerprint, kind, version),
        message,
        tooltip::Position::Bottom,
    )
    .style(theme::card::simple);
    button::device(content, msg)
}

pub fn disabled_device<'a, T: 'a + Clone, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    label: &'static str,
    msg: Option<T>,
) -> Element<'a, T> {
    let key = device_identity::<T, _, _, _>(None, fingerprint, kind, version);
    let content = Row::new()
        .push(key)
        .push(Space::with_width(15))
        .push(Space::with_width(Length::Fill))
        .push(text::text(label))
        .push(Space::with_width(Length::Fill))
        .align_y(Vertical::Center);
    button::device(content, msg)
}

pub fn unrelated_device<'a, T: 'a + Clone, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    msg: Option<T>,
) -> Element<'a, T> {
    disabled_device(
        kind,
        version,
        fingerprint,
        "This signing device is not related to this Liana wallet.",
        msg,
    )
}

pub fn processing_device<'a, T: 'a + Clone, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(device_identity(
            alias.map(|a| a.to_string()),
            fingerprint,
            kind,
            version,
        ))
        .push(column(vec![
            text::p1_regular("Processing...").into(),
            text::p1_regular("Please check your device").into(),
        ]))
        .align_y(Alignment::Center);
    button::device(content, msg)
}

#[allow(clippy::too_many_arguments)]
pub fn selected_device<'a, T: 'static + Clone, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    warning: Option<&'static str>,
    account: Option<ChildNumber>,
    display_account: bool,
    msg: Option<T>,
) -> Element<'a, T> {
    let account = account.unwrap_or(ChildNumber::from_hardened_idx(0).expect("hardcoded"));
    let index = match account {
        ChildNumber::Hardened { index } => index,
        ChildNumber::Normal { .. } => unreachable!(),
    };
    let account_label = if display_account {
        Some(format!("Account #{index}"))
    } else {
        None
    };
    let content = Row::new()
        .push(device_identity(
            alias.map(|a| a.to_string()),
            fingerprint,
            kind,
            version,
        ))
        .push(Space::with_width(Length::Fill))
        .push_maybe(account_label.map(|a| column![Space::with_height(8), text::p1_bold(a)]))
        .push(Space::with_width(10))
        .push_maybe(warning.map(warning_tooltip))
        .push(success_marker::<T>(None))
        .align_y(Alignment::Center);
    button::device(content, msg)
}

pub fn signed_device<'a, T: 'a + Clone + 'static, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(device_identity(
            alias.map(|a| a.to_string()),
            fingerprint,
            kind,
            version,
        ))
        .push(success_marker(Some("Signed")))
        .align_y(Alignment::Center);
    button::device(content, msg)
}

pub fn registered_device<'a, T: 'a + Clone + 'static, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(device_identity(
            alias.map(|a| a.to_string()),
            fingerprint,
            kind,
            version,
        ))
        .push(success_marker(Some("Registered")))
        .align_y(Alignment::Center);
    button::device(content, msg)
}

pub fn wrong_network_device<'a, T: 'static + Clone, K: Display, V: Display>(
    kind: K,
    version: Option<V>,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(
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
            .width(Length::Fill),
        )
        .push(warning_tooltip(
            "The wrong bitcoin application is open or the device was initialized with the wrong network",
        ))
        .align_y(Alignment::Center);
    button::device(content, msg)
}

pub fn unsupported_device<'a, T: 'static + Clone, K: Display, V: Display>(
    kind: K,
    version: Option<V>,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(
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
            .width(Length::Fill),
        )
        .push(warning_tooltip(
            "Make sure your device is unlocked and a supported Bitcoin application is opened.",
        ))
        .align_y(Alignment::Center);
    button::device(content, msg)
}

pub fn unsupported_version_device<'a, T: 'static + Clone, K: Display, V: Display, S: Display>(
    kind: K,
    version: Option<V>,
    requested_version: S,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(
            column(vec![
                text::p1_bold("Unsupported firmware version").into(),
                text::p1_regular(format!("Install version {requested_version} or later")).into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption(kind.to_string()))
                    .push_maybe(version.map(|v| text::caption(v.to_string())))
                    .into(),
            ])
            .width(Length::Fill),
        )
        .push(warning_tooltip("Please upgrade firmware"))
        .align_y(Alignment::Center);
    button::device(content, msg)
}

pub fn taproot_unsupported_device<'a, T: 'static + Clone, K: Display>(
    kind: K,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(
            column(vec![
                text::p1_bold("This device doesn't support taproot miniscript").into(),
                text::caption(kind.to_string()).into(),
            ])
            .width(Length::Fill),
        )
        .align_y(Alignment::Center);
    button::device(content, msg)
}

pub fn signed_hot_key<'a, T: 'a + Clone + 'static, F: Display>(
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(hot_identity(
            alias.map(|a| a.to_string()),
            fingerprint,
            None,
        ))
        .push(success_marker(Some("Signed")))
        .align_y(Alignment::Center);
    button::device(content, msg)
}

pub fn selected_hot_key<'a, T: 'a + Clone + 'static, F: Display>(
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(hot_identity(
            alias.map(|a| a.to_string()),
            fingerprint,
            Some("(A derived key from a mnemonic stored locally)"),
        ))
        .push(success_marker::<T>(None))
        .align_y(Alignment::Center);
    button::device(content, msg)
}

pub fn unselected_hot_key<'a, T: 'a + Clone, F: Display>(
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = hot_identity(
        alias.map(|a| a.to_string()),
        fingerprint,
        Some("(A derived key from a mnemonic stored locally)"),
    );
    button::device(content, msg)
}

pub fn hot_key<'a, T: 'a + Clone, F: Display>(
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>> + Display>,
    can_sign: bool,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(hot_identity(
            alias.map(|a| a.to_string()),
            fingerprint,
            None,
        ))
        .push(Space::with_width(Length::Fixed(20.0)))
        .push_maybe(if !can_sign {
            Some(text::text(
                "This hot signer is not part of this spending path.",
            ))
        } else {
            None
        })
        .push(Space::with_width(Length::Fill))
        .align_y(Vertical::Center);
    button::device(content, msg)
}

pub fn selected_provider<'a, T: 'a + Clone + 'static, F: Display>(
    fingerprint: F,
    alias: impl Into<Cow<'a, str>> + Display,
    key_kind: impl Into<Cow<'a, str>> + Display,
    token: impl Into<Cow<'a, str>> + Display,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(provider_identity(
            fingerprint,
            Some(alias.to_string()),
            key_kind.to_string(),
            token.to_string(),
        ))
        .push(success_marker::<T>(None))
        .align_y(Alignment::Center);
    button::device(content, msg)
}

pub fn unselected_provider<'a, T: 'a + Clone, F: Display>(
    fingerprint: F,
    alias: impl Into<Cow<'a, str>> + Display,
    key_kind: impl Into<Cow<'a, str>> + Display,
    token: impl Into<Cow<'a, str>> + Display,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = provider_identity::<T, _>(
        fingerprint,
        Some(alias.to_string()),
        key_kind.to_string(),
        token.to_string(),
    );
    button::device(content, msg)
}

pub fn unsaved_provider<'a, T: 'a + Clone + 'static, F: Display>(
    fingerprint: F,
    key_kind: impl Into<Cow<'a, str>> + Display,
    token: impl Into<Cow<'a, str>> + Display,
    msg: Option<T>,
) -> Element<'a, T> {
    let content = Row::new()
        .push(provider_identity::<T, _>(
            fingerprint,
            None,
            key_kind.to_string(),
            token.to_string(),
        ))
        .push(success_marker::<T>(None))
        .align_y(Alignment::Center);
    button::device(content, msg)
}
