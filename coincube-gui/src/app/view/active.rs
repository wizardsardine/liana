use coincube_ui::{component::text::*, widget::*};
use iced::{widget::Column, widget::Space, Length};

use crate::app::view::message::Message;

pub fn active_overview_view(wallet_name: &str) -> Element<Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(h3("Active - Overview"))
        .push(text(format!("Wallet: {}", wallet_name)))
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(text("This is a placeholder for the Active Overview page.").size(15))
        .push(text("Lightning Network overview will be displayed here.").size(15))
        .into()
}

pub fn active_send_view(wallet_name: &str) -> Element<Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(h3("Active - Send"))
        .push(text(format!("Wallet: {}", wallet_name)))
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(text("This is a placeholder for the Active Send page.").size(15))
        .push(text("Lightning Network send functionality will be added here.").size(15))
        .into()
}

pub fn active_receive_view(wallet_name: &str) -> Element<Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(h3("Active - Receive"))
        .push(text(format!("Wallet: {}", wallet_name)))
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(text("This is a placeholder for the Active Receive page.").size(15))
        .push(text("Lightning Network receive functionality will be added here.").size(15))
        .into()
}

pub fn active_transactions_view(wallet_name: &str) -> Element<Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(h3("Active - Transactions"))
        .push(text(format!("Wallet: {}", wallet_name)))
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(text("This is a placeholder for the Active Transactions page.").size(15))
        .push(text("Lightning Network transaction history will be displayed here.").size(15))
        .into()
}

pub fn active_settings_view(wallet_name: &str) -> Element<Message> {
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(h3("Active - Settings"))
        .push(text(format!("Wallet: {}", wallet_name)))
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(text("This is a placeholder for the Active Settings page.").size(15))
        .push(text("Lightning Network settings will be configured here.").size(15))
        .into()
}
