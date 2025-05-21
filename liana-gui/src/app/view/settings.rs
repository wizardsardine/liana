use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use iced::alignment::Vertical;
use iced::widget::{Column, Rule};
use iced::{
    alignment,
    widget::{radio, scrollable, tooltip as iced_tooltip, Space},
    Alignment, Length,
};

use liana::{
    descriptors::{LianaDescriptor, LianaPolicy},
    miniscript::bitcoin::{bip32::Fingerprint, Network},
};
use lianad::config::BitcoindRpcAuth;

use super::{dashboard, message::*};

use liana_ui::{
    component::{badge, button, card, form, separation, text::*, tooltip::tooltip},
    icon,
    theme::{self},
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        settings::ProviderKey,
        view::{hw, warning::warn},
    },
    hw::HardwareWallet,
    node::{
        bitcoind::{RpcAuthType, RpcAuthValues},
        electrum,
    },
};

fn header(title: &str, msg: SettingsMessage) -> Row<'static, Message> {
    Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(
            Button::new(text("Settings").size(30).bold())
                .style(theme::button::transparent)
                .on_press(Message::Menu(Menu::Settings)),
        )
        .push(icon::chevron_right().size(30))
        .push(
            Button::new(text(title).size(30).bold())
                .style(theme::button::transparent)
                .on_press(Message::Settings(msg)),
        )
}

fn settings_section(
    title: &str,
    tool_tip: Option<&'static str>,
    icon: liana_ui::widget::Text<'static>,
    msg: Message,
) -> Container<'static, Message> {
    let tt = tool_tip.map(tooltip);
    Container::new(
        Button::new(
            Row::new()
                .push(badge::badge(icon))
                .push(text(title).bold())
                .push_maybe(tt)
                .padding(10)
                .spacing(20)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .style(theme::button::transparent_border)
        .on_press(msg),
    )
    .width(Length::Fill)
    .style(theme::card::simple)
}

fn export_section(
    title: &str,
    description: &str,
    icon: liana_ui::widget::Text<'static>,
    msg: Message,
) -> Container<'static, Message> {
    Container::new(
        Button::new(
            Row::new()
                .push(badge::badge(icon))
                .push(
                    Column::new()
                        .push(text(title).bold())
                        .push(caption(description)),
                )
                .padding(10)
                .spacing(20)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .style(theme::button::transparent_border)
        .on_press(msg),
    )
    .width(Length::Fill)
    .style(theme::card::simple)
}

pub fn list(cache: &Cache, is_remote_backend: bool) -> Element<Message> {
    let header = Button::new(text("Settings").size(30).bold())
        .style(theme::button::transparent)
        .on_press(Message::Menu(Menu::Settings));

    let node = settings_section(
        "Node",
        None,
        icon::bitcoin_icon(),
        Message::Settings(SettingsMessage::EditBitcoindSettings),
    );

    let backend = settings_section(
        "Backend",
        None,
        icon::bitcoin_icon(),
        Message::Settings(SettingsMessage::EditRemoteBackendSettings),
    );

    let wallet = settings_section(
        "Wallet",
        None,
        icon::wallet_icon(),
        Message::Settings(SettingsMessage::EditWalletSettings),
    );

    let import_export = settings_section(
        "Import/Export",
        None,
        icon::wallet_icon(),
        Message::Settings(SettingsMessage::ImportExportSection),
    );

    let about = settings_section(
        "About",
        None,
        icon::tooltip_icon(),
        Message::Settings(SettingsMessage::AboutSection),
    );

    dashboard(
        &Menu::Settings,
        cache,
        None,
        Column::new()
            .spacing(20)
            .width(Length::Fill)
            .push(header)
            .push(if !is_remote_backend { node } else { backend })
            .push(wallet)
            .push(import_export)
            .push(about),
    )
}

pub fn bitcoind_settings<'a>(
    cache: &'a Cache,
    warning: Option<&Error>,
    settings: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let header = header("Node", SettingsMessage::EditBitcoindSettings);

    dashboard(
        &Menu::Settings,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(header)
            .push(Column::with_children(settings).spacing(20)),
    )
}

pub fn import_export<'a>(cache: &'a Cache, warning: Option<&Error>) -> Element<'a, Message> {
    let header = header("Import/Export", SettingsMessage::ImportExportSection);

    let description = Row::new()
        .push(Space::with_width(15))
        .push(text(
            "A collection of the export and import functions present in Liana.",
        ))
        .push(Space::with_width(Length::Fill));

    let export_descriptor = export_section(
        "Descriptor only",
        "Descriptor file only, to use with other wallets.",
        icon::backup_icon(),
        Message::Settings(SettingsMessage::ExportDescriptor),
    );

    let export_transactions = export_section(
        "Transactions table",
        ".CSV file of past transactions, for accounting purposes.",
        icon::backup_icon(),
        Message::Settings(SettingsMessage::ExportTransactions),
    );

    let export_labels = export_section(
        "BIP 329 labels",
        "Bip 329 label export, compatible with other wallets.",
        icon::backup_icon(),
        Message::Settings(SettingsMessage::ExportLabels),
    );

    let export_wallet = export_section(
        "Back up wallet",
        "File with wallet info needed to restore on other devices (no private keys).",
        icon::backup_icon(),
        Message::Settings(SettingsMessage::ExportWallet),
    );

    let import_wallet = export_section(
        "Restore wallet",
        "Upload a backup file to update wallet info.",
        icon::restore_icon(),
        Message::Settings(SettingsMessage::ImportWallet),
    );

    let separator = Row::new()
        .push(Space::with_width(30))
        .push(text("Other formats"))
        .push(Space::with_width(15))
        .push(Rule::horizontal(2))
        .push(Space::with_width(30))
        .align_y(Vertical::Center);

    dashboard(
        &Menu::Settings,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(header)
            .push(description)
            .push(export_wallet)
            .push(import_wallet)
            .push(separator)
            .push(export_labels)
            .push(export_transactions)
            .push(export_descriptor)
            .width(Length::Fill),
    )
}

pub fn about_section<'a>(
    cache: &'a Cache,
    warning: Option<&Error>,
    lianad_version: Option<&String>,
) -> Element<'a, Message> {
    let header = header("About", SettingsMessage::AboutSection);

    let content = card::simple(
        Column::new()
            .push(
                Row::new()
                    .push(badge::badge(icon::tooltip_icon()))
                    .push(text("Version").bold())
                    .padding(10)
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
            )
            .push(separation().width(Length::Fill))
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(
                Row::new().push(Space::with_width(Length::Fill)).push(
                    Column::new()
                        .push(text(format!("liana-gui v{}", crate::VERSION)))
                        .push_maybe(
                            lianad_version.map(|version| text(format!("lianad v{}", version))),
                        ),
                ),
            ),
    );

    dashboard(
        &Menu::Settings,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(header)
            .push(content)
            .width(Length::Fill),
    )
}

pub fn remote_backend_section<'a>(
    cache: &'a Cache,
    email_form: &form::Value<String>,
    processing: bool,
    success: bool,
    warning: Option<&Error>,
) -> Element<'a, Message> {
    let header = header("Backend", SettingsMessage::EditRemoteBackendSettings);

    let content = card::simple(
        Column::new()
            .spacing(20)
            .push(text("Grant access to wallet to another user"))
            .push(
                form::Form::new_trimmed("User email", email_form, |email| {
                    Message::Settings(SettingsMessage::RemoteBackendSettings(
                        RemoteBackendSettingsMessage::EditInvitationEmail(email),
                    ))
                })
                .warning("Email is invalid")
                .size(P1_SIZE)
                .padding(10),
            )
            .push(
                Row::new()
                    .push_maybe(if success {
                        Some(text("Invitation was sent").style(theme::text::success))
                    } else {
                        None
                    })
                    .push(Space::with_width(Length::Fill))
                    .push(button::secondary(None, "Send invitation").on_press_maybe(
                        if !processing && email_form.valid {
                            Some(Message::Settings(SettingsMessage::RemoteBackendSettings(
                                RemoteBackendSettingsMessage::SendInvitation,
                            )))
                        } else {
                            None
                        },
                    )),
            ),
    )
    .width(Length::Fill);

    dashboard(
        &Menu::Settings,
        cache,
        warning,
        Column::new().spacing(20).push(header).push(content),
    )
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
    let mut col = Column::new().spacing(20);
    if is_configured_node_type && blockheight != 0 {
        col = col
            .push(
                Row::new()
                    .push(
                        Row::new()
                            .push(badge::badge(icon::network_icon()))
                            .push(
                                Column::new()
                                    .push(text("Network:"))
                                    .push(text(network.to_string()).bold()),
                            )
                            .spacing(10)
                            .width(Length::FillPortion(1)),
                    )
                    .push(
                        Row::new()
                            .push(badge::badge(icon::block_icon()))
                            .push(
                                Column::new()
                                    .push(text("Block Height:"))
                                    .push(text(blockheight.to_string()).bold()),
                            )
                            .spacing(10)
                            .width(Length::FillPortion(1)),
                    ),
            )
            .push(separation().width(Length::Fill));
    }

    col = col
        .push(
            [RpcAuthType::CookieFile, RpcAuthType::UserPass]
                .iter()
                .fold(
                    Row::new()
                        .push(text("RPC authentication:").small().bold())
                        .spacing(10),
                    |row, auth_type| {
                        row.push(radio(
                            format!("{}", auth_type),
                            *auth_type,
                            Some(*selected_auth_type),
                            SettingsEditMessage::BitcoindRpcAuthTypeSelected,
                        ))
                        .spacing(30)
                        .align_y(Alignment::Center)
                    },
                ),
        )
        .push(match selected_auth_type {
            RpcAuthType::CookieFile => Column::new()
                .push(
                    form::Form::new_trimmed(
                        "Cookie file path",
                        &rpc_auth_vals.cookie_path,
                        |value| SettingsEditMessage::FieldEdited("cookie_file_path", value),
                    )
                    .warning("Please enter a valid filesystem path")
                    .size(P1_SIZE)
                    .padding(5),
                )
                .spacing(5),
            RpcAuthType::UserPass => Column::new()
                .push(
                    Row::new()
                        .push(
                            form::Form::new_trimmed("User", &rpc_auth_vals.user, |value| {
                                SettingsEditMessage::FieldEdited("user", value)
                            })
                            .warning("Please enter a valid user")
                            .size(P1_SIZE)
                            .padding(5),
                        )
                        .push(
                            form::Form::new_trimmed("Password", &rpc_auth_vals.password, |value| {
                                SettingsEditMessage::FieldEdited("password", value)
                            })
                            .warning("Please enter a valid password")
                            .size(P1_SIZE)
                            .padding(5),
                        )
                        .spacing(10),
                )
                .spacing(5),
        })
        .push(
            Column::new()
                .push(text("Socket address:").bold().small())
                .push(
                    form::Form::new_trimmed("Socket address:", addr, |value| {
                        SettingsEditMessage::FieldEdited("socket_address", value)
                    })
                    .warning("Please enter a valid address")
                    .size(P1_SIZE)
                    .padding(5),
                )
                .spacing(5),
        );

    let mut cancel_button = button::transparent(None, " Cancel ").padding(5);
    let mut confirm_button = button::secondary(None, " Save ").padding(5);
    if !processing {
        cancel_button = cancel_button.on_press(SettingsEditMessage::Cancel);
        confirm_button = confirm_button.on_press(SettingsEditMessage::Confirm);
    }

    card::simple(Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(badge::badge(icon::bitcoin_icon()))
                    .push(text("Bitcoin Core").bold())
                    .padding(10)
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
            )
            .push(separation().width(Length::Fill))
            .push(col)
            .push(
                Container::new(
                    Row::new()
                        .push(cancel_button)
                        .push(confirm_button)
                        .spacing(10)
                        .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .align_x(alignment::Horizontal::Right),
            )
            .spacing(20),
    ))
    .width(Length::Fill)
    .into()
}

pub fn bitcoind<'a>(
    is_configured_node_type: bool,
    network: Network,
    config: &lianad::config::BitcoindConfig,
    blockheight: i32,
    is_running: Option<bool>,
    can_edit: bool,
) -> Element<'a, SettingsEditMessage> {
    let mut col = Column::new().spacing(20);
    if is_configured_node_type && blockheight != 0 {
        col = col
            .push(
                Row::new()
                    .push(
                        Row::new()
                            .push(badge::badge(icon::network_icon()))
                            .push(
                                Column::new()
                                    .push(text("Network:"))
                                    .push(text(network.to_string()).bold()),
                            )
                            .spacing(10)
                            .width(Length::FillPortion(1)),
                    )
                    .push(
                        Row::new()
                            .push(badge::badge(icon::block_icon()))
                            .push(
                                Column::new()
                                    .push(text("Block Height:"))
                                    .push(text(blockheight.to_string()).bold()),
                            )
                            .spacing(10)
                            .width(Length::FillPortion(1)),
                    ),
            )
            .push(separation().width(Length::Fill));
    }

    let mut rows = vec![];
    if is_configured_node_type {
        match &config.rpc_auth {
            BitcoindRpcAuth::CookieFile(path) => {
                rows.push(("Cookie file path:", path.to_str().unwrap().to_string()));
            }
            BitcoindRpcAuth::UserPass(user, password) => {
                rows.push(("User:", user.clone()));
                rows.push(("Password:", password.clone()));
            }
        }
        rows.push(("Socket address:", config.addr.to_string()));
    }

    let mut col_fields = Column::new();
    for (k, v) in rows {
        col_fields = col_fields.push({
            let t = if k == "Password:" {
                "*".to_string().repeat(v.len())
            } else {
                v.clone()
            };
            Row::new()
                .push(Container::new(text(k).bold().small()).width(Length::FillPortion(1)))
                .push(
                    Container::new(
                        scrollable(
                            Column::new()
                                .push(Space::with_height(Length::Fixed(10.0)))
                                .push(text(t).small())
                                // Space between the text and the scrollbar
                                .push(Space::with_height(Length::Fixed(10.0))),
                        )
                        .direction(scrollable::Direction::Horizontal(
                            scrollable::Scrollbar::new().width(2).scroller_width(2),
                        )),
                    )
                    .align_x(alignment::Horizontal::Right)
                    .padding(10)
                    .width(Length::FillPortion(3)),
                )
                .push(Space::with_width(10))
                .push(
                    Button::new(icon::clipboard_icon())
                        .style(theme::button::transparent_border)
                        .on_press(SettingsEditMessage::Clipboard(v.to_string())),
                )
                .align_y(Alignment::Center)
        });
    }

    card::simple(Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(
                        Row::new()
                            .push(badge::badge(icon::bitcoin_icon()))
                            .push(text("Bitcoin Core").bold())
                            .push_maybe(if is_configured_node_type {
                                Some(is_running_label(is_running))
                            } else {
                                None
                            })
                            .spacing(20)
                            .align_y(Alignment::Center)
                            .width(Length::Fill),
                    )
                    .push(if can_edit {
                        Button::new(icon::pencil_icon())
                            .style(theme::button::transparent_border)
                            .on_press(SettingsEditMessage::Select)
                    } else {
                        Button::new(icon::pencil_icon()).style(theme::button::transparent_border)
                    })
                    .align_y(Alignment::Center),
            )
            .push(separation().width(Length::Fill))
            .push(col.push(col_fields))
            .spacing(20),
    ))
    .width(Length::Fill)
    .into()
}

pub fn electrum_edit<'a>(
    is_configured_node_type: bool,
    network: Network,
    blockheight: i32,
    addr: &form::Value<String>,
    processing: bool,
) -> Element<'a, SettingsEditMessage> {
    let mut col = Column::new().spacing(20);
    if is_configured_node_type && blockheight != 0 {
        col = col
            .push(
                Row::new()
                    .push(
                        Row::new()
                            .push(badge::badge(icon::network_icon()))
                            .push(
                                Column::new()
                                    .push(text("Network:"))
                                    .push(text(network.to_string()).bold()),
                            )
                            .spacing(10)
                            .width(Length::FillPortion(1)),
                    )
                    .push(
                        Row::new()
                            .push(badge::badge(icon::block_icon()))
                            .push(
                                Column::new()
                                    .push(text("Block Height:"))
                                    .push(text(blockheight.to_string()).bold()),
                            )
                            .spacing(10)
                            .width(Length::FillPortion(1)),
                    ),
            )
            .push(separation().width(Length::Fill));
    }

    col = col.push(
        Column::new()
            .push(text("Address:").bold().small())
            .push(
                form::Form::new_trimmed("127:0.0.1:50001", addr, |value| {
                    SettingsEditMessage::FieldEdited("address", value)
                })
                .warning("Please enter a valid address")
                .size(P1_SIZE)
                .padding(5),
            )
            .push(text(electrum::ADDRESS_NOTES).size(P2_SIZE))
            .spacing(5),
    );

    let mut cancel_button = button::transparent(None, " Cancel ").padding(5);
    let mut confirm_button = button::secondary(None, " Save ").padding(5);
    if !processing {
        cancel_button = cancel_button.on_press(SettingsEditMessage::Cancel);
        confirm_button = confirm_button.on_press(SettingsEditMessage::Confirm);
    }

    card::simple(Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(badge::badge(icon::bitcoin_icon()))
                    .push(text("Electrum").bold())
                    .padding(10)
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
            )
            .push(separation().width(Length::Fill))
            .push(col)
            .push(
                Container::new(
                    Row::new()
                        .push(cancel_button)
                        .push(confirm_button)
                        .spacing(10)
                        .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .align_x(alignment::Horizontal::Right),
            )
            .spacing(20),
    ))
    .width(Length::Fill)
    .into()
}

pub fn electrum<'a>(
    is_configured_node_type: bool,
    network: Network,
    config: &lianad::config::ElectrumConfig,
    blockheight: i32,
    is_running: Option<bool>,
    can_edit: bool,
) -> Element<'a, SettingsEditMessage> {
    let mut col = Column::new().spacing(20);
    if is_configured_node_type && blockheight != 0 {
        col = col
            .push(
                Row::new()
                    .push(
                        Row::new()
                            .push(badge::badge(icon::network_icon()))
                            .push(
                                Column::new()
                                    .push(text("Network:"))
                                    .push(text(network.to_string()).bold()),
                            )
                            .spacing(10)
                            .width(Length::FillPortion(1)),
                    )
                    .push(
                        Row::new()
                            .push(badge::badge(icon::block_icon()))
                            .push(
                                Column::new()
                                    .push(text("Block Height:"))
                                    .push(text(blockheight.to_string()).bold()),
                            )
                            .spacing(10)
                            .width(Length::FillPortion(1)),
                    ),
            )
            .push(separation().width(Length::Fill));
    }

    let rows = if is_configured_node_type {
        vec![("Address:", config.addr.to_string())]
    } else {
        vec![]
    };

    let mut col_fields = Column::new();
    for (k, v) in rows {
        col_fields = col_fields.push(
            Row::new()
                .push(Container::new(text(k).bold().small()).width(Length::Fill))
                .push(text(v.clone()).small())
                .push(Space::with_width(10))
                .push(
                    Button::new(icon::clipboard_icon())
                        .style(theme::button::transparent_border)
                        .on_press(SettingsEditMessage::Clipboard(v.to_string())),
                )
                .align_y(Alignment::Center),
        );
    }

    card::simple(Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(
                        Row::new()
                            .push(badge::badge(icon::bitcoin_icon()))
                            .push(text("Electrum").bold())
                            .push_maybe(if is_configured_node_type {
                                Some(is_running_label(is_running))
                            } else {
                                None
                            })
                            .spacing(20)
                            .align_y(Alignment::Center)
                            .width(Length::Fill),
                    )
                    .push(if can_edit {
                        Button::new(icon::pencil_icon())
                            .style(theme::button::transparent_border)
                            .on_press(SettingsEditMessage::Select)
                    } else {
                        Button::new(icon::pencil_icon()).style(theme::button::transparent_border)
                    })
                    .align_y(Alignment::Center),
            )
            .push(separation().width(Length::Fill))
            .push(col.push(col_fields))
            .spacing(20),
    ))
    .width(Length::Fill)
    .into()
}

pub fn is_running_label<'a, T: 'a>(is_running: Option<bool>) -> Container<'a, T> {
    if let Some(running) = is_running {
        if running {
            Container::new(
                Row::new()
                    .push(icon::dot_icon().size(5).style(theme::text::success))
                    .push(text("Running").small().style(theme::text::success))
                    .align_y(Alignment::Center),
            )
        } else {
            Container::new(
                Row::new()
                    .push(icon::dot_icon().size(5).style(theme::text::error))
                    .push(text("Not running").small().style(theme::text::error))
                    .align_y(Alignment::Center),
            )
        }
    } else {
        Container::new(Column::new())
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
    card::simple(Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(badge::badge(icon::block_icon()))
                    .push(text("Blockchain rescan").bold().width(Length::Fill))
                    .push_maybe(if success {
                        Some(
                            text("Successfully rescanned the blockchain")
                                .style(theme::text::success),
                        )
                    } else {
                        None
                    })
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
            )
            .push(separation().width(Length::Fill))
            .push(if let Some(p) = scan_progress {
                Container::new(
                    Column::new()
                        .width(Length::Fill)
                        .push(ProgressBar::new(0.0..=1.0, p as f32).width(Length::Fill))
                        .push(text(format!("Rescanning...{:.2}%", p * 100.0))),
                )
            } else {
                Container::new(
                    Column::new()
                        .spacing(10)
                        .push(
                            Row::new()
                                .push(text("Year:").bold().small())
                                .push(
                                    form::Form::new_trimmed("2022", year, |value| {
                                        SettingsEditMessage::FieldEdited("rescan_year", value)
                                    })
                                    .size(P1_SIZE)
                                    .padding(5),
                                )
                                .push(text("Month:").bold().small())
                                .push(
                                    form::Form::new_trimmed("12", month, |value| {
                                        SettingsEditMessage::FieldEdited("rescan_month", value)
                                    })
                                    .size(P1_SIZE)
                                    .padding(5),
                                )
                                .push(text("Day:").bold().small())
                                .push(
                                    form::Form::new_trimmed("31", day, |value| {
                                        SettingsEditMessage::FieldEdited("rescan_day", value)
                                    })
                                    .size(P1_SIZE)
                                    .padding(5),
                                )
                                .align_y(Alignment::Center)
                                .spacing(10),
                        )
                        .push_maybe(if invalid_date {
                            Some(p1_regular("Provided date is invalid").style(theme::text::error))
                        } else {
                            None
                        })
                        .push_maybe(if past_possible_height {
                            Some(
                                p1_regular("Provided date earlier than the node prune height")
                                    .style(theme::text::error),
                            )
                        } else {
                            None
                        })
                        .push_maybe(if future_date {
                            Some(
                                p1_regular("Provided date is in the future")
                                    .style(theme::text::error),
                            )
                        } else {
                            None
                        })
                        .push(
                            if can_edit
                                && !invalid_date
                                && !processing
                                && (is_ok_and(&u32::from_str(&year.value), |&v| v > 0)
                                    && is_ok_and(&u32::from_str(&month.value), |&v| {
                                        v > 0 && v <= 12
                                    })
                                    && is_ok_and(&u32::from_str(&day.value), |&v| v > 0 && v <= 31))
                            {
                                Row::new().push(Column::new().width(Length::Fill)).push(
                                    button::primary(None, "Start rescan")
                                        .on_press(SettingsEditMessage::Confirm)
                                        .width(Length::Shrink),
                                )
                            } else if processing {
                                Row::new().push(Column::new().width(Length::Fill)).push(
                                    button::secondary(None, "Starting rescan...")
                                        .width(Length::Shrink),
                                )
                            } else {
                                Row::new().push(Column::new().width(Length::Fill)).push(
                                    button::secondary(None, "Start rescan").width(Length::Shrink),
                                )
                            },
                        ),
                )
            })
            .spacing(20),
    ))
    .width(Length::Fill)
    .into()
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
    warning: Option<&Error>,
    descriptor: &'a LianaDescriptor,
    wallet_alias: &'a form::Value<String>,
    keys_aliases: &'a [(Fingerprint, form::Value<String>)],
    provider_keys: &'a HashMap<Fingerprint, ProviderKey>,
    processing: bool,
    updated: bool,
) -> Element<'a, Message> {
    let header = header("Wallet", SettingsMessage::EditWalletSettings);

    let import_export = Row::new()
        .push(
            button::secondary(Some(icon::backup_icon()), "Backup")
                .on_press(Message::Settings(SettingsMessage::ExportWallet)),
        )
        .push(Space::with_width(10))
        .push(
            button::secondary(Some(icon::restore_icon()), "Restore")
                .on_press(Message::Settings(SettingsMessage::ImportWallet)),
        )
        .push(Space::with_width(Length::Fill));

    let descr = card::simple(
        Column::new()
            .push(text("Wallet descriptor:").bold())
            .push(
                scrollable(
                    Column::new()
                        .push(text(descriptor.to_string()).small())
                        .push(Space::with_height(Length::Fixed(5.0))),
                )
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(5).scroller_width(5),
                )),
            )
            .push(
                Row::new()
                    .spacing(10)
                    .push(Column::new().width(Length::Fill))
                    .push(
                        button::secondary(Some(icon::clipboard_icon()), "Copy")
                            .on_press(Message::Clipboard(descriptor.to_string())),
                    )
                    .push(
                        button::secondary(Some(icon::chip_icon()), "Register on hardware device")
                            .on_press(Message::Settings(SettingsMessage::RegisterWallet)),
                    ),
            )
            .spacing(10),
    )
    .width(Length::Fill);

    let aliases = card::simple(
        Column::new()
            .push(text("Wallet alias:").bold())
            .push(
                form::Form::new("Alias", wallet_alias, move |msg| {
                    Message::Settings(SettingsMessage::WalletAliasEdited(msg))
                })
                .warning("Please enter alias that is not too long")
                .size(P1_SIZE)
                .padding(10),
            )
            .push(text("Fingerprint aliases:").bold())
            .push(keys_aliases.iter().fold(
                Column::new().spacing(10),
                |col, (fingerprint, name)| {
                    let fg = *fingerprint;
                    col.push(
                        Row::new()
                            .spacing(10)
                            .align_y(Alignment::Center)
                            .push(text(fg.to_string()).bold().width(Length::Fixed(100.0)))
                            .push(
                                form::Form::new("Alias", name, move |msg| {
                                    Message::Settings(SettingsMessage::FingerprintAliasEdited(
                                        fg, msg,
                                    ))
                                })
                                .warning("Please enter correct alias")
                                .size(P1_SIZE)
                                .padding(10),
                            ),
                    )
                },
            ))
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(Space::with_width(Length::Fill))
                    .push_maybe(if updated {
                        Some(
                            Row::new()
                                .align_y(Alignment::Center)
                                .push(icon::circle_check_icon().style(theme::text::success))
                                .push(text("Updated").style(theme::text::success)),
                        )
                    } else {
                        None
                    })
                    .push(if !processing && wallet_alias.valid {
                        button::secondary(None, "Update")
                            .on_press(Message::Settings(SettingsMessage::Save))
                    } else {
                        button::secondary(None, "Updating")
                    }),
            )
            .spacing(10),
    )
    .width(Length::Fill);

    dashboard(
        &Menu::Settings,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(header)
            .push(import_export)
            .push(descr)
            .push(
                card::simple(display_policy(
                    descriptor.policy(),
                    keys_aliases,
                    provider_keys,
                ))
                .width(Length::Fill),
            )
            .push(aliases),
    )
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

    let mut col = Column::new().push(
        Row::new()
            .spacing(5)
            .push(
                text(format!(
                    "{} signature{}",
                    primary_threshold,
                    if primary_threshold > 1 { "s" } else { "" }
                ))
                .bold(),
            )
            .push(if primary_keys.len() > 1 {
                text(format!("out of {} by", primary_keys.len()))
            } else {
                text("by")
            })
            .push(
                primary_keys
                    .iter()
                    .enumerate()
                    .fold(Row::new().spacing(5), |row, (i, k)| {
                        let content = if let Some(alias) = keys_aliases
                            .iter()
                            .find(|(fg, a)| fg == k && !a.value.is_empty())
                            .map(|(_, f)| &f.value)
                        {
                            Container::new(
                                iced_tooltip::Tooltip::new(
                                    text(alias).bold(),
                                    text(k.to_string()),
                                    iced_tooltip::Position::Bottom,
                                )
                                .style(theme::card::simple),
                            )
                        } else {
                            Container::new(text(format!("[{}]", k)).bold())
                        };
                        if primary_keys.len() == 1 || i == primary_keys.len() - 1 {
                            row.push(content)
                        } else if i <= primary_keys.len() - 2 {
                            row.push(content).push(text("and"))
                        } else {
                            row.push(content).push(text(","))
                        }
                    }),
            )
            .push(text("can always spend this wallet's funds (Primary path)")),
    );
    for (i, (sequence, recovery_path)) in recovery_paths.iter().enumerate() {
        let (threshold, recovery_keys) = recovery_path.thresh_origins();

        // The iteration over an HashMap keys can have a different order at each refresh
        let mut recovery_keys: Vec<Fingerprint> = recovery_keys.into_keys().collect();
        recovery_keys.sort();

        col = col.push(
            Row::new()
                .spacing(5)
                .push(
                    text(format!(
                        "{} signature{}",
                        threshold,
                        if threshold > 1 { "s" } else { "" }
                    ))
                    .bold(),
                )
                .push(if recovery_keys.len() > 1 {
                    text(format!("out of {} by", recovery_keys.len()))
                } else {
                    text("by")
                })
                .push(recovery_keys.iter().enumerate().fold(
                    Row::new().spacing(5),
                    |row, (i, k)| {
                        let content = if let Some(alias) = keys_aliases
                            .iter()
                            .find(|(fg, a)| fg == k && !a.value.is_empty())
                            .map(|(_, f)| &f.value)
                        {
                            Container::new(
                                iced_tooltip::Tooltip::new(
                                    text(alias).bold(),
                                    text(k.to_string()),
                                    iced_tooltip::Position::Bottom,
                                )
                                .style(theme::card::simple),
                            )
                        } else {
                            Container::new(text(format!("[{}]", k)).bold())
                        };
                        if recovery_keys.len() == 1 || i == recovery_keys.len() - 1 {
                            row.push(content)
                        } else if i <= recovery_keys.len() - 2 {
                            row.push(content).push(text("and"))
                        } else {
                            row.push(content).push(text(","))
                        }
                    },
                ))
                .push(text("can spend coins inactive for"))
                .push(
                    text(format!(
                        "{} blocks (~{})",
                        sequence,
                        expire_message_units(*sequence as u32).join(",")
                    ))
                    .bold(),
                )
                .push(text(
                    // If max timelock and all keys are from provider, then it's a safety net path.
                    if *sequence == u16::MAX
                        && recovery_keys
                            .iter()
                            .all(|fg| provider_keys.contains_key(fg))
                    {
                        "(Safety Net path)".to_string()
                    } else {
                        format!("(Recovery path #{})", i + 1)
                    },
                )),
        );
    }
    Column::new()
        .spacing(10)
        .push(text("The wallet policy:").bold())
        .push(scrollable(col).direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new().width(5).scroller_width(5),
        )))
        .into()
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
        [(n_years, "y"), (n_months, "m"), (n_days, "d")]
            .iter()
            .filter_map(|(n, u)| {
                if *n != 0 {
                    Some(format!("{}{}", n, u))
                } else {
                    None
                }
            })
            .collect()
    } else {
        n_minutes -= n_days * 1440;
        let n_hours = n_minutes / 60;
        n_minutes -= n_hours * 60;
        [(n_hours, "h"), (n_minutes, "m")]
            .iter()
            .filter_map(|(n, u)| {
                if *n != 0 {
                    Some(format!("{}{}", n, u))
                } else {
                    None
                }
            })
            .collect()
    }
}

pub fn register_wallet_modal<'a>(
    warning: Option<&Error>,
    hws: &'a [HardwareWallet],
    processing: bool,
    chosen_hw: Option<usize>,
    registered: &HashSet<Fingerprint>,
) -> Element<'a, Message> {
    Column::new()
        .push_maybe(warning.map(|w| warn(Some(w))))
        .push(card::simple(
            Column::new()
                .push(
                    Column::new()
                        .push(text("Select device:").bold().width(Length::Fill))
                        .spacing(10)
                        .push(hws.iter().enumerate().fold(
                            Column::new().spacing(10),
                            |col, (i, hw)| {
                                col.push(hw::hw_list_view_for_registration(
                                    i,
                                    hw,
                                    Some(i) == chosen_hw,
                                    processing,
                                    hw.fingerprint()
                                        .map(|f| registered.contains(&f))
                                        .unwrap_or(false)
                                        || if let HardwareWallet::Supported { registered, .. } = hw
                                        {
                                            registered == &Some(true)
                                        } else {
                                            false
                                        },
                                ))
                            },
                        ))
                        .width(Length::Fill),
                )
                .spacing(20)
                .width(Length::Fill)
                .align_x(Alignment::Center),
        ))
        .width(Length::Fixed(500.0))
        .into()
}
