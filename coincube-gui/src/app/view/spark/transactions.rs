//! View renderer for [`crate::app::state::spark::transactions::SparkTransactions`].
//!
//! Renders one of four states:
//! - [`SparkTransactionsStatus::Unavailable`] — no backend wired.
//! - [`SparkTransactionsStatus::Loading`] — first fetch in flight.
//! - [`SparkTransactionsStatus::Error`] — bridge returned an error.
//! - [`SparkTransactionsStatus::Loaded`] — list populated (possibly empty).
//!
//! Row layout: [direction arrow] [amount] [time ago] [status pill].
//! Intentionally minimal — the real `TransactionListItem` widget used by
//! the Liquid panel expects domain types we haven't mapped for Spark
//! yet, so Phase 4b uses a straightforward row builder.

use coincube_ui::{
    color,
    component::text::{h2, p1_regular, p2_regular},
    theme,
    widget::{Column, Container, Element, Row},
};
use coincube_spark_protocol::PaymentSummary;
use iced::widget::{scrollable, Space};
use iced::{Alignment, Length};

use crate::app::view::Message;
use crate::utils::format_time_ago;

/// Tri-state status the Transactions panel can be in.
#[derive(Debug, Clone)]
pub enum SparkTransactionsStatus {
    Unavailable,
    Loading,
    Error(String),
    Loaded(Vec<PaymentSummary>),
}

/// View wrapper that renders the Transactions panel.
pub struct SparkTransactionsView {
    pub status: SparkTransactionsStatus,
}

impl SparkTransactionsView {
    pub fn render<'a>(self) -> Element<'a, Message> {
        let heading = Container::new(h2("Spark — Transactions"));

        let body: Element<'_, Message> = match self.status {
            SparkTransactionsStatus::Unavailable => Column::new()
                .push(p1_regular(
                    "Spark is not configured for this cube. Set up a Spark \
                     signer to see your payment history here.",
                ))
                .into(),
            SparkTransactionsStatus::Loading => Column::new()
                .push(p1_regular("Loading payment history from the Spark bridge…"))
                .into(),
            SparkTransactionsStatus::Error(err) => Column::new()
                .spacing(10)
                .push(p1_regular("Failed to load payment history"))
                .push(p2_regular(err))
                .into(),
            SparkTransactionsStatus::Loaded(payments) if payments.is_empty() => Column::new()
                .push(p1_regular(
                    "No payments yet. Incoming and outgoing payments will \
                     appear here once the Send / Receive panels land.",
                ))
                .into(),
            SparkTransactionsStatus::Loaded(payments) => {
                let mut list = Column::new().spacing(8);
                for payment in &payments {
                    list = list.push(payment_row(payment));
                }
                scrollable(list).height(Length::Fill).into()
            }
        };

        Column::new()
            .spacing(20)
            .push(heading)
            .push(body)
            .into()
    }
}

fn payment_row<'a>(payment: &PaymentSummary) -> Element<'a, Message> {
    let is_receive = payment.direction.eq_ignore_ascii_case("Receive");
    let arrow = if is_receive { "↓" } else { "↑" };
    let amount_str = if is_receive {
        format!("+{} sats", payment.amount_sat.unsigned_abs())
    } else {
        format!("-{} sats", payment.amount_sat.unsigned_abs())
    };
    let time_ago = format_time_ago(payment.timestamp as i64);

    // Use the fg color directly via `.color(...)` so the pill reads as
    // a quick at-a-glance status without needing a custom theme token.
    let status_color = if payment.status.eq_ignore_ascii_case("Completed")
        || payment.status.eq_ignore_ascii_case("Complete")
    {
        color::GREEN
    } else if payment.status.eq_ignore_ascii_case("Failed")
        || payment.status.eq_ignore_ascii_case("TimedOut")
    {
        color::RED
    } else {
        color::GREY_3
    };

    Container::new(
        Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(p1_regular(arrow))
            .push(
                Column::new()
                    .spacing(4)
                    .push(p1_regular(amount_str))
                    .push(p2_regular(time_ago).style(theme::text::secondary)),
            )
            .push(Space::new().width(Length::Fill))
            .push(
                p2_regular(payment.status.clone())
                    .style(move |_| iced::widget::text::Style { color: Some(status_color) }),
            ),
    )
    .padding(12)
    .style(theme::card::simple)
    .into()
}
