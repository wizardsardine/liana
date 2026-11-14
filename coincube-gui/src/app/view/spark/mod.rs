//! View-layer types for the Spark wallet panels.
//!
//! Modules: Overview (balance + "Stable" badge), Send (BOLT11 /
//! BIP21 / LNURL-pay), Receive (BOLT11 invoice, on-chain deposit
//! address with claim lifecycle), Transactions (recent payments),
//! and Settings (Stable Balance toggle, default Lightning backend
//! picker, diagnostics). [`SparkPlaceholderView`] is kept around
//! as a generic "coming soon" slot for future panels.

use coincube_ui::locale::copy as localized_copy;
pub mod last_tx;
pub mod overview;
pub mod receive;
pub mod send;
pub mod settings;
pub mod sideshift_receive;
pub mod swap;
pub mod transactions;

pub use overview::{SparkOverviewView, SparkPaymentMethod, SparkRecentTransaction, SparkStatus};
pub use receive::SparkReceiveView;
pub use send::SparkSendView;
pub use settings::{SparkSettingsStatus, SparkSettingsView};
pub use sideshift_receive::spark_sideshift_receive_view;
pub use transactions::{SparkTransactionsStatus, SparkTransactionsView};

/// The inside of a slim route tile, shared by Send and Receive: the asset
/// logo (its network badge composited in the lower-right corner), a small
/// sentence-case label with an optional right-aligned hint, "**Asset** where",
/// and — on a tile that opens a picker — a chevron. The caller draws the
/// tile's surface (a plain container, or the button that makes it tappable).
pub(crate) fn route_tile_body<'a, M: 'a>(
    logo: coincube_ui::widget::Element<'a, M>,
    label: &'static str,
    hint: Option<&'static str>,
    asset: String,
    place: String,
    // `Some(open)` for a tile that opens a menu: the chevron points up while
    // its menu is open.
    chevron: Option<bool>,
) -> coincube_ui::widget::Element<'a, M> {
    route_tile_body_with_detail(logo, label, hint, asset, place, None, chevron)
}

/// [`route_tile_body`] with a `detail` line under the asset, such as Swap's
/// "Available 34,034 SATS".
pub(crate) fn route_tile_body_with_detail<'a, M: 'a>(
    logo: coincube_ui::widget::Element<'a, M>,
    label: &'static str,
    hint: Option<&'static str>,
    asset: String,
    place: String,
    detail: Option<String>,
    chevron: Option<bool>,
) -> coincube_ui::widget::Element<'a, M> {
    use coincube_ui::component::text::*;
    use coincube_ui::theme;
    use coincube_ui::widget::{Column, ColumnExt, Container, Row};
    use iced::widget::Space;
    use iced::{Alignment, Length};

    let mut caption = Row::new().spacing(8).align_y(Alignment::Center).push(
        text(localized_copy(label))
            .size(CAPTION_SIZE + 1)
            .style(theme::text::secondary),
    );
    if let Some(hint) = hint {
        caption = caption.push(Space::new().width(Length::Fill)).push(
            text(localized_copy(hint))
                .size(CAPTION_SIZE + 1)
                .style(theme::text::secondary),
        );
    }
    let mut row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(logo)
        .push(
            Column::new()
                .spacing(2)
                .width(Length::Fill)
                .push(caption)
                .push(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::End)
                        .push(
                            text(asset)
                                .size(P1_SIZE + 2)
                                .bold()
                                .style(theme::text::primary),
                        )
                        .push(text(place).size(P1_SIZE + 2).style(theme::text::secondary)),
                )
                .push_maybe(detail.map(|detail| {
                    text(detail)
                        .size(CAPTION_SIZE + 1)
                        .style(theme::text::secondary)
                })),
        );
    if let Some(open) = chevron {
        row = row.push(
            if open {
                coincube_ui::icon::up_icon()
            } else {
                coincube_ui::icon::down_icon()
            }
            .size(16)
            .style(theme::text::secondary),
        );
    }
    Container::new(row)
        .padding([12, 14])
        .width(Length::Fill)
        .into()
}

/// The pinned head of a Spark sub-page such as Send or Receive: `title`, the
/// wallet line, then Back to Spark as the last action.
pub fn sub_page_head<'a>(
    cache: &crate::app::cache::Cache,
    title: &'a str,
) -> coincube_ui::component::tenshu::PageHead<'a, crate::app::view::Message> {
    crate::app::view::spark_page_head(cache, title).back(
        "Spark",
        crate::app::view::Message::Menu(crate::app::menu::Menu::Spark(
            crate::app::menu::SparkSubMenu::Overview,
        )),
    )
}

/// The flow on Spark Send's head rule.
pub const SEND_STEPS: [&str; 3] = ["Details", "Review", "Sent"];

/// What a Spark Send or Receive side panel lists (`.aside`): each balance in
/// its own unit, then their total in the display fiat.
#[derive(Clone, Copy)]
pub struct SparkBalances<'a> {
    /// The bitcoin balance alone, without USDB.
    pub btc_sats: u64,
    pub stable: Option<&'a coincube_spark_protocol::UsdbBalanceSnapshot>,
    pub fiat_converter: Option<crate::app::view::FiatAmountConverter>,
    /// The dedicated BTC/USD quote. USDB counts toward the total only at it.
    pub btc_usd_price: Option<f64>,
    pub bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    pub masked: bool,
}

impl<'a> SparkBalances<'a> {
    pub fn new(
        cache: &crate::app::cache::Cache,
        btc_sats: u64,
        stable: Option<&'a coincube_spark_protocol::UsdbBalanceSnapshot>,
    ) -> Self {
        Self {
            btc_sats,
            stable,
            fiat_converter: cache
                .fiat_price
                .as_ref()
                .and_then(|p| std::convert::TryInto::try_into(p).ok()),
            btc_usd_price: crate::app::state::spark::reference_btc_usd_price(cache),
            bitcoin_unit: cache.bitcoin_unit,
            masked: cache.balance_amounts_masked(),
        }
    }

    /// The rows as (label, figure, unit), then the total, if it can be
    /// valued: without a fiat price, or with USDB but no BTC/USD quote, there
    /// is none.
    pub fn rows(&self) -> (Vec<(&'static str, String, String)>, Option<String>) {
        use crate::app::breez_spark::assets::{
            format_token_display_u128, stable_token_as_sats_u128,
        };
        use coincube_ui::component::amount::DisplayAmount;
        let usdb_units = self.stable.map_or(0, |sb| sb.balance);
        let mask = |figure: String| {
            if self.masked {
                coincube_ui::component::tenshu::MASK.to_owned()
            } else {
                figure
            }
        };
        let rows = overview::spark_assets(self.btc_sats, usdb_units)
            .into_iter()
            .map(|asset| match asset {
                overview::SparkAsset::Bitcoin => (
                    "Bitcoin",
                    mask(
                        coincube_core::miniscript::bitcoin::Amount::from_sat(self.btc_sats)
                            .to_formatted_string_with_unit(self.bitcoin_unit),
                    ),
                    self.bitcoin_unit.label().to_owned(),
                ),
                overview::SparkAsset::UsDollar => (
                    "US Dollar",
                    mask(format_token_display_u128(
                        usdb_units,
                        self.stable.map_or(0, |sb| sb.decimals),
                    )),
                    "USDB".to_owned(),
                ),
            })
            .collect();
        let price = self.btc_usd_price.filter(|p| *p > 0.0 && p.is_finite());
        let total = self
            .fiat_converter
            .filter(|_| usdb_units == 0 || price.is_some())
            .map(|converter| {
                let usdb_sats = self.stable.map_or(0, |sb| {
                    stable_token_as_sats_u128(sb.balance, sb.decimals, price)
                });
                let fiat = converter.convert(coincube_core::miniscript::bitcoin::Amount::from_sat(
                    self.btc_sats.saturating_add(usdb_sats),
                ));
                mask(format!(
                    "{} {}",
                    fiat.to_formatted_string(),
                    fiat.currency()
                ))
            });
        (rows, total)
    }

    /// "Available in Spark", the balances and their total.
    pub fn view<'b>(&self) -> coincube_ui::widget::Element<'b, crate::app::view::Message> {
        use coincube_ui::component::tenshu::text::{body, caption, label};
        use coincube_ui::widget::{Column, Container, Row};
        use iced::Length;
        let (rows, total) = self.rows();
        let row = |name: String, figure: String, unit: String| {
            Row::new()
                .spacing(12)
                .align_y(iced::Alignment::Center)
                .push(Container::new(body(name)).width(Length::Fill))
                .push(label(figure))
                .push(caption(unit))
        };
        let mut column = Column::new()
            .spacing(10)
            .push(caption(localized_copy("Available in Spark")).size(13));
        for (name, figure, unit) in rows {
            column = column.push(row(name.to_owned(), figure, unit));
        }
        if let Some(total) = total {
            column = column.push(iced::widget::rule::horizontal(1)).push(row(
                "Total".to_owned(),
                total,
                String::new(),
            ));
        }
        column.into()
    }
}

/// Messages for the Swap panel ([`crate::app::state::spark::swap`]).
#[derive(Debug, Clone)]
pub enum SparkSwapMessage {
    AmountEdited(String),
    /// Convert all of the source asset.
    Max,
    /// Flip the direction.
    Reverse,
    GetQuote,
    /// Leave Review for the form.
    EditAmount,
    /// Swap with the estimate on Review.
    Confirm,
    /// Start over after a result.
    SwapAgain,
    BalanceLoaded(Option<(u64, Option<coincube_spark_protocol::UsdbBalanceSnapshot>)>),
    LimitsLoaded {
        from: coincube_spark_protocol::WalletAsset,
        limits: Option<coincube_spark_protocol::ConversionLimits>,
    },
    /// What auto-convert does next (`None`: it couldn't be read), for the
    /// read numbered `read`.
    AutoConvertLoaded {
        read: u64,
        line: Option<crate::app::state::spark::swap::AutoConvertLine>,
    },
    QuoteLoaded {
        request: u64,
        result: Result<coincube_spark_protocol::PrepareSwapOk, String>,
    },
    Swapped {
        request: u64,
        result: crate::app::state::spark::swap::SwapAttempt,
    },
    /// Spark's latest payments, for the side panel's Recent in Spark.
    /// `App` hands them to [`SparkSwap::set_recent_payments`] with the cache,
    /// which prices them.
    ///
    /// [`SparkSwap::set_recent_payments`]: crate::app::state::spark::swap::SparkSwap::set_recent_payments
    PaymentsLoaded(Vec<coincube_spark_protocol::PaymentSummary>),
    PaymentsFailed(String),
    /// Open the payment numbered so in Recent in Spark in Activity.
    SelectTransaction(usize),
    /// See all in Activity.
    History,
}

/// View-level messages for the Spark Overview panel.
#[derive(Debug, Clone)]
pub enum SparkOverviewMessage {
    /// Bridge returned `get_info` + `list_payments` success.
    DataLoaded {
        balance: coincube_core::miniscript::bitcoin::Amount,
        /// The validated USDB holding from `get_info` (`usdb_holding`), when
        /// there is one.
        usdb_balance: Option<coincube_spark_protocol::UsdbBalanceSnapshot>,
        recent_payments: Vec<coincube_spark_protocol::PaymentSummary>,
    },
    /// Bridge returned an error response.
    Error(String),
    /// Phase 6: bridge returned the current Stable Balance flag,
    /// fetched alongside `get_info` in `reload`. Drives the
    /// "Stable" badge next to the balance line.
    StableBalanceLoaded(bool),
    /// Sidebar / card actions that navigate to sibling Spark panels.
    SendBtc,
    /// The US Dollar row's Send: Spark's Send, paying from US Dollar.
    SendUsdb,
    ReceiveBtc,
    /// The US Dollar row's Receive: Spark's Receive with USDB sent and
    /// received.
    ReceiveUsdb,
    /// The "conversion in progress" notice: open Spark's Receive on the
    /// cross-network conversion.
    ViewConversion,
    History,
    SelectTransaction(String),
    /// Use the existing Spark Settings handler for an in-place landing toggle.
    ToggleStableBalance(bool),
    /// Request whole-tree focus after a pointer interaction with a landing action.
    FocusAction(iced::advanced::widget::Id),
    /// Forwarded to the top-level handler to flip the global
    /// fiat-native ↔ bitcoin-native display mode.
    FlipDisplayMode,
    /// Open the share panel for this Lightning address.
    ShareLightningAddress(String),
    /// Close the share panel (Escape or a press outside it).
    CloseLightningShare,
    /// Copy the shared Lightning address.
    CopyLightningAddress(String),
    /// Save the shared Lightning address's QR code as a PNG.
    SaveLightningQr(String),
    /// Open Settings › Spark › Lightning address with its change form.
    ChangeLightningAddress,
    /// Open Settings › Spark › Lightning address, where the claim form lives.
    ClaimLightningAddress,
}

/// View-level messages for the Spark Transactions panel.
#[derive(Debug, Clone)]
pub enum SparkTransactionsMessage {
    /// Bridge returned `list_payments` success. The `u64` is the
    /// fetch-generation token — the panel discards any response whose
    /// token isn't the latest, so a stale pagination response can't
    /// overwrite data from a newer reload.
    DataLoaded(u64, Vec<coincube_spark_protocol::PaymentSummary>),
    /// Bridge returned an error response for `list_payments`. The `u64`
    /// is the fetch-generation token (see [`Self::DataLoaded`]).
    Error(u64, String),
    /// User clicked a row. Opens the detail pane for the payment
    /// at that index in the current `recent_transactions` list.
    Select(usize),
    /// Preselect a specific payment to display in the detail pane —
    /// used by Overview / Send / Receive to hand off a row when the
    /// user taps there. The panel switches into detail mode without
    /// needing an index lookup against its own list.
    Preselect(crate::app::view::spark::SparkRecentTransaction),
    /// Empty-state navigation: "Send sats" button.
    SendBtc,
    /// Empty-state navigation: "Receive sats" button.
    ReceiveBtc,
    /// Go to the previous page in the paginated list.
    PrevPage,
    /// Go to the next page in the paginated list.
    NextPage,
}

/// View-level messages for the Spark Settings panel.
#[derive(Debug, Clone)]
pub enum SparkSettingsMessage {
    /// Bridge `get_info` reload succeeded — the subprocess is
    /// reachable and the SDK is past init. Drives the "Bridge
    /// status" card on the Settings page. `recoverable_funds_sats` is
    /// `get_info`'s figure for funds awaiting on-chain recovery.
    BridgeReachable { recoverable_funds_sats: u64 },
    /// Bridge `get_info` reload failed. Carries the error string
    /// for the diagnostic card.
    BridgeError(String),
    /// Bridge returned the current Stable Balance + private mode
    /// state. Fired from the panel's `reload` task so the view can
    /// reflect whatever the SDK persisted across restarts.
    UserSettingsLoaded(coincube_spark_protocol::GetUserSettingsOk),
    /// The user flipped the Stable Balance toggle — fires a
    /// `set_stable_balance` RPC on the bridge.
    StableBalanceToggled(bool),
    /// `set_stable_balance` RPC finished. `Ok(enabled)` carries the
    /// new state so the view can update immediately without
    /// re-fetching; `Err` surfaces the SDK error.
    StableBalanceSaved(Result<bool, String>),
    /// The auto-convert state from `get_auto_convert` (`None` if it
    /// couldn't be read): the switch, the breaker and the post-swap hold.
    AutoConvertLoaded(Option<coincube_spark_protocol::AutoConvertState>),
    /// The switch was flipped off: ask before turning it off.
    AutoConvertOffRequested,
    /// Keep auto-convert: close the question.
    AutoConvertOffCancelled,
    /// Show or hide the Spark wallet service's technical details on
    /// Connections.
    ToggleTechnicalDetails,
    /// The user typed in the Spark nickname field on Balances & conversion.
    NicknameEdited(String),
    /// Return in the nickname field, or leaving the tab with a typed name.
    /// The App saves it to this Cube's settings.
    NicknameSubmitted(String),
    /// The App saved this nickname (`None`: plain "Spark"), or found it
    /// already saved. The field shows the saved name again unless the user
    /// has typed another since.
    NicknameSaved(Option<String>),
}

/// View-level messages for the Phase 4c Spark Send panel. Drives the
/// state machine in [`crate::app::state::spark::send::SparkSend`].
#[derive(Debug, Clone)]
pub enum SparkSendMessage {
    DestinationInputChanged(String),
    AmountInputChanged(String),
    /// Open the "THEY RECEIVE" picker modal (bitcoin rails + USDt/USDC).
    OpenReceivePicker,
    /// Dismiss the "THEY RECEIVE" picker without changing the selection.
    CloseReceivePicker,
    /// Pick what the recipient receives (a bitcoin rail or a stablecoin).
    SetReceiveTarget(crate::app::state::spark::send::SparkSendTarget),
    PrepareRequested,
    PreparationFinished {
        generation: u64,
        result: Result<crate::app::state::spark::send::PreparedSend, String>,
        recipient: Option<(
            crate::services::branta::LookupTicket,
            crate::services::branta::LookupResult,
        )>,
    },
    OpenRecipientIdentity(usize),
    /// Open the "You send from" picker (S4).
    OpenPayFromPicker,
    ClosePayFromPicker,
    /// Pay this send from Bitcoin or US Dollar.
    SelectPayFrom(coincube_spark_protocol::WalletAsset),
    /// Open Send already paying from this balance (the Overview row's Send).
    /// Unlike [`Self::SelectPayFrom`] it doesn't wait for an amount: the
    /// coverage check runs again at Prepare.
    StartFrom(coincube_spark_protocol::WalletAsset),
    ConfirmRequested,
    /// Review's Back: leave the quote for the form, keeping what was typed.
    EditDetails,
    SendSucceeded(coincube_spark_protocol::SendPaymentOk),
    SendFailed(String),
    /// Reset back to the `Idle` phase, clearing inputs and any
    /// prepared/sent state. Fired from the "Send another" / "Try
    /// again" / "Cancel" buttons.
    Reset,
    /// The sent receipt's View in Activity: [`Self::Reset`], then Activity.
    ViewActivity,
    /// The Spark wallet balance for the YOU SEND card — BTC sats plus the Stable
    /// Balance snapshot, folded into a unified total by the handler. `None` when
    /// the `get_info` fetch failed; the card keeps its last value.
    BalanceLoaded(Option<(u64, Option<coincube_spark_protocol::UsdbBalanceSnapshot>)>),
    /// A `list_payments` RPC completed — used to populate the Last
    /// Transactions section under the Send form.
    PaymentsLoaded(Vec<coincube_spark_protocol::PaymentSummary>),
    /// A `list_payments` RPC failed — silently clears the list.
    PaymentsFailed(String),
    /// User tapped a row in Last Transactions.
    SelectTransaction(usize),
    /// User tapped "View All Transactions".
    History,

    // ── Cross-chain stablecoin send ─────────────────────────────────────
    /// Prepare looked the destination up because Details hadn't yet. With
    /// one network to send on (or one already chosen), the quote follows at
    /// once; an EVM address several networks reach goes back to Details to
    /// choose.
    CrossChainRoutesLoaded(
        u64,
        Result<coincube_spark_protocol::CrossChainRoutesOk, String>,
    ),
    /// The destination stopped changing long enough to look up which network
    /// it belongs to (stablecoin targets only). Carries the lookup sequence;
    /// a stale one is ignored.
    DetectDestinationNetwork(u64),
    /// The bridge answered that lookup.
    DestinationNetworkDetected(
        u64,
        Result<coincube_spark_protocol::CrossChainRoutesOk, String>,
    ),
    /// User chose the network an EVM address should be paid on (display
    /// name, as [`crate::app::state::spark::cross_chain::chain_name`] gives it).
    ChooseNetwork(String),
    /// User asked for the route's minimum in the Amount field.
    UseMinimum,
    /// User ticked (or cleared) Review's "they use this network" box, asked
    /// for only when they chose the network themselves.
    AcknowledgeNetwork(bool),
    /// User picked how far the price may move (basis points). Re-quotes.
    SlippagePreset(u32),
    /// The countdown ticked. Recomputes the quote's remaining life and, at
    /// zero, blocks Confirm until the user re-quotes.
    QuoteTick,
    /// User asked for a fresh quote after the previous one expired.
    ReQuoteRequested,
    /// User asked to retry a *failed* cross-chain send.
    ///
    /// Distinct from [`Self::ConfirmRequested`]: a retry has to re-run the
    /// prepare (the bridge consumed the old handle when it executed the failed
    /// send), while deliberately keeping the original idempotency key so the
    /// retry can't double-pay. Only offered when the route's
    /// [`RetryPolicy`](crate::app::state::spark::cross_chain::RetryPolicy)
    /// permits it.
    CrossChainRetryRequested,
    /// A cross-chain send failed. Carries the route's retry policy, so the
    /// panel can tell "safe to retry" apart from "check the payment's state
    /// first, because a retry might pay twice".
    CrossChainSendFailed(String, crate::app::state::spark::cross_chain::RetryPolicy),

    // ── Unknown payment outcome ─────────────────────────────────────────
    /// The send was dispatched and no answer came back — the transport timed
    /// out, or the bridge went away after being handed the payment.
    ///
    /// **This is not a failure**, and the panel must never render it as one.
    /// `request_id` identifies the request so a late answer can still be
    /// claimed from the client; see
    /// [`SparkClient::take_late_outcome`](crate::app::breez_spark::client::SparkClient::take_late_outcome).
    SendOutcomeUnknown {
        request_id: u64,
        message: String,
    },
    /// Ask again whether the unknown payment landed. Fired on entering the
    /// unknown state and by the "Check again" button.
    ReconcileRequested,
    /// The result of that check.
    ReconcileFinished(crate::app::state::spark::send::ReconcileOutcome),
    /// The user chose to send again after reconciliation found no trace of the
    /// payment. Deliberately **not** [`Self::Reset`]: the original idempotency
    /// key and intent are kept, so the SDK dedups if the first attempt did land
    /// after all.
    ResendAfterUnknownRequested,
}

/// View-level messages for the Phase 4c Spark Receive panel.
#[derive(Debug, Clone)]
pub enum SparkReceiveMessage {
    /// Open the unified "THEY SEND" picker modal (Bitcoin rails + cross-network
    /// assets) that replaces the old Method chips + "Receive from another
    /// network" card.
    OpenSenderPicker,
    /// Dismiss the "THEY SEND" picker without changing the selection.
    CloseSenderPicker,
    /// Open the "YOU RECEIVE" picker: keep bitcoin, or convert it to USDB as
    /// it arrives (the wallet's auto-convert setting).
    OpenReceiverPicker,
    /// Dismiss the "YOU RECEIVE" picker without changing anything.
    CloseReceiverPicker,
    /// Picked in the "YOU RECEIVE" picker: `true` for US Dollar (auto-convert
    /// on), `false` for Bitcoin (off). Saved through the shared Settings
    /// owner, the same path Overview's switch uses.
    SelectReceiveAs(bool),
    /// The wallet's auto-convert state, read on reload. `None` if it
    /// couldn't be read.
    AutoConvertLoaded(Option<bool>),
    /// Ask Spark for a fresh on-chain deposit address instead of the static
    /// one (for a payer who shouldn't see the reused address).
    NewOnchainAddress,
    /// Opened from a Spark balance row's Receive: USDB sets USDB sent on
    /// Spark; Bitcoin leaves that for the default Bitcoin rail and keeps any
    /// other Bitcoin rail already picked.
    StartFrom(coincube_spark_protocol::WalletAsset),
    /// Picked a Bitcoin rail (Lightning / on-chain / Spark) or USDB on Spark
    /// in the picker — sets the receive method and closes the modal.
    SelectSenderRail(crate::app::state::spark::receive::SparkReceiveMethod),
    /// Picked a cross-network asset (by `DepositOption` key) in the picker —
    /// launches the SideShift flow pre-selected to that asset. Mainnet only.
    SelectSenderCrossNetwork(String),
    /// Bring back the cross-network conversion that was set aside when
    /// another rail was picked mid-shift (the "Conversion in progress" notice).
    ShowConversion,
    AmountInputChanged(String),
    DescriptionInputChanged(String),
    GenerateRequested,
    GenerateSucceeded(coincube_spark_protocol::ReceivePaymentOk),
    GenerateFailed(String),
    /// Forwarded from the app-level Spark event handler when a
    /// `PaymentSucceeded` event arrives. Carries the payment's
    /// amount (signed sats — positive for incoming) and an optional
    /// BOLT11 string from the SDK Payment's details. Phase 4f
    /// uses the BOLT11 to correlate against the panel's currently
    /// displayed invoice — events for unrelated payments are
    /// ignored. Pre-Phase-4f BOLT11-less events (Spark-native /
    /// on-chain / token) still trigger the auto-advance.
    PaymentReceived {
        amount_sat: i64,
        bolt11: Option<String>,
        /// The token a token payment moved; its `amount_sat` is then that
        /// token's base units. `None` for bitcoin.
        token: Option<coincube_spark_protocol::PaymentToken>,
    },
    /// Phase 4f: a `Method::ListUnclaimedDeposits` RPC came back with
    /// a fresh deposit list.
    PendingDepositsLoaded(Vec<coincube_spark_protocol::DepositInfo>),
    /// Phase 4f: a `Method::ListUnclaimedDeposits` RPC failed. We
    /// log + clear the displayed list rather than surface a hard
    /// error in the UI — the panel's primary purpose is generating
    /// invoices, not managing deposits, so a deposits-list failure
    /// shouldn't block the rest of the panel.
    PendingDepositsFailed(String),
    /// Phase 4f: user clicked "Claim" on a specific (txid, vout).
    ClaimDepositRequested {
        txid: String,
        vout: u32,
    },
    /// Phase 4f: a `claim_deposit` RPC succeeded. Triggers a deposits
    /// reload so the row disappears. Carries the `(txid, vout)` that was
    /// claimed, since another claim may have started before this result.
    ClaimDepositSucceeded {
        txid: String,
        vout: u32,
        ok: coincube_spark_protocol::ClaimDepositOk,
    },
    /// Phase 4f: a `claim_deposit` RPC failed. Surface the SDK error
    /// in the panel and keep the row.
    ClaimDepositFailed {
        txid: String,
        vout: u32,
        error: String,
    },
    /// Phase 4f: app-level signal that the bridge emitted a
    /// `DepositsChanged` event. The panel re-fetches the list.
    DepositsChanged,
    /// An Esplora `/tx/<txid>/status` + tip query batch came back with
    /// per-deposit confirmation counts. Map key is `(txid, vout)`,
    /// value is `0` for mempool / unconfirmed and `>= 1` once mined.
    /// Used to render "X / 3 confirmations" on immature rows.
    DepositConfirmationsUpdated(std::collections::HashMap<(String, u32), u32>),
    /// 30s subscription tick while immature deposits are on screen —
    /// fires a fresh Esplora batch so the confirmation counts update
    /// between block arrivals without waiting for a `DepositsChanged`
    /// event (the SDK only re-emits at mature/refund-status boundaries,
    /// not on every new confirmation).
    RefreshConfirmations,
    /// An Esplora fetch of a cross-network swap's settle transaction, by the
    /// txid SideShift reported, came back. Feeds the swap's "X of 3" count
    /// while the wallet's pending deposits don't list that transaction yet.
    /// `None` when the fetch failed.
    SettleConfirmationsUpdated {
        txid: String,
        confirmations: Option<u32>,
    },
    /// Re-read only the swap's settle confirmations by txid (panel reload).
    RefreshSettleConfirmations,
    Reset,
    /// The received receipt's View in Activity: [`Self::Reset`], then Activity.
    ViewActivity,
    /// The Spark wallet balance for the YOU RECEIVE card — BTC sats plus the
    /// Stable Balance snapshot, folded into a unified total by the handler.
    /// `None` when `get_info` failed; the card keeps its last value.
    BalanceLoaded(Option<(u64, Option<coincube_spark_protocol::UsdbBalanceSnapshot>)>),
    /// A `list_payments` RPC completed — used to populate the Last
    /// Transactions section under the Receive form.
    PaymentsLoaded(Vec<coincube_spark_protocol::PaymentSummary>),
    /// A `list_payments` RPC failed — silently clears the list.
    PaymentsFailed(String),
    /// User tapped a row in Last Transactions.
    SelectTransaction(usize),
    /// User tapped "View All Transactions".
    History,
    /// User tapped "Copy" under the generated payment request. Routed
    /// through the panel rather than the app-level `Message::Clipboard`
    /// so the handler can pair the clipboard write with a confirmation
    /// toast naming the rail that was copied — the same pattern Vault
    /// and Liquid Receive use.
    CopyPaymentRequest(String),
}

#[cfg(test)]
mod balances_tests {
    use super::SparkBalances;
    use crate::app::cache::FiatPriceRequest;
    use crate::app::view::FiatAmountConverter;
    use crate::services::fiat::{Currency, PriceSource};
    use coincube_spark_protocol::UsdbBalanceSnapshot;
    use coincube_ui::component::amount::BitcoinDisplayUnit;

    fn usdb(dollars: u64) -> UsdbBalanceSnapshot {
        UsdbBalanceSnapshot {
            token_identifier: "btkn1synthetic".into(),
            balance: u128::from(dollars) * 1_000_000,
            decimals: 6,
            ticker: "USDB".into(),
        }
    }

    fn balances(
        btc_sats: u64,
        stable: Option<&UsdbBalanceSnapshot>,
        btc_usd_price: Option<f64>,
    ) -> SparkBalances<'_> {
        SparkBalances {
            btc_sats,
            stable,
            fiat_converter: FiatAmountConverter::new(
                50_000.0,
                None,
                FiatPriceRequest::new(PriceSource::Coincube, Currency::USD),
            )
            .ok(),
            btc_usd_price,
            bitcoin_unit: BitcoinDisplayUnit::Sats,
            masked: false,
        }
    }

    fn names(rows: &[(&'static str, String, String)]) -> Vec<&'static str> {
        rows.iter().map(|(name, _, _)| *name).collect()
    }

    #[test]
    fn the_side_panel_lists_each_balance_in_its_unit_then_the_total() {
        let stable = usdb(100);
        let (rows, total) = balances(200_000, Some(&stable), Some(50_000.0)).rows();
        assert_eq!(names(&rows), ["Bitcoin", "US Dollar"]);
        assert_eq!(rows[0].2, "SATS");
        assert_eq!((rows[1].1.as_str(), rows[1].2.as_str()), ("100.00", "USDB"));
        // 200,000 sats is $100 at $50,000, plus $100 of USDB.
        assert_eq!(total.as_deref(), Some("200.00 USD"));

        // Bitcoin alone: no US Dollar row, and the total is the bitcoin.
        let (rows, total) = balances(200_000, None, None).rows();
        assert_eq!(names(&rows), ["Bitcoin"]);
        assert_eq!(total.as_deref(), Some("100.00 USD"));

        // USDB alone: only its row.
        let (rows, _) = balances(0, Some(&stable), Some(50_000.0)).rows();
        assert_eq!(names(&rows), ["US Dollar"]);
    }

    #[test]
    fn usdb_without_a_btc_usd_quote_leaves_the_total_out() {
        let stable = usdb(100);
        let (rows, total) = balances(200_000, Some(&stable), None).rows();
        assert_eq!(names(&rows), ["Bitcoin", "US Dollar"]);
        assert_eq!(total, None);
    }

    #[test]
    fn hidden_amounts_mask_every_figure() {
        let stable = usdb(100);
        let mut hidden = balances(200_000, Some(&stable), Some(50_000.0));
        hidden.masked = true;
        let (rows, total) = hidden.rows();
        let mask = coincube_ui::component::tenshu::MASK;
        assert!(rows.iter().all(|(_, figure, _)| figure == mask));
        assert_eq!(total.as_deref(), Some(mask));
    }
}
