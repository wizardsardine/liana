use iced::{
    alignment,
    widget::{button, column, container, row, Space},
    Alignment, Length,
};
use liana_ui::{
    color,
    component::{amount::Amount, event, hw, separation, text::*},
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
                color_row(color::GREY_7, "GREY #3F3F3F"),
                color_row(color::GREY_6, "GREY #202020"),
                color_row(color::GREY_5, "GREY #272727"),
                color_row(color::GREY_4, "GREY #424242"),
                color_row(color::GREY_3, "GREY #717171"),
                color_row(color::GREY_2, "GREY #CCCCCC"),
                color_row(color::GREY_1, "GREY #E6E6E6"),
                color_row(color::WHITE, "WHITE #FFFFFF"),
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
        container(Space::with_width(Length::Fixed(100.0)))
            .height(Length::Fixed(100.0))
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
                button_row(theme::Button::Transparent, "Transparent"),
                button_row(theme::Button::TransparentBorder, "Transparent Border"),
                button_row(theme::Button::Border, "Border"),
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
    .width(Length::Fixed(200.0))
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
                    .width(Length::Fixed(500.0))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Border),
                button(
                    hw::supported_hardware_wallet(
                        "ledger",
                        Some("v2.1.0"),
                        "f123de",
                        Some("Edouard key")
                    )
                    .width(Length::Fixed(500.0))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Border),
                button(
                    hw::unregistered_hardware_wallet(
                        "ledger",
                        Some("v2.1.0"),
                        "f123de",
                        Some("Edouard key")
                    )
                    .width(Length::Fixed(500.0))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Border),
                button(
                    hw::unsupported_hardware_wallet("ledger", Some("v2.1.0"))
                        .width(Length::Fixed(500.0))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Border),
                button(
                    hw::processing_hardware_wallet(
                        "ledger",
                        Some("v2.1.0"),
                        "f123de",
                        Some("Edouard key")
                    )
                    .width(Length::Fixed(500.0))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Border),
                button(
                    hw::sign_success_hardware_wallet(
                        "ledger",
                        Some("v2.1.0"),
                        "f123de",
                        Some("Edouard key")
                    )
                    .width(Length::Fixed(500.0))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Border),
                button(
                    hw::registration_success_hardware_wallet(
                        "ledger",
                        Some("v2.1.0"),
                        "f123de",
                        Some("Edouard key")
                    )
                    .width(Length::Fixed(500.0))
                )
                .on_press(Message::Ignore)
                .style(theme::Button::Border),
            ]
            .spacing(20)
        ]
        .spacing(100)
        .into()
    }
}

pub struct Events {}

impl Section for Events {
    fn title(&self) -> &'static str {
        "Events "
    }
    fn view(&self) -> Element<Message> {
        let d = chrono::NaiveDate::from_ymd_opt(2015, 6, 3).unwrap();
        let t = chrono::NaiveTime::from_hms_milli_opt(12, 34, 56, 789).unwrap();
        column![
            h1(self.title()),
            column![
                event::unconfirmed_outgoing_event(&Amount::from_sat(32934234), Message::Ignore),
                event::confirmed_outgoing_event(
                    chrono::NaiveDateTime::new(d, t),
                    &Amount::from_sat(32934234),
                    Message::Ignore
                ),
                event::unconfirmed_incoming_event(&Amount::from_sat(32934234), Message::Ignore),
                event::confirmed_incoming_event(
                    chrono::NaiveDateTime::new(d, t),
                    &Amount::from_sat(32934234),
                    Message::Ignore
                )
            ]
            .spacing(20)
        ]
        .spacing(100)
        .into()
    }
}
