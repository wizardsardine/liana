use coincube_core::miniscript::bitcoin;
use coincube_ui::{color, component::*, icon::*, theme};
use iced::{widget, Alignment, Length};

use crate::app::view;

pub(super) fn webview_ux<'a>(
    active: &'a iced_wry::IcedWebview,
    network: &'a bitcoin::Network,
) -> coincube_ui::widget::Element<'a, view::Message> {
    let col = iced::widget::column![
        active.view(Length::Fixed(640.0), Length::Fixed(600.0)),
        // Network display banner
        widget::Space::new().height(Length::Fixed(15.0)),
        {
            let (network_name, network_color) = match network {
                bitcoin::Network::Bitcoin => ("Bitcoin Mainnet", color::GREEN),
                bitcoin::Network::Testnet => ("Bitcoin Testnet", color::ORANGE),
                bitcoin::Network::Testnet4 => ("Bitcoin Testnet4", color::ORANGE),
                bitcoin::Network::Signet => ("Bitcoin Signet", color::BLUE),
                bitcoin::Network::Regtest => ("Bitcoin Regtest", color::RED),
            };

            iced::widget::row![
                // currently selected bitcoin network display
                text::text("Network: ").size(12).color(color::GREY_3),
                text::text(network_name).size(12).color(network_color),
                // render a button that closes the webview
                widget::Space::new().width(Length::Fixed(20.0)),
                {
                    button::secondary(Some(arrow_back()), "Start Over")
                        .on_press(view::BuySellMessage::ResetWidget)
                        .width(iced::Length::Fixed(300.0))
                }
            ]
            .spacing(5)
            .align_y(Alignment::Center)
        }
    ];

    let elem: iced::Element<view::BuySellMessage, theme::Theme> = col.into();
    elem.map(|b| view::Message::BuySell(b))
}
