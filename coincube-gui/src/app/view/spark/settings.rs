//! View renderer for [`crate::app::state::spark::settings::SparkSettings`].
//!
//! Renders:
//! - A "Stable Balance" toggle card (USD-pegging feature) with a
//!   clear on/off status line and a toggle button. Disabled while
//!   the bridge is unavailable or an `update_user_settings` RPC
//!   is in flight.
//! - A small "Bridge status" diagnostic card showing whether the
//!   Spark bridge subprocess is reachable (`get_info` round-trip
//!   successful on the last reload).
//!
//! The Default Lightning backend picker lives on the app-level
//! **Settings → Lightning** page, not here. The balance, identity
//! pubkey, and network read-outs moved elsewhere too: balance is
//! already in the Overview/Send panels, the network lives in
//! **Settings → General**, and the identity pubkey wasn't actually
//! useful to surface.

use coincube_ui::{
    color,
    component::{
        button,
        text::{h2, h4_bold, p1_regular, p2_regular},
    },
    theme,
    widget::{Column, Container, Element, Row},
};
use iced::widget::Space;
use iced::{Alignment, Length};

use crate::app::view::{Message, SparkSettingsMessage};

/// Coarse status the panel knows about. Only "Unavailable" needs
/// its own rendering branch; the other variants all render the
/// same content (Stable Balance card + bridge status card) and
/// differ only in what the bridge-status card says.
#[derive(Debug, Clone)]
pub enum SparkSettingsStatus {
    /// No Spark signer or the bridge subprocess failed to spawn.
    Unavailable,
    /// First `get_info` round-trip still in flight.
    Loading,
    /// Last `get_info` call failed.
    Error(String),
    /// Last `get_info` call succeeded — bridge is reachable.
    Connected,
}

pub struct SparkSettingsView {
    pub status: SparkSettingsStatus,
    /// Phase 6: Stable Balance on/off. `None` means the first
    /// `get_user_settings` RPC hasn't returned yet — the toggle
    /// renders as "Loading…" in that state.
    pub stable_balance_active: Option<bool>,
    /// Phase 6: `true` while a `set_stable_balance` RPC is in
    /// flight. Disables the toggle buttons so the user can't queue
    /// a second flip mid-rpc.
    pub stable_balance_saving: bool,
}

impl SparkSettingsView {
    pub fn render<'a>(self) -> Element<'a, Message> {
        let heading = Container::new(h2("Spark — Settings"));

        if matches!(self.status, SparkSettingsStatus::Unavailable) {
            let body = Column::new()
                .spacing(10)
                .push(p1_regular(
                    "Spark is not configured for this cube, or the bridge \
                     subprocess failed to spawn.",
                ))
                .push(p2_regular(
                    "Configure a Spark signer on this cube and restart the \
                     app to connect. If you already have one configured, \
                     check the stderr logs from coincube-spark-bridge to \
                     see why the spawn failed — the bridge binary must be \
                     locatable via COINCUBE_SPARK_BRIDGE_PATH or sit \
                     alongside the main coincube binary.",
                ));
            return Column::new().spacing(20).push(heading).push(body).into();
        }

        let bridge_reachable = matches!(self.status, SparkSettingsStatus::Connected);
        let stable_balance_card = stable_balance_card(
            self.stable_balance_active,
            self.stable_balance_saving,
            bridge_reachable,
        );
        let bridge_status_card = bridge_status_card(&self.status);

        Column::new()
            .spacing(20)
            .push(heading)
            .push(stable_balance_card)
            .push(bridge_status_card)
            .into()
    }
}

fn stable_balance_card<'a>(
    active: Option<bool>,
    saving: bool,
    spark_available: bool,
) -> Element<'a, Message> {
    let status_line: Element<'_, Message> = match (active, spark_available) {
        (_, false) => p2_regular("Spark bridge unavailable — toggle disabled.").into(),
        (None, true) => p2_regular("Loading…").into(),
        (Some(true), true) => p2_regular("Stable Balance is ON").into(),
        (Some(false), true) => p2_regular("Stable Balance is OFF").into(),
    };

    let can_toggle = spark_available && active.is_some() && !saving;
    let target = active.unwrap_or(false);
    let (button_label, next_state) = if target {
        ("Turn off", false)
    } else {
        ("Turn on", true)
    };
    let toggle_btn = button::primary(None, button_label).width(Length::Fixed(140.0));
    let toggle_btn: Element<'_, Message> = if can_toggle {
        toggle_btn
            .on_press(Message::SparkSettings(
                SparkSettingsMessage::StableBalanceToggled(next_state),
            ))
            .into()
    } else {
        toggle_btn.on_press_maybe(None).into()
    };

    Container::new(
        Column::new()
            .spacing(10)
            .push(h4_bold("Stable Balance"))
            .push(p2_regular(
                "Keep a portion of your Bitcoin balance pegged to \
                 USD. Your spendable balance stays stable against \
                 fiat even as BTC price moves. You can still send \
                 Bitcoin normally — the wallet automatically \
                 converts between the stable and Bitcoin balances \
                 as needed.",
            ))
            .push(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push(status_line)
                    .push(Space::new().width(Length::Fill))
                    .push(toggle_btn),
            ),
    )
    .padding(12)
    .style(theme::card::simple)
    .into()
}

/// Small live diagnostic card — shows whether the Spark bridge
/// subprocess is reachable. Green check for a healthy round-trip,
/// red X for an error, a neutral dot while loading.
fn bridge_status_card<'a>(status: &SparkSettingsStatus) -> Element<'a, Message> {
    let (indicator_char, indicator_color, headline, detail): (
        &'static str,
        iced::Color,
        &'static str,
        String,
    ) = match status {
        SparkSettingsStatus::Loading => (
            "●",
            color::GREY_3,
            "Checking bridge…",
            "Waiting for the first get_info round-trip.".to_string(),
        ),
        SparkSettingsStatus::Connected => (
            "✓",
            color::GREEN,
            "Connected",
            "The Spark bridge subprocess is reachable and \
             responding to JSON-RPC requests over stdio."
                .to_string(),
        ),
        SparkSettingsStatus::Error(err) => (
            "✗",
            color::RED,
            "Disconnected",
            format!(
                "The last get_info call failed. Restarting the cube \
                 re-spawns the bridge. Error: {}",
                err
            ),
        ),
        SparkSettingsStatus::Unavailable => (
            "✗",
            color::RED,
            "Unavailable",
            "No Spark signer configured, or the bridge subprocess \
             failed to spawn."
                .to_string(),
        ),
    };

    Container::new(
        Column::new()
            .spacing(8)
            .push(h4_bold("Bridge status"))
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(iced::widget::text(indicator_char).size(18).style(
                        move |_: &theme::Theme| iced::widget::text::Style {
                            color: Some(indicator_color),
                        },
                    ))
                    .push(p1_regular(headline)),
            )
            .push(p2_regular(detail)),
    )
    .padding(12)
    .style(theme::card::simple)
    .into()
}
