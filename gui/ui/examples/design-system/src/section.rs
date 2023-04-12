use iced::{
    alignment,
    widget::{button, column, container, row, Space},
    Alignment, Length,
};
use liana_ui::{
    color,
    component::{hw, separation, text::*},
    theme,
    widget::Element,
};

use super::{Message, Section};

pub struct Overview {}

impl Section for Overview {
    fn title(&self) -> &'static str {
        "Overview"
    }

    fn view(&self) -> Element<Message> {
        column![
            h1("Hello"),
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
            h1(self.title()),
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

pub struct Typography {}

impl Section for Typography {
    fn title(&self) -> &'static str {
        "Typography"
    }

    fn view(&self) -> Element<Message> {
        column![
            h1(self.title()),
            column![
                column![
                    h2("Font-Family"),
                    separation().width(Length::Fill),
                    h1("IBM Plex Sans"),
                ]
                .spacing(10),
                column![
                    h2("Heading"),
                    separation().width(Length::Fill),
                    h1("H1. Heading 40 bold"),
                    h2("H2. Heading 29 bold"),
                    h3("H3. Heading 24 bold"),
                    h4_bold("H4. Heading 20 bold"),
                    h4_regular("H4. Heading 20 regular"),
                    h5_medium("H5. Heading 18 medium"),
                    h5_regular("H5. Heading 18 regular"),
                ]
                .spacing(10),
                column![
                    h2("Body"),
                    separation().width(Length::Fill),
                    p1_bold("P1. Body 16 bold"),
                    p1_medium("P1. Body 16 medium"),
                    p1_regular("P1. Body 16 regular"),
                    p2_medium("P2. Body 14 medium"),
                    p2_regular("P2. Body 14 regular"),
                    caption("Caption Body 12 regular"),
                ]
                .spacing(10),
            ]
            .spacing(50)
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
            h1(self.title()),
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

pub struct HardwareWallets {}

impl Section for HardwareWallets {
    fn title(&self) -> &'static str {
        "Hardware wallets"
    }

    fn view(&self) -> Element<Message> {
        column![
            h1(self.title()),
            column![
                button(
                    hw::supported_hardware_wallet(
                        "ledger",
                        Some("v2.1.0"),
                        "f123de",
                        None::<String>
                    )
                    .width(Length::Units(500))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Secondary),
                button(
                    hw::supported_hardware_wallet(
                        "ledger",
                        Some("v2.1.0"),
                        "f123de",
                        Some("Edouard key")
                    )
                    .width(Length::Units(500))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Secondary),
                button(
                    hw::unregistered_hardware_wallet(
                        "ledger",
                        Some("v2.1.0"),
                        "f123de",
                        Some("Edouard key")
                    )
                    .width(Length::Units(500))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Secondary),
                button(
                    hw::unsupported_hardware_wallet("ledger", Some("v2.1.0"))
                        .width(Length::Units(500))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Secondary),
                button(
                    hw::processing_hardware_wallet(
                        "ledger",
                        Some("v2.1.0"),
                        "f123de",
                        Some("Edouard key")
                    )
                    .width(Length::Units(500))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Secondary),
                button(
                    hw::sign_success_hardware_wallet(
                        "ledger",
                        Some("v2.1.0"),
                        "f123de",
                        Some("Edouard key")
                    )
                    .width(Length::Units(500))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Secondary),
                button(
                    hw::registration_success_hardware_wallet(
                        "ledger",
                        Some("v2.1.0"),
                        "f123de",
                        Some("Edouard key")
                    )
                    .width(Length::Units(500))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Secondary),
            ]
            .spacing(20)
        ]
        .spacing(100)
        .into()
    }
}
