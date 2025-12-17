use coincube_ui::{component::text::*, widget::*};
use iced::{widget::Column, widget::Space, Length};

use crate::app::view::message::Message;

pub fn active_transactions_view(wallet_name: &str) -> Element<Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(h3("Active - Transactions"))
        .push(text(format!("Wallet: {}", wallet_name)))
        .push(Space::with_height(Length::Fixed(20.0)))
        .push(text("This is a placeholder for the Active Transactions page.").size(15))
        .push(text("Lightning Network transaction history will be displayed here.").size(15))
        .into()
}
