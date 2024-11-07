pub mod amount;
pub mod badge;
pub mod button;
pub mod card;
pub mod collapse;
pub mod event;
pub mod form;
pub mod hw;
pub mod modal;
pub mod notification;
pub mod spinner;
pub mod text;
pub mod toast;
pub mod tooltip;

use bitcoin::Network;
pub use tooltip::tooltip;

use iced::Length;

use crate::{theme, widget::*};

use self::text::Text;

pub fn separation<'a, T: 'a>() -> Container<'a, T> {
    Container::new(Column::new().push(text::text(" ")))
        .style(theme::Container::Border)
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
                    Network::Regtest => "REGTEST WALLET",
                    _ => unreachable!(),
                })
                .bold(),
            )
            .push(text::text(", COINS HAVE "))
            .push(text::text("NO VALUE").bold())
            .align_items(iced::Alignment::Center),
    )
    .padding(5)
    .width(Length::Fill)
    .center_x()
    .style(theme::Container::Banner)
}
