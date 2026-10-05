use std::str::FromStr;

use iced::{
    alignment,
    widget::{column, row, Column, Space},
    Alignment, Length,
};

use liana::miniscript::bitcoin::Network;
use liana_ui::{
    component::{
        badge::{self, Tile},
        button::{self, btn_cancel, btn_icon_edit, btn_save, btn_start_rescan},
        card,
        checkbox::labelled_radio,
        form,
        panels::setting::{header, SectionKind},
        scrollable, separation,
        text::new,
    },
    icon,
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{Container, Element, ProgressBar, Row, SpaceExt},
};
use lianad::config::BitcoindRpcAuth;

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{
            dashboard,
            message::{Message, SettingsEditMessage, SettingsMessage},
            settings::SETTING_MSG,
        },
    },
    node::{
        bitcoind::{RpcAuthType, RpcAuthValues},
        electrum::{self, validate_domain_checkbox},
    },
    t,
};

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
    let settings = Column::with_children(settings).spacing(VSpacing::L);

    let content = column![header, settings].spacing(VSpacing::L);

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
            row![new::b3(t!("installer-rpc-auth"))]
                .spacing(HSpacing::XL)
                .align_y(Alignment::Center),
            |row, auth_type| {
                row.push(labelled_radio(
                    auth_type,
                    auth_type == selected_auth_type,
                    SettingsEditMessage::BitcoindRpcAuthTypeSelected(*auth_type),
                ))
            },
        );
    let auth_fields = match selected_auth_type {
        RpcAuthType::CookieFile => {
            let cookie_path = form::Form::new_trimmed(
                &t!("settings-cookie-file-path"),
                &rpc_auth_vals.cookie_path,
                |value| SettingsEditMessage::FieldEdited("cookie_file_path", value),
            )
            .warning(t!("settings-valid-filesystem-path"));
            column![cookie_path]
        }
        RpcAuthType::UserPass => {
            let user =
                form::Form::new_trimmed(&t!("installer-user"), &rpc_auth_vals.user, |value| {
                    SettingsEditMessage::FieldEdited("user", value)
                })
                .warning(t!("settings-valid-user"));
            let password = form::Form::new_trimmed(
                &t!("installer-password"),
                &rpc_auth_vals.password,
                |value| SettingsEditMessage::FieldEdited("password", value),
            )
            .warning(t!("settings-valid-password"));
            column![row![user, password].spacing(HSpacing::M)]
        }
    };
    let address_label = new::b3(t!("settings-socket-address"));
    let address_input = form::Form::new_trimmed(&t!("settings-socket-address"), addr, |value| {
        SettingsEditMessage::FieldEdited("socket_address", value)
    })
    .warning(t!("settings-valid-address"));
    let address = column![address_label, address_input].spacing(VSpacing::XS);
    let fields = column![node_info, auth_type, auth_fields, address].spacing(VSpacing::L);

    let title = row![badge::tile(Tile::Bitcoin), new::h3_semi("Bitcoin Core")]
        .spacing(HSpacing::L)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let actions = edit_actions(processing);

    let content =
        column![title, separation().width(Length::Fill), fields, actions].spacing(VSpacing::L);

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
        let label = new::b5_bold(k).width(Length::FillPortion(1));
        let value = Container::new(scrollable::horizontal_thin(column![
            Space::with_height(VSpacing::S),
            new::caption(t)
        ]))
        .align_x(alignment::Horizontal::Right)
        .padding(10)
        .width(Length::FillPortion(3));
        let copy = button::btn_copy(Some(SettingsEditMessage::Clipboard(v.to_string())));
        col_fields = col_fields.push(
            row![label, value, Space::with_width(HSpacing::M), copy].align_y(Alignment::Center),
        );
    }
    let fields = column![node_info, col_fields].spacing(VSpacing::L);

    let running = is_running
        .filter(|_| is_configured_node_type)
        .map(is_running_label);
    let title = row![
        badge::tile(Tile::Bitcoin),
        new::h3_semi("Bitcoin Core"),
        running
    ]
    .spacing(HSpacing::L)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    let edit = btn_icon_edit(can_edit.then_some(SettingsEditMessage::Select));
    let header = row![title, edit].align_y(Alignment::Center);

    let content = column![header, separation().width(Length::Fill), fields].spacing(VSpacing::L);

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
    let address_label = new::b3(t!("common-address-label"));
    let address_input = form::Form::new_trimmed("127:0.0.1:50001", addr, |value| {
        SettingsEditMessage::FieldEdited("address", value)
    })
    .warning(t!("settings-valid-address"));
    let address_notes = new::small_caption(electrum::ADDRESS_NOTES).style(theme::text::secondary);
    let address =
        column![address_label, address_input, checkbox, address_notes].spacing(VSpacing::XS);
    let fields = column![node_info, address].spacing(VSpacing::L);

    let title = row![badge::tile(Tile::Bitcoin), new::h3_semi("Electrum")]
        .spacing(HSpacing::L)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    let actions = edit_actions(processing);

    let content =
        column![title, separation().width(Length::Fill), fields, actions].spacing(VSpacing::L);

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
        let label = new::b5_bold(k).width(Length::Fill);
        let value = new::caption(v.clone());
        let copy = button::btn_copy(Some(SettingsEditMessage::Clipboard(v.to_string())));
        col_fields = col_fields.push(
            row![label, value, Space::with_width(HSpacing::M), copy].align_y(Alignment::Center),
        );
    }
    let fields = column![node_info, col_fields].spacing(VSpacing::L);

    let running = is_running
        .filter(|_| is_configured_node_type)
        .map(is_running_label);
    let title = row![
        badge::tile(Tile::Bitcoin),
        new::h3_semi("Electrum"),
        running
    ]
    .spacing(HSpacing::L)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    let edit = btn_icon_edit(can_edit.then_some(SettingsEditMessage::Select));
    let header = row![title, edit].align_y(Alignment::Center);

    let content = column![header, separation().width(Length::Fill), fields].spacing(VSpacing::L);

    card::simple(content).width(Length::Fill).into()
}

fn node_info<'a>(
    is_configured_node_type: bool,
    network: Network,
    blockheight: i32,
) -> Option<Element<'a, SettingsEditMessage>> {
    (is_configured_node_type && blockheight != 0).then(|| {
        let network_info = row![
            badge::tile(Tile::Network),
            column![
                new::caption(t!("settings-network")).style(theme::text::secondary),
                new::b5_bold(network.to_string())
            ]
        ]
        .spacing(HSpacing::L)
        .width(Length::FillPortion(1));
        let blockheight_info = row![
            badge::tile(Tile::Block),
            column![
                new::caption(t!("settings-block-height")).style(theme::text::secondary),
                new::b5_bold(blockheight.to_string())
            ]
        ]
        .spacing(HSpacing::L)
        .width(Length::FillPortion(1));
        column![
            row![network_info, blockheight_info],
            separation().width(Length::Fill)
        ]
        .spacing(VSpacing::L)
        .into()
    })
}

fn edit_actions<'a>(processing: bool) -> Row<'a, SettingsEditMessage> {
    let cancel_button = btn_cancel((!processing).then_some(SettingsEditMessage::Cancel));
    let confirm_button = btn_save((!processing).then_some(SettingsEditMessage::Confirm), false);
    row![Space::fill_width(), cancel_button, confirm_button]
        .spacing(HSpacing::M)
        .align_y(Alignment::Center)
}

pub fn is_running_label<'a, T: 'a>(running: bool) -> Row<'a, T> {
    if running {
        let dot = icon::dot_icon().size(5).style(theme::text::success);
        let label = new::small_caption(t!("settings-running")).style(theme::text::success);
        row![dot, label].align_y(Alignment::Center)
    } else {
        let dot = icon::dot_icon().size(5).style(theme::text::error);
        let label = new::small_caption(t!("settings-not-running")).style(theme::text::error);
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
    let title = new::h3_semi(t!("settings-blockchain-rescan")).width(Length::Fill);
    let success_msg =
        success.then_some(new::caption(t!("settings-rescan-success")).style(theme::text::success));
    let header = row![badge::tile(Tile::Block), title, success_msg]
        .spacing(HSpacing::L)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    let body = if let Some(p) = scan_progress {
        let progress_bar = ProgressBar::new(0.0..=1.0, p as f32).length(Length::Fill);
        let progress = new::caption(t!(
            "settings-rescanning",
            progress = format!("{:.2}", p * 100.0)
        ));
        column![progress_bar, progress].width(Length::Fill)
    } else {
        let year_label = new::b3(t!("settings-year"));
        let year_input = form::Form::new_trimmed("2022", year, |value| {
            SettingsEditMessage::FieldEdited("rescan_year", value)
        });
        let month_label = new::b3(t!("settings-month"));
        let month_input = form::Form::new_trimmed("12", month, |value| {
            SettingsEditMessage::FieldEdited("rescan_month", value)
        });
        let day_label = new::b3(t!("settings-day"));
        let day_input = form::Form::new_trimmed("31", day, |value| {
            SettingsEditMessage::FieldEdited("rescan_day", value)
        });
        let date = row![
            year_label,
            year_input,
            month_label,
            month_input,
            day_label,
            day_input
        ]
        .align_y(Alignment::Center)
        .spacing(HSpacing::M);
        let invalid_date_error = invalid_date
            .then_some(new::caption(t!("settings-date-invalid")).style(theme::text::error));
        let past_possible_height_error = past_possible_height
            .then_some(new::caption(t!("settings-date-before-prune")).style(theme::text::error));
        let future_date_error = future_date
            .then_some(new::caption(t!("settings-date-future")).style(theme::text::error));
        let can_start = can_edit
            && !invalid_date
            && !processing
            && (is_ok_and(&u32::from_str(&year.value), |&v| v > 0)
                && is_ok_and(&u32::from_str(&month.value), |&v| v > 0 && v <= 12)
                && is_ok_and(&u32::from_str(&day.value), |&v| v > 0 && v <= 31));
        let start = btn_start_rescan(
            processing,
            can_start.then_some(SettingsEditMessage::Confirm),
        );
        let start = row![Space::fill_width(), start];
        column![
            date,
            invalid_date_error,
            past_possible_height_error,
            future_date_error,
            start
        ]
        .spacing(VSpacing::S)
    };

    let content = column![header, separation().width(Length::Fill), body].spacing(VSpacing::L);

    card::simple(content).width(Length::Fill).into()
}

fn is_ok_and<T, E>(res: &Result<T, E>, f: impl FnOnce(&T) -> bool) -> bool {
    if let Ok(v) = res {
        f(v)
    } else {
        false
    }
}
