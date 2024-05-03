use std::borrow::Cow;
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
                            text::p1_bold(message_clone.to_string()).style(color::LIGHT_BLACK),
                        )
                        .width(Length::Fill),
                    )
                    .push(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text::p1_bold("Learn more").style(color::LIGHT_BLACK))
                            .push(icon::collapse_icon().style(color::LIGHT_BLACK)),
                    ),
            )
            .style(theme::Button::Transparent)
        },
        move || {
            Button::new(
                Row::new()
                    .push(
                        Container::new(text::p1_bold(message.to_owned()).style(color::LIGHT_BLACK))
                            .width(Length::Fill),
                    )
                    .push(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text::p1_bold("Learn more").style(color::LIGHT_BLACK))
                            .push(icon::collapsed_icon().style(color::LIGHT_BLACK)),
                    ),
            )
            .style(theme::Button::Transparent)
        },
        move || Element::<'a, T>::from(text::p2_regular(error.to_owned())),
    )))
    .padding(15)
    .style(theme::Container::Card(theme::Card::Warning))
    .width(Length::Fill)
}

pub fn processing_hardware_wallet<'a, T: 'a, K: Display, V: Display, F: Display>(
    kind: K,
    version: Option<V>,
    fingerprint: F,
    alias: Option<impl Into<Cow<'a, str>>>,
) -> Container<'a, T> {
    container(
        row(vec![
            column(vec![
                Row::new()
                    .spacing(5)
                    .push_maybe(alias.map(|a| text::p1_bold(a)))
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
        .align_items(Alignment::Center),
    )
    .style(theme::Container::Notification(theme::Notification::Pending))
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
                            text::p1_bold(message_clone.to_string()).style(color::LIGHT_BLACK),
                        )
                        .width(Length::Fill),
                    )
                    .push(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text::p1_bold("Learn more").style(color::LIGHT_BLACK))
                            .push(icon::collapse_icon().style(color::LIGHT_BLACK)),
                    ),
            )
            .style(theme::Button::Transparent)
        },
        move || {
            Button::new(
                Row::new()
                    .push(
                        Container::new(text::p1_bold(message.to_owned()).style(color::LIGHT_BLACK))
                            .width(Length::Fill),
                    )
                    .push(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text::p1_bold("Learn more").style(color::LIGHT_BLACK))
                            .push(icon::collapsed_icon().style(color::LIGHT_BLACK)),
                    ),
            )
            .style(theme::Button::Transparent)
        },
        move || Element::<'a, T>::from(text::p2_regular(error.to_owned())),
    )))
    .padding(10)
    .style(theme::Container::Notification(theme::Notification::Error))
    .width(Length::Fill)
}
