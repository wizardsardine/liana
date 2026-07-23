//! View renderer for [`crate::app::state::spark::settings::SparkSettings`].
//!
//! Renders the General sub-page:
//! - A "Stable Balance" toggle card (USD-pegging feature) with a
//!   clear on/off status line and a toggle button. Disabled while
//!   the bridge is unavailable or an `update_user_settings` RPC
//!   is in flight.
//! - A small "Bridge status" diagnostic card showing whether the
//!   Spark bridge subprocess is reachable (`get_info` round-trip
//!   successful on the last reload).
//!
//! The Lightning Address sub-page lives in [`lightning_address`] and
//! is rendered by `App::view` directly so it can pass a borrow of the
//! `ConnectCubePanel` into the form (the `State::view` trait signature
//! doesn't reach Connect state).

pub mod lightning_address;

use coincube_ui::{
    color,
    component::{
        button,
        text::{h3, h4_bold, p1_regular, p2_regular, Text as _},
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
    /// Render the General sub-page. The Lightning Address sub-page is
    /// dispatched at the App level (see [`super::lightning_address`])
    /// because it needs a borrow of `ConnectCubePanel` that the
    /// `State::view` trait signature can't provide.
    pub fn render<'a>(self) -> Element<'a, Message> {
        // Match the Cube/Vault settings pages: plain `h3` heading, no
        // container wrapper. The rail already shows Spark → Settings.
        let heading = h3("Settings").bold();

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
    let status_line: Element<'_, Message> =
        p2_regular(stable_balance_status_text(active, spark_available)).into();

    let toggle = stable_balance_toggle_state(active, saving, spark_available);
    let toggle_btn = button::primary(None, toggle.button_label).width(Length::Fixed(140.0));
    let toggle_btn: Element<'_, Message> = if toggle.can_toggle {
        toggle_btn
            .on_press(Message::SparkSettings(
                SparkSettingsMessage::StableBalanceToggled(toggle.next_state),
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

fn stable_balance_status_text(active: Option<bool>, spark_available: bool) -> &'static str {
    match (active, spark_available) {
        (_, false) => "Spark bridge unavailable — toggle disabled.",
        (None, true) => "Loading…",
        (Some(true), true) => "Stable Balance is ON",
        (Some(false), true) => "Stable Balance is OFF",
    }
}

#[derive(Debug, PartialEq, Eq)]
struct StableBalanceToggleState {
    button_label: &'static str,
    next_state: bool,
    can_toggle: bool,
}

fn stable_balance_toggle_state(
    active: Option<bool>,
    saving: bool,
    spark_available: bool,
) -> StableBalanceToggleState {
    let can_toggle = spark_available && active.is_some() && !saving;
    let target = active.unwrap_or(false);
    let (button_label, next_state) = if target {
        ("Turn off", false)
    } else {
        ("Turn on", true)
    };
    StableBalanceToggleState {
        button_label,
        next_state,
        can_toggle,
    }
}

/// Small live diagnostic card — shows whether the Spark bridge
/// subprocess is reachable. Green check for a healthy round-trip,
/// red X for an error, a neutral dot while loading.
fn bridge_status_card<'a>(status: &SparkSettingsStatus) -> Element<'a, Message> {
    let (indicator_char, indicator_color, headline, detail) = bridge_status_copy(status);

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

fn bridge_status_copy(
    status: &SparkSettingsStatus,
) -> (&'static str, iced::Color, &'static str, String) {
    match status {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_balance_status_text_covers_loading_on_off_and_unavailable() {
        assert_eq!(
            stable_balance_status_text(None, false),
            "Spark bridge unavailable — toggle disabled."
        );
        assert_eq!(stable_balance_status_text(None, true), "Loading…");
        assert_eq!(
            stable_balance_status_text(Some(true), true),
            "Stable Balance is ON"
        );
        assert_eq!(
            stable_balance_status_text(Some(false), true),
            "Stable Balance is OFF"
        );
    }

    #[test]
    fn stable_balance_toggle_state_only_enables_when_loaded_available_and_idle() {
        assert_eq!(
            stable_balance_toggle_state(Some(true), false, true),
            StableBalanceToggleState {
                button_label: "Turn off",
                next_state: false,
                can_toggle: true,
            }
        );
        assert_eq!(
            stable_balance_toggle_state(Some(false), false, true),
            StableBalanceToggleState {
                button_label: "Turn on",
                next_state: true,
                can_toggle: true,
            }
        );
        assert!(!stable_balance_toggle_state(None, false, true).can_toggle);
        assert!(!stable_balance_toggle_state(Some(true), true, true).can_toggle);
        assert!(!stable_balance_toggle_state(Some(true), false, false).can_toggle);
    }

    #[test]
    fn bridge_status_copy_names_loading_connected_error_and_unavailable() {
        let loading = bridge_status_copy(&SparkSettingsStatus::Loading);
        assert_eq!(loading.0, "●");
        assert_eq!(loading.1, color::GREY_3);
        assert_eq!(loading.2, "Checking bridge…");

        let connected = bridge_status_copy(&SparkSettingsStatus::Connected);
        assert_eq!(connected.0, "✓");
        assert_eq!(connected.1, color::GREEN);
        assert_eq!(connected.2, "Connected");

        let error = bridge_status_copy(&SparkSettingsStatus::Error("boom".to_string()));
        assert_eq!(error.0, "✗");
        assert_eq!(error.1, color::RED);
        assert_eq!(error.2, "Disconnected");
        assert!(error.3.contains("boom"));

        let unavailable = bridge_status_copy(&SparkSettingsStatus::Unavailable);
        assert_eq!(unavailable.0, "✗");
        assert_eq!(unavailable.1, color::RED);
        assert_eq!(unavailable.2, "Unavailable");
    }
}
