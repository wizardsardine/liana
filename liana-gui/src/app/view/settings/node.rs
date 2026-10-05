use std::str::FromStr;

use iced::{
    alignment,
    widget::{column, radio, row, Column, Space},
    Alignment, Length,
};

use liana::miniscript::bitcoin::Network;
use liana_ui::{
    component::{
        badge,
        button::{self, btn_icon_edit},
        card, form,
        panels::setting::{header, SectionKind},
        scrollable, separation,
        text::{legacy, Text},
    },
    icon, theme,
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
    let settings = Column::with_children(settings).spacing(20);

    let content = column![header, settings].spacing(20);

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
