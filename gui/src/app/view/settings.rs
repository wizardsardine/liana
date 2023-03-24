use std::collections::HashSet;
use std::str::FromStr;

use iced::{alignment, widget::Space, Alignment, Length};

use liana::miniscript::bitcoin::{util::bip32::Fingerprint, Network};

use super::{dashboard, message::*};

use liana_ui::{
    color,
    component::{badge, button, card, form, separation, text::*, tooltip::tooltip},
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{hw, warning::warn},
    },
    hw::HardwareWallet,
};

pub fn list(cache: &Cache) -> Element<Message> {
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
            .push(
                Container::new(
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
                    .style(theme::Button::Secondary)
                    .on_press(Message::Settings(SettingsMessage::EditBitcoindSettings))
                )
                .width(Length::Fill)
                .style(theme::Container::Card(theme::Card::Simple))
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
                    .style(theme::Button::Secondary)
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
                    .style(theme::Button::Secondary)
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
                    .style(theme::Button::Secondary)
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
                        .push(Space::with_height(Length::Units(10)))
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
    cookie_path: &form::Value<String>,
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
            Column::new()
                .push(text("Cookie file path:").bold().small())
                .push(
                    form::Form::new("Cookie file path", cookie_path, |value| {
                        SettingsEditMessage::FieldEdited("cookie_file_path", value)
                    })
                    .warning("Please enter a valid filesystem path")
                    .size(20)
                    .padding(5),
                )
                .spacing(5),
        )
        .push(
            Column::new()
                .push(text("Socket address:").bold().small())
                .push(
                    form::Form::new("Socket address:", addr, |value| {
                        SettingsEditMessage::FieldEdited("socket_address", value)
                    })
                    .warning("Please enter a valid address")
                    .size(20)
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

    let rows = vec![
        (
            "Cookie file path:",
            config.cookie_path.to_str().unwrap().to_string(),
        ),
        ("Socket address:", config.addr.to_string()),
    ];

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
                    .push(icon::dot_icon().size(5).style(color::legacy::SUCCESS))
                    .push(text("Running").small().style(color::legacy::SUCCESS))
                    .align_items(Alignment::Center),
            )
        } else {
            Container::new(
                Row::new()
                    .push(icon::dot_icon().size(5).style(color::legacy::ALERT))
                    .push(text("Not running").small().style(color::legacy::ALERT))
                    .align_items(Alignment::Center),
            )
        }
    } else {
        Container::new(Column::new())
    }
}

pub fn rescan<'a>(
    year: &form::Value<String>,
    month: &form::Value<String>,
    day: &form::Value<String>,
    scan_progress: Option<f64>,
    success: bool,
    processing: bool,
    can_edit: bool,
) -> Element<'a, SettingsEditMessage> {
    card::simple(Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(badge::Badge::new(icon::block_icon()))
                    .push(text("Rescan blockchain").bold().width(Length::Fill))
                    .push_maybe(if success {
                        Some(text("Rescan was successful").style(color::legacy::SUCCESS))
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
                        .push(text(format!("Rescan...{:.2}%", p * 100.0))),
                )
            } else {
                Container::new(
                    Column::new()
                        .spacing(10)
                        .push(
                            Row::new()
                                .push(text("Year:").bold().small())
                                .push(
                                    form::Form::new("2022", year, |value| {
                                        SettingsEditMessage::FieldEdited("rescan_year", value)
                                    })
                                    .size(20)
                                    .padding(5),
                                )
                                .push(text("Month:").bold().small())
                                .push(
                                    form::Form::new("12", month, |value| {
                                        SettingsEditMessage::FieldEdited("rescan_month", value)
                                    })
                                    .size(20)
                                    .padding(5),
                                )
                                .push(text("Day:").bold().small())
                                .push(
                                    form::Form::new("31", day, |value| {
                                        SettingsEditMessage::FieldEdited("rescan_day", value)
                                    })
                                    .size(20)
                                    .padding(5),
                                )
                                .align_items(Alignment::Center)
                                .spacing(10),
                        )
                        .push(
                            if can_edit
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
                            .on_press(Message::Settings(SettingsMessage::AboutSection)),
                    ),
            )
            .push(card::simple(
                Column::new()
                    .push(text("Wallet descriptor:").bold())
                    .push(text(descriptor.to_owned()).small())
                    .push(
                        Row::new()
                            .spacing(10)
                            .push(Column::new().width(Length::Fill))
                            .push(
                                button::border(Some(icon::clipboard_icon()), "Copy")
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
                                    .push(text(fg.to_string()).bold().width(Length::Units(100)))
                                    .push(
                                        form::Form::new("Alias", name, move |msg| {
                                            Message::Settings(
                                                SettingsMessage::FingerprintAliasEdited(fg, msg),
                                            )
                                        })
                                        .warning("Please enter correct alias")
                                        .size(20)
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
                                        .push(
                                            icon::circle_check_icon().style(color::legacy::SUCCESS),
                                        )
                                        .push(text("Updated").style(color::legacy::SUCCESS)),
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
                        .push(
                            Row::new()
                                .push(text("Select device:").bold().width(Length::Fill))
                                .push(button::border(None, "Refresh").on_press(Message::Reload))
                                .align_items(Alignment::Center),
                        )
                        .spacing(10)
                        .push(hws.iter().enumerate().fold(
                            Column::new().spacing(10),
                            |col, (i, hw)| {
                                col.push(hw::hw_list_view(
                                    i,
                                    hw,
                                    Some(i) == chosen_hw,
                                    processing,
                                    hw.fingerprint().and_then(|f| {
                                        if registered.contains(&f) {
                                            Some("Registered")
                                        } else {
                                            None
                                        }
                                    }),
                                ))
                            },
                        ))
                        .width(Length::Fill),
                )
                .spacing(20)
                .width(Length::Fill)
                .align_items(Alignment::Center),
        ))
        .width(Length::Units(500))
        .into()
}
