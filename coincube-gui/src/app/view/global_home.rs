use coincube_ui::{component::text::*, widget::*};
use iced::{widget::Column, widget::Space, Length};

use crate::app::view::message::Message;

pub fn global_home_view(wallet_name: &str) -> Element<Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(h3("Welcome to COINCUBE"))
        .push(text(format!("Wallet: {}", wallet_name)))
        .push(Space::with_height(Length::Fixed(20.0)))
        .push(text("This is a placeholder for the global home page.").size(15))
        .push(text("Content will be added here soon.").size(15))
        .into()
}
