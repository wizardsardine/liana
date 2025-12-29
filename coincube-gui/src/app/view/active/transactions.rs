use breez_sdk_liquid::prelude::{Payment, PaymentType};
use coincube_ui::{
    component::{button, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{Column, Container, Row, Space},
    Alignment, Length,
};

use crate::app::menu::Menu;
use crate::app::view::message::Message;

pub fn active_transactions_view(
    payments: &[Payment],
    balance_sat: u64,
    balance_usd: f64,
    _loading: bool,
) -> Element<Message> {
    let mut content = Column::new().spacing(30).width(Length::Fill).padding(40);

    if payments.is_empty() {
        // Empty state
        content = content.push(
            Column::new()
                .spacing(20)
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .push(Space::new().height(Length::Fixed(100.0)))
                .push(h2("No transactions yet").style(theme::text::primary))
                .push(
                    text("Your Lightning wallet is ready. Once you send or receive\nsats, they'll show up here.")
                        .size(16)
                        .style(theme::text::secondary)
                        .wrapping(iced::widget::text::Wrapping::Word)
                        .align_x(iced::alignment::Horizontal::Center),
                )
                .push(Space::new().height(Length::Fixed(20.0)))
                .push(
                    Row::new()
                        .spacing(15)
                        .push(
                            button::primary(None, "Send sats")
                                .on_press(Message::Menu(Menu::Active(crate::app::menu::ActiveSubMenu::Send)))
                                .padding(15)
                                .width(Length::Fixed(150.0)),
                        )
                        .push(
                            button::transparent_border(None, "Receive sats")
                                .on_press(Message::Menu(Menu::Active(crate::app::menu::ActiveSubMenu::Receive)))
                                .padding(15)
                                .width(Length::Fixed(150.0)),
                        ),
                ),
        );
    } else {
        // Show balance
        let balance_btc = balance_sat as f64 / 100_000_000.0;
        content = content.push(
            Column::new()
                .spacing(8)
                .push(
                    text(format!("{:.8} sats", balance_btc))
                        .size(32)
                        .style(theme::text::warning),
                )
                .push(
                    text(format!("~ $ {:.2}", balance_usd))
                        .size(18)
                        .style(theme::text::secondary),
                ),
        );

        // Transaction list
        for payment in payments {
            content = content.push(transaction_row(payment));
        }
    }

    content.into()
}

fn transaction_row(payment: &Payment) -> Element<Message> {
    let is_receive = matches!(payment.payment_type, PaymentType::Receive);
    let sign = if is_receive { "+" } else { "-" };
    let amount_color = if is_receive {
        theme::text::success
    } else {
        theme::text::secondary
    };

    // Format timestamp
    let time_text = format_time_ago(payment.timestamp as u64);

    // Get description or default
    let description = "Payment".to_string();

    Container::new(
        Row::new()
            .spacing(15)
            .align_y(Alignment::Center)
            .push(
                Container::new(icon::lightning_icon().style(theme::text::warning))
                    .width(Length::Fixed(30.0))
                    .center_x(Length::Fixed(30.0)),
            )
            .push(
                Column::new()
                    .spacing(5)
                    .width(Length::Fill)
                    .push(text(description).size(16))
                    .push(text(time_text).size(14).style(theme::text::secondary)),
            )
            .push(
                Column::new()
                    .spacing(5)
                    .align_x(Alignment::End)
                    .push(
                        text(format!("{} {} sats", sign, payment.amount_sat))
                            .size(16)
                            .style(amount_color),
                    )
                    .push(
                        text(format!("about $ {:.2}", payment.amount_sat as f64 / 100.0))
                            .size(14)
                            .style(theme::text::secondary),
                    ),
            ),
    )
    .padding(20)
    .width(Length::Fill)
    .style(theme::container::border_grey)
    .into()
}

fn format_time_ago(timestamp: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let elapsed = now.saturating_sub(timestamp);

    if elapsed < 60 {
        format!("{} seconds ago", elapsed)
    } else if elapsed < 3600 {
        let minutes = elapsed / 60;
        format!(
            "{} minute{} ago",
            minutes,
            if minutes == 1 { "" } else { "s" }
        )
    } else if elapsed < 86400 {
        let hours = elapsed / 3600;
        format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
    } else if elapsed < 604800 {
        let days = elapsed / 86400;
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    } else if elapsed < 2592000 {
        let weeks = elapsed / 604800;
        format!("{} week{} ago", weeks, if weeks == 1 { "" } else { "s" })
    } else {
        let months = elapsed / 2592000;
        format!("{} month{} ago", months, if months == 1 { "" } else { "s" })
    }
}
