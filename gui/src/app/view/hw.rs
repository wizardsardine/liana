use iced::{widget::tooltip, Alignment, Length};

use liana_ui::{
    color,
    component::text::{text, Text},
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{app::view::message::*, hw::HardwareWallet};

pub fn hw_list_view<'a>(
    i: usize,
    hw: &'a HardwareWallet,
    chosen: bool,
    processing: bool,
    status: Option<&'a str>,
) -> Element<'a, Message> {
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
            .push_maybe(status.map(|v| {
                Row::new()
                    .align_items(Alignment::Center)
                    .spacing(5)
                    .push(icon::circle_check_icon().style(color::legacy::SUCCESS))
                    .push(text(v).style(color::legacy::SUCCESS))
            }))
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
