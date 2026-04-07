pub mod amount;
pub mod badge;
pub mod button;
pub mod card;
pub mod collapse;
pub mod form;
pub mod hw;
pub mod modal;
pub mod notification;
pub mod quote_display;
pub mod spinner;
pub mod text;
pub mod toast;
pub mod tooltip;
pub mod transaction;

use bitcoin::Network;
pub use tooltip::tooltip;

use iced::Length;

use crate::{theme, widget::*};

use self::text::Text;

pub fn separation<'a, T: 'a>() -> Container<'a, T> {
    Container::new(Column::new().push(iced::widget::space()))
        .style(theme::container::border)
        .height(Length::Fixed(1.0))
        .width(Length::Fill)
}

pub fn received_celebration_page<'a, M: Clone + 'a>(
    amount_display: &'a str,
    quote: &'a quote_display::Quote,
    image_handle: &'a iced::widget::image::Handle,
    on_dismiss: M,
) -> Element<'a, M> {
    use quote_display::{self as qd, QuoteDisplayProps};

    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center)
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(qd::display(
            &QuoteDisplayProps::new("transaction-received", quote, image_handle).image_size(480),
        ))
        .push(text::h3("Payment received!"))
        .push(
            Row::new()
                .spacing(5)
                .push(
                    iced::widget::text(amount_display)
                        .size(20)
                        .color(crate::color::GREEN)
                        .font(iced::Font {
                            style: iced::font::Style::Italic,
                            ..Default::default()
                        }),
                )
                .push(
                    iced::widget::text("has arrived.")
                        .size(20)
                        .font(iced::Font {
                            style: iced::font::Style::Italic,
                            ..Default::default()
                        }),
                ),
        )
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(
            button::primary(None, "Back")
                .width(Length::Fixed(150.0))
                .on_press(on_dismiss),
        )
        .into()
}

pub fn network_banner<'a, T: 'a>(network: Network) -> Container<'a, T> {
    Container::new(
        Row::new()
            .push(super::icon::warning_icon())
            .push(text::text("THIS IS A "))
            .push(
                text::text(match network {
                    Network::Signet => "SIGNET WALLET",
                    Network::Testnet => "TESTNET WALLET",
                    Network::Testnet4 => "TESTNET4 WALLET",
                    Network::Regtest => "REGTEST WALLET",
                    _ => unreachable!(),
                })
                .bold(),
            )
            .push(text::text(", COINS HAVE "))
            .push(text::text("NO VALUE").bold())
            .align_y(iced::Alignment::Center),
    )
    .padding(5)
    .center_x(Length::Fill)
    .style(theme::banner::network)
}
