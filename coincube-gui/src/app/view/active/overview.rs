use breez_sdk_liquid::prelude::{Payment, PaymentType};
use coincube_ui::{component::{button, text::*}, icon, theme, widget::*};
use iced::{widget::{Column, Container, Row, Space}, Alignment, Length};

use crate::app::view::message::Message;
use crate::app::menu::{ActiveSubMenu, Menu};

pub fn active_overview_view(
    balance_btc: f64,
    balance_usd: f64,
    recent_payment: Option<&Payment>,
    _loading: bool,
) -> Element<Message> {
    let mut content = Column::new()
        .spacing(40)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(40);

    // Balance display
    content = content.push(
        Column::new()
            .spacing(10)
            .align_x(Alignment::Center)
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(
                        text(format!("{:.8}", balance_btc))
                            .size(48)
                            .style(theme::text::warning),
                    )
                    .push(
                        text("BTC")
                            .size(48)
                            .style(theme::text::warning),
                    ),
            )
            .push(
                text(format!("$ {:.2}", balance_usd))
                    .size(20)
                    .style(theme::text::secondary),
            ),
    );

    // Recent transaction
    if let Some(payment) = recent_payment {
        content = content.push(recent_transaction_card(payment));
    }
    
    // Add space before buttons
    content = content.push(Space::new().height(50.0));

    // Action buttons
    content = content.push(
        Row::new()
            .spacing(20)
            .push(
                button::primary(None, "Send")
                    .on_press(Message::Menu(Menu::Active(ActiveSubMenu::Send)))
                    .padding(15)
                    .width(Length::Fixed(220.0)),
            )
            .push(
                button::transparent_border(None, "Receive")
                    .on_press(Message::Menu(Menu::Active(ActiveSubMenu::Receive)))
                    .padding(15)
                    .width(Length::Fixed(220.0)),
            ),
    );

    content.into()
}

fn recent_transaction_card(payment: &Payment) -> Element<Message> {
    let is_receive = matches!(payment.payment_type, PaymentType::Receive);
    let sign = if is_receive { "+" } else { "-" };
    let amount_color = if is_receive { 
        theme::text::success 
    } else { 
        theme::text::secondary 
    };

    // Format timestamp
    let time_text = format_time_ago(payment.timestamp as u64);

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
                    .push(text("Zap! (Description)").size(16))
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
    .max_width(600)
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
        format!("{} minute{} ago", minutes, if minutes == 1 { "" } else { "s" })
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
