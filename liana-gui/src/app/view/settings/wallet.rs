use std::collections::{HashMap, HashSet};

use iced::{
    widget::{column, row, tooltip::Position, Column, Space},
    Alignment, Length,
};

use liana::{
    descriptors::{LianaDescriptor, LianaPolicy},
    miniscript::bitcoin::bip32::Fingerprint,
};
use liana_ui::{
    component::{
        button::{self, btn_backup_encrypt_descriptor, btn_register_on_device, btn_update},
        card, form,
        panels::setting::{header, SectionKind},
        scrollable,
        text::new,
        tooltip_custom,
    },
    icon,
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{Element, Row, SpaceExt},
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        settings::ProviderKey,
        view::{
            dashboard,
            message::{Message, SettingsMessage},
            settings::SETTING_MSG,
            warning::warn,
        },
    },
    hw::HardwareWallet,
    t,
    view::hw::{device_list_entry, HwRowMode},
};

#[allow(clippy::too_many_arguments)]
pub fn wallet_settings<'a>(
    cache: &'a Cache,
    warning: Option<&'a Error>,
    descriptor: &'a LianaDescriptor,
    wallet_alias: &'a form::Value<String>,
    keys_aliases: &'a [(Fingerprint, form::Value<String>)],
    provider_keys: &'a HashMap<Fingerprint, ProviderKey>,
    processing: bool,
    updated: bool,
) -> Element<'a, Message> {
    let header = header(
        Some(SETTING_MSG),
        Some(SectionKind::Wallet.title()),
        Some(SettingsMessage::EditWalletSettings.into()),
    );

    // ------------------------- Descriptor card -------------------------
    let title = new::h3_semi(t!("settings-wallet-descriptor"));
    let descriptor_s =
        scrollable::horizontal_thin(new::caption(descriptor.to_string())).width(Length::Fill);

    let backup_msg = Message::Settings(SettingsMessage::ExportEncryptedDescriptor);
    let btn_backup = btn_backup_encrypt_descriptor(backup_msg);

    let btn_copy = button::btn_copy(Some(Message::Clipboard(descriptor.to_string())));
    let btn_register = btn_register_on_device(Message::Settings(SettingsMessage::RegisterWallet));

    let descriptor_row = row![descriptor_s, btn_copy]
        .spacing(HSpacing::M)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let btn_row = row![Space::fill_width(), btn_backup, btn_register]
        .spacing(HSpacing::M)
        .width(Length::Fill)
        .wrap();
    let descriptor_card = card::simple(
        column![title, descriptor_row, btn_row]
            .spacing(VSpacing::S)
            .width(Length::Fill),
    )
    .width(Length::Fill);

    // --------------------------- Policy card ---------------------------
    let policy_card = card::simple(display_policy(
        descriptor.policy(),
        keys_aliases,
        provider_keys,
    ))
    .width(Length::Fill);

    // -------------------------- Aliases card ---------------------------
    let w_alias_title = new::h3_semi(t!("settings-wallet-alias")).into();
    let w_alias_input = form::Form::new(&t!("settings-alias"), wallet_alias, move |msg| {
        Message::Settings(SettingsMessage::WalletAliasEdited(msg))
    })
    .warning(t!("settings-alias-too-long"))
    .into();

    let k_alias_title = new::h3_semi(t!("settings-fingerprint-aliases")).into();

    fn key_alias_entry<'a>(
        fg: &'a Fingerprint,
        name: &'a form::Value<String>,
    ) -> Element<'a, Message> {
        let fg = *fg;
        let fingerprint = new::b5_bold(fg.to_string()).width(100);
        let alias = form::Form::new(&t!("settings-alias"), name, move |msg| {
            Message::Settings(SettingsMessage::FingerprintAliasEdited(fg, msg))
        })
        .warning(t!("settings-correct-alias"));
        row![fingerprint, alias]
            .spacing(HSpacing::M)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into()
    }

    let mut col_content: Vec<Element<'a, Message>> =
        vec![w_alias_title, w_alias_input, k_alias_title];

    for (fg, name) in keys_aliases.iter() {
        col_content.push(key_alias_entry(fg, name));
    }

    let update_msg =
        (!processing && wallet_alias.valid).then_some(Message::Settings(SettingsMessage::Save));
    let updated_label = updated.then_some(
        row![
            icon::circle_check_icon().style(theme::text::success),
            new::caption(t!("settings-updated")).style(theme::text::success)
        ]
        .align_y(Alignment::Center),
    );
    let last_row =
        row![Space::fill_width(), updated_label, btn_update(update_msg)].align_y(Alignment::Center);

    col_content.push(last_row.into());

    let alias_card = card::simple(
        Column::from_vec(col_content)
            .spacing(VSpacing::S)
            .width(Length::Fill),
    )
    .width(Length::Fill);

    let content = column![header, descriptor_card, policy_card, alias_card].spacing(VSpacing::L);

    dashboard(&Menu::Settings, cache, warning, content)
}

fn display_policy<'a>(
    policy: LianaPolicy,
    keys_aliases: &'a [(Fingerprint, form::Value<String>)],
    provider_keys: &'a HashMap<Fingerprint, ProviderKey>,
) -> Element<'a, Message> {
    let (primary_threshold, primary_keys) = policy.primary_path().thresh_origins();
    let recovery_paths = policy.recovery_paths();

    // The iteration over an HashMap keys can have a different order at each refresh
    let mut primary_keys: Vec<Fingerprint> = primary_keys.into_keys().collect();
    primary_keys.sort();

    let primary_signatures = new::b5_bold(t!("policy-signatures", count = primary_threshold));
    let primary_by = if primary_keys.len() > 1 {
        new::caption(t!("policy-out-of-by", count = primary_keys.len()))
    } else {
        new::caption(t!("policy-by"))
    };
    let primary_signers = signers_row(&primary_keys, keys_aliases);
    let primary_path = row![
        primary_signatures,
        primary_by,
        primary_signers,
        new::caption(t!("policy-primary-path"))
    ]
    .spacing(HSpacing::S);

    let mut paths = column![primary_path];
    for (i, (sequence, recovery_path)) in recovery_paths.iter().enumerate() {
        let (threshold, recovery_keys) = recovery_path.thresh_origins();

        // The iteration over an HashMap keys can have a different order at each refresh
        let mut recovery_keys: Vec<Fingerprint> = recovery_keys.into_keys().collect();
        recovery_keys.sort();

        let signatures = new::b5_bold(t!("policy-signatures", count = threshold));
        let by = if recovery_keys.len() > 1 {
            new::caption(t!("policy-out-of-by", count = recovery_keys.len()))
        } else {
            new::caption(t!("policy-by"))
        };
        let signers = signers_row(&recovery_keys, keys_aliases);
        let duration = new::b5_bold(t!(
            "policy-block-duration",
            blocks = sequence,
            duration = expire_message_units(*sequence as u32).join(",")
        ));
        let path_name = new::caption(
            // If max timelock and all keys are from provider, then it's a safety net path.
            if *sequence == u16::MAX
                && recovery_keys
                    .iter()
                    .all(|fg| provider_keys.contains_key(fg))
            {
                t!("policy-safety-net-path")
            } else {
                t!("policy-recovery-path", number = i + 1)
            },
        );

        paths = paths.push(
            row![
                signatures,
                by,
                signers,
                new::caption(t!("policy-inactive-for")),
                duration,
                path_name
            ]
            .spacing(HSpacing::S),
        );
    }

    let title = new::h3_semi(t!("policy-wallet-policy"));

    column![title, scrollable::horizontal_thin(paths)]
        .spacing(VSpacing::S)
        .into()
}

fn signers_row<'a>(
    keys: &[Fingerprint],
    keys_aliases: &'a [(Fingerprint, form::Value<String>)],
) -> Row<'a, Message> {
    keys.iter()
        .enumerate()
        .fold(Row::new().spacing(HSpacing::S), |row, (i, k)| {
            let content: Element<'a, Message> = if let Some(alias) = keys_aliases
                .iter()
                .find(|(fg, a)| fg == k && !a.value.is_empty())
                .map(|(_, f)| &f.value)
            {
                tooltip_custom(
                    new::caption(k.to_string()),
                    new::b5_bold(alias),
                    Position::Bottom,
                )
                .into()
            } else {
                new::b5_bold(format!("[{k}]")).into()
            };
            if i + 1 < keys.len() {
                row.push(content).push(new::caption(t!("common-and")))
            } else {
                row.push(content)
            }
        })
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
        let mut units = Vec::new();
        if n_years != 0 {
            units.push(t!("duration-years-compact", count = n_years));
        }
        if n_months != 0 {
            units.push(t!("duration-months-compact", count = n_months));
        }
        if n_days != 0 {
            units.push(t!("duration-days-compact", count = n_days));
        }
        units
    } else {
        n_minutes -= n_days * 1440;
        let n_hours = n_minutes / 60;
        n_minutes -= n_hours * 60;
        let mut units = Vec::new();
        if n_hours != 0 {
            units.push(t!("duration-hours-compact", count = n_hours));
        }
        if n_minutes != 0 {
            units.push(t!("duration-minutes-compact", count = n_minutes));
        }
        units
    }
}

pub fn register_wallet_modal<'a>(
    warning: Option<&Error>,
    hws: &'a [HardwareWallet],
    processing: bool,
    chosen_hw: Option<usize>,
    registered: &HashSet<Fingerprint>,
) -> Element<'a, Message> {
    let signers =
        hws.iter()
            .enumerate()
            .fold(Column::new().spacing(VSpacing::S), |col, (i, hw)| {
                col.push(device_list_entry(
                    hw,
                    HwRowMode::Registration {
                        chosen: Some(i) == chosen_hw,
                        processing,
                        complete: hw
                            .fingerprint()
                            .map(|f| registered.contains(&f))
                            .unwrap_or(false)
                            || if let HardwareWallet::Supported { registered, .. } = hw {
                                registered == &Some(true)
                            } else {
                                false
                            },
                        descriptor: None,
                        device_must_support_taproot: false,
                    },
                    move || Message::SelectHardwareWallet(i),
                ))
            });

    let title = new::h3_semi(t!("settings-select-device")).width(Length::Fill);
    let devices = column![title, signers]
        .spacing(VSpacing::S)
        .width(Length::Fill);
    let warning = warning.map(|w| warn(Some(w)));

    column![warning, card::simple(devices)].width(500).into()
}
