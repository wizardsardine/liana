use coincube_ui::{component::text::*, widget::*};
use iced::{widget::Column, widget::Space, Length};

use crate::app::view::message::Message;

pub fn active_receive_view(wallet_name: &str) -> Element<Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(h3("Active - Receive"))
        .push(text(format!("Wallet: {}", wallet_name)))
        .push(Space::with_height(Length::Fixed(20.0)))
        .push(text("This is a placeholder for the Active Receive page.").size(15))
        .push(text("Lightning Network receive functionality will be added here.").size(15))
        .into()
}
