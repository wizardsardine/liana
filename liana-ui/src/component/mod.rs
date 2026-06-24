pub mod address;
pub mod amount;
pub mod badge;
pub mod button;
pub mod card;
pub mod checkbox;
pub mod collapse;
pub mod combobox;
pub mod form;
pub mod label;
pub mod list;
pub mod modal;
pub mod notification;
pub mod panels;
pub mod pick_list;
pub mod pill;
pub mod scrollable;
pub mod spinner;
pub mod tab;
pub mod text;
pub mod toast;
pub mod tooltip;

use bitcoin::Network;
pub use tooltip::{tooltip, tooltip_custom};

use iced::{widget::row, Alignment, Length, Padding};

use crate::{theme, widget::*};

use self::text::Text;

pub fn separation<'a, T: 'a>() -> Container<'a, T> {
    Container::new(Column::new().push(text::text(" ")))
        .style(theme::container::border)
        .height(Length::Fixed(1.0))
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

pub fn section<'a, T: 'a>(title: impl std::fmt::Display) -> Row<'a, T> {
    let title = card::section(text::new::d4(title))
        .padding(Padding {
            top: 10.0,
            right: 24.0,
            bottom: 10.0,
            left: 18.0,
        })
        .align_y(Alignment::Center);
    row![title, separator()]
        .align_y(Alignment::Center)
        .spacing(10)
        .width(Length::Fill)
}

pub fn separator<'a, T: 'a>() -> Element<'a, T> {
    iced::widget::rule::horizontal(4)
        .style(theme::rule::separator)
        .into()
}
