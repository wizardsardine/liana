//! Real Spark Receive panel — Phase 4c.
//!
//! State machine (per picked method):
//!
//! ```text
//! Idle { method } ──(Generate)──▶ Generating { method }
//!                                       │
//!                                  ┌────┴─────┐
//!                                  ▼          ▼
//!                          Generated(ok)   Error(msg)
//!                                  │          │
//!                                  └── Reset ─┘
//!                                  ▼
//!                               Idle { method }
//! ```
//!
//! The user picks a method (BOLT11 Lightning / on-chain Bitcoin),
//! optionally fills in amount + description for BOLT11, clicks
//! Generate, sees the result as a copyable text string. QR codes,
//! Lightning Address display, and the on-chain claim lifecycle all
//! land in Phase 4d.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use coincube_spark_protocol::{DepositInfo, ReceivePaymentOk};
use coincube_ui::widget::Element;
use iced::{widget::qr_code, Subscription, Task};

use crate::app::cache::Cache;
use crate::app::menu::{Menu, SparkSubMenu};
use crate::app::message::Message;
use crate::app::state::{redirect, State};
use crate::app::view::spark::SparkReceiveView;
use crate::app::view::spark::SparkRecentTransaction;
use crate::app::wallets::SparkBackend;

use super::esplora::DEPOSIT_MATURITY_CONFIRMATIONS;
use super::sideshift_receive::SparkSideshiftReceiveFlow;

/// Which receive flow the user has picked.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SparkReceiveMethod {
    Bolt11,
    LightningAddress,
    OnchainBitcoin,
    Spark,
    /// US Dollar (USDB) sent to the Spark address. It is the same address
    /// as [`Self::Spark`], but the payer sends the USDB token, which stays
    /// USDB whatever auto-convert says. Where conversions run (mainnet) only.
    UsdbSpark,
}

impl SparkReceiveMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bolt11 => "Lightning (BOLT11)",
            Self::LightningAddress => "Lightning address",
            Self::OnchainBitcoin => "On-chain Bitcoin",
            Self::Spark => "Spark",
            Self::UsdbSpark => "US Dollar (USDB) on Spark",
        }
    }
}

#[derive(Debug, Clone)]
pub enum SparkReceivePhase {
    /// User is picking/configuring a method.
    Idle,
    /// `receive_bolt11` or `receive_onchain` RPC is in flight.
    Generating,
    /// RPC succeeded — `payment_request` is the copyable result.
    Generated(ReceivePaymentOk),
    /// A `PaymentSucceeded` event arrived while the panel was in
    /// `Generated` state. Carries the running sum of all qualifying
    /// `PaymentReceived` events that have arrived since this phase
    /// was entered, plus a `count` so the celebration label can say
    /// "2 payments" when multiple back-to-back receives stack up
    /// before the user dismisses the screen.
    Received { amount_sat: i64, count: u32 },
    /// RPC failed — user-visible error.
    Error(String),
}

/// Real Spark Receive panel.
pub struct SparkReceive {
    backend: Option<Arc<SparkBackend>>,
    /// The Spark wallet's unified balance in sats (BTC + Stable Balance), shown
    /// on the YOU RECEIVE card. Refreshed on reload via `get_info`; `0` until
    /// the first fetch.
    balance_sats: u64,
    /// The bitcoin balance alone and the USDB holding, for the side panel.
    btc_balance_sats: u64,
    usdb_balance: Option<coincube_spark_protocol::UsdbBalanceSnapshot>,
    /// The "receive from another network" (SideShift → BTC) sub-flow, when the
    /// user has entered it. While `Some`, it owns the panel: view, update and
    /// subscription all delegate to it. Mirrors how the Liquid Receive panel
    /// hosts its own SideShift flow.
    sideshift_flow: Option<SparkSideshiftReceiveFlow>,
    /// A conversion with a live shift, set aside because the user picked
    /// another rail. It keeps polling and still catches its arrival; the
    /// "Conversion in progress" notice puts it back. At most one conversion is
    /// live at a time, so this and an active `sideshift_flow` never coexist.
    parked_shift: Option<SparkSideshiftReceiveFlow>,
    /// Currently selected method. Toggling methods resets the phase.
    pub method: SparkReceiveMethod,
    /// Whether the "THEY SEND" menu is open under its tile.
    sender_picker_open: bool,
    /// Whether the "YOU RECEIVE" menu is open under its tile.
    receiver_picker_open: bool,
    /// The wallet's auto-convert state (incoming bitcoin → USDB), which the
    /// YOU RECEIVE tile shows and its picker changes. `None` until read.
    auto_convert: Option<bool>,
    /// Amount input for BOLT11. Ignored for on-chain.
    pub amount_input: String,
    /// Invoice description shown to the payer for BOLT11. Ignored for on-chain.
    pub description_input: String,
    phase: SparkReceivePhase,
    /// Pre-rendered QR code for the current `Generated` payment
    /// request. Built once when `GenerateSucceeded` fires so the view
    /// renderer doesn't have to re-encode the (potentially long)
    /// BOLT11 invoice on every frame. `None` when no invoice is on
    /// screen, or when encoding failed (unlikely for BOLT11/BTC
    /// addresses but handled gracefully).
    pub qr_data: Option<qr_code::Data>,
    /// QR for the Connect-owned address and the source value used to encode it.
    lightning_address_qr: Option<(String, qr_code::Data)>,
    /// Phase 4f: pending on-chain deposits surfaced by the SDK's
    /// `list_unclaimed_deposits` RPC. Refreshed on panel reload and on
    /// every `Event::DepositsChanged`. The view renders this as a
    /// dedicated "Pending deposits" card below the main phase body.
    pub pending_deposits: Vec<DepositInfo>,
    /// Phase 4f: tracks which deposits are currently being claimed so
    /// the UI can disable each row's button while its RPC is in flight.
    /// Keyed by `(txid, vout)`; each entry is removed when that
    /// deposit's RPC finishes (success or failure). A set, because a
    /// second deposit can be claimed before the first answers.
    pub claiming: HashSet<(String, u32)>,
    /// Phase 4f: surface a transient claim error to the user. Cleared
    /// on the next reload or successful claim.
    pub claim_error: Option<String>,
    /// Deferred claims (SDK 0.28.0+): nothing claimed yet, and the SDK
    /// keeps trying on its own. Each deposit's `(txid, vout)` keeps its
    /// own neutral note until it is retried, claimed or leaves the list.
    pub claim_notices: HashMap<(String, u32), String>,
    /// Live on-chain confirmation count per pending deposit, fetched
    /// from a public Esplora ([`esplora::fetch_confirmations`]). The
    /// SDK only tells us `is_mature: bool`, so we query Esplora
    /// ourselves to surface progress like "1 / 3 confirmations" on
    /// rows that haven't matured yet. Entries are dropped when the
    /// deposit list refreshes; missing keys render the SDK's plain
    /// "Waiting for confirmations" fallback text.
    pub pending_deposit_confirmations: HashMap<(String, u32), u32>,
    /// Phase 4f: the BOLT11 invoice string of the currently-displayed
    /// generated invoice, captured at `GenerateSucceeded` time. Used
    /// by the auto-advance handler to correlate `PaymentSucceeded`
    /// events against THIS invoice instead of accepting any
    /// incoming payment. `None` while in idle / error / received
    /// phases.
    pub displayed_invoice: Option<String>,
    /// Formatted amount string for the celebration screen.
    received_amount_display: String,
    /// Quote context key for the celebration screen (e.g. "lightning-receive").
    received_celebration_context: String,
    /// Quote and image handle for the celebration screen.
    received_quote: coincube_ui::component::quote_display::Quote,
    received_image_handle: iced::widget::image::Handle,
    /// Last few payments fetched from the bridge, rendered under the
    /// receive form. Populated on reload and after an incoming payment.
    recent_transactions: Vec<SparkRecentTransaction>,
}

impl SparkReceive {
    /// Whether a static (Spark or on-chain) address should be fetched now:
    /// those methods show no button, only "Getting your address…".
    fn static_address_due(&self) -> bool {
        auto_generates(self.method)
            && matches!(self.phase, SparkReceivePhase::Idle)
            && self.sideshift_flow.is_none()
    }

    fn fetch_static_address_if_due(&self) -> Task<Message> {
        if self.static_address_due() {
            Task::done(Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::GenerateRequested,
            )))
        } else {
            Task::none()
        }
    }

    /// A Stable Balance save settled (from any panel). `Some` is the saved
    /// value; `None` (failed or dropped) re-reads the SDK so the YOU RECEIVE
    /// tile never shows a choice that was not saved.
    pub fn stable_balance_settled(&mut self, saved: Option<bool>) -> Task<Message> {
        match saved {
            Some(enabled) => {
                self.auto_convert = Some(enabled);
                Task::none()
            }
            None => fetch_auto_convert_task(self.backend.clone()),
        }
    }

    pub fn new(backend: Option<Arc<SparkBackend>>) -> Self {
        Self {
            backend,
            balance_sats: 0,
            btc_balance_sats: 0,
            usdb_balance: None,
            sideshift_flow: None,
            parked_shift: None,
            method: SparkReceiveMethod::Bolt11,
            sender_picker_open: false,
            receiver_picker_open: false,
            auto_convert: None,
            amount_input: String::new(),
            description_input: String::new(),
            phase: SparkReceivePhase::Idle,
            qr_data: None,
            lightning_address_qr: None,
            pending_deposits: Vec::new(),
            claiming: HashSet::new(),
            claim_error: None,
            claim_notices: HashMap::new(),
            pending_deposit_confirmations: HashMap::new(),
            displayed_invoice: None,
            received_amount_display: String::new(),
            received_celebration_context: "lightning-receive".to_string(),
            received_quote: coincube_ui::component::quote_display::random_quote(
                "lightning-receive",
            ),
            received_image_handle: coincube_ui::component::quote_display::image_handle_for_context(
                "lightning-receive",
            ),
            recent_transactions: Vec::new(),
        }
    }

    pub fn phase(&self) -> &SparkReceivePhase {
        &self.phase
    }

    pub fn sync_lightning_address_qr(&mut self, address: Option<&str>) {
        self.lightning_address_qr = address.and_then(|address| {
            qr_code::Data::new(address)
                .ok()
                .map(|qr| (address.to_owned(), qr))
        });
    }

    /// The cross-network shift-status poll on its own, exposed so the app can
    /// keep it alive while the user is on another panel. A swap settles and its
    /// bitcoin is auto-claimed on a ~30-minute timeline; if the settle isn't
    /// observed until the user happens back on this screen, the deposit can
    /// already be claimed by then and the flow hangs on "Bitcoin arriving"
    /// (GlobalHome can only register the arrival while the deposit is still
    /// pending). Polling stops on its own once the shift is terminal, so this
    /// only stays live through the pre-settle window. Returns `none` when there
    /// is no active flow.
    pub fn sideshift_poll_subscription(&self) -> Subscription<Message> {
        Subscription::batch(
            self.sideshift_flow
                .iter()
                .chain(self.parked_shift.iter())
                .map(SparkSideshiftReceiveFlow::subscription),
        )
    }

    /// The cross-network conversion with a shift, on screen or set aside:
    /// what it deposits, and whether its bitcoin has arrived. For the notice
    /// on Spark's Overview. `None` once it is closed, or before it has a shift.
    pub fn conversion_summary(&self) -> Option<(crate::services::sideshift::DepositOption, bool)> {
        use super::sideshift_receive::SparkShiftPhase;
        self.sideshift_flow
            .iter()
            .chain(self.parked_shift.iter())
            .find(|flow| {
                matches!(
                    flow.phase(),
                    SparkShiftPhase::Active | SparkShiftPhase::Arrived
                )
            })
            .map(|flow| (flow.selected(), *flow.phase() == SparkShiftPhase::Arrived))
    }

    /// Leave the cross-network flow for another rail. A conversion with a live
    /// shift is set aside, not dropped: money may be on its way, and its page
    /// (Shift ID, refund address, progress) must stay reachable. One that has
    /// no shift yet, or whose bitcoin has arrived, is simply closed.
    fn set_aside_sideshift_flow(&mut self) {
        let Some(flow) = self.sideshift_flow.take() else {
            return;
        };
        if keeps_running(flow.phase()) && self.parked_shift.is_none() {
            self.parked_shift = Some(flow);
        }
    }

    /// Put the set-aside conversion back on screen.
    fn show_parked_shift(&mut self) {
        if let Some(flow) = self.parked_shift.take() {
            self.sideshift_flow = Some(flow);
            self.sender_picker_open = false;
            self.receiver_picker_open = false;
        }
    }

    /// The live conversion, on screen or set aside.
    fn live_shift(&self) -> Option<&SparkSideshiftReceiveFlow> {
        self.sideshift_flow
            .iter()
            .chain(self.parked_shift.iter())
            .find(|flow| keeps_running(flow.phase()))
    }

    /// A shift read back from the Cube's settings at unlock. Ignored when this
    /// session already has a live one (the record is that same shift). It goes
    /// on screen, replacing a flow that has no shift yet (one the user opened
    /// before the read finished): flow messages reach the on-screen flow first,
    /// so a restored shift parked behind it would never hear its own.
    fn restore_shift(&mut self, record: &crate::app::settings::PendingSparkShift) -> Task<Message> {
        if self.live_shift().is_some() || self.parked_shift.is_some() {
            return Task::none();
        }
        let Some(backend) = self.backend.clone() else {
            return Task::none();
        };
        let Some(flow) = SparkSideshiftReceiveFlow::restored(backend, record) else {
            // The stored asset is no longer offered; nothing could show it.
            return super::sideshift_receive::store_record(None);
        };
        let poll = flow.poll_now();
        self.sideshift_flow = Some(flow);
        poll
    }
}

/// Whether a conversion in this phase is still going somewhere and must not be
/// dropped by picking another rail: its shift exists and its bitcoin hasn't
/// arrived.
fn keeps_running(phase: &super::sideshift_receive::SparkShiftPhase) -> bool {
    *phase == super::sideshift_receive::SparkShiftPhase::Active
}

impl State for SparkReceive {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> Element<'a, crate::app::view::Message> {
        let backend_available = self.backend.is_some();
        let connect_signed_in = cache.connect_authenticated;
        let lightning_address = connect_signed_in
            .then_some(cache.lightning_address.as_deref())
            .flatten();
        // The cross-network (SideShift) flow renders *inline* below the two-card
        // selector — not as a takeover — so its refund/deposit steps sit on the
        // same page as the Bitcoin rail forms. Its messages route back through
        // `SparkSideshiftReceive`.
        let (sideshift_body, cross_network_selected) = match &self.sideshift_flow {
            Some(flow) => (
                Some(
                    crate::app::view::spark::spark_sideshift_receive_view(
                        flow,
                        cache.bitcoin_unit,
                        cache.balance_amounts_masked(),
                        self.swap_settle_confirmations(),
                        cache.reduce_motion,
                    )
                    .map(crate::app::view::Message::SparkSideshiftReceive),
                ),
                Some(flow.selected()),
            ),
            None => (None, None),
        };
        // The received celebration takes the whole page, head included.
        let head = (!matches!(self.phase, SparkReceivePhase::Received { .. })).then(|| {
            // What the payer sends is picked from the THEY SEND tile.
            crate::app::view::spark::sub_page_head(cache, "Receive")
        });
        crate::app::view::dashboard_payment(
            menu,
            cache,
            head,
            SparkReceiveView {
                backend_available,
                method: self.method,
                amount_input: &self.amount_input,
                description_input: &self.description_input,
                phase: &self.phase,
                qr_data: self.qr_data.as_ref(),
                connect_signed_in,
                lightning_address,
                lightning_address_qr: self.lightning_address_qr.as_ref().and_then(
                    |(source, qr)| (lightning_address == Some(source.as_str())).then_some(qr),
                ),
                pending_deposits: &self.pending_deposits,
                claiming: &self.claiming,
                claim_error: self.claim_error.as_deref(),
                claim_notices: &self.claim_notices,
                pending_deposit_confirmations: &self.pending_deposit_confirmations,
                network: cache.network,
                cube_name: &cache.cube_name,
                received_amount_display: &self.received_amount_display,
                received_celebration_context: &self.received_celebration_context,
                received_quote: &self.received_quote,
                received_image_handle: &self.received_image_handle,
                recent_transactions: &self.recent_transactions,
                balance_sats: self.balance_sats,
                balances: crate::app::view::spark::SparkBalances::new(
                    cache,
                    self.btc_balance_sats,
                    self.usdb_balance.as_ref(),
                ),
                bitcoin_unit: cache.bitcoin_unit,
                show_direction_badges: cache.show_direction_badges,
                mask_amounts: cache.balance_amounts_masked(),
                sideshift_body,
                cross_network_selected,
                parked_conversion: self.parked_shift.as_ref().map(|flow| {
                    (
                        flow.selected(),
                        *flow.phase() == super::sideshift_receive::SparkShiftPhase::Arrived,
                    )
                }),
                open_menu: if self.sender_picker_open {
                    Some(crate::app::view::spark::receive::ReceiveMenu::TheySend)
                } else if self.receiver_picker_open {
                    Some(crate::app::view::spark::receive::ReceiveMenu::YouReceive)
                } else {
                    None
                },
                receive_usd: self.auto_convert == Some(true),
                lightning_address_setup: &cache.lightning_address_setup,
                swap_deposit: self
                    .swap_settle_deposit()
                    .filter(|d| quietly_settling(d, &self.claim_notices))
                    .map(|d| (d.txid.as_str(), d.vout)),
            }
            .render(),
        )
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        _wallet: Option<Arc<crate::app::wallet::Wallet>>,
    ) -> Task<Message> {
        // Refresh the pending-deposits list whenever the panel
        // becomes active. Errors degrade silently — the rest of the
        // panel still works. Also fetch the recent-payments list so
        // the "Last transactions" section under the form is fresh.
        Task::batch(vec![
            fetch_deposits_task(self.backend.clone()),
            fetch_payments_task(self.backend.clone()),
            fetch_balance_task(self.backend.clone()),
            fetch_auto_convert_task(self.backend.clone()),
            // A static address left on screen from before, or picked before
            // the panel was ever shown, is (re)fetched without a button.
            self.fetch_static_address_if_due(),
            self.settle_refresh_on_reload()
                .map_or_else(Task::none, |msg| {
                    Task::done(Message::View(crate::app::view::Message::SparkReceive(msg)))
                }),
        ])
    }

    fn subscription(&self) -> Subscription<Message> {
        // The cross-network flow now renders inline, so its shift-status poll
        // AND the deposit-confirmation poll can be live at once — batch them.
        // Esplora doesn't push, so we tick on a fixed cadence while at least one
        // immature deposit is on screen so the "X / 3 confirmations" badge keeps
        // updating between blocks (the SDK only re-emits `DepositsChanged` at
        // maturity / refund-status transitions, not on every new confirmation).
        let mut subs = vec![self.sideshift_poll_subscription()];
        // The swap's settle transaction gets the same tick even before the
        // wallet lists it as a pending deposit: the SDK may not list it until
        // it confirms, and nothing else would move the count off zero.
        if self.pending_deposits.iter().any(|d| !d.is_mature)
            || self.swap_settle_txid_to_poll().is_some()
        {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(30)).map(|_| {
                    Message::View(crate::app::view::Message::SparkReceive(
                        crate::app::view::SparkReceiveMessage::RefreshConfirmations,
                    ))
                }),
            );
        }
        Subscription::batch(subs)
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        // Bridge-flow messages go to the flow, whenever it's up.
        if let Message::View(crate::app::view::Message::SparkSideshiftReceive(ref msg)) = message {
            if let crate::app::view::SparkSideshiftReceiveMessage::Restore(record) = msg {
                return self.restore_shift(record);
            }
            // Read before the flow is borrowed: whether the swap was already
            // polling its settle txid when this message arrived.
            let was_polling = self.swap_settle_txid_to_poll().is_some();
            // A set-aside conversion still hears its polls and its arrival;
            // only one flow exists to answer them when it is on screen.
            let Some(flow) = self.sideshift_flow.as_mut().or(self.parked_shift.as_mut()) else {
                return Task::none();
            };
            // `Back`/`Reset` from the flow's *entry* screen means "leave the
            // bridge", so tear it down and fall back to the ordinary receive
            // form. The arrived screen's "Done" button lands here too — once the
            // bitcoin is in the wallet the swap is finished, so treat it the same
            // way. The flow resets itself for any other phase.
            let leaving = matches!(
                msg,
                crate::app::view::SparkSideshiftReceiveMessage::Back
                    | crate::app::view::SparkSideshiftReceiveMessage::Reset
            ) && matches!(
                flow.phase(),
                super::sideshift_receive::SparkShiftPhase::Setup
                    | super::sideshift_receive::SparkShiftPhase::Arrived
            );
            if leaving {
                // Only the on-screen flow has buttons to leave by.
                self.sideshift_flow = None;
                return Task::none();
            }
            let task = flow.update(msg);
            // The status poll that reports the settle txid is the moment to
            // read its confirmations, rather than one tick later.
            if was_polling {
                return task;
            }
            return Task::batch([task, self.settle_confirmations_task(cache.network)]);
        }

        let Message::View(crate::app::view::Message::SparkReceive(msg)) = message else {
            return Task::none();
        };

        use crate::app::view::SparkReceiveMessage;

        // The cross-network flow renders *inline*, so the two-card / THEY SEND
        // picker AND the pending-deposits card stay on screen while it's active —
        // only the Bitcoin invoice form's messages are gated out (its widgets
        // aren't shown). Let the picker, `DepositsChanged`, and the whole
        // pending-deposit lifecycle (list refresh, confirmation ticks, claim +
        // its result) through so those on-screen controls keep working mid-swap.
        if self.sideshift_flow.is_some()
            && !matches!(
                msg,
                SparkReceiveMessage::DepositsChanged
                    | SparkReceiveMessage::OpenSenderPicker
                    | SparkReceiveMessage::CloseSenderPicker
                    | SparkReceiveMessage::OpenReceiverPicker
                    | SparkReceiveMessage::CloseReceiverPicker
                    | SparkReceiveMessage::SelectReceiveAs(_)
                    | SparkReceiveMessage::AutoConvertLoaded(_)
                    | SparkReceiveMessage::SelectSenderRail(_)
                    | SparkReceiveMessage::StartFrom(_)
                    | SparkReceiveMessage::SelectSenderCrossNetwork(_)
                    | SparkReceiveMessage::ClaimDepositRequested { .. }
                    | SparkReceiveMessage::RefreshConfirmations
                    | SparkReceiveMessage::SettleConfirmationsUpdated { .. }
                    | SparkReceiveMessage::RefreshSettleConfirmations
                    | SparkReceiveMessage::DepositConfirmationsUpdated(_)
                    | SparkReceiveMessage::PendingDepositsLoaded(_)
                    | SparkReceiveMessage::PendingDepositsFailed(_)
                    | SparkReceiveMessage::ClaimDepositSucceeded { .. }
                    | SparkReceiveMessage::ClaimDepositFailed { .. }
            )
        {
            return Task::none();
        }

        match msg {
            SparkReceiveMessage::OpenSenderPicker => {
                // One menu at a time: the other tile is still reachable from
                // the keyboard while a menu is open.
                self.receiver_picker_open = false;
                self.sender_picker_open = true;
                Task::none()
            }
            SparkReceiveMessage::OpenReceiverPicker => {
                // US Dollar exists where conversions run (mainnet); elsewhere
                // Bitcoin is the only thing Spark receives, so nothing to pick.
                if crate::app::state::spark::swap::swap_available(cache.network) {
                    self.sender_picker_open = false;
                    self.receiver_picker_open = true;
                }
                Task::none()
            }
            SparkReceiveMessage::CloseReceiverPicker => {
                self.receiver_picker_open = false;
                Task::none()
            }
            SparkReceiveMessage::SelectReceiveAs(usd) => {
                self.receiver_picker_open = false;
                if self.auto_convert != Some(!usd) {
                    // Already that way, or not read yet.
                    return Task::none();
                }
                // The tile keeps the saved value: the shared save owner may
                // drop this request (a save already running) or fail it, and
                // `stable_balance_settled` updates the tile when it lands.
                Task::done(Message::View(crate::app::view::Message::SparkSettings(
                    crate::app::view::SparkSettingsMessage::StableBalanceToggled(usd),
                )))
            }
            SparkReceiveMessage::AutoConvertLoaded(state) => {
                if state.is_some() {
                    self.auto_convert = state;
                }
                Task::none()
            }
            SparkReceiveMessage::NewOnchainAddress => {
                if self.method != SparkReceiveMethod::OnchainBitcoin {
                    return Task::none();
                }
                let Some(backend) = self.backend.clone() else {
                    return Task::none();
                };
                self.phase = SparkReceivePhase::Generating;
                Task::perform(
                    async move { backend.receive_onchain(Some(true)).await },
                    |result| match result {
                        Ok(ok) => Message::View(crate::app::view::Message::SparkReceive(
                            SparkReceiveMessage::GenerateSucceeded(ok),
                        )),
                        Err(e) => Message::View(crate::app::view::Message::SparkReceive(
                            SparkReceiveMessage::GenerateFailed(e.to_string()),
                        )),
                    },
                )
            }
            SparkReceiveMessage::CloseSenderPicker => {
                self.sender_picker_open = false;
                Task::none()
            }
            SparkReceiveMessage::StartFrom(asset) => {
                let method = match asset {
                    coincube_spark_protocol::WalletAsset::Usdb => SparkReceiveMethod::UsdbSpark,
                    coincube_spark_protocol::WalletAsset::Bitcoin
                        if self.method == SparkReceiveMethod::UsdbSpark =>
                    {
                        SparkReceiveMethod::Bolt11
                    }
                    // A Bitcoin rail (or a stablecoin swap) already set stays.
                    coincube_spark_protocol::WalletAsset::Bitcoin => return Task::none(),
                };
                self.update(
                    None,
                    cache,
                    Message::View(crate::app::view::Message::SparkReceive(
                        SparkReceiveMessage::SelectSenderRail(method),
                    )),
                )
            }
            SparkReceiveMessage::SelectSenderRail(method) => {
                // A Bitcoin rail (or USDB on Spark): clear any cross-network
                // flow, set the method, close the picker.
                self.sender_picker_open = false;
                // USDB exists where conversions run; the picker hides it
                // elsewhere and this is the backstop.
                if method == SparkReceiveMethod::UsdbSpark
                    && !crate::app::state::spark::swap::swap_available(cache.network)
                {
                    return Task::none();
                }
                if method == SparkReceiveMethod::LightningAddress {
                    self.sync_lightning_address_qr(
                        cache
                            .connect_authenticated
                            .then_some(cache.lightning_address.as_deref())
                            .flatten(),
                    );
                    // A signed-in Cube with no Lightning address gets a random
                    // one (the Connect Cube panel decides whether it may).
                    if let Some(assign) = auto_assign_lightning_address(cache) {
                        if self.sideshift_flow.is_some() || self.method != method {
                            self.set_aside_sideshift_flow();
                            self.method = method;
                            self.phase = SparkReceivePhase::Idle;
                            self.qr_data = None;
                            self.displayed_invoice = None;
                        }
                        return assign;
                    }
                }
                if self.sideshift_flow.is_none() && self.method == method {
                    return Task::none();
                }
                self.set_aside_sideshift_flow();
                self.method = method;
                self.phase = SparkReceivePhase::Idle;
                self.qr_data = None;
                self.displayed_invoice = None;
                // Spark and on-chain addresses are static and reusable, so
                // there is nothing to configure: show the address straight
                // away rather than behind a Generate button.
                if auto_generates(method) {
                    return Task::done(Message::View(crate::app::view::Message::SparkReceive(
                        SparkReceiveMessage::GenerateRequested,
                    )));
                }
                Task::none()
            }
            SparkReceiveMessage::SelectSenderCrossNetwork(key) => {
                // A cross-network asset: hand off to the SideShift flow,
                // pre-selected so it skips its own asset picker. Mainnet only —
                // the picker hides these off mainnet; this is the backstop.
                self.sender_picker_open = false;
                if !crate::app::state::spark::cross_chain::supported_on(cache.network) {
                    return Task::none();
                }
                let Some(backend) = self.backend.clone() else {
                    return Task::none();
                };
                let Some(option) = crate::services::sideshift::deposit_option_by_key(&key) else {
                    return Task::none();
                };
                // One conversion at a time: while one is live, picking any
                // other network shows that one rather than replacing it, so its
                // Shift ID and progress can't be lost to a stray click.
                if self.parked_shift.is_some() {
                    self.show_parked_shift();
                    return Task::none();
                }
                if self
                    .sideshift_flow
                    .as_ref()
                    .is_some_and(|flow| keeps_running(flow.phase()))
                {
                    return Task::none();
                }
                self.sideshift_flow =
                    Some(SparkSideshiftReceiveFlow::new_preselected(backend, option));
                Task::none()
            }
            SparkReceiveMessage::ShowConversion => {
                self.show_parked_shift();
                Task::none()
            }
            SparkReceiveMessage::AmountInputChanged(value) => {
                self.amount_input = value;
                self.phase = SparkReceivePhase::Idle;
                self.qr_data = None;
                self.displayed_invoice = None;
                Task::none()
            }
            SparkReceiveMessage::DescriptionInputChanged(value) => {
                self.description_input = value;
                self.phase = SparkReceivePhase::Idle;
                self.qr_data = None;
                self.displayed_invoice = None;
                Task::none()
            }
            SparkReceiveMessage::GenerateRequested => {
                if self.method == SparkReceiveMethod::LightningAddress {
                    return Task::none();
                }
                let Some(backend) = self.backend.clone() else {
                    self.phase =
                        SparkReceivePhase::Error("Spark backend is not available.".to_string());
                    return Task::none();
                };
                self.phase = SparkReceivePhase::Generating;
                match self.method {
                    SparkReceiveMethod::Bolt11 => {
                        let amount_sat = if self.amount_input.trim().is_empty() {
                            None
                        } else {
                            match self.amount_input.trim().parse::<u64>() {
                                Ok(n) => Some(n),
                                Err(_) => {
                                    self.phase = SparkReceivePhase::Error(
                                        "Amount must be a whole number of sats.".to_string(),
                                    );
                                    return Task::none();
                                }
                            }
                        };
                        let description = self.description_input.clone();
                        Task::perform(
                            async move { backend.receive_bolt11(amount_sat, description, None).await },
                            |result| match result {
                                Ok(ok) => Message::View(crate::app::view::Message::SparkReceive(
                                    SparkReceiveMessage::GenerateSucceeded(ok),
                                )),
                                Err(e) => Message::View(crate::app::view::Message::SparkReceive(
                                    SparkReceiveMessage::GenerateFailed(e.to_string()),
                                )),
                            },
                        )
                    }
                    SparkReceiveMethod::OnchainBitcoin => Task::perform(
                        async move { backend.receive_onchain(None).await },
                        |result| match result {
                            Ok(ok) => Message::View(crate::app::view::Message::SparkReceive(
                                SparkReceiveMessage::GenerateSucceeded(ok),
                            )),
                            Err(e) => Message::View(crate::app::view::Message::SparkReceive(
                                SparkReceiveMessage::GenerateFailed(e.to_string()),
                            )),
                        },
                    ),
                    // USDB is sent to the same Spark address as bitcoin.
                    SparkReceiveMethod::Spark | SparkReceiveMethod::UsdbSpark => {
                        Task::perform(async move { backend.receive_spark().await }, |result| {
                            match result {
                                Ok(ok) => Message::View(crate::app::view::Message::SparkReceive(
                                    SparkReceiveMessage::GenerateSucceeded(ok),
                                )),
                                Err(e) => Message::View(crate::app::view::Message::SparkReceive(
                                    SparkReceiveMessage::GenerateFailed(e.to_string()),
                                )),
                            }
                        })
                    }
                    SparkReceiveMethod::LightningAddress => Task::none(),
                }
            }
            SparkReceiveMessage::GenerateSucceeded(ok) => {
                // Encode the QR eagerly so the view renderer doesn't
                // re-encode on every frame.
                self.qr_data = qr_code::Data::new(&ok.payment_request).ok();
                // Only capture the payment request for BOLT11 — it's
                // the correlation key used by PaymentReceived to match
                // the event's bolt11 field against the displayed
                // invoice. For on-chain receives the payment_request
                // is a Bitcoin address (no bolt11 on the event), so
                // displayed_invoice stays None and the (None, _) =>
                // true arm in the correlation check auto-advances on
                // any incoming payment while the address is on screen.
                self.displayed_invoice = if self.method == SparkReceiveMethod::Bolt11 {
                    Some(ok.payment_request.clone())
                } else {
                    None
                };
                self.phase = SparkReceivePhase::Generated(ok);
                Task::none()
            }
            SparkReceiveMessage::GenerateFailed(err) => {
                self.qr_data = None;
                self.displayed_invoice = None;
                self.phase = SparkReceivePhase::Error(err);
                Task::none()
            }
            SparkReceiveMessage::PaymentReceived {
                amount_sat,
                bolt11,
                token,
            } => {
                // Accept events while either showing an invoice
                // (`Generated`) — first arrival — or already on the
                // celebration screen (`Received`) — back-to-back
                // deposits accumulate into the running total instead
                // of being silently dropped. Idle / error / generating
                // phases stay no-op.
                let already_celebrating = matches!(self.phase, SparkReceivePhase::Received { .. });
                if !already_celebrating && !matches!(self.phase, SparkReceivePhase::Generated(_)) {
                    return Task::none();
                }

                // Only incoming payments (positive amount) should
                // trigger the celebration. Outgoing events with
                // negative amounts are skipped.
                let is_incoming = amount_sat > 0;
                if !is_incoming {
                    return Task::none();
                }
                // USDB on Spark waits for USDB itself (by identifier, not
                // ticker: any token can call itself USDB); every other method
                // for bitcoin. A token's amount is in its own base units, so
                // it can't be counted as sats (or the reverse), and another
                // token arriving at the same address is not this payment.
                let wanted = if self.method == SparkReceiveMethod::UsdbSpark {
                    token.as_ref().is_some_and(|token| token.usdb)
                } else {
                    token.is_none()
                };
                if !wanted {
                    return Task::none();
                }

                // Correlate the event with the currently displayed
                // receive method so we only celebrate the payment the
                // user is actually waiting for:
                //
                // - Bolt11 invoice displayed + matching bolt11 event:
                //   the invoice was paid → advance.
                // - Bolt11 invoice displayed + event without bolt11:
                //   unrelated non-Lightning payment → skip.
                // - No invoice displayed (on-chain flow) + event
                //   without bolt11: on-chain deposit / Spark-native
                //   transfer → advance.
                // - No invoice displayed (on-chain flow) + event with
                //   bolt11: unrelated Lightning payment → skip.
                //
                // BOLT11 comparison is case-insensitive — canonical
                // form is lowercase but some SDKs hand back mixed case.
                //
                // For follow-ups during the celebration we only
                // aggregate when the panel was generating an on-chain /
                // Spark address AND the event has no bolt11 — a BOLT11
                // invoice is single-use and unrelated Lightning
                // activity that happens to settle while the on-chain
                // celebration is on screen would otherwise inflate the
                // running total. For BOLT11 celebrations we never
                // aggregate: a second event after invoice settlement
                // is by definition a different payment.
                let matches_invoice = if already_celebrating {
                    self.method != SparkReceiveMethod::Bolt11 && bolt11.is_none()
                } else {
                    match (&self.displayed_invoice, &bolt11) {
                        (Some(displayed), Some(event_bolt11)) => {
                            displayed.eq_ignore_ascii_case(event_bolt11)
                        }
                        (Some(_), None) => false,
                        (None, None) => true,
                        (None, Some(_)) => false,
                    }
                };
                if !matches_invoice {
                    return Task::none();
                }

                self.qr_data = None;
                self.displayed_invoice = None;
                let (running_total, count) = match self.phase {
                    SparkReceivePhase::Received {
                        amount_sat: prev,
                        count,
                    } => (prev.saturating_add(amount_sat), count.saturating_add(1)),
                    _ => (amount_sat, 1),
                };
                // The receipt shows how many payments the total covers.
                self.received_amount_display = match &token {
                    Some(token) => format!(
                        "{} {}",
                        crate::app::breez_spark::assets::format_token_display_u128(
                            u128::from(running_total.unsigned_abs()),
                            token.decimals,
                        ),
                        token.ticker
                    ),
                    None => format!(
                        "{} sats",
                        coincube_ui::component::amount::format_u64_as_string(
                            running_total.unsigned_abs(),
                            ","
                        )
                    ),
                };
                // Pick celebration image based on receive method.
                // Only re-roll the quote on the first arrival so the
                // imagery doesn't flicker when a follow-up deposit
                // bumps the total.
                if !already_celebrating {
                    let context = if self.method == SparkReceiveMethod::Bolt11 {
                        "lightning-receive"
                    } else {
                        "spark-receive"
                    };
                    self.received_celebration_context = context.to_string();
                    self.received_quote =
                        coincube_ui::component::quote_display::random_quote(context);
                    self.received_image_handle =
                        coincube_ui::component::quote_display::image_handle_for_context(context);
                }
                self.phase = SparkReceivePhase::Received {
                    amount_sat: running_total,
                    count,
                };
                // Surface the just-received payment in the Last
                // Transactions list the moment it arrives.
                fetch_payments_task(self.backend.clone())
            }
            SparkReceiveMessage::PendingDepositsLoaded(deposits) => {
                self.pending_deposits = deposits;
                self.claim_error = None;
                // Drop confirmation entries for deposits that left
                // the list (claimed or refunded), then kick off a
                // fresh Esplora fetch for any immature deposit still
                // in the list. Mature deposits skip the fetch — they
                // already have a Claim button and the per-confirmation
                // count is no longer useful.
                let live_keys: std::collections::HashSet<(String, u32)> = self
                    .pending_deposits
                    .iter()
                    .map(|d| (d.txid.clone(), d.vout))
                    .collect();
                // Keep every live deposit's deferral note across refreshes.
                self.claim_notices.retain(|k, _| live_keys.contains(k));
                self.pending_deposit_confirmations
                    .retain(|k, _| live_keys.contains(k));
                self.remember_swap_confirmations();
                refresh_confirmations_task(cache.network, &self.pending_deposits)
            }
            SparkReceiveMessage::PendingDepositsFailed(err) => {
                tracing::warn!("Spark list_unclaimed_deposits failed: {}", err);
                // Don't surface as a hard error — the rest of the
                // panel still works. Just clear the displayed list.
                self.pending_deposits.clear();
                self.pending_deposit_confirmations.clear();
                Task::none()
            }
            SparkReceiveMessage::DepositConfirmationsUpdated(map) => {
                // Merge rather than replace: a partial fetch (one
                // deposit's GET failed) shouldn't wipe the confirmation
                // count for the others.
                //
                // Plain overwrite per key — we deliberately do NOT
                // `max` here. A reorg can legitimately drop a
                // deposit's confirmation count (the block at height
                // H got replaced, so the tx is back in the mempool
                // until it re-confirms), and the user should see
                // that reflected. The cost is that if the 30s
                // subscription tick fires a second fetch before the
                // first completes (possible when many immature
                // deposits serialize through Esplora's 8s per-request
                // timeout) and the slower one lands after the faster
                // one, the displayed count can briefly flicker
                // backward until the next tick corrects it — but
                // both responses are valid past-chain snapshots, so
                // it's stale display, not wrong data.
                for (key, confs) in map {
                    self.pending_deposit_confirmations.insert(key, confs);
                }
                self.remember_swap_confirmations();
                Task::none()
            }
            SparkReceiveMessage::RefreshSettleConfirmations => {
                self.settle_confirmations_task(cache.network)
            }
            SparkReceiveMessage::RefreshConfirmations => Task::batch([
                refresh_confirmations_task(cache.network, &self.pending_deposits),
                self.settle_confirmations_task(cache.network),
            ]),
            SparkReceiveMessage::SettleConfirmationsUpdated {
                txid,
                confirmations,
            } => {
                let Some(confirmations) = confirmations else {
                    return Task::none();
                };
                // Only while the swap still polls this txid: a late answer for
                // a swap that has moved on (arrived, reset, another shift), or
                // one landing after the wallet listed the deposit (whose own
                // count then rules), is not this swap's count.
                let ours = self
                    .swap_settle_txid_to_poll()
                    .is_some_and(|hash| hash.eq_ignore_ascii_case(&txid));
                if !ours {
                    return Task::none();
                }
                let Some(flow) = self.sideshift_flow.as_mut() else {
                    return Task::none();
                };
                let previous = flow.last_settle_confirmations().unwrap_or(0);
                flow.note_settle_confirmations(Some(confirmations));
                // A new block may be what makes the SDK list the deposit (or
                // mature it), so re-read the pending deposits when it lands.
                if confirmations > previous {
                    fetch_deposits_task(self.backend.clone())
                } else {
                    Task::none()
                }
            }
            SparkReceiveMessage::ClaimDepositRequested { txid, vout } => {
                let Some(backend) = self.backend.clone() else {
                    return Task::none();
                };
                let deposit = (txid.clone(), vout);
                self.claim_error = None;
                // A note about another deposit stays on that row.
                self.claim_notices.remove(&deposit);
                self.claiming.insert(deposit);
                let claimed_txid = txid.clone();
                Task::perform(
                    async move { backend.claim_deposit(txid, vout).await },
                    move |result| {
                        let msg = match result {
                            Ok(ok) => {
                                crate::app::view::SparkReceiveMessage::ClaimDepositSucceeded {
                                    txid: claimed_txid,
                                    vout,
                                    ok,
                                }
                            }
                            Err(e) => crate::app::view::SparkReceiveMessage::ClaimDepositFailed {
                                txid: claimed_txid,
                                vout,
                                error: e.to_string(),
                            },
                        };
                        Message::View(crate::app::view::Message::SparkReceive(msg))
                    },
                )
            }
            SparkReceiveMessage::ClaimDepositSucceeded { txid, vout, ok } => {
                // Key everything by the deposit this result is for, not by
                // whichever claim was started last.
                let deposit = (txid, vout);
                self.claiming.remove(&deposit);
                self.claim_error = None;
                if let Some(deferral) = &ok.deferred {
                    // Nothing changed: no splash, no refetch (a reload would
                    // not show anything new), and the row stays pending.
                    self.claim_notices
                        .insert(deposit, claim_deferral_notice(deferral));
                    return Task::none();
                }
                self.claim_notices.remove(&deposit);
                let mut tasks = vec![
                    // Drop the claimed row from the pending-deposits list.
                    fetch_deposits_task(self.backend.clone()),
                    // Refresh "Last transactions" so the just-claimed deposit
                    // shows without navigating away and back — a claim fires only
                    // `DepositsChanged`, not a payment event, so nothing else
                    // repopulates the payments list here.
                    fetch_payments_task(self.backend.clone()),
                ];
                // The bitcoin just landed in the spendable balance — fire the
                // global "received" splash, same as an auto-claimed swap. A
                // pre-maturity claim (SDK 0.25.0+) settles asynchronously and
                // carries no amount yet; its `PaymentSucceeded` fires the
                // splash when the transfer lands instead.
                if let Some(amount_sat) = ok.amount_sat {
                    tasks.push(Task::done(Message::ShowReceivedCelebration {
                        context: "spark-receive".to_string(),
                        amount_sat,
                    }));
                }
                Task::batch(tasks)
            }
            SparkReceiveMessage::ClaimDepositFailed { txid, vout, error } => {
                let deposit = (txid, vout);
                self.claiming.remove(&deposit);
                self.claim_notices.remove(&deposit);
                self.claim_error = Some(error);
                Task::none()
            }
            SparkReceiveMessage::DepositsChanged => Task::batch(vec![
                fetch_deposits_task(self.backend.clone()),
                // Also refresh Last transactions: a claim (or any deposit landing)
                // surfaces a new payment, and the panel won't otherwise repopulate
                // the list until the next navigation.
                fetch_payments_task(self.backend.clone()),
            ]),
            SparkReceiveMessage::ViewActivity => Task::done(Message::View(
                crate::app::view::Message::SparkReceive(SparkReceiveMessage::Reset),
            ))
            .chain(redirect(Menu::Cube(
                crate::app::menu::CubeSubMenu::Activity,
            ))),
            SparkReceiveMessage::Reset => {
                self.qr_data = None;
                self.displayed_invoice = None;
                self.phase = SparkReceivePhase::Idle;
                // Spark and on-chain have no button in `Idle`; `Try again`
                // has to fetch the address itself.
                self.fetch_static_address_if_due()
            }
            SparkReceiveMessage::BalanceLoaded(balance) => {
                if let Some((btc_sats, stable)) = balance {
                    self.balance_sats =
                        super::unified_spark_balance_sats(btc_sats, stable.as_ref(), cache);
                    self.btc_balance_sats = btc_sats;
                    self.usdb_balance = stable;
                }
                Task::none()
            }
            SparkReceiveMessage::PaymentsLoaded(payments) => {
                self.recent_transactions = super::recent_transactions_from(&payments, cache);
                Task::none()
            }
            SparkReceiveMessage::PaymentsFailed(err) => {
                tracing::warn!("spark receive list_payments failed: {}", err);
                self.recent_transactions.clear();
                Task::none()
            }
            SparkReceiveMessage::SelectTransaction(idx) => {
                if let Some(payment) = self.recent_transactions.get(idx).cloned() {
                    Task::batch(vec![
                        redirect(Menu::Spark(SparkSubMenu::Transactions(None))),
                        Task::done(Message::View(crate::app::view::Message::SparkTransactions(
                            crate::app::view::SparkTransactionsMessage::Preselect(payment),
                        ))),
                    ])
                } else {
                    Task::none()
                }
            }
            SparkReceiveMessage::History => redirect(Menu::Spark(SparkSubMenu::Transactions(None))),
            SparkReceiveMessage::CopyPaymentRequest(text) => {
                if self.method == SparkReceiveMethod::LightningAddress
                    && (!cache.connect_authenticated
                        || cache.lightning_address.as_deref() != Some(text.as_str()))
                {
                    return Task::none();
                }
                // Pair the clipboard write with the global toast overlay so
                // the button gives the same feedback as every other Copy in
                // the app. The label names the rail the request belongs to —
                // an on-chain deposit address and a BOLT11 invoice look
                // nothing alike, and the confirmation is a cheap way to say
                // which one landed on the clipboard.
                let label = match self.method {
                    SparkReceiveMethod::Bolt11 => "Copied Lightning invoice to clipboard",
                    SparkReceiveMethod::LightningAddress => "Copied Lightning address to clipboard",
                    SparkReceiveMethod::OnchainBitcoin => "Copied Bitcoin address to clipboard",
                    SparkReceiveMethod::Spark | SparkReceiveMethod::UsdbSpark => {
                        "Copied Spark address to clipboard"
                    }
                };
                Task::batch([
                    iced::clipboard::write(text),
                    Task::done(Message::View(crate::app::view::Message::ShowToast(
                        log::Level::Info,
                        label.to_string(),
                    ))),
                ])
            }
        }
    }
}

/// Panel-local thin wrapper around the shared
/// [`super::fetch_payments_task`] helper — only the message variants
/// differ between the Send and Receive panels.
fn fetch_payments_task(backend: Option<Arc<SparkBackend>>) -> Task<Message> {
    super::fetch_payments_task(
        backend,
        |payments| {
            Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::PaymentsLoaded(payments),
            ))
        },
        |err| {
            Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::PaymentsFailed(err),
            ))
        },
    )
}

/// Spark-Receive-flavoured wrapper around [`super::fetch_balance_task`].
/// Ask the Connect Cube panel to give this Cube a random Lightning address,
/// when it is signed in and has none. The panel checks the rest (address
/// known to be empty, nothing in flight, no failure awaiting a retry).
fn auto_assign_lightning_address(cache: &Cache) -> Option<Task<Message>> {
    (cache.connect_authenticated
        && cache.lightning_address.is_none()
        && cache.lightning_address_setup == crate::app::cache::LightningAddressSetup::Idle)
        .then(|| {
            Task::done(Message::View(crate::app::view::Message::ConnectCube(
                crate::app::view::ConnectCubeMessage::AutoAssignLightningAddress { retry: false },
            )))
        })
}

/// Methods whose address is static and needs no input, so Receive fetches
/// it as soon as the method is picked.
pub(crate) fn auto_generates(method: SparkReceiveMethod) -> bool {
    matches!(
        method,
        SparkReceiveMethod::Spark
            | SparkReceiveMethod::UsdbSpark
            | SparkReceiveMethod::OnchainBitcoin
    )
}

fn fetch_auto_convert_task(backend: Option<Arc<SparkBackend>>) -> Task<Message> {
    let Some(backend) = backend else {
        return Task::none();
    };
    Task::perform(
        async move { backend.client().get_auto_convert().await.ok() },
        |state| {
            Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::AutoConvertLoaded(
                    state.map(|state| state.enabled),
                ),
            ))
        },
    )
}

fn fetch_balance_task(backend: Option<Arc<SparkBackend>>) -> Task<Message> {
    super::fetch_balance_task(backend, |balance| {
        Message::View(crate::app::view::Message::SparkReceive(
            crate::app::view::SparkReceiveMessage::BalanceLoaded(balance),
        ))
    })
}

/// Kick off an Esplora confirmation-count fetch for every immature
/// deposit in `deposits`. Returns `Task::none()` when there's nothing
/// to fetch, or when `network` has no public Esplora to hit — keeps
/// the call site at `PendingDepositsLoaded` / `RefreshConfirmations`
/// branch-free.
fn refresh_confirmations_task(
    network: coincube_core::miniscript::bitcoin::Network,
    deposits: &[DepositInfo],
) -> Task<Message> {
    if !super::esplora::is_supported(network) {
        return Task::none();
    }
    let targets: Vec<(String, u32)> = deposits
        .iter()
        .filter(|d| !d.is_mature)
        .map(|d| (d.txid.clone(), d.vout))
        .collect();
    if targets.is_empty() {
        return Task::none();
    }
    Task::perform(
        super::esplora::fetch_confirmations(network, targets),
        |map| {
            Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::DepositConfirmationsUpdated(map),
            ))
        },
    )
}

/// Fire a `list_unclaimed_deposits` RPC and translate the result into
/// the appropriate view message. Pulled out as a helper so the
/// `reload`, `ClaimDepositSucceeded`, and `DepositsChanged` paths can
/// share it without duplicating the closure boilerplate.
fn fetch_deposits_task(backend: Option<Arc<SparkBackend>>) -> Task<Message> {
    let Some(backend) = backend else {
        return Task::none();
    };
    Task::perform(
        async move { backend.list_unclaimed_deposits().await },
        |result| match result {
            Ok(ok) => Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::PendingDepositsLoaded(ok.deposits),
            )),
            Err(e) => Message::View(crate::app::view::Message::SparkReceive(
                crate::app::view::SparkReceiveMessage::PendingDepositsFailed(e.to_string()),
            )),
        },
    )
}

/// What a deferred claim (SDK 0.28.0+) means for the user, under the Pending
/// deposits card. Nothing went wrong, so it says why it waits and that it
/// will be claimed without them.
pub(crate) fn claim_deferral_notice(deferral: &coincube_spark_protocol::ClaimDeferral) -> String {
    use coincube_spark_protocol::ClaimDeferral;
    match deferral {
        ClaimDeferral::MaxFeeExceeded {
            required_fee_sats,
            max_fee_sats,
        } => coincube_ui::localized_format!(
            "Not claimed yet: claiming now would cost {} sats, more than the {}-sat limit. \
             It will be claimed automatically once the deposit matures.",
            "Aún no reclamado: reclamarlo ahora costaría {} sats, más que el límite de {} sats. \
             Se reclamará automáticamente cuando el depósito madure.",
            required_fee_sats,
            max_fee_sats
        ),
        ClaimDeferral::NoEarlyClaimAvailable => coincube_ui::localized_format!(
            "Not claimed yet: it needs more confirmations. It will be claimed automatically.",
            "Aún no reclamado: necesita más confirmaciones. Se reclamará automáticamente."
        ),
        ClaimDeferral::ProviderDeclined { message } => coincube_ui::localized_format!(
            "Not claimed yet: the claim service declined ({}). It will be claimed automatically \
             once the deposit matures.",
            "Aún no reclamado: el servicio de reclamación lo rechazó ({}). Se reclamará \
             automáticamente cuando el depósito madure.",
            message
        ),
    }
}

impl SparkReceive {
    /// The pending deposit that is the active swap's settle output — only once
    /// SideShift has sent the bitcoin; earlier, an amount match would be an
    /// unrelated deposit.
    fn swap_settle_deposit(&self) -> Option<&DepositInfo> {
        let flow = self.sideshift_flow.as_ref()?;
        if !flow.settle_output_live() {
            return None;
        }
        settle_deposit(
            flow.settle_hash(),
            flow.settle_amount_sat(),
            &self.pending_deposits,
        )
    }

    /// The swap's settle confirmations: the count fetched for its pending
    /// deposit, else the last count seen (by txid before the wallet listed
    /// the deposit, or before it left that list once claimed), else zero for
    /// a deposit listed before any count is known.
    ///
    /// A deposit the wallet has just listed has no fetched count yet; it must
    /// not read as zero over a count already seen by txid.
    fn swap_settle_confirmations(&self) -> Option<u32> {
        let flow = self.sideshift_flow.as_ref()?;
        if !flow.settle_output_live() {
            return None;
        }
        let (hash, sats) = (flow.settle_hash(), flow.settle_amount_sat());
        known_settle_confirmations(
            hash,
            sats,
            &self.pending_deposits,
            &self.pending_deposit_confirmations,
        )
        .or(flow.last_settle_confirmations())
        .or_else(|| {
            settle_confirmations(
                hash,
                sats,
                &self.pending_deposits,
                &self.pending_deposit_confirmations,
            )
        })
    }

    /// The swap's settle txid, when its confirmations must be read straight
    /// from Esplora: SideShift has sent the bitcoin and reported the txid, the
    /// wallet's pending deposits don't list that transaction (those are
    /// already polled), and it hasn't reached maturity yet.
    fn swap_settle_txid_to_poll(&self) -> Option<&str> {
        let flow = self.sideshift_flow.as_ref()?;
        if !flow.settle_output_live() || self.swap_settle_deposit().is_some() {
            return None;
        }
        if flow.last_settle_confirmations().unwrap_or(0) >= DEPOSIT_MATURITY_CONFIRMATIONS {
            return None;
        }
        // The hash goes into an Esplora URL path: only a txid's shape.
        flow.settle_hash()
            .filter(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
    }

    /// What reloading the panel asks for, so a swap's settle count is fresh on
    /// return rather than up to one 30s tick late: a fetch started while the
    /// panel was off-screen answers to no one (its reply only reaches the
    /// current panel), and the status poll fetches only when polling starts.
    /// `reload` has no network to fetch with, so it sends
    /// `RefreshSettleConfirmations`, which does; the pending deposits are
    /// re-read by reload itself.
    fn settle_refresh_on_reload(&self) -> Option<crate::app::view::SparkReceiveMessage> {
        self.swap_settle_txid_to_poll()
            .map(|_| crate::app::view::SparkReceiveMessage::RefreshSettleConfirmations)
    }

    /// Read the swap's settle transaction's confirmations from Esplora, when
    /// [`Self::swap_settle_txid_to_poll`] says it's needed.
    fn settle_confirmations_task(
        &self,
        network: coincube_core::miniscript::bitcoin::Network,
    ) -> Task<Message> {
        let Some(txid) = self.swap_settle_txid_to_poll() else {
            return Task::none();
        };
        if !super::esplora::is_supported(network) {
            return Task::none();
        }
        // Confirmations belong to the transaction; the vout is only a map key.
        let key = (txid.to_string(), 0);
        let txid = key.0.clone();
        Task::perform(
            super::esplora::fetch_confirmations(network, vec![key.clone()]),
            move |map| {
                Message::View(crate::app::view::Message::SparkReceive(
                    crate::app::view::SparkReceiveMessage::SettleConfirmationsUpdated {
                        txid,
                        confirmations: map.get(&key).copied(),
                    },
                ))
            },
        )
    }

    fn remember_swap_confirmations(&mut self) {
        let live = self.sideshift_flow.as_ref().and_then(|flow| {
            flow.settle_output_live()
                .then(|| {
                    known_settle_confirmations(
                        flow.settle_hash(),
                        flow.settle_amount_sat(),
                        &self.pending_deposits,
                        &self.pending_deposit_confirmations,
                    )
                })
                .flatten()
        });
        if let Some(flow) = self.sideshift_flow.as_mut() {
            flow.note_settle_confirmations(live);
        }
    }
}

/// Whether the swap's own pending deposit can be left out of the Pending
/// deposits card: only while it is simply confirming. A matured deposit (with
/// its Claim button), a failed claim (Retry) or a deferral note stays in the
/// card — the swap body shows progress, not those actions.
fn quietly_settling(deposit: &DepositInfo, notices: &HashMap<(String, u32), String>) -> bool {
    !deposit.is_mature
        && deposit.claim_error.is_none()
        && !notices.contains_key(&(deposit.txid.clone(), deposit.vout))
}

/// The wallet's pending (unclaimed) deposit that is a cross-network swap's
/// settle output: by SideShift's settle txid when it has reported one, else by
/// an amount that matches exactly one deposit — never a guess between two.
fn settle_deposit<'d>(
    settle_hash: Option<&str>,
    settle_amount_sat: Option<u64>,
    deposits: &'d [DepositInfo],
) -> Option<&'d DepositInfo> {
    match settle_hash {
        Some(hash) => deposits.iter().find(|d| d.txid.eq_ignore_ascii_case(hash)),
        None => {
            let sats = settle_amount_sat?;
            let mut matches = deposits.iter().filter(|d| d.amount_sat == sats);
            match (matches.next(), matches.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        }
    }
}

/// Confirmations on a cross-network swap's settle transaction (see
/// [`settle_deposit`]). A matured deposit counts as fully confirmed even if its
/// exact count hasn't been fetched. `None` until the settle shows up.
fn settle_confirmations(
    settle_hash: Option<&str>,
    settle_amount_sat: Option<u64>,
    deposits: &[DepositInfo],
    confirmations: &HashMap<(String, u32), u32>,
) -> Option<u32> {
    let deposit = settle_deposit(settle_hash, settle_amount_sat, deposits)?;
    let fetched = confirmations
        .get(&(deposit.txid.clone(), deposit.vout))
        .copied();
    if deposit.is_mature {
        Some(fetched.unwrap_or(0).max(DEPOSIT_MATURITY_CONFIRMATIONS))
    } else {
        Some(fetched.unwrap_or(0))
    }
}

/// [`settle_confirmations`], but only when a count is actually known: a
/// fetched count, or maturity. A deposit listed with no fetched count yet is
/// `None` here rather than zero, so it can't overwrite a count already seen.
fn known_settle_confirmations(
    settle_hash: Option<&str>,
    settle_amount_sat: Option<u64>,
    deposits: &[DepositInfo],
    confirmations: &HashMap<(String, u32), u32>,
) -> Option<u32> {
    let deposit = settle_deposit(settle_hash, settle_amount_sat, deposits)?;
    let fetched = confirmations
        .get(&(deposit.txid.clone(), deposit.vout))
        .copied();
    if deposit.is_mature {
        Some(fetched.unwrap_or(0).max(DEPOSIT_MATURITY_CONFIRMATIONS))
    } else {
        fetched
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::message::Message as AppMessage;
    use crate::app::state::State;
    use crate::app::view::{Message as ViewMessage, SparkReceiveMessage};
    use coincube_core::miniscript::bitcoin::Network;
    use coincube_spark_protocol::{PaymentSummary, UsdbBalanceSnapshot};

    fn receive_ok(payment_request: &str) -> ReceivePaymentOk {
        ReceivePaymentOk {
            payment_request: payment_request.to_string(),
            fee_sat: 7,
        }
    }

    #[test]
    fn settle_confirmations_find_the_swap_deposit_by_txid_then_by_unique_amount() {
        let mut a = deposit("aa11", 0, false);
        a.amount_sat = 31_250;
        let mut b = deposit("bb22", 1, false);
        b.amount_sat = 50_000;
        let deposits = vec![a, b];
        let confs: HashMap<(String, u32), u32> =
            [(("aa11".to_string(), 0), 1), (("bb22".to_string(), 1), 2)].into();

        // The settle txid wins, whatever the amount says.
        assert_eq!(
            settle_confirmations(Some("BB22"), Some(31_250), &deposits, &confs),
            Some(2)
        );
        // A txid that isn't (yet) a pending deposit: nothing to show.
        assert_eq!(
            settle_confirmations(Some("cc33"), Some(31_250), &deposits, &confs),
            None
        );
        // No txid: a unique exact amount match.
        assert_eq!(
            settle_confirmations(None, Some(31_250), &deposits, &confs),
            Some(1)
        );
        assert_eq!(settle_confirmations(None, None, &deposits, &confs), None);

        // Two deposits of the same amount: refuse to guess.
        let mut twin = deposit("dd44", 0, false);
        twin.amount_sat = 31_250;
        let ambiguous = vec![deposits[0].clone(), twin];
        assert_eq!(
            settle_confirmations(None, Some(31_250), &ambiguous, &confs),
            None
        );

        // Found but not yet counted reads 0; matured reads at least the threshold.
        assert_eq!(
            settle_confirmations(Some("aa11"), None, &deposits, &HashMap::new()),
            Some(0)
        );
        let mut mature = deposit("ee55", 0, true);
        mature.amount_sat = 1;
        assert_eq!(
            settle_confirmations(Some("ee55"), None, &[mature], &HashMap::new()),
            Some(DEPOSIT_MATURITY_CONFIRMATIONS)
        );
    }

    #[test]
    fn only_a_quietly_confirming_swap_deposit_leaves_the_pending_card() {
        let confirming = deposit("aa11", 0, false);
        assert!(quietly_settling(&confirming, &HashMap::new()));

        // Matured: its Claim button has to stay reachable.
        assert!(!quietly_settling(
            &deposit("aa11", 0, true),
            &HashMap::new()
        ));

        // A failed claim keeps its Retry.
        let mut failed = deposit("aa11", 0, false);
        failed.claim_error = Some("claim failed".into());
        assert!(!quietly_settling(&failed, &HashMap::new()));

        // A deferral note is only shown in the card.
        let notices = HashMap::from([(("aa11".to_string(), 0), "deferred".to_string())]);
        assert!(!quietly_settling(&confirming, &notices));
    }

    /// Picking under YOU RECEIVE flips auto-convert only from a known,
    /// different state; until it has been read nothing is changed.
    #[test]
    fn receive_as_follows_the_read_auto_convert_state() {
        let mut panel = SparkReceive::new(None);
        let cache = Cache {
            network: Network::Bitcoin,
            ..Cache::default()
        };
        update_with_cache(&mut panel, &cache, SparkReceiveMessage::OpenReceiverPicker);
        assert!(panel.receiver_picker_open);
        update_with_cache(
            &mut panel,
            &cache,
            SparkReceiveMessage::SelectReceiveAs(true),
        );
        assert!(!panel.receiver_picker_open);
        assert_eq!(panel.auto_convert, None, "unknown state is left alone");

        update(
            &mut panel,
            SparkReceiveMessage::AutoConvertLoaded(Some(false)),
        );
        update(&mut panel, SparkReceiveMessage::SelectReceiveAs(true));
        assert_eq!(
            panel.auto_convert,
            Some(false),
            "the tile waits for the save to land"
        );
        // A dropped or failed save leaves the saved value on the tile.
        let _ = panel.stable_balance_settled(None);
        assert_eq!(panel.auto_convert, Some(false));
        let _ = panel.stable_balance_settled(Some(true));
        assert_eq!(panel.auto_convert, Some(true));
        // A failed read keeps what is known.
        update(&mut panel, SparkReceiveMessage::AutoConvertLoaded(None));
        assert_eq!(panel.auto_convert, Some(true));

        // Off mainnet there is no US Dollar to pick.
        let mut regtest = SparkReceive::new(None);
        update_with_cache(
            &mut regtest,
            &Cache {
                network: Network::Regtest,
                ..Cache::default()
            },
            SparkReceiveMessage::OpenReceiverPicker,
        );
        assert!(!regtest.receiver_picker_open);
    }

    fn deposit(txid: &str, vout: u32, is_mature: bool) -> DepositInfo {
        DepositInfo {
            txid: txid.to_string(),
            vout,
            amount_sat: 12_345,
            is_mature,
            claim_error: None,
        }
    }

    fn payment(id: &str, amount_sat: i64) -> PaymentSummary {
        PaymentSummary {
            id: id.to_string(),
            amount_sat,
            fees_sat: 12,
            token_amount: None,
            token_decimals: None,
            token_ticker: None,
            timestamp: 1_700_000_000,
            status: "completed".to_string(),
            direction: "receive".to_string(),
            method: "spark".to_string(),
            description: None,
            details: Default::default(),
        }
    }

    fn usdb_balance(balance: u64, decimals: u32) -> UsdbBalanceSnapshot {
        UsdbBalanceSnapshot {
            token_identifier: "btkn1synthetic".into(),
            balance: balance.into(),
            decimals,
            ticker: "USDB".to_string(),
        }
    }

    fn update_with_cache(panel: &mut SparkReceive, cache: &Cache, msg: SparkReceiveMessage) {
        let _task = State::update(
            panel,
            None,
            cache,
            AppMessage::View(ViewMessage::SparkReceive(msg)),
        );
    }

    fn update(panel: &mut SparkReceive, msg: SparkReceiveMessage) {
        update_with_cache(panel, &Cache::default(), msg);
    }

    #[test]
    fn receive_method_labels_are_stable() {
        assert_eq!(SparkReceiveMethod::Bolt11.label(), "Lightning (BOLT11)");
        assert_eq!(
            SparkReceiveMethod::LightningAddress.label(),
            "Lightning address"
        );
        assert_eq!(
            SparkReceiveMethod::OnchainBitcoin.label(),
            "On-chain Bitcoin"
        );
        assert_eq!(SparkReceiveMethod::Spark.label(), "Spark");
    }

    #[test]
    fn lightning_address_selection_uses_only_signed_in_current_mirror() {
        let mut panel = SparkReceive::new(None);
        let mut cache = Cache {
            lightning_address: Some("synthetic@coincube.io".to_owned()),
            has_connect_session: true,
            ..Default::default()
        };
        update_with_cache(
            &mut panel,
            &cache,
            SparkReceiveMessage::SelectSenderRail(SparkReceiveMethod::LightningAddress),
        );
        assert_eq!(panel.method, SparkReceiveMethod::LightningAddress);
        assert!(panel.lightning_address_qr.is_none());
        cache.connect_authenticated = true;
        update_with_cache(
            &mut panel,
            &cache,
            SparkReceiveMessage::SelectSenderRail(SparkReceiveMethod::LightningAddress),
        );
        assert_eq!(
            panel
                .lightning_address_qr
                .as_ref()
                .map(|(value, _)| value.as_str()),
            Some("synthetic@coincube.io")
        );
        cache.connect_authenticated = false;
        update_with_cache(
            &mut panel,
            &cache,
            SparkReceiveMessage::SelectSenderRail(SparkReceiveMethod::LightningAddress),
        );
        assert!(panel.lightning_address_qr.is_none());
    }

    #[test]
    fn new_panel_starts_in_bolt11_idle_state() {
        let panel = SparkReceive::new(None);

        assert!(panel.backend.is_none());
        assert_eq!(panel.balance_sats, 0);
        assert_eq!(panel.method, SparkReceiveMethod::Bolt11);
        assert!(!panel.sender_picker_open);
        assert!(matches!(panel.phase(), SparkReceivePhase::Idle));
        assert!(panel.qr_data.is_none());
        assert!(panel.displayed_invoice.is_none());
        assert!(panel.pending_deposits.is_empty());
        assert!(panel.recent_transactions.is_empty());
    }

    #[test]
    fn generate_without_backend_surfaces_actionable_error() {
        let mut panel = SparkReceive::new(None);

        update(&mut panel, SparkReceiveMessage::GenerateRequested);

        assert!(matches!(
            panel.phase(),
            SparkReceivePhase::Error(msg) if msg == "Spark backend is not available."
        ));
    }

    #[test]
    fn input_edits_reset_generated_payment_state() {
        let mut panel = SparkReceive::new(None);

        update(
            &mut panel,
            SparkReceiveMessage::GenerateSucceeded(receive_ok("lnbc1invoice")),
        );
        assert!(matches!(panel.phase(), SparkReceivePhase::Generated(_)));
        assert!(panel.qr_data.is_some());
        assert_eq!(panel.displayed_invoice.as_deref(), Some("lnbc1invoice"));

        update(
            &mut panel,
            SparkReceiveMessage::AmountInputChanged("2500".to_string()),
        );

        assert_eq!(panel.amount_input, "2500");
        assert!(matches!(panel.phase(), SparkReceivePhase::Idle));
        assert!(panel.qr_data.is_none());
        assert!(panel.displayed_invoice.is_none());

        update(
            &mut panel,
            SparkReceiveMessage::GenerateSucceeded(receive_ok("lnbc1invoice2")),
        );
        update(
            &mut panel,
            SparkReceiveMessage::DescriptionInputChanged("for coffee".to_string()),
        );

        assert_eq!(panel.description_input, "for coffee");
        assert!(matches!(panel.phase(), SparkReceivePhase::Idle));
        assert!(panel.qr_data.is_none());
        assert!(panel.displayed_invoice.is_none());
    }

    #[test]
    fn generated_bolt11_payment_captures_invoice_for_correlation() {
        let mut panel = SparkReceive::new(None);

        update(
            &mut panel,
            SparkReceiveMessage::GenerateSucceeded(receive_ok("lnbc1invoice")),
        );

        assert!(matches!(panel.phase(), SparkReceivePhase::Generated(_)));
        assert_eq!(panel.displayed_invoice.as_deref(), Some("lnbc1invoice"));
        assert!(panel.qr_data.is_some());
    }

    #[test]
    fn generated_onchain_payment_does_not_capture_invoice() {
        let mut panel = SparkReceive::new(None);
        panel.method = SparkReceiveMethod::OnchainBitcoin;

        update(
            &mut panel,
            SparkReceiveMessage::GenerateSucceeded(receive_ok("bc1qaddress")),
        );

        assert!(matches!(panel.phase(), SparkReceivePhase::Generated(_)));
        assert!(panel.displayed_invoice.is_none());
        assert!(panel.qr_data.is_some());
    }

    #[test]
    fn failed_generation_clears_qr_and_invoice_state() {
        let mut panel = SparkReceive::new(None);
        update(
            &mut panel,
            SparkReceiveMessage::GenerateSucceeded(receive_ok("lnbc1invoice")),
        );

        update(
            &mut panel,
            SparkReceiveMessage::GenerateFailed("bridge down".to_string()),
        );

        assert!(matches!(
            panel.phase(),
            SparkReceivePhase::Error(msg) if msg == "bridge down"
        ));
        assert!(panel.qr_data.is_none());
        assert!(panel.displayed_invoice.is_none());
    }

    fn usdb_token() -> Option<coincube_spark_protocol::PaymentToken> {
        Some(coincube_spark_protocol::PaymentToken {
            identifier: "btkn1usdb".to_string(),
            ticker: "USDB".to_string(),
            decimals: 6,
            usdb: true,
        })
    }

    /// The US Dollar row's Receive opens USDB → USDB on mainnet; the Bitcoin
    /// row's leaves it for Lightning but keeps a Bitcoin rail already picked.
    #[test]
    fn start_from_a_balance_row_picks_what_the_payer_sends() {
        let mainnet = Cache {
            network: Network::Bitcoin,
            ..Cache::default()
        };
        let mut panel = SparkReceive::new(None);
        update_with_cache(
            &mut panel,
            &mainnet,
            SparkReceiveMessage::StartFrom(coincube_spark_protocol::WalletAsset::Usdb),
        );
        assert_eq!(panel.method, SparkReceiveMethod::UsdbSpark);
        assert!(auto_generates(panel.method), "the Spark address is fetched");

        update_with_cache(
            &mut panel,
            &mainnet,
            SparkReceiveMessage::StartFrom(coincube_spark_protocol::WalletAsset::Bitcoin),
        );
        assert_eq!(panel.method, SparkReceiveMethod::Bolt11);

        update_with_cache(
            &mut panel,
            &mainnet,
            SparkReceiveMessage::SelectSenderRail(SparkReceiveMethod::Spark),
        );
        update_with_cache(
            &mut panel,
            &mainnet,
            SparkReceiveMessage::StartFrom(coincube_spark_protocol::WalletAsset::Bitcoin),
        );
        assert_eq!(panel.method, SparkReceiveMethod::Spark);

        // USDB exists where conversions run: not on testnet.
        let testnet = Cache {
            network: Network::Testnet,
            ..Cache::default()
        };
        let mut panel = SparkReceive::new(None);
        update_with_cache(
            &mut panel,
            &testnet,
            SparkReceiveMessage::StartFrom(coincube_spark_protocol::WalletAsset::Usdb),
        );
        assert_eq!(panel.method, SparkReceiveMethod::Bolt11);
    }

    /// USDB on Spark celebrates the token in its own units and ignores
    /// bitcoin; the bitcoin rails ignore token payments.
    #[test]
    fn payment_received_reads_a_token_payment_as_its_token() {
        let mainnet = Cache {
            network: Network::Bitcoin,
            ..Cache::default()
        };
        let mut panel = SparkReceive::new(None);
        update_with_cache(
            &mut panel,
            &mainnet,
            SparkReceiveMessage::SelectSenderRail(SparkReceiveMethod::UsdbSpark),
        );
        update(
            &mut panel,
            SparkReceiveMessage::GenerateSucceeded(receive_ok("sp1qaddress")),
        );
        update(
            &mut panel,
            SparkReceiveMessage::PaymentReceived {
                amount_sat: 4_000,
                bolt11: None,
                token: None,
            },
        );
        assert!(matches!(panel.phase(), SparkReceivePhase::Generated(_)));

        // Another token, even one calling itself USDB, is not this payment.
        update(
            &mut panel,
            SparkReceiveMessage::PaymentReceived {
                amount_sat: 9_000_000,
                bolt11: None,
                token: Some(coincube_spark_protocol::PaymentToken {
                    identifier: "btkn1other".to_string(),
                    ticker: "USDB".to_string(),
                    decimals: 6,
                    usdb: false,
                }),
            },
        );
        assert!(matches!(panel.phase(), SparkReceivePhase::Generated(_)));

        update(
            &mut panel,
            SparkReceiveMessage::PaymentReceived {
                amount_sat: 12_500_000,
                bolt11: None,
                token: usdb_token(),
            },
        );
        assert!(matches!(panel.phase(), SparkReceivePhase::Received { .. }));
        assert_eq!(panel.received_amount_display, "12.50 USDB");

        let mut panel = SparkReceive::new(None);
        update(
            &mut panel,
            SparkReceiveMessage::SelectSenderRail(SparkReceiveMethod::Spark),
        );
        update(
            &mut panel,
            SparkReceiveMessage::GenerateSucceeded(receive_ok("sp1qaddress")),
        );
        update(
            &mut panel,
            SparkReceiveMessage::PaymentReceived {
                amount_sat: 12_500_000,
                bolt11: None,
                token: usdb_token(),
            },
        );
        assert!(matches!(panel.phase(), SparkReceivePhase::Generated(_)));
    }

    #[test]
    fn payment_received_ignores_idle_outgoing_and_unrelated_events() {
        let mut panel = SparkReceive::new(None);

        update(
            &mut panel,
            SparkReceiveMessage::PaymentReceived {
                amount_sat: 100,
                bolt11: None,
                token: None,
            },
        );
        assert!(matches!(panel.phase(), SparkReceivePhase::Idle));

        update(
            &mut panel,
            SparkReceiveMessage::GenerateSucceeded(receive_ok("lnbc1invoice")),
        );
        update(
            &mut panel,
            SparkReceiveMessage::PaymentReceived {
                amount_sat: -100,
                bolt11: Some("lnbc1invoice".to_string()),
                token: None,
            },
        );
        assert!(matches!(panel.phase(), SparkReceivePhase::Generated(_)));

        update(
            &mut panel,
            SparkReceiveMessage::PaymentReceived {
                amount_sat: 100,
                bolt11: Some("lnbc1other".to_string()),
                token: None,
            },
        );
        assert!(matches!(panel.phase(), SparkReceivePhase::Generated(_)));
    }

    #[test]
    fn payment_received_matches_bolt11_case_insensitively() {
        let mut panel = SparkReceive::new(None);

        update(
            &mut panel,
            SparkReceiveMessage::GenerateSucceeded(receive_ok("LNBC1Invoice")),
        );
        update(
            &mut panel,
            SparkReceiveMessage::PaymentReceived {
                amount_sat: 321,
                bolt11: Some("lnbc1invoice".to_string()),
                token: None,
            },
        );

        assert!(matches!(
            panel.phase(),
            SparkReceivePhase::Received {
                amount_sat: 321,
                count: 1
            }
        ));
        assert_eq!(panel.received_amount_display, "321 sats");
        assert_eq!(panel.received_celebration_context, "lightning-receive");
        assert!(panel.qr_data.is_none());
        assert!(panel.displayed_invoice.is_none());
    }

    #[test]
    fn onchain_receive_events_accumulate_while_celebrating() {
        let mut panel = SparkReceive::new(None);
        panel.method = SparkReceiveMethod::OnchainBitcoin;

        update(
            &mut panel,
            SparkReceiveMessage::GenerateSucceeded(receive_ok("bc1qaddress")),
        );
        update(
            &mut panel,
            SparkReceiveMessage::PaymentReceived {
                amount_sat: 100,
                bolt11: None,
                token: None,
            },
        );
        update(
            &mut panel,
            SparkReceiveMessage::PaymentReceived {
                amount_sat: 50,
                bolt11: None,
                token: None,
            },
        );

        assert!(matches!(
            panel.phase(),
            SparkReceivePhase::Received {
                amount_sat: 150,
                count: 2
            }
        ));
        assert_eq!(panel.received_amount_display, "150 sats");
        assert_eq!(panel.received_celebration_context, "spark-receive");
    }

    #[test]
    fn reset_returns_to_idle_and_clears_generated_artifacts() {
        let mut panel = SparkReceive::new(None);
        update(
            &mut panel,
            SparkReceiveMessage::GenerateSucceeded(receive_ok("lnbc1invoice")),
        );

        update(&mut panel, SparkReceiveMessage::Reset);

        assert!(matches!(panel.phase(), SparkReceivePhase::Idle));
        assert!(panel.qr_data.is_none());
        assert!(panel.displayed_invoice.is_none());
        assert!(
            !panel.static_address_due(),
            "an invoice waits for its button"
        );
    }

    #[test]
    fn try_again_refetches_static_addresses() {
        for method in [
            SparkReceiveMethod::Spark,
            SparkReceiveMethod::OnchainBitcoin,
        ] {
            let mut panel = SparkReceive::new(None);
            panel.method = method;
            update(
                &mut panel,
                SparkReceiveMessage::GenerateFailed("offline".to_string()),
            );
            assert!(!panel.static_address_due());

            update(&mut panel, SparkReceiveMessage::Reset);

            assert!(matches!(panel.phase(), SparkReceivePhase::Idle));
            assert!(panel.static_address_due(), "{:?} fetches again", method);
        }
    }

    #[test]
    fn sender_picker_opens_closes_and_rail_selection_resets_receive_state() {
        let mut panel = SparkReceive::new(None);
        update(
            &mut panel,
            SparkReceiveMessage::GenerateSucceeded(receive_ok("lnbc1invoice")),
        );

        update(&mut panel, SparkReceiveMessage::OpenSenderPicker);
        assert!(panel.sender_picker_open);

        update(&mut panel, SparkReceiveMessage::CloseSenderPicker);
        assert!(!panel.sender_picker_open);

        // One menu at a time, whichever tile opens it.
        update(&mut panel, SparkReceiveMessage::OpenSenderPicker);
        update(&mut panel, SparkReceiveMessage::OpenReceiverPicker);
        assert!(panel.receiver_picker_open && !panel.sender_picker_open);
        update(&mut panel, SparkReceiveMessage::OpenSenderPicker);
        assert!(panel.sender_picker_open && !panel.receiver_picker_open);
        update(&mut panel, SparkReceiveMessage::CloseSenderPicker);

        update(&mut panel, SparkReceiveMessage::OpenSenderPicker);
        update(
            &mut panel,
            SparkReceiveMessage::SelectSenderRail(SparkReceiveMethod::Spark),
        );

        assert!(!panel.sender_picker_open);
        assert_eq!(panel.method, SparkReceiveMethod::Spark);
        assert!(panel.sideshift_flow.is_none());
        assert!(matches!(panel.phase(), SparkReceivePhase::Idle));
        assert!(panel.qr_data.is_none());
        assert!(panel.displayed_invoice.is_none());
    }

    #[test]
    fn selecting_current_bitcoin_kind_keeps_generated_request_and_qr() {
        for (method, request) in [
            (SparkReceiveMethod::Bolt11, "lnbc1invoice"),
            (SparkReceiveMethod::Spark, "sp1qaddress"),
            (SparkReceiveMethod::OnchainBitcoin, "bc1qaddress"),
        ] {
            let mut panel = SparkReceive::new(None);
            panel.method = method;
            update(
                &mut panel,
                SparkReceiveMessage::GenerateSucceeded(receive_ok(request)),
            );
            assert!(panel.qr_data.is_some());

            update(&mut panel, SparkReceiveMessage::SelectSenderRail(method));

            assert_eq!(panel.method, method);
            assert!(
                matches!(panel.phase(), SparkReceivePhase::Generated(ok) if ok.payment_request == request)
            );
            assert!(panel.qr_data.is_some());
            if method == SparkReceiveMethod::Bolt11 {
                assert_eq!(panel.displayed_invoice.as_deref(), Some(request));
            }
        }
    }

    #[test]
    fn pending_deposits_reload_prunes_stale_confirmations_and_clears_error() {
        let mut panel = SparkReceive::new(None);
        panel.claim_error = Some("old claim error".to_string());
        panel
            .pending_deposit_confirmations
            .insert(("kept".to_string(), 0), 2);
        panel
            .pending_deposit_confirmations
            .insert(("stale".to_string(), 1), 6);

        update(
            &mut panel,
            SparkReceiveMessage::PendingDepositsLoaded(vec![deposit("kept", 0, false)]),
        );

        assert_eq!(panel.pending_deposits.len(), 1);
        assert_eq!(panel.pending_deposits[0].txid, "kept");
        assert!(panel.claim_error.is_none());
        assert_eq!(
            panel
                .pending_deposit_confirmations
                .get(&("kept".to_string(), 0)),
            Some(&2)
        );
        assert!(!panel
            .pending_deposit_confirmations
            .contains_key(&("stale".to_string(), 1)));
    }

    #[test]
    fn pending_deposit_failures_clear_list_and_confirmation_cache() {
        let mut panel = SparkReceive::new(None);
        panel.pending_deposits = vec![deposit("tx", 0, false)];
        panel
            .pending_deposit_confirmations
            .insert(("tx".to_string(), 0), 1);

        update(
            &mut panel,
            SparkReceiveMessage::PendingDepositsFailed("timeout".to_string()),
        );

        assert!(panel.pending_deposits.is_empty());
        assert!(panel.pending_deposit_confirmations.is_empty());
    }

    fn panel_with_settled_swap(settle_hash: &str) -> SparkReceive {
        let backend = Arc::new(SparkBackend::new(Arc::new(
            crate::app::breez_spark::SparkClient::disconnected_for_test(),
        )));
        let mut panel = SparkReceive::new(Some(backend.clone()));
        panel.sideshift_flow = Some(SparkSideshiftReceiveFlow::settled_for_test(
            backend,
            settle_hash,
        ));
        panel
    }

    /// The SDK may not list a settle transaction as a pending deposit until
    /// it confirms. The swap still reads its confirmations by txid, so the
    /// count can't sit at zero waiting on the deposit list.
    #[test]
    fn a_settled_swap_polls_its_settle_txid_until_the_wallet_lists_it() {
        let mut panel = panel_with_settled_swap(
            "4e0ca17b9c2d6f1a8b3e5d7c9f0a1b2c3d4e5f60718293a4b5c6d7e866d20f19",
        );
        assert_eq!(
            panel.swap_settle_txid_to_poll(),
            Some("4e0ca17b9c2d6f1a8b3e5d7c9f0a1b2c3d4e5f60718293a4b5c6d7e866d20f19")
        );

        // Listed as a pending deposit: the deposit poll covers it instead.
        panel.pending_deposits = vec![deposit(
            "4E0CA17B9C2D6F1A8B3E5D7C9F0A1B2C3D4E5F60718293A4B5C6D7E866D20F19",
            0,
            false,
        )];
        assert_eq!(panel.swap_settle_txid_to_poll(), None);

        // Not listed, but already mature: nothing left to count.
        panel.pending_deposits.clear();
        panel
            .sideshift_flow
            .as_mut()
            .unwrap()
            .note_settle_confirmations(Some(DEPOSIT_MATURITY_CONFIRMATIONS));
        assert_eq!(panel.swap_settle_txid_to_poll(), None);
    }

    #[test]
    fn settle_txid_confirmations_feed_the_swap_count() {
        let mut panel = panel_with_settled_swap(
            "4e0ca17b9c2d6f1a8b3e5d7c9f0a1b2c3d4e5f60718293a4b5c6d7e866d20f19",
        );
        assert_eq!(panel.swap_settle_confirmations(), None);

        update(
            &mut panel,
            SparkReceiveMessage::SettleConfirmationsUpdated {
                txid: "4E0CA17B9C2D6F1A8B3E5D7C9F0A1B2C3D4E5F60718293A4B5C6D7E866D20F19"
                    .to_string(),
                confirmations: Some(1),
            },
        );
        assert_eq!(panel.swap_settle_confirmations(), Some(1));

        // A failed fetch, or one for another transaction, changes nothing.
        for (txid, confirmations) in [
            (
                "4e0ca17b9c2d6f1a8b3e5d7c9f0a1b2c3d4e5f60718293a4b5c6d7e866d20f19",
                None,
            ),
            ("other", Some(2)),
        ] {
            update(
                &mut panel,
                SparkReceiveMessage::SettleConfirmationsUpdated {
                    txid: txid.to_string(),
                    confirmations,
                },
            );
            assert_eq!(panel.swap_settle_confirmations(), Some(1));
        }

        // An empty deposit list keeps the count rather than resetting it.
        update(
            &mut panel,
            SparkReceiveMessage::PendingDepositsLoaded(vec![]),
        );
        assert_eq!(panel.swap_settle_confirmations(), Some(1));
    }

    /// Bugbot on #1229: once the wallet lists the settle deposit, its count
    /// hasn't been fetched yet. That must not reset a count already read by
    /// txid, nor store zero over it; a count fetched for the deposit then
    /// takes over, even a lower one (a reorg).
    #[test]
    fn a_newly_listed_settle_deposit_keeps_the_count_read_by_txid() {
        let mut panel = panel_with_settled_swap(
            "4e0ca17b9c2d6f1a8b3e5d7c9f0a1b2c3d4e5f60718293a4b5c6d7e866d20f19",
        );
        update(
            &mut panel,
            SparkReceiveMessage::SettleConfirmationsUpdated {
                txid: "4e0ca17b9c2d6f1a8b3e5d7c9f0a1b2c3d4e5f60718293a4b5c6d7e866d20f19"
                    .to_string(),
                confirmations: Some(2),
            },
        );
        assert_eq!(panel.swap_settle_confirmations(), Some(2));

        update(
            &mut panel,
            SparkReceiveMessage::PendingDepositsLoaded(vec![deposit(
                "4E0CA17B9C2D6F1A8B3E5D7C9F0A1B2C3D4E5F60718293A4B5C6D7E866D20F19",
                0,
                false,
            )]),
        );
        assert_eq!(panel.swap_settle_confirmations(), Some(2));
        assert_eq!(
            panel
                .sideshift_flow
                .as_ref()
                .unwrap()
                .last_settle_confirmations(),
            Some(2)
        );

        update(
            &mut panel,
            SparkReceiveMessage::DepositConfirmationsUpdated(HashMap::from([(
                (
                    "4E0CA17B9C2D6F1A8B3E5D7C9F0A1B2C3D4E5F60718293A4B5C6D7E866D20F19".to_string(),
                    0,
                ),
                1,
            )])),
        );
        assert_eq!(panel.swap_settle_confirmations(), Some(1));
    }

    /// Returning to the panel re-reads a waiting settle count at once; with
    /// nothing to poll, reload asks for nothing extra.
    #[test]
    fn reload_refreshes_a_waiting_settle_count() {
        let mut panel = panel_with_settled_swap(
            "4e0ca17b9c2d6f1a8b3e5d7c9f0a1b2c3d4e5f60718293a4b5c6d7e866d20f19",
        );
        assert!(matches!(
            panel.settle_refresh_on_reload(),
            Some(SparkReceiveMessage::RefreshSettleConfirmations)
        ));

        panel.pending_deposits = vec![deposit(
            "4E0CA17B9C2D6F1A8B3E5D7C9F0A1B2C3D4E5F60718293A4B5C6D7E866D20F19",
            0,
            false,
        )];
        assert!(panel.settle_refresh_on_reload().is_none());
        assert!(SparkReceive::new(None).settle_refresh_on_reload().is_none());
    }

    const SETTLE_TXID: &str = "4e0ca17b9c2d6f1a8b3e5d7c9f0a1b2c3d4e5f60718293a4b5c6d7e866d20f19";

    /// #1230's rule survives this change: a set-aside conversion still hears
    /// its own messages (here its arrival) while another rail is on screen.
    #[test]
    fn a_set_aside_shift_still_hears_its_arrival() {
        let mut panel = panel_with_settled_swap(SETTLE_TXID);
        panel.set_aside_sideshift_flow();
        assert!(panel.sideshift_flow.is_none());
        assert!(panel.parked_shift.is_some());

        let _task = State::update(
            &mut panel,
            None,
            &Cache::default(),
            AppMessage::View(ViewMessage::SparkSideshiftReceive(
                crate::app::view::SparkSideshiftReceiveMessage::Arrived { amount_sat: 21_000 },
            )),
        );
        let parked = panel.parked_shift.as_ref().expect("still set aside");
        assert_eq!(parked.arrived_amount_sat(), Some(21_000));
    }

    /// A txid answer counts only while the swap still polls that txid: not
    /// once the wallet lists the deposit (its own count rules), not for a
    /// swap that has arrived, and never for another transaction.
    #[test]
    fn late_txid_answers_are_ignored_once_the_swap_moved_on() {
        let reply = |panel: &mut SparkReceive, confirmations| {
            update(
                panel,
                SparkReceiveMessage::SettleConfirmationsUpdated {
                    txid: SETTLE_TXID.to_string(),
                    confirmations: Some(confirmations),
                },
            )
        };

        let mut listed = panel_with_settled_swap(SETTLE_TXID);
        update(
            &mut listed,
            SparkReceiveMessage::PendingDepositsLoaded(vec![deposit(SETTLE_TXID, 0, true)]),
        );
        assert_eq!(
            listed.swap_settle_confirmations(),
            Some(DEPOSIT_MATURITY_CONFIRMATIONS)
        );
        reply(&mut listed, 2);
        assert_eq!(
            listed
                .sideshift_flow
                .as_ref()
                .unwrap()
                .last_settle_confirmations(),
            Some(DEPOSIT_MATURITY_CONFIRMATIONS)
        );

        let mut arrived = panel_with_settled_swap(SETTLE_TXID);
        let _task = State::update(
            &mut arrived,
            None,
            &Cache::default(),
            AppMessage::View(ViewMessage::SparkSideshiftReceive(
                crate::app::view::SparkSideshiftReceiveMessage::Arrived { amount_sat: 21_000 },
            )),
        );
        reply(&mut arrived, 1);
        assert_eq!(
            arrived
                .sideshift_flow
                .as_ref()
                .unwrap()
                .last_settle_confirmations(),
            None
        );
    }

    /// A settle hash that isn't a txid is never put into an Esplora URL.
    #[test]
    fn a_malformed_settle_hash_is_not_polled() {
        for hash in ["abc123", "../../api/blocks", &"g".repeat(64)] {
            assert_eq!(
                panel_with_settled_swap(hash).swap_settle_txid_to_poll(),
                None
            );
        }
        assert_eq!(
            panel_with_settled_swap(SETTLE_TXID).swap_settle_txid_to_poll(),
            Some(SETTLE_TXID)
        );
    }

    /// A deposit listed before any count is known still reads "0 of 3".
    #[test]
    fn a_listed_settle_deposit_with_no_count_reads_zero() {
        let mut panel = panel_with_settled_swap(
            "4e0ca17b9c2d6f1a8b3e5d7c9f0a1b2c3d4e5f60718293a4b5c6d7e866d20f19",
        );
        panel.pending_deposits = vec![deposit(
            "4E0CA17B9C2D6F1A8B3E5D7C9F0A1B2C3D4E5F60718293A4B5C6D7E866D20F19",
            0,
            false,
        )];
        assert_eq!(panel.swap_settle_confirmations(), Some(0));
    }

    #[test]
    fn confirmation_updates_merge_and_can_move_backward() {
        let mut panel = SparkReceive::new(None);
        panel
            .pending_deposit_confirmations
            .insert(("a".to_string(), 0), 6);
        panel
            .pending_deposit_confirmations
            .insert(("b".to_string(), 1), 1);

        update(
            &mut panel,
            SparkReceiveMessage::DepositConfirmationsUpdated(HashMap::from([
                (("a".to_string(), 0), 3),
                (("c".to_string(), 2), 0),
            ])),
        );

        assert_eq!(
            panel
                .pending_deposit_confirmations
                .get(&("a".to_string(), 0)),
            Some(&3)
        );
        assert_eq!(
            panel
                .pending_deposit_confirmations
                .get(&("b".to_string(), 1)),
            Some(&1)
        );
        assert_eq!(
            panel
                .pending_deposit_confirmations
                .get(&("c".to_string(), 2)),
            Some(&0)
        );
    }

    #[test]
    fn claim_result_messages_update_claim_error_state() {
        let mut panel = SparkReceive::new(None);

        panel.claim_error = Some("old claim error".to_string());
        update(
            &mut panel,
            SparkReceiveMessage::ClaimDepositRequested {
                txid: "tx".to_string(),
                vout: 0,
            },
        );
        assert!(panel.claiming.is_empty());
        assert_eq!(panel.claim_error.as_deref(), Some("old claim error"));

        panel.claiming.insert(("tx".to_string(), 0));
        update(
            &mut panel,
            SparkReceiveMessage::ClaimDepositFailed {
                txid: "tx".to_string(),
                vout: 0,
                error: "claim failed".to_string(),
            },
        );
        assert!(panel.claiming.is_empty());
        assert_eq!(panel.claim_error.as_deref(), Some("claim failed"));

        panel.claiming.insert(("tx".to_string(), 0));
        panel.claim_error = Some("old".to_string());
        update(
            &mut panel,
            SparkReceiveMessage::ClaimDepositSucceeded {
                txid: "tx".to_string(),
                vout: 0,
                ok: coincube_spark_protocol::ClaimDepositOk {
                    payment_id: Some("payment".to_string()),
                    amount_sat: Some(5_000),
                    deferred: None,
                },
            },
        );
        assert!(panel.claiming.is_empty());
        assert!(panel.claim_error.is_none());
        assert!(panel.claim_notices.is_empty());
    }

    /// 0.28.0: a deferred claim leaves a neutral note, not an error, and
    /// keeps it across a reload until the deposit leaves the list.
    #[test]
    fn a_deferred_claim_leaves_a_note_until_the_deposit_is_claimed() {
        let mut panel = SparkReceive::new(None);
        panel.claiming.insert(("tx".to_string(), 0));
        update(
            &mut panel,
            SparkReceiveMessage::ClaimDepositSucceeded {
                txid: "tx".to_string(),
                vout: 0,
                ok: coincube_spark_protocol::ClaimDepositOk {
                    payment_id: None,
                    amount_sat: None,
                    deferred: Some(coincube_spark_protocol::ClaimDeferral::NoEarlyClaimAvailable),
                },
            },
        );
        assert!(panel.claiming.is_empty());
        assert!(panel.claim_error.is_none());
        let deposit_key = ("tx".to_string(), 0);
        let note = panel
            .claim_notices
            .get(&deposit_key)
            .expect("deferral note")
            .clone();
        assert!(note.contains("claimed automatically"), "{}", note);

        update(
            &mut panel,
            SparkReceiveMessage::PendingDepositsLoaded(vec![
                deposit("tx", 0, true),
                deposit("other", 1, true),
            ]),
        );
        assert_eq!(panel.claim_notices.get(&deposit_key), Some(&note));

        // The deferred deposit is claimed while another is still pending:
        // the note was about the first one only, so it goes.
        update(
            &mut panel,
            SparkReceiveMessage::PendingDepositsLoaded(vec![deposit("other", 1, true)]),
        );
        assert!(panel.claim_notices.is_empty());
    }

    #[test]
    fn deferred_claims_keep_notes_for_both_outputs() {
        for (first, second) in [(0, 1), (1, 0)] {
            let mut panel = SparkReceive::new(None);
            for vout in [first, second] {
                update(
                    &mut panel,
                    SparkReceiveMessage::ClaimDepositSucceeded {
                        txid: "shared-tx".to_string(),
                        vout,
                        ok: coincube_spark_protocol::ClaimDepositOk {
                            payment_id: None,
                            amount_sat: None,
                            deferred: Some(
                                coincube_spark_protocol::ClaimDeferral::NoEarlyClaimAvailable,
                            ),
                        },
                    },
                );
            }
            for vout in [first, second] {
                assert!(
                    panel
                        .claim_notices
                        .contains_key(&("shared-tx".to_string(), vout)),
                    "each output must keep its note in either response order"
                );
            }
            assert_eq!(panel.claim_notices.len(), 2);

            // A list refresh retains both notes; removing one output only
            // removes that output's note, even with a shared transaction id.
            update(
                &mut panel,
                SparkReceiveMessage::PendingDepositsLoaded(vec![
                    deposit("shared-tx", first, true),
                    deposit("shared-tx", second, true),
                ]),
            );
            assert_eq!(panel.claim_notices.len(), 2);
            update(
                &mut panel,
                SparkReceiveMessage::PendingDepositsLoaded(vec![deposit(
                    "shared-tx",
                    second,
                    true,
                )]),
            );
            assert_eq!(panel.claim_notices.len(), 1);
            assert!(panel
                .claim_notices
                .contains_key(&("shared-tx".to_string(), second)));
            update(
                &mut panel,
                SparkReceiveMessage::PendingDepositsLoaded(vec![]),
            );
            assert!(panel.claim_notices.is_empty());
        }
    }

    #[test]
    fn claim_lifecycle_clears_only_its_own_notice() {
        let a = ("shared-tx".to_string(), 0);
        let b = ("shared-tx".to_string(), 1);
        for message in [
            SparkReceiveMessage::ClaimDepositRequested {
                txid: a.0.clone(),
                vout: a.1,
            },
            SparkReceiveMessage::ClaimDepositSucceeded {
                txid: a.0.clone(),
                vout: a.1,
                ok: coincube_spark_protocol::ClaimDepositOk {
                    payment_id: Some("payment".to_string()),
                    amount_sat: None,
                    deferred: None,
                },
            },
            SparkReceiveMessage::ClaimDepositFailed {
                txid: a.0.clone(),
                vout: a.1,
                error: "claim failed".to_string(),
            },
        ] {
            let client = crate::app::breez_spark::SparkClient::disconnected_for_test();
            let mut panel = SparkReceive::new(Some(Arc::new(SparkBackend::new(Arc::new(client)))));
            panel.claim_notices.insert(a.clone(), "A waits".to_string());
            panel.claim_notices.insert(b.clone(), "B waits".to_string());
            update(&mut panel, message);
            assert_eq!(
                panel.claim_notices,
                HashMap::from([(b.clone(), "B waits".to_string())])
            );
        }
    }

    /// Two claims in flight: each result is keyed by the deposit it was
    /// for, whichever claim started last and whichever answers first.
    #[test]
    fn overlapping_claims_key_results_by_their_own_deposit() {
        let a = ("a".to_string(), 0);
        let b = ("b".to_string(), 1);
        let deferred = |txid: &str, vout| SparkReceiveMessage::ClaimDepositSucceeded {
            txid: txid.to_string(),
            vout,
            ok: coincube_spark_protocol::ClaimDepositOk {
                payment_id: None,
                amount_sat: None,
                deferred: Some(coincube_spark_protocol::ClaimDeferral::NoEarlyClaimAvailable),
            },
        };
        let claimed = |txid: &str, vout| SparkReceiveMessage::ClaimDepositSucceeded {
            txid: txid.to_string(),
            vout,
            ok: coincube_spark_protocol::ClaimDepositOk {
                payment_id: Some("payment".to_string()),
                amount_sat: None,
                deferred: None,
            },
        };

        // A then B started; A's deferral answers first. The note is A's,
        // and B's row stays "Claiming…".
        let mut panel = SparkReceive::new(None);
        panel.claiming.insert(a.clone());
        panel.claiming.insert(b.clone());
        update(&mut panel, deferred("a", 0));
        assert_eq!(panel.claim_notices.keys().collect::<Vec<_>>(), vec![&a]);
        assert_eq!(panel.claiming, HashSet::from([b.clone()]));
        // B then succeeds: A's note stays on A's row.
        update(&mut panel, claimed("b", 1));
        assert_eq!(panel.claim_notices.keys().collect::<Vec<_>>(), vec![&a]);
        assert!(panel.claiming.is_empty());

        // B answers first, then A's deferral: the note is still A's.
        let mut panel = SparkReceive::new(None);
        panel.claiming.insert(a.clone());
        panel.claiming.insert(b.clone());
        update(&mut panel, claimed("b", 1));
        assert_eq!(panel.claiming, HashSet::from([a.clone()]));
        update(&mut panel, deferred("a", 0));
        assert_eq!(panel.claim_notices.keys().collect::<Vec<_>>(), vec![&a]);
        assert!(panel.claiming.is_empty());

        // B failing does not drop the note about A.
        update(
            &mut panel,
            SparkReceiveMessage::ClaimDepositFailed {
                txid: "b".to_string(),
                vout: 1,
                error: "claim failed".to_string(),
            },
        );
        assert_eq!(panel.claim_notices.keys().collect::<Vec<_>>(), vec![&a]);
        assert_eq!(panel.claim_error.as_deref(), Some("claim failed"));
    }

    #[test]
    fn balance_loaded_updates_only_when_value_is_present() {
        let mut panel = SparkReceive::new(None);

        update(
            &mut panel,
            SparkReceiveMessage::BalanceLoaded(Some((123, None))),
        );
        assert_eq!(panel.balance_sats, 123);

        update(&mut panel, SparkReceiveMessage::BalanceLoaded(None));
        assert_eq!(panel.balance_sats, 123);
    }

    /// The side panel lists bitcoin and USDB apart, so the panel keeps both
    /// as loaded rather than only their unified sum.
    #[test]
    fn balance_loaded_keeps_bitcoin_and_usdb_apart_for_the_side_panel() {
        let mut panel = SparkReceive::new(None);
        let usdb = UsdbBalanceSnapshot {
            token_identifier: "btkn1synthetic".into(),
            balance: 5_000_000,
            decimals: 6,
            ticker: "USDB".into(),
        };
        update(
            &mut panel,
            SparkReceiveMessage::BalanceLoaded(Some((123, Some(usdb.clone())))),
        );
        assert_eq!(panel.btc_balance_sats, 123);
        assert_eq!(panel.usdb_balance.map(|sb| sb.balance), Some(usdb.balance));
    }

    /// The Receive head carries only Back to Spark: what the payer sends —
    /// Bitcoin rail or stablecoin — is picked from the THEY SEND tile.
    #[test]
    fn the_receive_head_has_no_kind_toggle() {
        let panel = SparkReceive::new(None);
        let cache = Cache {
            network: Network::Bitcoin,
            ..Cache::default()
        };
        let menu = Menu::Spark(crate::app::menu::SparkSubMenu::Receive);
        let probe = crate::app::view::shared::landing::capture::inspect(
            "spark-receive-head",
            panel.view(&menu, &cache),
            (1440, 900),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        assert!(probe.contains("Back to Spark"));
        assert!(!probe.contains("Stablecoin (cross-chain)"));
        for id in [
            "spark-receive-kind-bitcoin",
            "spark-receive-kind-stablecoin",
        ] {
            assert!(!probe.has_focus(iced::advanced::widget::Id::new(id)));
        }
    }

    #[test]
    fn balance_loaded_folds_stable_balance_using_cache_price() {
        let mut panel = SparkReceive::new(None);
        let cache = Cache {
            btc_usd_price: Some(50_000.0),
            ..Cache::default()
        };

        update_with_cache(
            &mut panel,
            &cache,
            SparkReceiveMessage::BalanceLoaded(Some((123, Some(usdb_balance(2_500_000, 6))))),
        );

        assert_eq!(panel.balance_sats, 5_123);
    }

    #[test]
    fn payments_loaded_keeps_only_five_recent_rows() {
        let mut panel = SparkReceive::new(None);
        let payments: Vec<_> = (0..7)
            .map(|i| payment(&format!("payment-{i}"), 1_000 + i))
            .collect();

        update(&mut panel, SparkReceiveMessage::PaymentsLoaded(payments));

        assert_eq!(panel.recent_transactions.len(), 5);
        assert_eq!(panel.recent_transactions[0].id, "payment-0");
        assert_eq!(panel.recent_transactions[4].id, "payment-4");
        assert_eq!(panel.recent_transactions[4].amount.to_sat(), 1_004);
    }

    #[test]
    fn payments_failed_clears_recent_transactions() {
        let mut panel = SparkReceive::new(None);
        panel.recent_transactions.push(SparkRecentTransaction {
            id: "id".to_string(),
            description: "test payment".to_string(),
            time_ago: "just now".to_string(),
            timestamp: 1,
            amount: coincube_core::miniscript::bitcoin::Amount::from_sat(1),
            fees_sat: coincube_core::miniscript::bitcoin::Amount::from_sat(0),
            fiat_amount: None,
            is_incoming: true,
            status: crate::app::wallets::DomainPaymentStatus::Complete,
            method: crate::app::view::spark::SparkPaymentMethod::Spark,
            token_display: None,
        });

        update(
            &mut panel,
            SparkReceiveMessage::PaymentsFailed("bridge down".to_string()),
        );

        assert!(panel.recent_transactions.is_empty());
    }

    #[test]
    fn cross_network_selection_without_backend_or_support_is_noop() {
        let mut panel = SparkReceive::new(None);
        panel.sender_picker_open = true;
        let unsupported = Cache {
            network: Network::Regtest,
            ..Cache::default()
        };

        update_with_cache(
            &mut panel,
            &unsupported,
            SparkReceiveMessage::SelectSenderCrossNetwork("usdt:tron".to_string()),
        );

        assert!(!panel.sender_picker_open);
        assert!(panel.sideshift_flow.is_none());
        assert_eq!(panel.method, SparkReceiveMethod::Bolt11);

        update_with_cache(
            &mut panel,
            &Cache::default(),
            SparkReceiveMessage::SelectSenderCrossNetwork("usdt:tron".to_string()),
        );

        assert!(panel.sideshift_flow.is_none());
        assert_eq!(panel.method, SparkReceiveMethod::Bolt11);
    }

    fn backed_panel() -> SparkReceive {
        let client = crate::app::breez_spark::SparkClient::disconnected_for_test();
        SparkReceive::new(Some(Arc::new(SparkBackend::new(Arc::new(client)))))
    }

    fn mainnet() -> Cache {
        Cache {
            network: Network::Bitcoin,
            ..Cache::default()
        }
    }

    fn restore(panel: &mut SparkReceive, record: crate::app::settings::PendingSparkShift) {
        let _task = State::update(
            panel,
            None,
            &mainnet(),
            AppMessage::View(ViewMessage::SparkSideshiftReceive(
                crate::app::view::SparkSideshiftReceiveMessage::Restore(Box::new(record)),
            )),
        );
    }

    fn shown_shift_id(panel: &SparkReceive) -> Option<&str> {
        panel
            .sideshift_flow
            .as_ref()
            .and_then(|flow| flow.shift())
            .map(|shift| shift.id.as_str())
    }

    /// After unlock, the stored conversion is back on Spark's Receive, and a
    /// second read of the same record doesn't replace it.
    #[test]
    fn a_stored_conversion_is_restored_once() {
        let record = crate::app::settings::PendingSparkShift::sample();
        let mut panel = backed_panel();
        restore(&mut panel, record.clone());
        assert_eq!(shown_shift_id(&panel), Some(record.shift.id.as_str()));
        assert!(panel.parked_shift.is_none());

        let mut other = record.clone();
        other.shift.id = "another".to_string();
        restore(&mut panel, other);
        assert_eq!(shown_shift_id(&panel), Some(record.shift.id.as_str()));
        assert!(panel.parked_shift.is_none());
    }

    /// Picking another rail mid-conversion sets it aside, still reachable;
    /// the notice brings it back, Shift ID and all.
    #[test]
    fn another_rail_sets_a_live_conversion_aside_and_the_notice_brings_it_back() {
        let record = crate::app::settings::PendingSparkShift::sample();
        let mut panel = backed_panel();
        restore(&mut panel, record.clone());

        update_with_cache(
            &mut panel,
            &mainnet(),
            SparkReceiveMessage::SelectSenderRail(SparkReceiveMethod::Spark),
        );
        assert!(panel.sideshift_flow.is_none());
        assert_eq!(panel.method, SparkReceiveMethod::Spark);
        assert!(panel.parked_shift.is_some());
        let (option, arrived) = panel.conversion_summary().unwrap();
        assert_eq!(option.key(), record.deposit_option);
        assert!(!arrived);

        update_with_cache(&mut panel, &mainnet(), SparkReceiveMessage::ShowConversion);
        assert_eq!(shown_shift_id(&panel), Some(record.shift.id.as_str()));
        assert!(panel.parked_shift.is_none());
    }

    /// One conversion at a time: picking a network while one is live shows
    /// that one instead of starting another over it.
    #[test]
    fn picking_a_network_while_a_conversion_is_live_shows_that_conversion() {
        let record = crate::app::settings::PendingSparkShift::sample();
        let other_key = crate::services::sideshift::deposit_options()
            .iter()
            .map(|o| o.key())
            .find(|k| *k != record.deposit_option)
            .unwrap();
        let mut panel = backed_panel();
        restore(&mut panel, record.clone());

        // On screen: the pick is ignored.
        update_with_cache(
            &mut panel,
            &mainnet(),
            SparkReceiveMessage::SelectSenderCrossNetwork(other_key.clone()),
        );
        assert_eq!(shown_shift_id(&panel), Some(record.shift.id.as_str()));

        // Set aside: the pick brings it back.
        update_with_cache(
            &mut panel,
            &mainnet(),
            SparkReceiveMessage::SelectSenderRail(SparkReceiveMethod::Spark),
        );
        update_with_cache(
            &mut panel,
            &mainnet(),
            SparkReceiveMessage::SelectSenderCrossNetwork(other_key),
        );
        assert_eq!(shown_shift_id(&panel), Some(record.shift.id.as_str()));
        assert!(panel.parked_shift.is_none());
    }

    /// A conversion with no shift yet has nothing to lose: another rail
    /// simply closes it.
    #[test]
    fn another_rail_closes_a_conversion_that_has_no_shift_yet() {
        let mut panel = backed_panel();
        let key = crate::services::sideshift::deposit_options()[0].key();
        update_with_cache(
            &mut panel,
            &mainnet(),
            SparkReceiveMessage::SelectSenderCrossNetwork(key),
        );
        assert!(panel.sideshift_flow.is_some());

        update_with_cache(
            &mut panel,
            &mainnet(),
            SparkReceiveMessage::SelectSenderRail(SparkReceiveMethod::Spark),
        );
        assert!(panel.sideshift_flow.is_none());
        assert!(panel.parked_shift.is_none());
        assert_eq!(panel.conversion_summary(), None);
    }

    /// The settings read behind a restore can finish after the user opened a
    /// conversion of their own; one with no shift yet gives way to the
    /// restored shift, which must hear its own polls.
    #[test]
    fn a_restore_replaces_a_conversion_that_has_no_shift_yet() {
        let record = crate::app::settings::PendingSparkShift::sample();
        let mut panel = backed_panel();
        let key = crate::services::sideshift::deposit_options()[0].key();
        update_with_cache(
            &mut panel,
            &mainnet(),
            SparkReceiveMessage::SelectSenderCrossNetwork(key),
        );
        assert!(shown_shift_id(&panel).is_none());

        restore(&mut panel, record.clone());
        assert_eq!(shown_shift_id(&panel), Some(record.shift.id.as_str()));
        assert!(panel.parked_shift.is_none());
    }
}
