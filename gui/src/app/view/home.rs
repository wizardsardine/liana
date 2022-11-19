use iced::{
    pure::{column, Element},
    Alignment,
};
use liana::miniscript::bitcoin;

use crate::ui::component::text::*;

use super::message::Message;

pub fn home_view(balance: &bitcoin::Amount) -> Element<Message> {
    column()
        .push(column().padding(40))
        .push(text(&format!("{} BTC", balance.to_btc())).bold().size(50))
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}
