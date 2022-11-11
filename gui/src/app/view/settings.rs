use std::str::FromStr;

use iced::{
    alignment,
    pure::{column, container, progress_bar, row, widget, Element},
    Alignment, Length,
};

use minisafe::miniscript::bitcoin;

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
    cache: &'a Cache,
    warning: Option<&Error>,
    settings: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    dashboard(
        &Menu::Settings,
        cache,
        warning,
        widget::Column::with_children(settings).spacing(20),
    )
}

pub fn bitcoind_edit<'a>(
    network: bitcoin::Network,
    blockheight: i32,
    addr: &form::Value<String>,
    cookie_path: &form::Value<String>,
    processing: bool,
) -> Element<'a, SettingsMessage> {
    let mut col = column().spacing(20);
    if blockheight != 0 {
        col = col
            .push(
                row()
                    .push(
                        row()
                            .push(badge::Badge::new(icon::network_icon()))
                            .push(
                                column()
                                    .push(text("Network:"))
                                    .push(text(&network.to_string()).bold()),
                            )
                            .spacing(10)
                            .width(Length::FillPortion(1)),
                    )
                    .push(
                        row()
                            .push(badge::Badge::new(icon::block_icon()))
                            .push(
                                column()
                                    .push(text("Block Height:"))
                                    .push(text(&blockheight.to_string()).bold()),
                            )
                            .spacing(10)
                            .width(Length::FillPortion(1)),
                    ),
            )
            .push(separation().width(Length::Fill));
    }

    col = col
        .push(
            column()
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
            column()
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

    card::simple(container(
        column()
            .push(
                row()
                    .push(badge::Badge::new(icon::bitcoin_icon()))
                    .push(text("Bitcoind"))
                    .padding(10)
                    .spacing(20)
                    .align_items(Alignment::Center)
                    .width(Length::Fill),
            )
            .push(separation().width(Length::Fill))
            .push(col)
            .push(
                container(
                    row()
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
    config: &minisafe::config::BitcoindConfig,
    blockheight: i32,
    is_running: Option<bool>,
    can_edit: bool,
) -> Element<'a, SettingsMessage> {
    let mut col = column().spacing(20);
    if blockheight != 0 {
        col = col
            .push(
                row()
                    .push(
                        row()
                            .push(badge::Badge::new(icon::network_icon()))
                            .push(
                                column()
                                    .push(text("Network:"))
                                    .push(text(&network.to_string()).bold()),
                            )
                            .spacing(10)
                            .width(Length::FillPortion(1)),
                    )
                    .push(
                        row()
                            .push(badge::Badge::new(icon::block_icon()))
                            .push(
                                column()
                                    .push(text("Block Height:"))
                                    .push(text(&blockheight.to_string()).bold()),
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

    let mut col_fields = column();
    for (k, v) in rows {
        col_fields = col_fields.push(
            row()
                .push(container(text(k).bold().small()).width(Length::Fill))
                .push(text(&v).small()),
        );
    }

    card::simple(container(
        column()
            .push(
                row()
                    .push(
                        row()
                            .push(badge::Badge::new(icon::bitcoin_icon()))
                            .push(text("Bitcoind"))
                            .push(is_running_label(is_running))
                            .spacing(20)
                            .align_items(Alignment::Center)
                            .width(Length::Fill),
                    )
                    .push(if can_edit {
                        widget::Button::new(icon::pencil_icon())
                            .style(button::Style::TransparentBorder)
                            .on_press(SettingsMessage::Edit)
                    } else {
                        widget::Button::new(icon::pencil_icon())
                            .style(button::Style::TransparentBorder)
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
            container(
                row()
                    .push(icon::dot_icon().size(5).color(color::SUCCESS))
                    .push(text("Running").small().color(color::SUCCESS))
                    .align_items(Alignment::Center),
            )
        } else {
            container(
                row()
                    .push(icon::dot_icon().size(5).color(color::ALERT))
                    .push(text("Not running").small().color(color::ALERT))
                    .align_items(Alignment::Center),
            )
        }
    } else {
        container(column())
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
    card::simple(container(
        column()
            .push(
                row()
                    .push(badge::Badge::new(icon::block_icon()))
                    .push(text("Rescan blockchain").width(Length::Fill))
                    .push_maybe(if success {
                        Some(text("Rescan was successful").color(color::SUCCESS))
                    } else {
                        None
                    })
                    .spacing(20)
                    .align_items(Alignment::Center)
                    .width(Length::Fill),
            )
            .push(separation().width(Length::Fill))
            .push(if let Some(p) = scan_progress {
                container(
                    column()
                        .width(Length::Fill)
                        .push(progress_bar(0.0..=1.0, p as f32).width(Length::Fill))
                        .push(text(&format!("Rescan...{:.2}%", p * 100.0))),
                )
            } else {
                container(
                    column()
                        .spacing(10)
                        .push(
                            row()
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
                                row().push(column().width(Length::Fill)).push(
                                    button::primary(None, "Start rescan")
                                        .on_press(SettingsMessage::ConfirmEdit)
                                        .width(Length::Shrink),
                                )
                            } else if processing {
                                row().push(column().width(Length::Fill)).push(
                                    button::primary(None, "Starting rescan...")
                                        .width(Length::Shrink),
                                )
                            } else {
                                row().push(column().width(Length::Fill)).push(
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
