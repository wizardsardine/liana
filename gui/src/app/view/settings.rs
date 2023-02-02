use std::str::FromStr;

use iced::{
    alignment,
    widget::{self, Column, Container, ProgressBar, Row, Space},
    Alignment, Element, Length,
};

use liana::miniscript::bitcoin;

use super::{
    dashboard,
    message::{Message, SettingsMessage},
};

use crate::{
    app::{cache::Cache, error::Error, menu::Menu},
    ui::{
        color,
        component::{badge, button, card, form, separation, text::*},
        icon,
        util::Collection,
    },
};

pub fn list<'a>(
    lianad_version: Option<&'a String>,
    cache: &'a Cache,
    warning: Option<&Error>,
    settings: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    dashboard(
        &Menu::Settings,
        cache,
        warning,
        widget::Column::with_children(settings)
            .spacing(20)
            .push(card::simple(
                Column::new()
                    .push(
                        Row::new()
                            .push(badge::Badge::new(icon::recovery_icon()))
                            .push(text("Recovery").bold())
                            .padding(10)
                            .spacing(20)
                            .align_items(Alignment::Center)
                            .width(Length::Fill),
                    )
                    .push(separation().width(Length::Fill))
                    .push(Space::with_height(Length::Units(10)))
                    .push(text("In case of loss of the main key, the recovery key can move the funds after a certain time."))
                    .push(Space::with_height(Length::Units(10)))
                    .push(
                        Row::new()
                            .push(Space::with_width(Length::Fill))
                            .push(button::primary(None, "Recover funds").on_press(Message::Menu(Menu::Recovery))),
                    ),
            ))
            .push(
                card::simple(
                    Column::new()
                        .push(
                            Row::new()
                                .push(badge::Badge::new(icon::tooltip_icon()))
                                .push(text("About").bold())
                                .padding(10)
                                .spacing(20)
                                .align_items(Alignment::Center)
                                .width(Length::Fill),
                        )
                        .push(separation().width(Length::Fill))
                        .push(Space::with_height(Length::Units(10)))
                        .push(
                            Row::new().push(Space::with_width(Length::Fill)).push(Column::new()
                                .push(text(format!("liana-gui v{}", crate::VERSION)))
                                .push_maybe(lianad_version.map(|version| text(format!("lianad v{}", version)))))
                        )
                ).width(Length::Fill)
            )
    )
}

pub fn bitcoind_edit<'a>(
    network: bitcoin::Network,
    blockheight: i32,
    addr: &form::Value<String>,
    cookie_path: &form::Value<String>,
    processing: bool,
) -> Element<'a, SettingsMessage> {
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
                        SettingsMessage::FieldEdited("cookie_file_path", value)
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
                        SettingsMessage::FieldEdited("socket_address", value)
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
        cancel_button = cancel_button.on_press(SettingsMessage::CancelEdit);
        confirm_button = confirm_button.on_press(SettingsMessage::ConfirmEdit);
    }

    card::simple(Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(badge::Badge::new(icon::bitcoin_icon()))
                    .push(text("Bitcoind").bold())
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
    network: bitcoin::Network,
    config: &liana::config::BitcoindConfig,
    blockheight: i32,
    is_running: Option<bool>,
    can_edit: bool,
) -> Element<'a, SettingsMessage> {
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
                            .push(text("Bitcoind").bold())
                            .push(is_running_label(is_running))
                            .spacing(20)
                            .align_items(Alignment::Center)
                            .width(Length::Fill),
                    )
                    .push(if can_edit {
                        widget::Button::new(icon::pencil_icon())
                            .style(button::Style::TransparentBorder.into())
                            .on_press(SettingsMessage::Edit)
                    } else {
                        widget::Button::new(icon::pencil_icon())
                            .style(button::Style::TransparentBorder.into())
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

pub fn is_running_label<'a, T: 'a>(is_running: Option<bool>) -> widget::Container<'a, T> {
    if let Some(running) = is_running {
        if running {
            Container::new(
                Row::new()
                    .push(icon::dot_icon().size(5).style(color::SUCCESS))
                    .push(text("Running").small().style(color::SUCCESS))
                    .align_items(Alignment::Center),
            )
        } else {
            Container::new(
                Row::new()
                    .push(icon::dot_icon().size(5).style(color::ALERT))
                    .push(text("Not running").small().style(color::ALERT))
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
) -> Element<'a, SettingsMessage> {
    card::simple(Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(badge::Badge::new(icon::block_icon()))
                    .push(text("Rescan blockchain").bold().width(Length::Fill))
                    .push_maybe(if success {
                        Some(text("Rescan was successful").style(color::SUCCESS))
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
                                        SettingsMessage::FieldEdited("rescan_year", value)
                                    })
                                    .size(20)
                                    .padding(5),
                                )
                                .push(text("Month:").bold().small())
                                .push(
                                    form::Form::new("12", month, |value| {
                                        SettingsMessage::FieldEdited("rescan_month", value)
                                    })
                                    .size(20)
                                    .padding(5),
                                )
                                .push(text("Day:").bold().small())
                                .push(
                                    form::Form::new("31", day, |value| {
                                        SettingsMessage::FieldEdited("rescan_day", value)
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
                                        .on_press(SettingsMessage::ConfirmEdit)
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
