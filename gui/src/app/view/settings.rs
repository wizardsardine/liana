use std::collections::HashSet;
use std::str::FromStr;

use iced::{
    alignment,
    widget::{radio, scrollable, Space},
    Alignment, Length,
};

use liana::{
    config::BitcoindRpcAuth,
    miniscript::bitcoin::{bip32::Fingerprint, Network},
};

use super::{dashboard, message::*};

use liana_ui::{
    color,
    component::{badge, button, card, form, separation, text::*, tooltip::tooltip},
    icon, theme,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{hw, warning::warn},
    },
    bitcoind::{RpcAuthType, RpcAuthValues},
    hw::HardwareWallet,
};

pub fn list(cache: &Cache, is_remote_backend: bool) -> Element<Message> {
    dashboard(
        &Menu::Settings,
        cache,
        None,
        Column::new()
            .spacing(20)
            .width(Length::Fill)
            .push(
                Button::new(text("Settings").size(30).bold())
                    .style(theme::Button::Transparent)
                    .on_press(Message::Menu(Menu::Settings)))
            .push_maybe(
                if !is_remote_backend {
                    Some(Container::new(
                        Button::new(
                            Row::new()
                                .push(badge::Badge::new(icon::bitcoin_icon()))
                                .push(text("Bitcoin Core").bold())
                                .padding(10)
                                .spacing(20)
                                .align_items(Alignment::Center)
                                .width(Length::Fill),
                        )
                        .width(Length::Fill)
                        .style(theme::Button::TransparentBorder)
                        .on_press(Message::Settings(SettingsMessage::EditBitcoindSettings))
                    )
                    .width(Length::Fill)
                    .style(theme::Container::Card(theme::Card::Simple)))
                } else {
                    None
                }
            )
            .push(
                Container::new(
                    Button::new(
                        Row::new()
                            .push(badge::Badge::new(icon::wallet_icon()))
                            .push(text("Wallet").bold())
                            .padding(10)
                            .spacing(20)
                            .align_items(Alignment::Center)
                            .width(Length::Fill),
                    )
                    .width(Length::Fill)
                    .style(theme::Button::TransparentBorder)
                    .on_press(Message::Settings(SettingsMessage::EditWalletSettings))
                )
                .width(Length::Fill)
                .style(theme::Container::Card(theme::Card::Simple))
            )
            .push(
                Container::new(
                    Button::new(
                        Row::new()
                            .push(badge::Badge::new(icon::recovery_icon()))
                            .push(text("Recovery").bold())
                            .push(tooltip("In case of loss of the main key, the recovery key can move the funds after a certain time."))
                            .padding(10)
                            .spacing(20)
                            .align_items(Alignment::Center)
                            .width(Length::Fill),
                    )
                    .width(Length::Fill)
                    .style(theme::Button::TransparentBorder)
                    .on_press(Message::Menu(Menu::Recovery))
                )
                .width(Length::Fill)
                .style(theme::Container::Card(theme::Card::Simple))
            )
            .push(
                Container::new(
                    Button::new(
                        Row::new()
                            .push(badge::Badge::new(icon::tooltip_icon()))
                            .push(text("About").bold())
                            .padding(10)
                            .spacing(20)
                            .align_items(Alignment::Center)
                            .width(Length::Fill),
                    )
                    .width(Length::Fill)
                    .style(theme::Button::TransparentBorder)
                    .on_press(Message::Settings(SettingsMessage::AboutSection))
                )
                .width(Length::Fill)
                .style(theme::Container::Card(theme::Card::Simple))
            )
    )
}
pub fn bitcoind_settings<'a>(
    cache: &'a Cache,
    warning: Option<&Error>,
    settings: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    dashboard(
        &Menu::Settings,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(
                Row::new()
                    .spacing(10)
                    .align_items(Alignment::Center)
                    .push(
                        Button::new(text("Settings").size(30).bold())
                            .style(theme::Button::Transparent)
                            .on_press(Message::Menu(Menu::Settings)),
                    )
                    .push(icon::chevron_right().size(30))
                    .push(
                        Button::new(text("Bitcoin Core").size(30).bold())
                            .style(theme::Button::Transparent)
                            .on_press(Message::Settings(SettingsMessage::EditBitcoindSettings)),
                    ),
            )
            .push(Column::with_children(settings).spacing(20)),
    )
}

pub fn about_section<'a>(
    cache: &'a Cache,
    warning: Option<&Error>,
    lianad_version: Option<&String>,
) -> Element<'a, Message> {
    dashboard(
        &Menu::Settings,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(
                Row::new()
                    .spacing(10)
                    .align_items(Alignment::Center)
                    .push(
                        Button::new(text("Settings").size(30).bold())
                            .style(theme::Button::Transparent)
                            .on_press(Message::Menu(Menu::Settings)),
                    )
                    .push(icon::chevron_right().size(30))
                    .push(
                        Button::new(text("About").size(30).bold())
                            .style(theme::Button::Transparent)
                            .on_press(Message::Settings(SettingsMessage::AboutSection)),
                    ),
            )
            .push(
                card::simple(
                    Column::new()
                        .push(
                            Row::new()
                                .push(badge::Badge::new(icon::tooltip_icon()))
                                .push(text("Version").bold())
                                .padding(10)
                                .spacing(20)
                                .align_items(Alignment::Center)
                                .width(Length::Fill),
                        )
                        .push(separation().width(Length::Fill))
                        .push(Space::with_height(Length::Fixed(10.0)))
                        .push(
                            Row::new().push(Space::with_width(Length::Fill)).push(
                                Column::new()
                                    .push(text(format!("liana-gui v{}", crate::VERSION)))
                                    .push_maybe(
                                        lianad_version
                                            .map(|version| text(format!("lianad v{}", version))),
                                    ),
                            ),
                        ),
                )
                .width(Length::Fill),
            ),
    )
}

pub fn bitcoind_edit<'a>(
    network: Network,
    blockheight: i32,
    addr: &form::Value<String>,
    rpc_auth_vals: &RpcAuthValues,
    selected_auth_type: &RpcAuthType,
    processing: bool,
) -> Element<'a, SettingsEditMessage> {
    let mut col = Column::new().spacing(20);
    if blockheight != 0 {
        col = col
            .push(
                Row::new()
                    .push(
                        Row::new()
                            .push(badge::Badge::new(icon::network_icon()))
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
                            .push(badge::Badge::new(icon::block_icon()))
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
                        .align_items(Alignment::Center)
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
    let mut confirm_button = button::primary(None, " Save ").padding(5);
    if !processing {
        cancel_button = cancel_button.on_press(SettingsEditMessage::Cancel);
        confirm_button = confirm_button.on_press(SettingsEditMessage::Confirm);
    }

    card::simple(Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(badge::Badge::new(icon::bitcoin_icon()))
                    .push(text("Bitcoin Core").bold())
                    .padding(10)
                    .spacing(20)
                    .align_items(Alignment::Center)
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
                        .align_items(Alignment::Center),
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
    network: Network,
    config: &liana::config::BitcoindConfig,
    blockheight: i32,
    is_running: Option<bool>,
    can_edit: bool,
) -> Element<'a, SettingsEditMessage> {
    let mut col = Column::new().spacing(20);
    if blockheight != 0 {
        col = col
            .push(
                Row::new()
                    .push(
                        Row::new()
                            .push(badge::Badge::new(icon::network_icon()))
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
                            .push(badge::Badge::new(icon::block_icon()))
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

    let mut col_fields = Column::new();
    for (k, v) in rows {
        col_fields = col_fields.push(
            Row::new()
                .push(Container::new(text(k).bold().small()).width(Length::Fill))
                .push(text(v).small()),
        );
    }

    card::simple(Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(
                        Row::new()
                            .push(badge::Badge::new(icon::bitcoin_icon()))
                            .push(text("Bitcoin Core").bold())
                            .push(is_running_label(is_running))
                            .spacing(20)
                            .align_items(Alignment::Center)
                            .width(Length::Fill),
                    )
                    .push(if can_edit {
                        Button::new(icon::pencil_icon())
                            .style(theme::Button::TransparentBorder)
                            .on_press(SettingsEditMessage::Select)
                    } else {
                        Button::new(icon::pencil_icon()).style(theme::Button::TransparentBorder)
                    })
                    .align_items(Alignment::Center),
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
                    .push(icon::dot_icon().size(5).style(color::GREEN))
                    .push(text("Running").small().style(color::GREEN))
                    .align_items(Alignment::Center),
            )
        } else {
            Container::new(
                Row::new()
                    .push(icon::dot_icon().size(5).style(color::RED))
                    .push(text("Not running").small().style(color::RED))
                    .align_items(Alignment::Center),
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
                    .push(badge::Badge::new(icon::block_icon()))
                    .push(text("Blockchain rescan").bold().width(Length::Fill))
                    .push_maybe(if success {
                        Some(text("Successfully rescanned the blockchain").style(color::GREEN))
                    } else {
                        None
                    })
                    .spacing(20)
                    .align_items(Alignment::Center)
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
                                .align_items(Alignment::Center)
                                .spacing(10),
                        )
                        .push_maybe(if invalid_date {
                            Some(p1_regular("Provided date is invalid").style(color::RED))
                        } else {
                            None
                        })
                        .push_maybe(if past_possible_height {
                            Some(
                                p1_regular("Provided date earlier than the node prune height")
                                    .style(color::RED),
                            )
                        } else {
                            None
                        })
                        .push_maybe(if future_date {
                            Some(p1_regular("Provided date is in the future").style(color::RED))
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
                                    button::primary(None, "Starting rescan...")
                                        .width(Length::Shrink),
                                )
                            } else {
                                Row::new().push(Column::new().width(Length::Fill)).push(
                                    button::primary(None, "Start rescan").width(Length::Shrink),
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

pub fn wallet_settings<'a>(
    cache: &'a Cache,
    warning: Option<&Error>,
    descriptor: &'a str,
    keys_aliases: &[(Fingerprint, form::Value<String>)],
    processing: bool,
    updated: bool,
) -> Element<'a, Message> {
    dashboard(
        &Menu::Settings,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(
                Row::new()
                    .spacing(10)
                    .align_items(Alignment::Center)
                    .push(
                        Button::new(text("Settings").size(30).bold())
                            .style(theme::Button::Transparent)
                            .on_press(Message::Menu(Menu::Settings)),
                    )
                    .push(icon::chevron_right().size(30))
                    .push(
                        Button::new(text("Wallet").size(30).bold())
                            .style(theme::Button::Transparent)
                            .on_press(Message::Settings(SettingsMessage::EditWalletSettings)),
                    ),
            )
            .push(card::simple(
                Column::new()
                    .push(text("Wallet descriptor:").bold())
                    .push(
                        scrollable(
                            Column::new()
                                .push(text(descriptor.to_owned()).small())
                                .push(Space::with_height(Length::Fixed(5.0))),
                        )
                        .direction(scrollable::Direction::Horizontal(
                            scrollable::Properties::new().width(5).scroller_width(5),
                        )),
                    )
                    .push(
                        Row::new()
                            .spacing(10)
                            .push(Column::new().width(Length::Fill))
                            .push(
                                button::secondary(Some(icon::clipboard_icon()), "Copy")
                                    .on_press(Message::Clipboard(descriptor.to_owned())),
                            )
                            .push(
                                button::primary(
                                    Some(icon::chip_icon()),
                                    "Register on hardware device",
                                )
                                .on_press(Message::Settings(SettingsMessage::RegisterWallet)),
                            ),
                    )
                    .spacing(10),
            ))
            .push(card::simple(
                Column::new()
                    .push(text("Fingerprint aliases:").bold())
                    .push(keys_aliases.iter().fold(
                        Column::new().spacing(10),
                        |col, (fingerprint, name)| {
                            let fg = *fingerprint;
                            col.push(
                                Row::new()
                                    .spacing(10)
                                    .align_items(Alignment::Center)
                                    .push(text(fg.to_string()).bold().width(Length::Fixed(100.0)))
                                    .push(
                                        form::Form::new("Alias", name, move |msg| {
                                            Message::Settings(
                                                SettingsMessage::FingerprintAliasEdited(fg, msg),
                                            )
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
                            .align_items(Alignment::Center)
                            .push(Space::with_width(Length::Fill))
                            .push_maybe(if updated {
                                Some(
                                    Row::new()
                                        .align_items(Alignment::Center)
                                        .push(icon::circle_check_icon().style(color::GREEN))
                                        .push(text("Updated").style(color::GREEN)),
                                )
                            } else {
                                None
                            })
                            .push(if !processing {
                                button::primary(None, "Update")
                                    .on_press(Message::Settings(SettingsMessage::Save))
                            } else {
                                button::primary(None, "Updating")
                            }),
                    )
                    .spacing(10),
            )),
    )
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
                .align_items(Alignment::Center),
        ))
        .width(Length::Fixed(500.0))
        .into()
}
