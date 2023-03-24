use iced::{
    alignment,
    widget::{button, column, container, row, Space},
    Alignment, Length,
};
use liana_ui::{color, component::text::*, theme, widget::Element};

use super::{Message, Section};

pub struct Overview {}

impl Section for Overview {
    fn title(&self) -> &'static str {
        "Overview"
    }

    fn view(&self) -> Element<Message> {
        column![
            text("Hello").bold().size(50),
            column![text(
                "This is the Liana design system for the Iced framework"
            )]
            .spacing(10)
        ]
        .spacing(100)
        .into()
    }
}

pub struct Colors {}

impl Section for Colors {
    fn title(&self) -> &'static str {
        "Colors"
    }

    fn view(&self) -> Element<Message> {
        column![
            text(self.title()).bold().size(50),
            column![
                color_row(color::BLACK, "BLACK (0,0,0)"),
                color_row(color::LIGHT_BLACK, "LIGHT_BLACK #141414 original design"),
                color_row(color::GREEN, "GREEN #00FF66 original design"),
                color_row(color::DARK_GREY, "DARK_GREY #555555"),
                color_row(color::GREY, "GREY #CCCCCC original design"),
                color_row(color::LIGHT_GREY, "LIGHT_GREY #E6E6E6 original design"),
                color_row(color::RED, "RED #F04359"),
                color_row(color::ORANGE, "ORANGE #FFa700")
            ]
            .spacing(10)
        ]
        .spacing(100)
        .into()
    }
}

fn color_row<'a, T: 'a>(color: iced::Color, label: &'static str) -> Element<'a, T> {
    row![
        container(Space::with_width(Length::Units(100)))
            .height(Length::Units(100))
            .style(theme::Container::Custom(color)),
        text(label)
    ]
    .spacing(10)
    .align_items(Alignment::Center)
    .spacing(10)
    .into()
}

pub struct Buttons {}

impl Section for Buttons {
    fn title(&self) -> &'static str {
        "Buttons"
    }

    fn view(&self) -> Element<Message> {
        column![
            text(self.title()).bold().size(50),
            column![
                button_row(theme::Button::Primary, "Primary"),
                button_row(theme::Button::Secondary, "Secondary"),
                button_row(theme::Button::Destructive, "Destructive"),
                button_row(theme::Button::Transparent, "Transparent")
            ]
            .spacing(20)
        ]
        .spacing(100)
        .into()
    }
}

fn button_row(style: theme::Button, label: &'static str) -> Element<Message> {
    button(
        container(text(label))
            .width(Length::Fill)
            .align_x(alignment::Horizontal::Center),
    )
    .width(Length::Units(200))
    .padding(5)
    .style(style)
    .on_press(Message::Ignore)
    .into()
}
