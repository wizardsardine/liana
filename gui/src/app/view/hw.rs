use iced::{widget::tooltip, Alignment, Length};

use liana_ui::{
    color,
    component::text::{text, Text},
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{app::view::message::*, hw::HardwareWallet};

pub fn hw_list_view(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
    processing: bool,
    signed: bool,
) -> Element<Message> {
    let mut bttn = Button::new(
        Row::new()
            .push(
                Column::new()
                    .push(text(format!("{}", hw.kind())).bold())
                    .push(match hw {
                        HardwareWallet::Supported {
                            fingerprint,
                            version,
                            registered,
                            ..
                        } => Row::new()
                            .align_items(Alignment::Center)
                            .spacing(5)
                            .push(text(format!("fingerprint: {}", fingerprint)).small())
                            .push_maybe(
                                version
                                    .as_ref()
                                    .map(|v| text(format!("version: {}", v)).small()),
                            )
                            .push_maybe(registered.and_then(|registered|
                                    if !registered {
                                        Some(Row::new()
                                            .spacing(5)
                                            .align_items(Alignment::Center)
                                            .push(text("unregistered").small())
                                            .push(
                                                tooltip::Tooltip::new(
                                                    icon::warning_icon(),
                                                    "Policy is not registered on the device.\n You can register it in the settings.",
                                                    tooltip::Position::Bottom,
                                                ).style(theme::Container::Card(theme::Card::Simple))))
                                    } else {
                                        None
                                    })
                            ),
                        HardwareWallet::Unsupported {
                            version, message, ..
                        } => Row::new()
                            .spacing(5)
                            .push_maybe(
                                version
                                    .as_ref()
                                    .map(|v| text(format!("version: {}", v)).small()),
                            )
                            .push(
                                tooltip::Tooltip::new(
                                    icon::warning_icon(),
                                    message,
                                    tooltip::Position::Bottom,
                                )
                                .style(theme::Container::Card(theme::Card::Simple)),
                            ),
                    })
                    .spacing(5)
                    .width(Length::Fill),
            )
            .push_maybe(if chosen && processing {
                Some(
                    Column::new()
                        .push(text("Processing..."))
                        .push(text("Please check your device").small()),
                )
            } else {
                None
            })
            .push_maybe(if signed {
                Some(
                    Row::new()
                        .align_items(Alignment::Center)
                        .spacing(5)
                        .push(icon::circle_check_icon().style(color::legacy::SUCCESS))
                        .push(text("Signed").style(color::legacy::SUCCESS)),
                )
            } else {
                None
            })
            .align_items(Alignment::Center)
            .width(Length::Fill),
    )
    .padding(10)
    .style(theme::Button::Secondary)
    .width(Length::Fill);
    if !processing {
        if let HardwareWallet::Supported { registered, .. } = hw {
            if *registered != Some(false) {
                bttn = bttn.on_press(Message::SelectHardwareWallet(i));
            }
        }
    }
    Container::new(bttn)
        .width(Length::Fill)
        .style(theme::Container::Card(theme::Card::Simple))
        .into()
}

pub fn hw_list_view_for_registration(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
    processing: bool,
    registered: bool,
) -> Element<Message> {
    let mut bttn = Button::new(
        Row::new()
            .push(
                Column::new()
                    .push(text(format!("{}", hw.kind())).bold())
                    .push(match hw {
                        HardwareWallet::Supported {
                            fingerprint,
                            version,
                            ..
                        } => Row::new()
                            .spacing(5)
                            .push(text(format!("fingerprint: {}", fingerprint)).small())
                            .push_maybe(
                                version
                                    .as_ref()
                                    .map(|v| text(format!("version: {}", v)).small()),
                            ),
                        HardwareWallet::Unsupported {
                            version, message, ..
                        } => Row::new()
                            .spacing(5)
                            .push_maybe(
                                version
                                    .as_ref()
                                    .map(|v| text(format!("version: {}", v)).small()),
                            )
                            .push(
                                tooltip::Tooltip::new(
                                    icon::warning_icon(),
                                    message,
                                    tooltip::Position::Bottom,
                                )
                                .style(theme::Container::Card(theme::Card::Simple)),
                            ),
                    })
                    .spacing(5)
                    .width(Length::Fill),
            )
            .push_maybe(if chosen && processing {
                Some(
                    Column::new()
                        .push(text("Processing..."))
                        .push(text("Please check your device").small()),
                )
            } else {
                None
            })
            .push_maybe(if registered {
                Some(
                    Row::new()
                        .align_items(Alignment::Center)
                        .spacing(5)
                        .push(icon::circle_check_icon().style(color::legacy::SUCCESS))
                        .push(text("Registered").style(color::legacy::SUCCESS)),
                )
            } else {
                None
            })
            .align_items(Alignment::Center)
            .width(Length::Fill),
    )
    .padding(10)
    .style(theme::Button::Secondary)
    .width(Length::Fill);
    if !processing && hw.is_supported() {
        bttn = bttn.on_press(Message::SelectHardwareWallet(i));
    }
    Container::new(bttn)
        .width(Length::Fill)
        .style(theme::Container::Card(theme::Card::Simple))
        .into()
}
