pub mod general;

use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};

use iced::{
    alignment::{self, Vertical},
    widget::{column, radio, row, rule, tooltip as iced_tooltip, Column, Space},
    Alignment, Length,
};

use liana::{
    descriptors::{LianaDescriptor, LianaPolicy},
    miniscript::bitcoin::{bip32::Fingerprint, Network},
};
use liana_ui::{
    component::{
        self, badge,
        button::{
            self, btn_backup_encrypt_descriptor, btn_icon_edit, btn_register_on_device, btn_update,
        },
        card, form,
        panels::setting::{
            export_section, header, settings_section, ImportExportKind, SectionKind,
        },
        scrollable, separation,
        text::{legacy, Text},
    },
    icon,
    theme::{self},
    widget::*,
};
use lianad::config::BitcoindRpcAuth;

use super::{dashboard, message::*};

use crate::{
    app::{cache::Cache, error::Error, menu::Menu, settings::ProviderKey, view::warning::warn},
    help,
    hw::HardwareWallet,
    node::{
        bitcoind::{RpcAuthType, RpcAuthValues},
        electrum::{self, validate_domain_checkbox},
    },
    t,
    view::hw::{device_list_entry, HwRowMode},
};

const SETTING_MSG: Message = Message::Menu(Menu::Settings);

pub fn list(cache: &Cache, is_remote_backend: bool) -> Element<'_, Message> {
    let general = settings_section(
        SectionKind::General,
        Message::Settings(SettingsMessage::GeneralSection),
    );

    let node = settings_section(
        SectionKind::Node,
        Message::Settings(SettingsMessage::EditBitcoindSettings),
    );

    let backend = settings_section(
        SectionKind::Backend,
        Message::Settings(SettingsMessage::EditRemoteBackendSettings),
    );

    let wallet = settings_section(
        SectionKind::Wallet,
        Message::Settings(SettingsMessage::EditWalletSettings),
    );

    let import_export = settings_section(
        SectionKind::ImportExport,
        Message::Settings(SettingsMessage::ImportExportSection),
    );

    let about = settings_section(
        SectionKind::About,
        Message::Settings(SettingsMessage::AboutSection),
    );

    let backend = if !is_remote_backend { node } else { backend };

    #[rustfmt::skip]
    let entries = vec![
        general,
        backend,
        wallet,
        import_export,
        about
    ];

    let content = component::panels::setting::section_list(entries);
    dashboard(&Menu::Settings, cache, None, content)
}

pub fn link<'a>(url: &str, link_text: impl std::fmt::Display) -> Element<'a, Message> {
    let link_btn = button::link(Some(icon::link_icon()), link_text)
        .on_press(Message::OpenUrl(url.to_string()));
    let url_tooltip = Container::new(legacy::text(url))
        .style(theme::card::simple)
        .padding(10);

    iced_tooltip::Tooltip::new(link_btn, url_tooltip, iced_tooltip::Position::Bottom).into()
}

pub fn bitcoind_settings<'a>(
    cache: &'a Cache,
    warning: Option<&'a Error>,
    settings: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let header = header(
        Some(SETTING_MSG),
        Some(SectionKind::Node.title()),
        Some(SettingsMessage::EditBitcoindSettings.into()),
    );
    let settings = Column::with_children(settings).spacing(20);

    let content = column![header, settings].spacing(20);

    dashboard(&Menu::Settings, cache, warning, content)
}

pub fn import_export<'a>(cache: &'a Cache, warning: Option<&'a Error>) -> Element<'a, Message> {
    let header = header(
        Some(SETTING_MSG),
        Some(SectionKind::ImportExport.title()),
        Some(SettingsMessage::ImportExportSection.into()),
    );

    let description = row![
        Space::with_width(15),
        legacy::text(t!("settings-import-export-description")),
        Space::fill_width()
    ];

    let export_encrypted_descriptor = export_section(
        ImportExportKind::ExportEncryptedDescriptor,
        Message::Settings(SettingsMessage::ExportEncryptedDescriptor),
    );

    let export_descriptor = export_section(
        ImportExportKind::ExportDescriptor,
        Message::Settings(SettingsMessage::ExportPlaintextDescriptor),
    );

    let export_transactions = export_section(
        ImportExportKind::ExportTransactions,
        Message::Settings(SettingsMessage::ExportTransactions),
    );

    let export_labels = export_section(
        ImportExportKind::ExportLabels,
        Message::Settings(SettingsMessage::ExportLabels),
    );

    let export_wallet = export_section(
        ImportExportKind::ExportWallet,
        Message::Settings(SettingsMessage::ExportWallet),
    );

    let import_wallet = export_section(
        ImportExportKind::ImportWallet,
        Message::Settings(SettingsMessage::ImportWallet),
    );

    let separator = row![
        Space::with_width(30),
        legacy::text(t!("settings-other-formats")),
        Space::with_width(15),
        rule::horizontal(2),
        Space::with_width(30)
    ]
    .align_y(Vertical::Center);

    let content = column![
        header,
        description,
        export_encrypted_descriptor,
        export_wallet,
        import_wallet,
        separator,
        export_labels,
        export_transactions,
        export_descriptor
    ]
    .spacing(20)
    .width(Length::Fill);

    dashboard(&Menu::Settings, cache, warning, content)
}

pub fn about_section<'a>(
    cache: &'a Cache,
    warning: Option<&'a Error>,
    lianad_version: Option<&String>,
) -> Element<'a, Message> {
    let header = header(
        Some(SETTING_MSG),
        Some(SectionKind::About.title()),
        Some(SettingsMessage::AboutSection.into()),
    );

    let version_title = row![
        badge::tooltip(),
        legacy::text(t!("settings-version")).bold()
    ]
    .padding(10)
    .spacing(20)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    let gui_version = legacy::text(format!("liana-gui v{}", crate::VERSION));
    let daemon_version = lianad_version.map(|version| legacy::text(format!("lianad v{version}")));
    let versions = row![Space::fill_width(), column![gui_version, daemon_version]];
    let version_card = card::simple(column![
        version_title,
        separation().width(Length::Fill),
        Space::with_height(10),
        versions
    ]);

    let content = column![header, version_card]
        .spacing(20)
        .width(Length::Fill);

    dashboard(&Menu::Settings, cache, warning, content)
}

pub fn remote_backend_section<'a>(
    cache: &'a Cache,
    email_form: &form::Value<String>,
    processing: bool,
    success: bool,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let header = header(
        Some(SETTING_MSG),
        Some(SectionKind::Backend.title()),
        Some(SettingsMessage::EditRemoteBackendSettings.into()),
    );

    let description = legacy::text(t!("settings-grant-wallet-access"));
    let email = form::Form::new_trimmed(&t!("settings-user-email"), email_form, |email| {
        Message::Settings(SettingsMessage::RemoteBackendSettings(
            RemoteBackendSettingsMessage::EditInvitationEmail(email),
        ))
    })
    .warning(t!("settings-email-invalid"))
    .size(legacy::P1_SIZE)
    .padding(10);
    let invitation_sent =
        success.then_some(legacy::text(t!("settings-invitation-sent")).style(theme::text::success));
    let send_msg = (!processing && email_form.valid).then_some(Message::Settings(
        SettingsMessage::RemoteBackendSettings(RemoteBackendSettingsMessage::SendInvitation),
    ));
    let send = button::secondary(None, t!("btn-send-invitation")).on_press_maybe(send_msg);
    let actions = row![invitation_sent, Space::fill_width(), send];
    let invitation_card =
        card::simple(column![description, email, actions].spacing(20)).width(Length::Fill);
    let help_link = link(
        help::CHANGE_BACKEND_OR_NODE_URL,
        t!("settings-connect-own-node"),
    );

    let content = column![header, invitation_card, help_link].spacing(20);

    dashboard(&Menu::Settings, cache, warning, content)
}

pub fn bitcoind_edit<'a>(
    is_configured_node_type: bool,
    network: Network,
    blockheight: i32,
    addr: &form::Value<String>,
    rpc_auth_vals: &RpcAuthValues,
    selected_auth_type: &RpcAuthType,
    processing: bool,
) -> Element<'a, SettingsEditMessage> {
    let node_info = node_info(is_configured_node_type, network, blockheight);

    let auth_type = [RpcAuthType::CookieFile, RpcAuthType::UserPass]
        .iter()
        .fold(
            row![legacy::text(t!("installer-rpc-auth")).small().bold()].spacing(10),
            |row, auth_type| {
                row.push(radio(
                    format!("{auth_type}"),
                    *auth_type,
                    Some(*selected_auth_type),
                    SettingsEditMessage::BitcoindRpcAuthTypeSelected,
                ))
                .spacing(30)
                .align_y(Alignment::Center)
            },
        );
    let auth_fields = match selected_auth_type {
        RpcAuthType::CookieFile => {
            let cookie_path = form::Form::new_trimmed(
                &t!("settings-cookie-file-path"),
                &rpc_auth_vals.cookie_path,
                |value| SettingsEditMessage::FieldEdited("cookie_file_path", value),
            )
            .warning(t!("settings-valid-filesystem-path"))
            .size(legacy::P1_SIZE)
            .padding(5);
            column![cookie_path].spacing(5)
        }
        RpcAuthType::UserPass => {
            let user =
                form::Form::new_trimmed(&t!("installer-user"), &rpc_auth_vals.user, |value| {
                    SettingsEditMessage::FieldEdited("user", value)
                })
                .warning(t!("settings-valid-user"))
                .size(legacy::P1_SIZE)
                .padding(5);
            let password = form::Form::new_trimmed(
                &t!("installer-password"),
                &rpc_auth_vals.password,
                |value| SettingsEditMessage::FieldEdited("password", value),
            )
            .warning(t!("settings-valid-password"))
            .size(legacy::P1_SIZE)
            .padding(5);
            column![row![user, password].spacing(10)].spacing(5)
        }
    };
    let address_label = legacy::text(t!("settings-socket-address")).bold().small();
    let address_input = form::Form::new_trimmed(&t!("settings-socket-address"), addr, |value| {
        SettingsEditMessage::FieldEdited("socket_address", value)
    })
    .warning(t!("settings-valid-address"))
    .size(legacy::P1_SIZE)
    .padding(5);
    let address = column![address_label, address_input].spacing(5);
    let fields = column![node_info, auth_type, auth_fields, address].spacing(20);

    let title = row![badge::bitcoin(), legacy::text("Bitcoin Core").bold()]
        .padding(10)
        .spacing(20)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let actions = edit_actions(processing);

    let content = column![title, separation().width(Length::Fill), fields, actions].spacing(20);

    card::simple(content).width(Length::Fill).into()
}

pub fn bitcoind<'a>(
    is_configured_node_type: bool,
    network: Network,
    config: &lianad::config::BitcoindConfig,
    blockheight: i32,
    is_running: Option<bool>,
    can_edit: bool,
) -> Element<'a, SettingsEditMessage> {
    let node_info = node_info(is_configured_node_type, network, blockheight);

    let mut rows = vec![];
    if is_configured_node_type {
        match &config.rpc_auth {
            BitcoindRpcAuth::CookieFile(path) => {
                rows.push((
                    t!("settings-cookie-file-path"),
                    path.to_str().unwrap().to_string(),
                ));
            }
            BitcoindRpcAuth::UserPass(user, password) => {
                rows.push((t!("installer-user"), user.clone()));
                rows.push((t!("installer-password"), password.clone()));
            }
        }
        rows.push((t!("settings-socket-address"), config.addr.to_string()));
    }

    let mut col_fields = Column::new();
    for (k, v) in rows {
        let t = if k == t!("installer-password") {
            "*".to_string().repeat(v.len())
        } else {
            v.clone()
        };
        let label = legacy::text(k).bold().small().width(Length::FillPortion(1));
        let value = Container::new(scrollable::horizontal_thin(column![
            Space::with_height(10),
            legacy::text(t).small()
        ]))
        .align_x(alignment::Horizontal::Right)
        .padding(10)
        .width(Length::FillPortion(3));
        let copy = button::btn_copy(Some(SettingsEditMessage::Clipboard(v.to_string())));
        col_fields = col_fields
            .push(row![label, value, Space::with_width(10), copy].align_y(Alignment::Center));
    }
    let fields = column![node_info, col_fields].spacing(20);

    let running = is_running
        .filter(|_| is_configured_node_type)
        .map(is_running_label);
    let title = row![
        badge::bitcoin(),
        legacy::text("Bitcoin Core").bold(),
        running
    ]
    .spacing(20)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    let edit = btn_icon_edit(can_edit.then_some(SettingsEditMessage::Select));
    let header = row![title, edit].align_y(Alignment::Center);

    let content = column![header, separation().width(Length::Fill), fields].spacing(20);

    card::simple(content).width(Length::Fill).into()
}

pub fn electrum_edit<'a>(
    is_configured_node_type: bool,
    network: Network,
    blockheight: i32,
    addr: &form::Value<String>,
    processing: bool,
    validate_domain: bool,
) -> Element<'a, SettingsEditMessage> {
    let node_info = node_info(is_configured_node_type, network, blockheight);

    let checkbox = validate_domain_checkbox(addr, validate_domain, |b| {
        SettingsEditMessage::ValidateDomainEdited(b)
    });
    let address_label = legacy::text(t!("common-address-label")).bold().small();
    let address_input = form::Form::new_trimmed("127:0.0.1:50001", addr, |value| {
        SettingsEditMessage::FieldEdited("address", value)
    })
    .warning(t!("settings-valid-address"))
    .size(legacy::P1_SIZE)
    .padding(5);
    let address_notes = legacy::text(electrum::ADDRESS_NOTES).size(legacy::P2_SIZE);
    let address = column![address_label, address_input, checkbox, address_notes].spacing(5);
    let fields = column![node_info, address].spacing(20);

    let title = row![badge::bitcoin(), legacy::text("Electrum").bold()]
        .padding(10)
        .spacing(20)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let actions = edit_actions(processing);

    let content = column![title, separation().width(Length::Fill), fields, actions].spacing(20);

    card::simple(content).width(Length::Fill).into()
}

pub fn electrum<'a>(
    is_configured_node_type: bool,
    network: Network,
    config: &lianad::config::ElectrumConfig,
    blockheight: i32,
    is_running: Option<bool>,
    can_edit: bool,
) -> Element<'a, SettingsEditMessage> {
    let node_info = node_info(is_configured_node_type, network, blockheight);

    let rows = if is_configured_node_type {
        vec![(t!("common-address-label"), config.addr.to_string())]
    } else {
        vec![]
    };

    let mut col_fields = Column::new();
    for (k, v) in rows {
        let label = legacy::text(k).bold().small().width(Length::Fill);
        let value = legacy::text(v.clone()).small();
        let copy = button::btn_copy(Some(SettingsEditMessage::Clipboard(v.to_string())));
        col_fields = col_fields
            .push(row![label, value, Space::with_width(10), copy].align_y(Alignment::Center));
    }
    let fields = column![node_info, col_fields].spacing(20);

    let running = is_running
        .filter(|_| is_configured_node_type)
        .map(is_running_label);
    let title = row![badge::bitcoin(), legacy::text("Electrum").bold(), running]
        .spacing(20)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let edit = btn_icon_edit(can_edit.then_some(SettingsEditMessage::Select));
    let header = row![title, edit].align_y(Alignment::Center);

    let content = column![header, separation().width(Length::Fill), fields].spacing(20);

    card::simple(content).width(Length::Fill).into()
}

fn node_info<'a>(
    is_configured_node_type: bool,
    network: Network,
    blockheight: i32,
) -> Option<Element<'a, SettingsEditMessage>> {
    (is_configured_node_type && blockheight != 0).then(|| {
        let network_info = row![
            badge::network(),
            column![
                legacy::text(t!("settings-network")),
                legacy::text(network.to_string()).bold()
            ]
        ]
        .spacing(10)
        .width(Length::FillPortion(1));
        let blockheight_info = row![
            badge::block(),
            column![
                legacy::text(t!("settings-block-height")),
                legacy::text(blockheight.to_string()).bold()
            ]
        ]
        .spacing(10)
        .width(Length::FillPortion(1));
        column![
            row![network_info, blockheight_info],
            separation().width(Length::Fill)
        ]
        .spacing(20)
        .into()
    })
}

fn edit_actions<'a>(processing: bool) -> Row<'a, SettingsEditMessage> {
    let cancel_button = button::transparent(None, t!("btn-cancel"))
        .padding(5)
        .on_press_maybe((!processing).then_some(SettingsEditMessage::Cancel));
    let confirm_button = button::secondary(None, t!("btn-save"))
        .padding(5)
        .on_press_maybe((!processing).then_some(SettingsEditMessage::Confirm));
    row![Space::fill_width(), cancel_button, confirm_button]
        .spacing(10)
        .align_y(Alignment::Center)
}

pub fn is_running_label<'a, T: 'a>(running: bool) -> Row<'a, T> {
    if running {
        let dot = icon::dot_icon().size(5).style(theme::text::success);
        let label = legacy::text(t!("settings-running"))
            .small()
            .style(theme::text::success);
        row![dot, label].align_y(Alignment::Center)
    } else {
        let dot = icon::dot_icon().size(5).style(theme::text::error);
        let label = legacy::text(t!("settings-not-running"))
            .small()
            .style(theme::text::error);
        row![dot, label].align_y(Alignment::Center)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn rescan<'a>(
    year: &form::Value<String>,
    month: &form::Value<String>,
    day: &form::Value<String>,
    scan_progress: Option<f64>,
    success: bool,
    processing: bool,
    can_edit: bool,
    invalid_date: bool,
    past_possible_height: bool,
    future_date: bool,
) -> Element<'a, SettingsEditMessage> {
    let title = legacy::text(t!("settings-blockchain-rescan"))
        .bold()
        .width(Length::Fill);
    let success_msg =
        success.then_some(legacy::text(t!("settings-rescan-success")).style(theme::text::success));
    let header = row![badge::block(), title, success_msg]
        .spacing(20)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    let body = if let Some(p) = scan_progress {
        let progress_bar = ProgressBar::new(0.0..=1.0, p as f32).length(Length::Fill);
        let progress = legacy::text(t!(
            "settings-rescanning",
            progress = format!("{:.2}", p * 100.0)
        ));
        column![progress_bar, progress].width(Length::Fill)
    } else {
        let year_label = legacy::text(t!("settings-year")).bold().small();
        let year_input = form::Form::new_trimmed("2022", year, |value| {
            SettingsEditMessage::FieldEdited("rescan_year", value)
        })
        .size(legacy::P1_SIZE)
        .padding(5);
        let month_label = legacy::text(t!("settings-month")).bold().small();
        let month_input = form::Form::new_trimmed("12", month, |value| {
            SettingsEditMessage::FieldEdited("rescan_month", value)
        })
        .size(legacy::P1_SIZE)
        .padding(5);
        let day_label = legacy::text(t!("settings-day")).bold().small();
        let day_input = form::Form::new_trimmed("31", day, |value| {
            SettingsEditMessage::FieldEdited("rescan_day", value)
        })
        .size(legacy::P1_SIZE)
        .padding(5);
        let date = row![
            year_label,
            year_input,
            month_label,
            month_input,
            day_label,
            day_input
        ]
        .align_y(Alignment::Center)
        .spacing(10);
        let invalid_date_error = invalid_date
            .then_some(legacy::p1_regular(t!("settings-date-invalid")).style(theme::text::error));
        let past_possible_height_error = past_possible_height.then_some(
            legacy::p1_regular(t!("settings-date-before-prune")).style(theme::text::error),
        );
        let future_date_error = future_date
            .then_some(legacy::p1_regular(t!("settings-date-future")).style(theme::text::error));
        let can_start = can_edit
            && !invalid_date
            && !processing
            && (is_ok_and(&u32::from_str(&year.value), |&v| v > 0)
                && is_ok_and(&u32::from_str(&month.value), |&v| v > 0 && v <= 12)
                && is_ok_and(&u32::from_str(&day.value), |&v| v > 0 && v <= 31));
        let start = if can_start {
            button::primary(None, t!("btn-start-rescan"))
                .on_press(SettingsEditMessage::Confirm)
                .width(Length::Shrink)
        } else if processing {
            button::secondary(None, t!("btn-starting-rescan")).width(Length::Shrink)
        } else {
            button::secondary(None, t!("btn-start-rescan")).width(Length::Shrink)
        };
        let start = row![Space::fill_width(), start];
        column![
            date,
            invalid_date_error,
            past_possible_height_error,
            future_date_error,
            start
        ]
        .spacing(10)
    };

    let content = column![header, separation().width(Length::Fill), body].spacing(20);

    card::simple(content).width(Length::Fill).into()
}

fn is_ok_and<T, E>(res: &Result<T, E>, f: impl FnOnce(&T) -> bool) -> bool {
    if let Ok(v) = res {
        f(v)
    } else {
        false
    }
}

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
    let title = legacy::text(t!("settings-wallet-descriptor")).bold();
    let descriptor_s = scrollable::horizontal_thin(legacy::text(descriptor.to_string()).small())
        .width(Length::Fill);

    let backup_msg = Message::Settings(SettingsMessage::ExportEncryptedDescriptor);
    let btn_backup = btn_backup_encrypt_descriptor(backup_msg);

    let btn_copy = button::btn_copy(Some(Message::Clipboard(descriptor.to_string())));
    let btn_register = btn_register_on_device(Message::Settings(SettingsMessage::RegisterWallet));

    let descriptor_row = row![descriptor_s, btn_copy]
        .spacing(10)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let btn_row = row![Space::fill_width(), btn_backup, btn_register]
        .spacing(10)
        .width(Length::Fill)
        .wrap();
    let descriptor_card = card::simple(
        column![title, descriptor_row, btn_row]
            .spacing(10)
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
    let w_alias_title = legacy::text(t!("settings-wallet-alias")).bold().into();
    let w_alias_input = form::Form::new(&t!("settings-alias"), wallet_alias, move |msg| {
        Message::Settings(SettingsMessage::WalletAliasEdited(msg))
    })
    .warning(t!("settings-alias-too-long"))
    .size(legacy::P1_SIZE)
    .padding(10)
    .into();

    let k_alias_title = legacy::text(t!("settings-fingerprint-aliases"))
        .bold()
        .into();

    fn key_alias_entry<'a>(
        fg: &'a Fingerprint,
        name: &'a form::Value<String>,
    ) -> Element<'a, Message> {
        let fg = *fg;
        let fingerprint = legacy::text(fg.to_string()).bold().width(100);
        let alias = form::Form::new(&t!("settings-alias"), name, move |msg| {
            Message::Settings(SettingsMessage::FingerprintAliasEdited(fg, msg))
        })
        .warning(t!("settings-correct-alias"))
        .size(legacy::P1_SIZE)
        .padding(10);
        row![fingerprint, alias]
            .spacing(10)
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
            legacy::text(t!("settings-updated")).style(theme::text::success)
        ]
        .align_y(Alignment::Center),
    );
    let last_row =
        row![Space::fill_width(), updated_label, btn_update(update_msg)].align_y(Alignment::Center);

    col_content.push(last_row.into());

    let alias_card = card::simple(
        Column::from_vec(col_content)
            .spacing(10)
            .width(Length::Fill),
    )
    .width(Length::Fill);

    let content = column![header, descriptor_card, policy_card, alias_card].spacing(20);

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

    let primary_signatures =
        legacy::text(t!("policy-signatures", count = primary_threshold)).bold();
    let primary_by = if primary_keys.len() > 1 {
        legacy::text(t!("policy-out-of-by", count = primary_keys.len()))
    } else {
        legacy::text(t!("policy-by"))
    };
    let primary_signers = signers_row(&primary_keys, keys_aliases);
    let primary_path = row![
        primary_signatures,
        primary_by,
        primary_signers,
        legacy::text(t!("policy-primary-path"))
    ]
    .spacing(5);

    let mut paths = column![primary_path];
    for (i, (sequence, recovery_path)) in recovery_paths.iter().enumerate() {
        let (threshold, recovery_keys) = recovery_path.thresh_origins();

        // The iteration over an HashMap keys can have a different order at each refresh
        let mut recovery_keys: Vec<Fingerprint> = recovery_keys.into_keys().collect();
        recovery_keys.sort();

        let signatures = legacy::text(t!("policy-signatures", count = threshold)).bold();
        let by = if recovery_keys.len() > 1 {
            legacy::text(t!("policy-out-of-by", count = recovery_keys.len()))
        } else {
            legacy::text(t!("policy-by"))
        };
        let signers = signers_row(&recovery_keys, keys_aliases);
        let duration = legacy::text(t!(
            "policy-block-duration",
            blocks = sequence,
            duration = expire_message_units(*sequence as u32).join(",")
        ))
        .bold();
        let path_name = legacy::text(
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
                legacy::text(t!("policy-inactive-for")),
                duration,
                path_name
            ]
            .spacing(5),
        );
    }

    let title = legacy::text(t!("policy-wallet-policy")).bold();

    column![title, scrollable::horizontal_thin(paths)]
        .spacing(10)
        .into()
}

fn signers_row<'a>(
    keys: &[Fingerprint],
    keys_aliases: &'a [(Fingerprint, form::Value<String>)],
) -> Row<'a, Message> {
    keys.iter()
        .enumerate()
        .fold(Row::new().spacing(5), |row, (i, k)| {
            let content: Element<'a, Message> = if let Some(alias) = keys_aliases
                .iter()
                .find(|(fg, a)| fg == k && !a.value.is_empty())
                .map(|(_, f)| &f.value)
            {
                iced_tooltip::Tooltip::new(
                    legacy::text(alias).bold(),
                    legacy::text(k.to_string()),
                    iced_tooltip::Position::Bottom,
                )
                .style(theme::card::simple)
                .into()
            } else {
                legacy::text(format!("[{k}]")).bold().into()
            };
            if i + 1 < keys.len() {
                row.push(content).push(legacy::text(t!("common-and")))
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
    let signers = hws
        .iter()
        .enumerate()
        .fold(Column::new().spacing(10), |col, (i, hw)| {
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

    let title = legacy::text(t!("settings-select-device"))
        .bold()
        .width(Length::Fill);
    let devices = column![title, signers].spacing(10).width(Length::Fill);
    let warning = warning.map(|w| warn(Some(w)));

    column![warning, card::simple(devices)].width(500).into()
}
