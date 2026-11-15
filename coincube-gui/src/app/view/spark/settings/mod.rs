//! View renderer for [`crate::app::state::spark::settings::SparkSettings`].
//!
//! Renders the Balances & conversion tab (board H-Settings-Spark, plan S2):
//! - US dollar balance: the Auto-convert switch (bridge-driven since S2b),
//!   with its breaker state and a confirmation before it turns off, and
//!   "Pay from USDB when bitcoin runs short · Always on".
//! - Conversion: the max price change, read-only, and Swap by hand.
//! - The Spark nickname field.
//!
//! and the Connections tab, whose "Spark wallet service" row says in plain
//! words whether the Spark bridge subprocess is reachable (`get_info`
//! round-trip successful on the last reload), with the engineering detail
//! behind "Technical details".
//!
//! The Lightning address tab lives in [`lightning_address`] and
//! is rendered by `App::view` directly so it can pass a borrow of the
//! `ConnectCubePanel` into the form (the `State::view` trait signature
//! doesn't reach Connect state).

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
pub mod lightning_address;

use coincube_ui::{
    color,
    component::tenshu::{text_field, toggle, FieldSize},
    widget::{Column, Container, Element, Row},
};
use iced::widget::Space;
use iced::Length;

use crate::app::cache::Cache;
use crate::app::state::spark::settings::nickname_fits;
use crate::app::view::settings::page;
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
    /// The auto-convert breaker and hold, from `get_auto_convert`, once read.
    pub auto_convert: Option<coincube_spark_protocol::AutoConvertState>,
    /// Whether Swap exists here, for the Swap by hand row (mainnet only).
    pub swap_available: bool,
    /// Sats awaiting on-chain recovery (SDK 0.28.0+), from `get_info`.
    pub recoverable_funds_sats: u64,
}

pub const STABLE_BALANCE_ID: &str = "tenshu.settings.spark.auto-convert";
pub const SWAP_BY_HAND_ID: &str = "tenshu.settings.spark.swap-by-hand";
pub const KEEP_AUTO_CONVERT_ID: &str = "tenshu.settings.spark.keep-auto-convert";
pub const TURN_OFF_AUTO_CONVERT_ID: &str = "tenshu.settings.spark.turn-off-auto-convert";

/// What turning auto-convert off does (plan S2, master §3 decision 5).
/// Since S2b the bridge drives auto-convert and SDK Stable Balance stays
/// off, so turning it off converts nothing back.
pub const TURN_OFF_COPY: &str = "Your USDB stays USDB, and new bitcoin you receive stays bitcoin.";
pub const NICKNAME_ID: &str = "tenshu.settings.spark.nickname";

/// The Spark nickname (master plan §3 decision 6): the side panel's
/// "Spark · ‹nickname›". `input` is a name typed but not saved yet, `saved`
/// the Cube's. Return saves it, and so does leaving the tab.
pub fn nickname_group<'a>(input: Option<&'a str>, saved: Option<&'a str>) -> Element<'a, Message> {
    let value = input.or(saved).unwrap_or_default();
    let row = page::row(localized_copy("Nickname"))
        .description(localized_copy(
            "Shown in the side panel as “Spark · nickname”. Leave it empty to show “Spark”. \
             Saved when you press Return or leave this tab.",
        ))
        .control(
            Container::new(
                text_field(NICKNAME_ID, "Spark nickname", value, Message::FocusAction)
                    .hide_label()
                    .size(FieldSize::Compact)
                    .width(Length::Fixed(280.0))
                    .placeholder("Spark")
                    .error_maybe((!nickname_fits(value)).then_some("Use fewer than 64 characters."))
                    .on_input(|value| {
                        Message::SparkSettings(SparkSettingsMessage::NicknameEdited(value))
                    })
                    .on_submit(Message::SparkSettings(
                        SparkSettingsMessage::NicknameSubmitted(value.to_owned()),
                    )),
            )
            .width(Length::Fixed(280.0)),
        );
    page::group(
        localized_copy("Spark identity"),
        None,
        page::rows([row.into()]),
    )
}

impl SparkSettingsView {
    /// The Stable Balance group on Balances & conversion. The panel adds
    /// the nickname beneath it.
    pub fn render<'a>(self) -> Element<'a, Message> {
        if matches!(self.status, SparkSettingsStatus::Unavailable) {
            return page::group(
                localized_copy("Spark isn't available"),
                Some(
                    "Spark is not configured for this Cube, or the bridge subprocess failed to \
                     spawn. Configure a Spark signer on this Cube and restart the app to connect."
                        .into(),
                ),
                Space::new(),
            );
        }
        let bridge_reachable = matches!(self.status, SparkSettingsStatus::Connected);
        Column::new()
            .width(Length::Fill)
            .push(stable_balance_group(
                self.stable_balance_active,
                self.stable_balance_saving,
                bridge_reachable,
                self.auto_convert.as_ref(),
            ))
            .push(conversion_group(self.swap_available))
            .into()
    }

    /// The Connections tab: Spark's own health in plain words, the bridge
    /// detail behind "Technical details", and one row for the Cube's Connect
    /// session that opens Cube › Connections.
    pub fn render_connections<'a>(
        self,
        cache: &'a Cache,
        technical_details_open: bool,
    ) -> Element<'a, Message> {
        let (_, _, headline, detail) = bridge_status_copy(&self.status);
        let kind = match self.status {
            SparkSettingsStatus::Connected => page::Status::Ok,
            SparkSettingsStatus::Loading => page::Status::Busy,
            SparkSettingsStatus::Error(_) | SparkSettingsStatus::Unavailable => page::Status::Bad,
        };
        let mut service = page::row(localized_copy("Spark wallet service"))
            .accessory(page::status(kind, headline))
            .description(service_summary(&self.status))
            .line(page::link(
                TECHNICAL_DETAILS_ID,
                if technical_details_open {
                    "Hide technical details"
                } else {
                    "Technical details"
                },
                Message::SparkSettings(SparkSettingsMessage::ToggleTechnicalDetails),
            ))
            .top();
        if technical_details_open {
            service = service.line(
                Container::new(page::muted(detail).font(coincube_ui::font::MONO_REGULAR))
                    .max_width(560),
            );
        }
        if self.recoverable_funds_sats > 0 {
            service = service.line(page::muted(recoverable_funds_copy(
                self.recoverable_funds_sats,
            )));
        }
        page::group(
            localized_copy("Health"),
            None,
            page::rows([
                service.into(),
                crate::app::view::settings::connections::cube_connections_row(
                    cache,
                    "tenshu.settings.spark.cube-connections",
                ),
            ]),
        )
    }
}

pub const TECHNICAL_DETAILS_ID: &str = "tenshu.settings.spark.technical-details";

/// Funds that left the Spark balance and wait to be recovered on-chain (SDK
/// 0.28.0+). Tenshu can't run the recovery yet, so this only says they exist.
fn recoverable_funds_copy(sats: u64) -> String {
    coincube_ui::localized_format!(
        "{} sats left the Spark balance and are waiting to be recovered on-chain. COINCUBE Desktop can't \
         run that recovery yet.",
        "{} sats salieron del saldo de Spark y esperan ser recuperados en cadena. COINCUBE Desktop aún no \
         puede hacer esa recuperación.",
        sats
    )
}

/// Spark's health in plain words, under "Spark wallet service".
fn service_summary(status: &SparkSettingsStatus) -> &'static str {
    match status {
        SparkSettingsStatus::Connected => "Running. Payments and balances are up to date.",
        SparkSettingsStatus::Loading => "Checking that the Spark wallet service is running.",
        SparkSettingsStatus::Error(_) => {
            "Not responding. Payments and balances may be out of date until COINCUBE Desktop restarts it."
        }
        SparkSettingsStatus::Unavailable => "Spark isn't set up for this Cube.",
    }
}

fn stable_balance_group<'a>(
    active: Option<bool>,
    saving: bool,
    spark_available: bool,
    auto_convert: Option<&coincube_spark_protocol::AutoConvertState>,
) -> Element<'a, Message> {
    use coincube_spark_protocol::AutoConvertBreaker;
    let toggle_state = stable_balance_toggle_state(active, saving, spark_available);
    let paused = auto_convert.and_then(|state| match &state.breaker {
        AutoConvertBreaker::Paused { failures, reason } => Some((*failures, reason.clone())),
        AutoConvertBreaker::Ready => None,
    });
    let held = auto_convert.is_some_and(|state| state.held_after_swap);
    let (kind, word) = match (active, spark_available) {
        (_, false) => (page::Status::Bad, "Bridge unavailable"),
        (None, true) => (page::Status::Busy, "Loading…"),
        (Some(false), true) if paused.is_some() => (page::Status::Warn, "Paused"),
        (Some(true), true) if held => (page::Status::Busy, "Waiting"),
        (Some(true), true) => (page::Status::Ok, "On"),
        (Some(false), true) => (page::Status::Off, "Off"),
    };
    let auto = page::row(localized_copy("Auto-convert received bitcoin"))
        .accessory(page::status(kind, word))
        .description(localized_copy(
            "When on, new bitcoin payments received in Spark convert to USDB. Bitcoin you already \
             have and bitcoin from manual swaps stay bitcoin. Turning this off keeps your USDB \
             as USDB. Each conversion can fill slightly differently from the rate at the time.",
        ))
        .control(
            toggle(
                STABLE_BALANCE_ID,
                active.unwrap_or(false),
                Message::FocusAction,
            )
            .on_toggle_maybe(toggle_state.can_toggle.then_some(|on| {
                Message::SparkSettings(if on {
                    SparkSettingsMessage::StableBalanceToggled(true)
                } else {
                    // Off asks first: say what happens to the USDB.
                    SparkSettingsMessage::AutoConvertOffRequested
                })
            })),
        )
        .top();
    let pay_from = page::row(localized_copy("Pay from USDB when bitcoin runs short"))
        .accessory(page::status(page::Status::Ok, localized_copy("Always on")))
        .description(localized_copy(
            "A send uses bitcoin first, then converts just enough USDB to cover the rest. The \
             review step shows the conversion before you confirm.",
        ));
    let mut column = Column::new().width(Length::Fill);
    if let Some((failures, reason)) = paused {
        column = column.push(page::note(
            coincube_ui::component::tenshu::Tone::Warn,
            page::muted(localized_format!(
                "Auto-convert paused after {} failed conversions: {}. Turn it on again to retry.", "Auto-convertido se detuvo después de las conversiones fallidas de {}: {}. Enciende de nuevo para volver a entrar.",
                failures, reason
            )),
            None,
        ));
    }
    if held && active == Some(true) {
        column = column.push(page::note(
            coincube_ui::component::tenshu::Tone::Muted,
            page::muted(localized_copy(
                "Auto-convert is waiting while a swap out of USDB settles. Bitcoin received \
                 meanwhile stays bitcoin.",
            )),
            None,
        ));
    }
    column
        .push(page::group(
            localized_copy("US dollar balance (USDB)"),
            None,
            page::rows([auto.into(), pay_from.into()]),
        ))
        .into()
}

/// The conversion settings: the slippage cap every conversion uses, shown
/// read-only (master §3 decision 6), and the way to the Swap screen.
fn conversion_group<'a>(swap_available: bool) -> Element<'a, Message> {
    use coincube_ui::component::tenshu::{ActionButton, ActionKind};
    let cap = crate::app::view::spark::swap::price_change(
        coincube_spark_protocol::CONVERSION_MAX_SLIPPAGE_BPS,
    );
    let max = page::row(localized_copy("Max price change"))
        .accessory(page::muted(cap))
        .description(localized_copy(
            "A conversion that would fill worse than this doesn't go through.",
        ));
    let mut rows: Vec<Element<'a, Message>> = vec![max.into()];
    if swap_available {
        rows.push(
            page::row(localized_copy("Swap by hand"))
                .description(localized_copy(
                    "Move any amount between bitcoin and USDB. Also on the Spark page.",
                ))
                .control(
                    ActionButton::new(
                        iced::advanced::widget::Id::new(SWAP_BY_HAND_ID),
                        localized_copy("Swap BTC ⇄ USDB"),
                        ActionKind::Secondary,
                        Message::FocusAction,
                    )
                    .compact()
                    .on_press(Message::Menu(crate::app::menu::Menu::Spark(
                        crate::app::menu::SparkSubMenu::Swap,
                    ))),
                )
                .into(),
        );
    }
    page::group(localized_copy("Conversion"), None, page::rows(rows))
}

/// "Turn off auto-convert?", over the page.
pub fn turn_off_dialog<'a>() -> Element<'a, Message> {
    use coincube_ui::component::tenshu::{ActionButton, ActionKind};
    use coincube_ui::component::text::{h4_bold, p1_regular};
    Container::new(
        Column::new()
            .spacing(16)
            .push(h4_bold(localized_copy("Turn off auto-convert?")))
            .push(p1_regular(TURN_OFF_COPY))
            .push(
                Row::new()
                    .spacing(12)
                    .push(Space::new().width(Length::Fill))
                    .push(
                        ActionButton::new(
                            iced::advanced::widget::Id::new(KEEP_AUTO_CONVERT_ID),
                            localized_copy("Keep auto-convert"),
                            ActionKind::Secondary,
                            Message::FocusAction,
                        )
                        .on_press(Message::SparkSettings(
                            SparkSettingsMessage::AutoConvertOffCancelled,
                        )),
                    )
                    .push(
                        ActionButton::new(
                            iced::advanced::widget::Id::new(TURN_OFF_AUTO_CONVERT_ID),
                            localized_copy("Turn off"),
                            ActionKind::Primary,
                            Message::FocusAction,
                        )
                        .on_press(Message::SparkSettings(
                            SparkSettingsMessage::StableBalanceToggled(false),
                        )),
                    ),
            ),
    )
    .padding(24)
    .max_width(480)
    .style(coincube_ui::theme::card::modal)
    .into()
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

fn bridge_status_copy(
    status: &SparkSettingsStatus,
) -> (&'static str, iced::Color, &'static str, String) {
    match status {
        SparkSettingsStatus::Loading => (
            "●",
            color::GREY_3,
            localized_copy("Checking…"),
            "Bridge subprocess · JSON-RPC over stdio. Waiting for the first get_info round-trip."
                .to_string(),
        ),
        SparkSettingsStatus::Connected => (
            "✓",
            color::GREEN,
            localized_copy("Connected"),
            "Bridge subprocess · JSON-RPC over stdio. The last get_info round-trip succeeded."
                .to_string(),
        ),
        SparkSettingsStatus::Error(err) => (
            "✗",
            color::RED,
            localized_copy("Disconnected"),
            format!(
                "Bridge subprocess · JSON-RPC over stdio. The last get_info call failed; \
                 restarting the Cube re-spawns the bridge. Error: {}",
                err
            ),
        ),
        SparkSettingsStatus::Unavailable => (
            "✗",
            color::RED,
            localized_copy("Unavailable"),
            "Bridge subprocess · JSON-RPC over stdio. No Spark signer is configured, or the \
             bridge subprocess failed to spawn."
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Settings › Spark › Balances & conversion under the Settings head: the
    /// scope switch, the Spark tabs and the nickname (Tenshu v175 W5).
    /// Opt-in: `TOP_BAR_CAPTURE_DIR=<dir> cargo test -p coincube-gui --lib
    /// page_head_capture -- --ignored`.
    #[test]
    #[ignore = "writes PNG frames; needs a GPU adapter"]
    fn page_head_capture_settings_spark() {
        use crate::app::menu::{Menu, SparkSettingsOption, SparkSubMenu};
        use crate::app::state::preferences::BalancePrivacy;
        use crate::app::view::nav::shell::capture_tests;
        let Some(directory) = capture_tests::directory() else {
            return;
        };
        let mut renderer = capture_tests::renderer();
        let menu = Menu::Spark(SparkSubMenu::Settings(Some(SparkSettingsOption::General)));
        for (appearance, palette) in capture_tests::themes() {
            let mut cache = capture_tests::cache(appearance == "light", BalancePrivacy::Shown);
            cache.wallet_names.spark = Some("Daily spending".into());
            capture_tests::save_page_head_frames(
                &mut renderer,
                &directory,
                "settings-spark",
                appearance,
                &palette,
                || {
                    crate::app::view::dashboard(
                        &menu,
                        &cache,
                        Column::new()
                            .width(Length::Fill)
                            .push(
                                SparkSettingsView {
                                    status: SparkSettingsStatus::Connected,
                                    stable_balance_active: Some(false),
                                    stable_balance_saving: false,
                                    auto_convert: None,
                                    swap_available: true,
                                    recoverable_funds_sats: 0,
                                }
                                .render(),
                            )
                            .push(nickname_group(None, cache.wallet_names.spark.as_deref())),
                    )
                },
            );
        }
    }

    fn balances(
        active: bool,
        auto_convert: Option<coincube_spark_protocol::AutoConvertState>,
        swap_available: bool,
    ) -> crate::app::view::shared::landing::capture::Inspection {
        crate::app::view::shared::landing::capture::inspect(
            "spark-settings-balances",
            SparkSettingsView {
                status: SparkSettingsStatus::Connected,
                stable_balance_active: Some(active),
                stable_balance_saving: false,
                auto_convert,
                swap_available,
                recoverable_funds_sats: 0,
            }
            .render(),
            (1440, 900),
            "dark",
            coincube_ui::theme::Theme::dark(),
        )
    }

    /// H-Settings-Spark's rows, with S2's copy: turning auto-convert off
    /// keeps USDB, so nothing says it converts back.
    #[test]
    fn balances_and_conversion_rows_say_what_auto_convert_does() {
        let probe = balances(true, None, true);
        for label in [
            "US dollar balance (USDB)",
            "Auto-convert received bitcoin",
            "When on, new bitcoin payments received in Spark convert to USDB.",
            "have and bitcoin from manual swaps stay bitcoin.",
            "Turning this off keeps your USDB as USDB.",
            "Pay from USDB when bitcoin runs short",
            "Always on",
            "Conversion",
            "Max price change",
            "0.5%",
            "Swap by hand",
            "Swap BTC ⇄ USDB",
        ] {
            assert!(probe.contains(label), "{}", label);
        }
        assert!(!probe.contains("back to bitcoin") && !probe.contains("Stable Balance"));
        assert!(!balances(true, None, false).contains("Swap by hand"));
    }

    #[test]
    fn the_switch_asks_before_turning_off_and_turns_on_at_once() {
        let content = || {
            SparkSettingsView {
                status: SparkSettingsStatus::Connected,
                stable_balance_active: Some(true),
                stable_balance_saving: false,
                auto_convert: None,
                swap_available: true,
                recoverable_funds_sats: 0,
            }
            .render()
        };
        let off = crate::app::view::shared::landing::capture::activate(
            content(),
            iced::advanced::widget::Id::new(STABLE_BALANCE_ID),
            (1440, 900),
            true,
        );
        assert!(off.iter().any(|m| matches!(
            m,
            Message::SparkSettings(SparkSettingsMessage::AutoConvertOffRequested)
        )));
        assert!(!off.iter().any(|m| matches!(
            m,
            Message::SparkSettings(SparkSettingsMessage::StableBalanceToggled(false))
        )));
        let on = crate::app::view::shared::landing::capture::activate(
            SparkSettingsView {
                status: SparkSettingsStatus::Connected,
                stable_balance_active: Some(false),
                stable_balance_saving: false,
                auto_convert: None,
                swap_available: true,
                recoverable_funds_sats: 0,
            }
            .render(),
            iced::advanced::widget::Id::new(STABLE_BALANCE_ID),
            (1440, 900),
            true,
        );
        assert!(on.iter().any(|m| matches!(
            m,
            Message::SparkSettings(SparkSettingsMessage::StableBalanceToggled(true))
        )));

        let dialog = crate::app::view::shared::landing::capture::inspect(
            "spark-settings-turn-off",
            turn_off_dialog(),
            (1440, 900),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        assert!(dialog.contains("Turn off auto-convert?") && dialog.contains(TURN_OFF_COPY));
        for (id, expected) in [
            (TURN_OFF_AUTO_CONVERT_ID, true),
            (KEEP_AUTO_CONVERT_ID, false),
        ] {
            let messages = crate::app::view::shared::landing::capture::activate(
                turn_off_dialog(),
                iced::advanced::widget::Id::new(id),
                (1440, 900),
                true,
            );
            assert_eq!(
                messages.iter().any(|m| matches!(
                    m,
                    Message::SparkSettings(SparkSettingsMessage::StableBalanceToggled(false))
                )),
                expected,
                "{}",
                id
            );
        }
    }

    #[test]
    fn a_paused_breaker_shows_its_reason() {
        use coincube_spark_protocol::{AutoConvertBreaker, AutoConvertState};
        let probe = balances(
            false,
            Some(AutoConvertState {
                enabled: false,
                breaker: AutoConvertBreaker::Paused {
                    failures: 3,
                    reason: "price moved".into(),
                },
                held_after_swap: false,
            }),
            true,
        );
        assert!(probe.contains("Paused"));
        assert!(probe.contains("paused after 3 failed conversions: price moved"));
        let held = balances(
            true,
            Some(AutoConvertState {
                enabled: true,
                breaker: AutoConvertBreaker::Ready,
                held_after_swap: true,
            }),
            true,
        );
        assert!(held.contains("waiting while a swap out of USDB settles"));
    }

    /// Spark › Connections: Spark's own health in plain words, the bridge
    /// detail only behind Technical details, and one link row to Cube ›
    /// Connections instead of the account box.
    #[test]
    fn spark_connections_speaks_plainly_and_folds_the_bridge_detail() {
        let cache = Cache {
            connect_authenticated: true,
            connect_stream_status: crate::app::ConnectionStatus::Connected,
            ..Cache::default()
        };
        let view = || SparkSettingsView {
            status: SparkSettingsStatus::Connected,
            stable_balance_active: Some(false),
            stable_balance_saving: false,
            auto_convert: None,
            swap_available: true,
            recoverable_funds_sats: 0,
        };
        let inspect = |content| {
            crate::app::view::shared::landing::capture::inspect(
                "spark-settings-connections",
                content,
                (1440, 900),
                "dark",
                coincube_ui::theme::Theme::dark(),
            )
        };
        let closed = inspect(view().render_connections(&cache, false));
        for label in [
            "Spark wallet service",
            "Running. Payments and balances are up to date.",
            "Technical details",
            "Connect live updates",
            "Cube connections",
        ] {
            assert!(closed.contains(label), "{}", label);
        }
        for gone in ["JSON-RPC", "Cube service session", "Manage in Connect"] {
            assert!(!closed.contains(gone), "{}", gone);
        }
        let open = inspect(view().render_connections(&cache, true));
        assert!(open.contains("Bridge subprocess · JSON-RPC over stdio"));
        let toggled = crate::app::view::shared::landing::capture::activate(
            view().render_connections(&cache, false),
            iced::advanced::widget::Id::new(TECHNICAL_DETAILS_ID),
            (1440, 900),
            true,
        );
        assert!(toggled.iter().any(|m| matches!(
            m,
            Message::SparkSettings(SparkSettingsMessage::ToggleTechnicalDetails)
        )));
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
        assert_eq!(loading.2, "Checking…");

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
