use std::fmt::Display;

use crate::{
    color,
    component::{collapse, text},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{column, container, row},
    Alignment, Length,
};

pub fn warning<'a, T: 'a + Clone>(message: String, error: String) -> Container<'a, T> {
    let message_clone = message.clone();
    Container::new(Container::new(collapse::Collapse::new(
        move || {
            Button::new(
                Row::new()
                    .push(
                        Container::new(
                            text::p1_bold(message_clone.to_string()).color(color::LIGHT_BLACK),
                        )
                        .width(Length::Fill),
                    )
                    .push(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(10)
                            .push(text::p1_bold("Learn more").color(color::LIGHT_BLACK))
                            .push(icon::collapse_icon().color(color::LIGHT_BLACK)),
                    ),
            )
            .style(theme::button::transparent)
        },
        move || {
            Button::new(
                Row::new()
                    .push(
                        Container::new(text::p1_bold(message.to_owned()).color(color::LIGHT_BLACK))
                            .width(Length::Fill),
                    )
                    .push(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(10)
                            .push(text::p1_bold("Learn more").color(color::LIGHT_BLACK))
                            .push(icon::collapsed_icon().color(color::LIGHT_BLACK)),
                    ),
            )
            .style(theme::button::transparent)
        },
        move || Element::<'a, T>::from(text::p2_regular(error.to_owned())),
    )))
    .padding(15)
    .style(theme::banner::warning)
    .width(Length::Fill)
}

pub fn processing_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<&'a str>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(text::p1_bold))
                    .push(text::p1_regular(format!("#{}", fingerprint)))
                    .into(),
                Row::new()
                    .spacing(5)
                    .push(text::caption(kind.to_string()))
                    .push_maybe(version.map(|v| text::caption(v.to_string())))
                    .into(),
            ])
            .width(Length::Fill)
            .into(),
            column(vec![
                text::p2_regular("Processing...").into(),
                text::p2_regular("Please check your device").into(),
            ])
            .into(),
        ])
        .align_y(Alignment::Center),
    )
    .style(theme::notification::pending)
    .padding(10)
}

pub fn processing_hardware_wallet_error<'a, T: 'a + Clone>(
    message: String,
    error: String,
) -> Container<'a, T> {
    let message_clone = message.clone();
    Container::new(Container::new(collapse::Collapse::new(
        move || {
            Button::new(
                Row::new()
                    .push(
                        Container::new(
                            text::p1_bold(message_clone.to_string()).color(color::LIGHT_BLACK),
                        )
                        .width(Length::Fill),
                    )
                    .push(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(10)
                            .push(text::p1_bold("Learn more").color(color::LIGHT_BLACK))
                            .push(icon::collapse_icon().color(color::LIGHT_BLACK)),
                    ),
            )
            .style(theme::button::transparent)
        },
        move || {
            Button::new(
                Row::new()
                    .push(
                        Container::new(text::p1_bold(message.to_owned()).color(color::LIGHT_BLACK))
                            .width(Length::Fill),
                    )
                    .push(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(10)
                            .push(text::p1_bold("Learn more").color(color::LIGHT_BLACK))
                            .push(icon::collapsed_icon().color(color::LIGHT_BLACK)),
                    ),
            )
            .style(theme::button::transparent)
        },
        move || Element::<'a, T>::from(text::p2_regular(error.to_owned())),
    )))
    .padding(10)
    .style(theme::notification::error)
    .width(Length::Fill)
}
