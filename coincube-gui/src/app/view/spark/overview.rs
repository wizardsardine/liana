//! View renderer for the Spark Overview panel.
//!
//! Renders one of four states as plain text + a small heading:
//! - [`SparkStatus::Unavailable`] — the cube has no Spark signer or the
//!   bridge subprocess failed to spawn.
//! - [`SparkStatus::Loading`] — first `get_info` still in flight.
//! - [`SparkStatus::Connected`] — bridge returned balance + pubkey.
//!   Renders a "Stable" badge next to the balance when the SDK
//!   reports an active Stable Balance token.
//! - [`SparkStatus::Error`] — bridge returned an error response.
//!
//! A richer layout (balance card, fiat conversion, recent transactions)
//! copied from the Liquid panel is a future polish pass.

use coincube_ui::{
    component::text::{h2, p1_regular, p2_regular},
    widget::{Column, Container, Element, Row},
};
use iced::Length;
use iced::widget::Space;
use iced::Alignment;

use crate::app::state::spark::overview::SparkBalanceSnapshot;
use crate::app::view::Message;

/// High-level status of the Spark backend for the current cube.
#[derive(Debug, Clone)]
pub enum SparkStatus {
    /// No Spark signer configured for this cube (or bridge spawn failed).
    Unavailable,
    /// First `get_info` is still in flight.
    Loading,
    /// Bridge returned a balance snapshot.
    Connected(SparkBalanceSnapshot),
    /// Bridge returned an error response.
    Error(String),
}

/// View wrapper that knows how to render a [`SparkStatus`] as the Spark
/// Overview panel.
pub struct SparkOverviewView {
    pub status: SparkStatus,
    /// Phase 6: `true` when the SDK reports an active Stable Balance
    /// token. Drives the "Stable" badge rendered next to the balance
    /// line in the `Connected` state.
    pub stable_balance_active: bool,
}

impl SparkOverviewView {
    /// Build the Element to hand to [`crate::app::view::dashboard`].
    pub fn render<'a>(self) -> Element<'a, Message> {
        let heading = Container::new(h2("Spark Wallet"));

        let body: Element<'_, Message> = match self.status {
            SparkStatus::Unavailable => Column::new()
                .push(p1_regular(
                    "Spark is not configured for this cube yet. Set up a Spark \
                     signer and restart the app to connect the bridge.",
                ))
                .into(),
            SparkStatus::Loading => Column::new()
                .push(p1_regular("Connecting to the Spark bridge…"))
                .into(),
            SparkStatus::Connected(snapshot) => {
                let mut balance_row = Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(p1_regular(format!("Balance: {} sats", snapshot.balance_sats)));
                if self.stable_balance_active {
                    balance_row = balance_row.push(p2_regular("· Stable"));
                }
                Column::new()
                    .spacing(10)
                    .push(balance_row)
                    .push(p2_regular(format!("Identity: {}", snapshot.identity_pubkey)))
                    .push(Space::new().height(Length::Fixed(12.0)))
                    .push(p2_regular(
                        "This is the Phase 3 Spark Overview stub. Full panels \
                         (Send, Receive, Transactions, Settings) land in the next \
                         phase.",
                    ))
                    .into()
            }
            SparkStatus::Error(err) => Column::new()
                .spacing(10)
                .push(p1_regular("Spark bridge error"))
                .push(p2_regular(err))
                .into(),
        };

        Column::new()
            .spacing(20)
            .push(heading)
            .push(body)
            .into()
    }
}
