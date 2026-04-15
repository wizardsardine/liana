//! View renderer for [`crate::app::state::spark::settings::SparkSettings`].
//!
//! Renders:
//! - A "Stable Balance" toggle card (USD-pegging feature) with a
//!   clear on/off status line and a toggle button. Disabled while
//!   the bridge is unavailable or an `update_user_settings` RPC
//!   is in flight.
//! - A "Default Lightning backend" picker card with Spark/Liquid
//!   chips. Controls which backend fulfills incoming LN Address
//!   invoices for this cube.
//! - A read-only diagnostics card (identity pubkey, balance,
//!   network) fed by the bridge's `get_info` RPC.

use coincube_core::miniscript::bitcoin::Network;
use coincube_ui::{
    component::{
        button,
        text::{h2, h4_bold, p1_regular, p2_regular},
    },
    theme,
    widget::{Column, Container, Element, Row},
};
use iced::widget::Space;
use iced::{Alignment, Length};

use crate::app::state::spark::settings::SparkSettingsSnapshot;
use crate::app::view::{Message, SparkSettingsMessage};
use crate::app::wallets::WalletKind;

#[derive(Debug, Clone)]
pub enum SparkSettingsStatus {
    Unavailable,
    Loading,
    Error(String),
    Loaded(SparkSettingsSnapshot),
}

pub struct SparkSettingsView {
    pub status: SparkSettingsStatus,
    pub network: Network,
    /// Current `default_lightning_backend` preference, read from
    /// `Cache`. The picker reflects this value and fires a
    /// `DefaultLightningBackendChanged` message on click.
    pub default_lightning_backend: WalletKind,
    /// Whether the Spark backend is actually available for this cube.
    /// Drives whether the "Spark" chip is enabled — if the bridge is
    /// down the user shouldn't be able to pick it as the default.
    pub spark_available: bool,
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
        let picker = lightning_backend_picker(
            self.default_lightning_backend,
            self.spark_available,
        );
        let stable_balance_card = stable_balance_card(
            self.stable_balance_active,
            self.stable_balance_saving,
            self.spark_available,
        );

        let body: Element<'_, Message> = match self.status {
            SparkSettingsStatus::Unavailable => Column::new()
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
                ))
                .into(),
            SparkSettingsStatus::Loading => Column::new()
                .push(p1_regular("Fetching wallet info from the Spark bridge…"))
                .into(),
            SparkSettingsStatus::Error(err) => Column::new()
                .spacing(10)
                .push(p1_regular("Spark bridge error"))
                .push(p2_regular(err))
                .into(),
            SparkSettingsStatus::Loaded(snapshot) => Column::new()
                .spacing(14)
                .push(setting_row(
                    "Balance",
                    format!("{} sats", snapshot.balance_sats),
                ))
                .push(setting_row(
                    "Identity pubkey",
                    snapshot.identity_pubkey,
                ))
                .push(setting_row("Network", format_network(self.network)))
                .push(Space::new().height(Length::Fixed(12.0)))
                .push(diagnostic_note())
                .into(),
        };

        Column::new()
            .spacing(20)
            .push(heading)
            .push(stable_balance_card)
            .push(picker)
            .push(body)
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

    // Target state for the button: if currently ON, pressing turns
    // it OFF, and vice versa. When we don't know yet (loading) or
    // saving is in flight, press is disabled.
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

fn lightning_backend_picker<'a>(
    current: WalletKind,
    spark_available: bool,
) -> Element<'a, Message> {
    let spark_btn = if current == WalletKind::Spark {
        button::primary(None, "Spark")
    } else {
        button::transparent_border(None, "Spark")
    };
    let spark_btn = spark_btn.width(Length::Fixed(140.0));
    let spark_btn: Element<'_, Message> = if spark_available {
        spark_btn
            .on_press(Message::SparkSettings(
                SparkSettingsMessage::DefaultLightningBackendChanged(WalletKind::Spark),
            ))
            .into()
    } else {
        // No bridge — disable the chip so the user can't select a
        // backend that isn't wired up for this cube.
        spark_btn.on_press_maybe(None).into()
    };

    let liquid_btn = if current == WalletKind::Liquid {
        button::primary(None, "Liquid")
    } else {
        button::transparent_border(None, "Liquid")
    };
    let liquid_btn: Element<'_, Message> = liquid_btn
        .width(Length::Fixed(140.0))
        .on_press(Message::SparkSettings(
            SparkSettingsMessage::DefaultLightningBackendChanged(WalletKind::Liquid),
        ))
        .into();

    Container::new(
        Column::new()
            .spacing(10)
            .push(h4_bold("Default Lightning backend"))
            .push(p2_regular(
                "Chooses which wallet fulfills incoming Lightning \
                 Address invoices for this cube. Spark is the default \
                 when available; Liquid is the fallback and handles \
                 NIP-57 zaps (whose description is too long for \
                 Spark's invoice description field).",
            ))
            .push(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push(spark_btn)
                    .push(liquid_btn),
            ),
    )
    .padding(12)
    .style(theme::card::simple)
    .into()
}

fn setting_row<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    Container::new(
        Row::new()
            .spacing(20)
            .align_y(Alignment::Start)
            .push(
                Column::new()
                    .width(Length::FillPortion(1))
                    .push(h4_bold(label)),
            )
            .push(
                Column::new()
                    .width(Length::FillPortion(3))
                    .push(p1_regular(value)),
            ),
    )
    .padding(12)
    .style(theme::card::simple)
    .into()
}

fn format_network(network: Network) -> String {
    // We only currently support Bitcoin + Regtest on the Spark side
    // (see the network check in breez_spark::config). Render the
    // mainnet label as "Mainnet" instead of "bitcoin" for readability.
    match network {
        Network::Bitcoin => "Mainnet".to_string(),
        Network::Regtest => "Regtest".to_string(),
        other => format!("{}", other),
    }
}

fn diagnostic_note<'a>() -> Element<'a, Message> {
    Container::new(
        Column::new()
            .spacing(6)
            .push(h4_bold("Diagnostics"))
            .push(p2_regular(
                "The Spark SDK runs in a sibling process (coincube-spark-bridge) \
                 and talks to the gui over JSON-RPC on stdio. Restarting the \
                 cube re-spawns the bridge. Advanced controls (Stable Balance \
                 toggle, signer rotation, manual reconnect) land in a later \
                 phase.",
            )),
    )
    .padding(12)
    .style(theme::card::simple)
    .into()
}
