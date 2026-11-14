//! "Receive from another network" — deposit any supported asset, receive
//! **Bitcoin** in the Spark wallet.
//!
//! Deliberately a close copy of [`crate::app::state::liquid::sideshift_receive`]:
//! same phases, same affiliate → shift → poll sequence, same 10s poll cadence.
//! Three things differ, and each of them is the reason this isn't just a
//! parameter on the Liquid flow:
//!
//! 1. **The settle leg is on-chain Bitcoin.** SideShift has no Lightning rail,
//!    so shifts settle to the Spark wallet's *static* on-chain deposit address
//!    (`receive_onchain(new_address = false)`). That address is reusable, which
//!    is what makes it safe for a variable-amount shift. It also means funds
//!    arrive after ~1 confirmation, not instantly — the status copy says so.
//! 2. **A refund address is collected up front**, on the *origin* chain. A
//!    shift that fails or gets held has to have somewhere to send the money
//!    back to, and asking afterwards is useless.
//! 3. **The user receives BTC, not the asset they deposited.** The copy never
//!    says "receive USDT" — you deposit USDT and bitcoin arrives, at the shift
//!    rate.

use std::sync::Arc;
use std::time::Duration;

use coincube_ui::widget::Element;
use iced::{clipboard, widget::qr_code, Subscription, Task};

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::message::Message;
use crate::app::settings::PendingSparkShift;
use crate::app::view;
use crate::app::wallets::SparkBackend;
use crate::services::coincube::CoincubeClient;
use crate::services::sideshift::{
    deposit_options, validate_refund_address, DepositOption, ShiftResponse, ShiftStatusKind,
    SideshiftClient, SideshiftNetwork,
};

use view::SparkSideshiftReceiveMessage as Msg;

/// Same cadence as the pulsing Connect light.
const PULSE_INTERVAL: Duration = Duration::from_millis(800);

/// How often a restored, settled shift looks again for where its bitcoin is,
/// while that is still unknown (right after unlock the Spark wallet may not
/// have synced yet).
const ARRIVAL_RECHECK_INTERVAL: Duration = Duration::from_secs(30);

/// Payments read per page, and pages at most, when looking for a restored
/// shift's claimed deposit (newest first).
const PAYMENTS_PAGE: u32 = 100;
const PAYMENTS_PAGES: u32 = 5;

/// Where a restored, settled shift's bitcoin is, as the wallet shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoredSettle {
    /// Already claimed into the wallet, for this many sats.
    Claimed(u64),
    /// Still among the wallet's unclaimed deposits: register it with Home,
    /// which will see it and auto-claim it.
    StillPending,
    /// In neither place yet, or a lookup failed: look again later rather
    /// than guess.
    Unknown,
}

/// Where the flow is. Mirrors the Liquid flow's `ReceivePhase`, minus the
/// fixed-rate quote step: a BTC settle is always variable-rate here, because
/// the user is depositing an arbitrary amount of some other asset and we can't
/// meaningfully pre-commit a rate to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SparkShiftPhase {
    /// Picking what to deposit, and giving a refund address for that chain.
    Setup,
    /// Fetching the affiliate id from our backend.
    FetchingAffiliate,
    /// Resolving the Spark on-chain deposit address and creating the shift.
    CreatingShift,
    /// Shift is live — deposit address on screen, polling status.
    Active,
    /// The swap's bitcoin has landed in the Spark wallet and been claimed. A
    /// terminal *success* state, distinct from `Active`+`Settled` ("Bitcoin
    /// arriving"): SideShift's status can't report this — the arrival is an
    /// on-chain claim on our side — so it's driven by the global "received"
    /// event via [`Msg::Arrived`].
    Arrived,
    /// Terminal failure before a shift existed.
    Failed,
}

pub struct SparkSideshiftReceiveFlow {
    spark: Arc<SparkBackend>,
    coincube_client: CoincubeClient,
    sideshift_client: SideshiftClient,

    phase: SparkShiftPhase,
    selected: DepositOption,
    /// The asset was pre-chosen upstream (the Spark Receive "THEY SEND"
    /// picker), so the setup step shows a confirmation summary instead of its
    /// own asset picker. Cleared on reset.
    asset_preselected: bool,
    refund_address: String,
    /// Validation error for the refund address, shown inline under the field.
    refund_error: Option<String>,

    affiliate_id: Option<String>,
    shift: Option<ShiftResponse>,
    shift_status: Option<ShiftStatusKind>,
    /// The BTC amount SideShift last reported as the settle output, in sats.
    /// Sourced from the poll status (which carries the real figure once a
    /// deposit is detected), not the create-time estimate. Shown next to the
    /// "Bitcoin arriving" status so the user sees how much is on the way.
    settle_amount_sat: Option<u64>,
    /// The deposit SideShift has seen, in the deposit coin, as the decimal
    /// string it reports. Shown on the "Deposit received" tile and used to say
    /// why a refund happened when the amount was outside the shift's limits.
    deposit_amount: Option<String>,
    /// SideShift's settle txid once it has sent the bitcoin; matched against
    /// the wallet's pending deposits to show confirmation progress.
    settle_hash: Option<String>,
    /// The settle transaction's confirmations as last seen among the wallet's
    /// pending deposits. Kept once the deposit is claimed and leaves that list,
    /// so the bar doesn't fall back to "0 of 3" before `Arrived` lands.
    last_settle_confirmations: Option<u32>,
    /// The in-progress step's pulse phase (dim half), flipped by a slow tick
    /// while a shift is moving. The view holds it steady under reduce-motion.
    pulse_dim: bool,
    /// The actual amount claimed into the wallet on arrival (post network fee),
    /// in sats. Set when the flow reaches [`SparkShiftPhase::Arrived`].
    arrived_amount_sat: Option<u64>,
    qr_data: Option<qr_code::Data>,
    /// Put back from [`PendingSparkShift`] after a lock or restart, and the
    /// first status poll hasn't answered yet. The shift may have settled, and
    /// its bitcoin even arrived, while the app was closed; that first answer
    /// is checked against the wallet before anything is registered.
    restored: bool,
    /// A restored shift reported `Settled`, and where its bitcoin is (claimed,
    /// or still an unclaimed deposit) isn't known yet. Until it is, the arrival
    /// is not registered with Home: registering an already-claimed deposit
    /// would leave Home, and this page, waiting forever.
    arrival_unconfirmed: bool,

    loading: bool,
    error: Option<String>,
}

impl SparkSideshiftReceiveFlow {
    pub fn new(spark: Arc<SparkBackend>) -> Self {
        Self {
            spark,
            coincube_client: CoincubeClient::new(),
            sideshift_client: SideshiftClient::new(),
            phase: SparkShiftPhase::Setup,
            // Safe to index: `deposit_options` is a non-empty const list.
            selected: deposit_options()[0],
            asset_preselected: false,
            refund_address: String::new(),
            refund_error: None,
            affiliate_id: None,
            shift: None,
            shift_status: None,
            settle_amount_sat: None,
            deposit_amount: None,
            settle_hash: None,
            last_settle_confirmations: None,
            pulse_dim: false,
            arrived_amount_sat: None,
            qr_data: None,
            restored: false,
            arrival_unconfirmed: false,
            loading: false,
            error: None,
        }
    }

    /// Put a live shift back on screen from the Cube's settings. `None` when
    /// the stored asset is no longer offered: the flow can't render a deposit
    /// it doesn't know, and the record is then dropped.
    ///
    /// The status is unknown until [`Self::poll_now`] answers, so it starts at
    /// `Waiting`, the earliest a live shift can be.
    pub fn restored(spark: Arc<SparkBackend>, record: &PendingSparkShift) -> Option<Self> {
        let option = crate::services::sideshift::deposit_option_by_key(&record.deposit_option)?;
        let mut flow = Self::new_preselected(spark, option);
        flow.refund_address = record.refund_address.clone();
        flow.qr_data = qr_code::Data::new(&record.shift.deposit_address).ok();
        flow.shift = Some(record.shift.clone());
        // A shift whose deposit address has expired by the clock starts as
        // expired, so its address and QR are not offered while SideShift is
        // unreachable. The first successful poll still has the last word (a
        // deposit sent in time moves it on).
        flow.shift_status = Some(
            if deposit_window_closed(record.shift.expires_at.as_deref(), chrono::Utc::now()) {
                ShiftStatusKind::Expired
            } else {
                ShiftStatusKind::Waiting
            },
        );
        flow.phase = SparkShiftPhase::Active;
        flow.restored = true;
        Some(flow)
    }

    /// What to store so this shift survives a lock or restart. `None` before
    /// a shift exists.
    pub fn record(&self) -> Option<PendingSparkShift> {
        self.shift.as_ref().map(|shift| PendingSparkShift {
            shift: shift.clone(),
            deposit_option: self.selected.key(),
            refund_address: self.refund_address.trim().to_string(),
        })
    }

    /// Ask SideShift for the status now rather than at the next 10s tick, so a
    /// restored shift shows where it really is straight away.
    pub fn poll_now(&self) -> Task<Message> {
        self.poll_shift_status()
    }

    /// Launch the flow with the deposit asset already chosen (from the Spark
    /// Receive "THEY SEND" picker). The setup step then skips its own asset
    /// picker and goes straight to the refund address.
    pub fn new_preselected(spark: Arc<SparkBackend>, option: DepositOption) -> Self {
        let mut flow = Self::new(spark);
        flow.selected = option;
        flow.asset_preselected = true;
        flow
    }

    /// A live shift whose bitcoin SideShift has sent, in settle tx `settle_hash`.
    #[cfg(test)]
    pub(crate) fn settled_for_test(spark: Arc<SparkBackend>, settle_hash: &str) -> Self {
        let mut flow = Self::new(spark);
        flow.phase = SparkShiftPhase::Active;
        flow.shift_status = Some(ShiftStatusKind::Settled);
        flow.settle_hash = Some(settle_hash.to_string());
        flow
    }

    pub fn phase(&self) -> &SparkShiftPhase {
        &self.phase
    }
    pub fn selected(&self) -> DepositOption {
        self.selected
    }
    pub fn asset_preselected(&self) -> bool {
        self.asset_preselected
    }
    pub fn refund_address(&self) -> &str {
        &self.refund_address
    }
    pub fn refund_error(&self) -> Option<&str> {
        self.refund_error.as_deref()
    }
    /// Whether "Generate deposit address" is live: the refund address must
    /// validate against the origin chain first. The view disables the button
    /// otherwise; `Msg::Generate` re-checks, so this is UX, not the gate.
    pub fn can_generate(&self) -> bool {
        !self.loading
            && validate_refund_address(self.selected.network, &self.refund_address).is_ok()
    }
    pub fn shift(&self) -> Option<&ShiftResponse> {
        self.shift.as_ref()
    }
    pub fn shift_status(&self) -> Option<&ShiftStatusKind> {
        self.shift_status.as_ref()
    }
    pub fn settle_amount_sat(&self) -> Option<u64> {
        self.settle_amount_sat
    }
    pub fn deposit_amount(&self) -> Option<&str> {
        self.deposit_amount.as_deref()
    }
    pub fn settle_hash(&self) -> Option<&str> {
        self.settle_hash.as_deref()
    }
    pub fn pulse_dim(&self) -> bool {
        self.pulse_dim
    }
    /// SideShift has sent (or is sending) the bitcoin, so a pending deposit
    /// can be this swap's settle output. Before that, any amount match would
    /// be some other deposit.
    pub fn settle_output_live(&self) -> bool {
        self.phase == SparkShiftPhase::Active
            && matches!(
                self.shift_status,
                Some(ShiftStatusKind::Settling | ShiftStatusKind::Settled)
            )
    }
    pub fn last_settle_confirmations(&self) -> Option<u32> {
        self.last_settle_confirmations
    }
    /// Record the settle transaction's confirmations when they were found;
    /// `None` (not among pending deposits) keeps the last value.
    pub fn note_settle_confirmations(&mut self, confirmations: Option<u32>) {
        if confirmations.is_some() {
            self.last_settle_confirmations = confirmations;
        }
    }
    pub fn arrived_amount_sat(&self) -> Option<u64> {
        self.arrived_amount_sat
    }
    pub fn qr_data(&self) -> Option<&qr_code::Data> {
        self.qr_data.as_ref()
    }
    pub fn is_loading(&self) -> bool {
        self.loading
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Reset to the refund step. Clears the stored record when there was a
    /// shift: every button that resets one (Start over, Get a new address,
    /// Done) is on a finished shift's page.
    fn reset(&mut self) -> Task<Message> {
        // A restored shift SideShift hasn't answered for yet may only look
        // expired by the clock, with money in flight: keep its record (the
        // next restore asks SideShift again) rather than lose its Shift ID.
        let had_shift = self.shift.is_some() && !self.restored;
        // Keep `selected` / `asset_preselected`: inline, the asset is chosen via
        // the Spark Receive THEY SEND card, and "Start over" returns to the
        // refund step for the *same* asset, not a default.
        self.phase = SparkShiftPhase::Setup;
        self.refund_address.clear();
        self.refund_error = None;
        self.affiliate_id = None;
        self.shift = None;
        self.shift_status = None;
        self.settle_amount_sat = None;
        self.deposit_amount = None;
        self.settle_hash = None;
        self.last_settle_confirmations = None;
        self.pulse_dim = false;
        self.arrived_amount_sat = None;
        self.qr_data = None;
        self.restored = false;
        self.arrival_unconfirmed = false;
        self.loading = false;
        self.error = None;
        if had_shift {
            store_record(None)
        } else {
            Task::none()
        }
    }

    // ── Async steps ─────────────────────────────────────────────────────

    /// Fetch the SideShift affiliate id from **our own** API
    /// (`GET /api/v1/config/sideshift`) — not from SideShift. The name is
    /// misleading; the call isn't. Errors here mean the Coincube backend is
    /// unreachable or misconfigured, and must be reported that way, or support
    /// goes looking at a third party that was never contacted.
    fn fetch_affiliate_id(&self) -> Task<Message> {
        let client = self.coincube_client.clone();
        Task::perform(
            async move { client.get_sideshift_affiliate_id().await },
            |result| {
                Message::View(view::Message::SparkSideshiftReceive(Msg::AffiliateFetched(
                    result,
                )))
            },
        )
    }

    /// Resolve the Spark wallet's on-chain BTC deposit address, then create the
    /// shift that settles into it.
    ///
    /// `new_address = false` asks for the wallet's **static** deposit address.
    /// That matters: the shift settles at some unknown future point, for an
    /// amount we don't know in advance, and a rotating address could leave the
    /// settle pointing somewhere the wallet no longer watches.
    fn create_shift(&self, affiliate_id: &str) -> Task<Message> {
        let sideshift = self.sideshift_client.clone();
        let spark = self.spark.clone();
        let affiliate_id = affiliate_id.to_string();
        let option = self.selected;
        let refund_address = self.refund_address.trim().to_string();

        Task::perform(
            async move {
                let received = spark
                    .receive_onchain(Some(false))
                    .await
                    .map_err(|e| format!("Couldn't get a Bitcoin deposit address: {e}"))?;

                // The SDK may hand back a BIP21 URI rather than a bare address;
                // SideShift wants the address on its own.
                let settle_address = bare_bitcoin_address(&received.payment_request);

                sideshift
                    .create_btc_receive_shift(
                        option.coin,
                        option.network.network_slug(),
                        &settle_address,
                        &refund_address,
                        &affiliate_id,
                    )
                    .await
            },
            |result| {
                Message::View(view::Message::SparkSideshiftReceive(Msg::ShiftCreated(
                    result,
                )))
            },
        )
    }

    /// A restored shift reports `Settled`: its bitcoin may already have
    /// arrived while the app was closed. Find the settle transaction among the
    /// wallet's unclaimed deposits first, then among its claimed deposit
    /// payments (a few pages, newest first). Found in neither, or a lookup
    /// failed, is [`RestoredSettle::Unknown`] and is looked for again later.
    fn check_restored_arrival(&self, settle_hash: String) -> Task<Message> {
        let spark = self.spark.clone();
        Task::perform(
            async move {
                let Ok(unclaimed) = spark.list_unclaimed_deposits().await else {
                    return RestoredSettle::Unknown;
                };
                if unclaimed
                    .deposits
                    .iter()
                    .any(|d| d.txid.eq_ignore_ascii_case(&settle_hash))
                {
                    return RestoredSettle::StillPending;
                }
                for page in 0..PAYMENTS_PAGES {
                    let Ok(found) = spark
                        .list_payments(Some(PAYMENTS_PAGE), Some(page * PAYMENTS_PAGE))
                        .await
                    else {
                        return RestoredSettle::Unknown;
                    };
                    if let Some(amount_sat) = claimed_settle_amount(&found.payments, &settle_hash) {
                        return RestoredSettle::Claimed(amount_sat);
                    }
                    if found.payments.len() < PAYMENTS_PAGE as usize {
                        break;
                    }
                }
                RestoredSettle::Unknown
            },
            |found| {
                Message::View(view::Message::SparkSideshiftReceive(
                    Msg::RestoredSettleChecked(found),
                ))
            },
        )
    }

    /// Look again for a restored, settled shift's bitcoin: the deposits and
    /// payments when the settle hash is known, else SideShift's status, which
    /// carries the hash once it has sent the bitcoin.
    fn recheck_restored_arrival(&self) -> Task<Message> {
        if !self.arrival_unconfirmed || self.phase != SparkShiftPhase::Active {
            return Task::none();
        }
        match self.settle_hash.clone() {
            Some(hash) => self.check_restored_arrival(hash),
            None => self.poll_shift_status(),
        }
    }

    /// What a settle does: refresh the panel's deposits, and register the
    /// arrival with Home so the deposit is auto-claimed and celebrated.
    /// `deposit_seen`: the deposit was just seen among the unclaimed ones (a
    /// restored shift), so Home may read it vanishing as claimed even if the
    /// SDK claims it before Home looks.
    fn settled_tasks(&self, deposit_seen: bool) -> Task<Message> {
        // The amount is the swap's settle estimate, used only for the pending
        // indicator; the celebration uses the actual claimed amount. Read the
        // cached figure rather than this poll's `settle_sat`: a settle response
        // can omit `settleAmount`, and the cache still holds the value an
        // earlier poll reported.
        Task::batch([
            Task::done(Message::View(view::Message::SparkReceive(
                view::SparkReceiveMessage::DepositsChanged,
            ))),
            Task::done(Message::View(view::Message::Home(
                view::HomeMessage::SwapSettledAwaitingArrival {
                    amount_sat: self.settle_amount_sat.unwrap_or(0),
                    deposit_seen,
                },
            ))),
        ])
    }

    fn poll_shift_status(&self) -> Task<Message> {
        let Some(shift) = &self.shift else {
            return Task::none();
        };
        let client = self.sideshift_client.clone();
        let shift_id = shift.id.clone();
        Task::perform(
            async move { client.get_shift_status(&shift_id).await },
            |result| {
                Message::View(view::Message::SparkSideshiftReceive(Msg::StatusUpdated(
                    result,
                )))
            },
        )
    }

    // ── View ────────────────────────────────────────────────────────────

    pub fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        // Standalone, this flow has no view of the wallet's pending deposits,
        // so the confirmations line reads from zero until the inline Spark
        // Receive page (which has them) is the one showing it.
        let body = view::spark::spark_sideshift_receive_view(
            self,
            cache.bitcoin_unit,
            cache.balance_amounts_masked(),
            None,
            cache.reduce_motion,
        );
        view::dashboard(menu, cache, body.map(view::Message::SparkSideshiftReceive))
    }

    // ── Update ──────────────────────────────────────────────────────────

    pub fn update(&mut self, msg: &Msg) -> Task<Message> {
        match msg {
            Msg::SelectDeposit(key) => {
                if let Some(option) = crate::services::sideshift::deposit_option_by_key(key) {
                    // Changing chains invalidates the refund address — it was
                    // for the *old* origin chain, and silently carrying it over
                    // is exactly how a refund goes to the wrong network.
                    if deposit_selection_clears_refund(self.selected, option) {
                        self.refund_address.clear();
                        self.refund_error = None;
                    }
                    self.selected = option;
                    self.error = None;
                }
                Task::none()
            }

            Msg::RefundAddressEdited(value) => {
                self.refund_address = value.clone();
                // Validate as they type, but don't nag on an empty field.
                self.refund_error = refund_address_edit_error(self.selected.network, value);
                Task::none()
            }

            Msg::Generate => {
                // Hard gate: no shift is ever created without a refund address
                // we've validated against the origin chain.
                if let Err(e) = validate_refund_address(self.selected.network, &self.refund_address)
                {
                    self.refund_error = Some(e);
                    return Task::none();
                }
                self.refund_error = None;
                self.loading = true;
                self.error = None;
                self.phase = SparkShiftPhase::FetchingAffiliate;
                self.fetch_affiliate_id()
            }

            Msg::AffiliateFetched(result) => {
                if self.phase != SparkShiftPhase::FetchingAffiliate {
                    return Task::none();
                }
                match result {
                    Ok(id) => {
                        self.affiliate_id = Some(id.clone());
                        self.phase = SparkShiftPhase::CreatingShift;
                        self.create_shift(id)
                    }
                    Err(e) => {
                        self.loading = false;
                        self.phase = SparkShiftPhase::Failed;
                        // This step talks to the *Coincube* API, not SideShift
                        // (see `fetch_affiliate_id`). Naming SideShift here
                        // would send whoever debugs it to the wrong service —
                        // and reassure the user that a third party is at fault
                        // when it's ours. No deposit address exists yet, so
                        // nothing is at risk.
                        self.error = Some(nothing_sent_yet(&coincube_ui::localized_format!(
                            "Couldn't load swap settings from Coincube: {e}",
                            "No se pudo cargar la configuración de intercambio desde COINCUBE: {e}"
                        )));
                        Task::none()
                    }
                }
            }

            Msg::ShiftCreated(result) => {
                if self.phase != SparkShiftPhase::CreatingShift {
                    return Task::none();
                }
                self.loading = false;
                match result {
                    Ok(shift) => {
                        self.qr_data = qr_code::Data::new(&shift.deposit_address).ok();
                        self.shift = Some(shift.clone());
                        self.shift_status = Some(ShiftStatusKind::Waiting);
                        self.phase = SparkShiftPhase::Active;
                        // From here the user may leave, lock or quit; keep the
                        // shift so its page can come back.
                        return store_record(self.record());
                    }
                    Err(e) => {
                        self.phase = SparkShiftPhase::Failed;
                        // `create_shift` spans two services — the Spark bridge
                        // (deposit address) and SideShift (the shift itself) —
                        // and each already names itself in its error. Wrapping
                        // them in a single "SideShift failed" prefix would
                        // misattribute half of them, so pass the attributed
                        // error through as-is.
                        self.error = Some(nothing_sent_yet(e));
                    }
                }
                Task::none()
            }

            Msg::PollStatus => self.poll_shift_status(),

            Msg::StatusUpdated(result) => {
                // An answer for another shift (one polled before "Get a new
                // address" or "Start over" and answered after) must not touch
                // this one: an old `expired` would mark the new shift expired
                // and clear its stored record.
                let current = self.shift.as_ref().map(|shift| shift.id.as_str());
                if let Some(status) = result
                    .as_ref()
                    .ok()
                    .filter(|s| Some(s.id.as_str()) == current)
                {
                    let kind = ShiftStatusKind::from(status.status.as_str());
                    let settled = kind == ShiftStatusKind::Settled
                        && self.shift_status.as_ref() != Some(&ShiftStatusKind::Settled);
                    let first_after_restore = std::mem::take(&mut self.restored);
                    // A restored shift may already show as expired by the
                    // clock; SideShift confirming it is what clears the record.
                    let expired = kind == ShiftStatusKind::Expired
                        && (first_after_restore
                            || self.shift_status.as_ref() != Some(&ShiftStatusKind::Expired));
                    self.shift_status = Some(kind);
                    // SideShift reports the settle output (BTC, as a decimal
                    // string) once a deposit is detected. Keep the latest so the
                    // view can show how much bitcoin is arriving. Parse it exactly
                    // (BTC → sats) rather than via f64, which rounds.
                    let settle_sat = parse_btc_settle_amount_sats(status.settle_amount.as_deref());
                    if settle_sat.is_some() {
                        self.settle_amount_sat = settle_sat;
                    }
                    if let Some(amount) = status
                        .deposit_amount
                        .as_deref()
                        .map(str::trim)
                        .filter(|a| !a.is_empty())
                    {
                        self.deposit_amount = Some(amount.to_string());
                    }
                    if let Some(hash) = status
                        .settle_hash
                        .as_deref()
                        .map(str::trim)
                        .filter(|h| !h.is_empty())
                    {
                        self.settle_hash = Some(hash.to_string());
                    }
                    if expired {
                        // Nothing was deposited and nothing ever will be: this
                        // page stays up for now, but a restart needn't bring
                        // it back.
                        return store_record(None);
                    }
                    if settled {
                        // The bitcoin has landed at Spark's on-chain deposit
                        // address, which means it shows up as an *unclaimed
                        // deposit* — not as a payment in history. Two things
                        // happen on settle:
                        //   1. Kick the panel's deposit list so the pending
                        //      deposit is loaded when the user returns to it.
                        //   2. Register the arrival with the always-resident Home
                        //      state, which auto-claims the deposit once it matures
                        //      — wherever the user has since navigated — and fires
                        //      the "bitcoin received" celebration when it lands.
                        //
                        // A shift put back after a restart may have settled long
                        // ago, its deposit already claimed. Registering that would
                        // leave Home waiting for a deposit that never comes, so
                        // find where the bitcoin is first, and keep looking until
                        // the wallet shows it (it may not have synced yet).
                        if first_after_restore {
                            self.arrival_unconfirmed = true;
                        } else if !self.arrival_unconfirmed {
                            return self.settled_tasks(false);
                        }
                    }
                    if self.arrival_unconfirmed {
                        // Waiting to learn where the bitcoin is: look as soon
                        // as the settle hash is known. Without it, the recheck
                        // timer polls again (never straight from here, which
                        // would loop poll → answer → poll).
                        if let Some(hash) = self.settle_hash.clone() {
                            return self.check_restored_arrival(hash);
                        }
                    }
                }
                Task::none()
            }

            Msg::RecheckRestoredArrival => self.recheck_restored_arrival(),

            Msg::RestoredSettleChecked(found) => {
                if self.phase != SparkShiftPhase::Active || !self.arrival_unconfirmed {
                    return Task::none();
                }
                match found {
                    RestoredSettle::Claimed(amount_sat) => {
                        self.arrival_unconfirmed = false;
                        self.arrived_amount_sat = Some(*amount_sat);
                        self.phase = SparkShiftPhase::Arrived;
                        store_record(None)
                    }
                    RestoredSettle::StillPending => {
                        self.arrival_unconfirmed = false;
                        self.settled_tasks(true)
                    }
                    // The recheck subscription tries again.
                    RestoredSettle::Unknown => Task::none(),
                }
            }

            Msg::Arrived { amount_sat } => {
                if arrival_belongs_to_this_shift(&self.phase, self.shift_status.as_ref()) {
                    self.arrival_unconfirmed = false;
                    self.arrived_amount_sat = Some(*amount_sat);
                    self.phase = SparkShiftPhase::Arrived;
                    return store_record(None);
                }
                Task::none()
            }

            Msg::Copy => {
                let Some(shift) = &self.shift else {
                    return Task::none();
                };
                Task::batch([
                    clipboard::write(shift.deposit_address.clone()),
                    Task::done(Message::View(view::Message::ShowToast(
                        log::Level::Info,
                        "Copied deposit address to clipboard".to_string(),
                    ))),
                ])
            }

            Msg::CopyShiftId => {
                let Some(shift) = &self.shift else {
                    return Task::none();
                };
                Task::batch([
                    clipboard::write(shift.id.clone()),
                    Task::done(Message::View(view::Message::ShowToast(
                        log::Level::Info,
                        "Copied shift ID to clipboard".to_string(),
                    ))),
                ])
            }

            Msg::CopyRefundAddress => {
                let address = self.refund_address.trim();
                if address.is_empty() {
                    return Task::none();
                }
                Task::batch([
                    clipboard::write(address.to_string()),
                    Task::done(Message::View(view::Message::ShowToast(
                        log::Level::Info,
                        "Copied refund address to clipboard".to_string(),
                    ))),
                ])
            }

            Msg::PulseTick => {
                self.pulse_dim = !self.pulse_dim;
                Task::none()
            }

            // The Spark Receive panel builds the flow from the record; a flow
            // never restores into itself.
            Msg::Restore(_) => Task::none(),

            Msg::Back | Msg::Reset => self.reset(),
        }
    }

    // ── Subscription ────────────────────────────────────────────────────

    pub fn subscription(&self) -> Subscription<Message> {
        let is_terminal = self
            .shift_status
            .as_ref()
            .is_some_and(ShiftStatusKind::is_terminal);
        // `is_terminal` includes `Review` — a held shift will not advance on
        // its own, so polling it forever just burns requests and leaves the UI
        // implying progress. The status card shows a call to action instead.
        let poll = if polls_status(&self.phase, is_terminal, self.restored) {
            iced::time::every(Duration::from_secs(10))
                .map(|_| Message::View(view::Message::SparkSideshiftReceive(Msg::PollStatus)))
        } else {
            Subscription::none()
        };
        // The current tracker step pulses while the swap is moving — including
        // `Settled` ("Arriving in Spark"), which stops the status poll but is
        // still waiting on confirmations. Held, failed and finished shifts
        // don't pulse: nothing is happening that the user should wait on.
        let moving = self.phase == SparkShiftPhase::Active
            && !matches!(
                self.shift_status,
                Some(
                    ShiftStatusKind::Review
                        | ShiftStatusKind::Error
                        | ShiftStatusKind::Expired
                        | ShiftStatusKind::Refunded
                )
            );
        let pulse = if moving {
            iced::time::every(PULSE_INTERVAL)
                .map(|_| Message::View(view::Message::SparkSideshiftReceive(Msg::PulseTick)))
        } else {
            Subscription::none()
        };
        // A restored, settled shift whose bitcoin hasn't been found yet keeps
        // looking: the status poll above has stopped (`Settled` is terminal).
        let recheck = if self.phase == SparkShiftPhase::Active && self.arrival_unconfirmed {
            iced::time::every(ARRIVAL_RECHECK_INTERVAL).map(|_| {
                Message::View(view::Message::SparkSideshiftReceive(
                    Msg::RecheckRestoredArrival,
                ))
            })
        } else {
            Subscription::none()
        };
        Subscription::batch([poll, pulse, recheck])
    }
}

/// Store (`Some`) or clear (`None`) the Cube's [`PendingSparkShift`]. Home
/// owns the Cube's settings file, so the write goes through it.
pub fn store_record(record: Option<PendingSparkShift>) -> Task<Message> {
    Task::done(Message::View(view::Message::Home(
        view::HomeMessage::StoreSparkShift(record.map(Box::new)),
    )))
}

/// Whether the status poll runs: while the shift is live and not terminal, and
/// for a restored shift until SideShift has answered once — its status until
/// then is a guess (an expiry by the clock looks terminal), and one failed poll
/// at unlock must not leave it there.
fn polls_status(phase: &SparkShiftPhase, is_terminal: bool, restored: bool) -> bool {
    *phase == SparkShiftPhase::Active && (!is_terminal || restored)
}

/// Whether a shift's deposit window has closed by the clock: `expires_at` (as
/// SideShift writes it, RFC 3339) is at or before `now`. Unknown or unreadable
/// is not closed: SideShift's status decides then.
fn deposit_window_closed(expires_at: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> bool {
    expires_at
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at.trim()).ok())
        .is_some_and(|at| at <= now)
}

/// The amount a claimed deposit paid by `settle_hash` brought in, if the
/// wallet has one: a restored, settled shift whose bitcoin already arrived.
fn claimed_settle_amount(
    payments: &[coincube_spark_protocol::PaymentSummary],
    settle_hash: &str,
) -> Option<u64> {
    payments
        .iter()
        .find(|p| {
            p.method == "deposit"
                && p.details
                    .tx_id
                    .as_deref()
                    .is_some_and(|tx| tx.eq_ignore_ascii_case(settle_hash))
        })
        .map(|p| p.amount_sat.unsigned_abs())
}

/// Frame a setup-stage failure with the one fact that actually settles the
/// user's nerves: **no deposit address was ever produced, so nothing of theirs
/// is in flight.** Every failure this wraps happens strictly before a deposit
/// address exists, which is what makes the reassurance true rather than
/// hopeful — do not reuse it once a shift is live.
///
/// The detail is passed through verbatim: the underlying errors already name
/// the service that failed (Coincube, the Spark bridge, or SideShift), and
/// re-prefixing them here is how they end up misattributed.
/// Whether an `Arrived` signal — fired globally for *any* Spark claim — should
/// be taken as this shift's bitcoin landing and move it to the terminal
/// `Arrived` state.
///
/// True only when the swap is still live (`Active`) and has seen a deposit of
/// its own (status past `Waiting`). The deposit-seen guard stops an unrelated
/// claim, landing while this flow is still waiting to be funded, from hijacking
/// the screen. It stays permissive across every post-deposit status — including
/// `Review` (a hold only happens after the deposit is received) — so the arrival
/// is still caught even if the shift status lags behind the actual chain.
fn arrival_belongs_to_this_shift(
    phase: &SparkShiftPhase,
    status: Option<&ShiftStatusKind>,
) -> bool {
    *phase == SparkShiftPhase::Active
        && matches!(
            status,
            Some(
                ShiftStatusKind::Pending
                    | ShiftStatusKind::Processing
                    | ShiftStatusKind::Settling
                    | ShiftStatusKind::Settled
                    | ShiftStatusKind::Review
            )
        )
}

fn deposit_selection_clears_refund(current: DepositOption, next: DepositOption) -> bool {
    current.network != next.network
}

fn refund_address_edit_error(network: SideshiftNetwork, value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        validate_refund_address(network, value).err()
    }
}

fn nothing_sent_yet(detail: &str) -> String {
    coincube_ui::localized_format!(
        "{}. Nothing has been sent — try again.",
        "{}. No se envió nada. Inténtalo de nuevo.",
        detail.trim_end().trim_end_matches('.')
    )
}

/// Strip a BIP21 wrapper (`bitcoin:bc1…?amount=…`) down to the bare address.
/// SideShift's `settleAddress` must be an address, not a URI — handing it a URI
/// would either be rejected or, worse, settle somewhere unintended.
fn bare_bitcoin_address(payment_request: &str) -> String {
    payment_request
        .split_once(':')
        .map_or(payment_request, |(_scheme, rest)| rest)
        .split('?')
        .next()
        .unwrap_or(payment_request)
        .to_string()
}

fn parse_btc_settle_amount_sats(value: Option<&str>) -> Option<u64> {
    value.and_then(|s| {
        coincube_core::miniscript::bitcoin::Amount::from_str_in(
            s.trim(),
            coincube_core::miniscript::bitcoin::Denomination::Bitcoin,
        )
        .ok()
        .map(|a| a.to_sat())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::sideshift::deposit_option_by_key;

    /// The shift lifecycle, as the poller sees it: which states keep the
    /// subscription alive, and which stop it.
    ///
    /// Encoded against `ShiftStatusKind` rather than the flow (which needs a
    /// live backend to construct), because the bug this guards is a *status*
    /// being misclassified — a held shift read as in-flight polls forever.
    #[test]
    fn the_shift_lifecycle_stops_polling_exactly_at_the_terminal_states() {
        let in_flight = ["waiting", "pending", "processing", "settling", "refunding"];
        let terminal = ["settled", "expired", "refunded", "error", "review"];

        for s in in_flight {
            assert!(
                !ShiftStatusKind::from(s).is_terminal(),
                "{} must keep polling — the shift is still moving",
                s
            );
        }
        for s in terminal {
            assert!(
                ShiftStatusKind::from(s).is_terminal(),
                "{} must stop polling — the shift will not advance on its own",
                s
            );
        }
    }

    #[test]
    fn an_arrival_only_lands_on_a_live_shift_that_has_seen_its_deposit() {
        use ShiftStatusKind::*;

        // Accept: the swap is Active and its status is past `Waiting`, so the
        // claim that just fired is this shift's bitcoin landing.
        for status in [Pending, Processing, Settling, Settled, Review] {
            assert!(
                arrival_belongs_to_this_shift(&SparkShiftPhase::Active, Some(&status)),
                "Active + {:?} should accept the arrival",
                status
            );
        }

        // Ignore: still waiting to be funded (or no status yet), so a claim
        // firing now belongs to some other deposit, not this one.
        assert!(!arrival_belongs_to_this_shift(
            &SparkShiftPhase::Active,
            Some(&Waiting)
        ));
        assert!(!arrival_belongs_to_this_shift(
            &SparkShiftPhase::Active,
            None
        ));

        // Ignore: not a live shift — nothing to advance.
        for phase in [
            SparkShiftPhase::Setup,
            SparkShiftPhase::Arrived,
            SparkShiftPhase::Failed,
        ] {
            assert!(
                !arrival_belongs_to_this_shift(&phase, Some(&Settled)),
                "{:?} must ignore the arrival even at Settled",
                phase
            );
        }
    }

    #[test]
    fn a_held_shift_stops_the_poller_and_asks_the_user_to_act() {
        // The failure mode: `review` treated as in-flight. The user watches a
        // spinner forever while SideShift waits on them to make contact.
        let review = ShiftStatusKind::Review;
        assert!(review.is_terminal());
        assert!(review.needs_user_action());
    }

    #[test]
    fn changing_the_deposit_chain_must_not_carry_over_the_refund_address() {
        // A refund address is chain-specific. Silently keeping a Tron address
        // after the user switches to Solana would send the refund into the
        // void — so the flow clears it, and this pins that behaviour.
        //
        // Asserted against the validator the flow calls, since constructing the
        // flow needs a live SparkBackend.
        let tron = deposit_option_by_key("usdt:tron").unwrap();
        let solana = deposit_option_by_key("usdt:solana").unwrap();
        assert_ne!(tron.network, solana.network);

        let tron_address = "TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC";
        assert!(validate_refund_address(tron.network, tron_address).is_ok());
        // Carried over to Solana, the very same address is invalid — which is
        // why the flow drops it rather than leaving it in the box.
        assert!(validate_refund_address(solana.network, tron_address).is_err());
    }

    #[test]
    fn deposit_selection_clears_refund_only_when_the_origin_chain_changes() {
        let eth_usdt = deposit_option_by_key("usdt:ethereum").unwrap();
        let eth_usdc = deposit_option_by_key("usdc:ethereum").unwrap();
        let tron_usdt = deposit_option_by_key("usdt:tron").unwrap();

        assert!(
            !deposit_selection_clears_refund(eth_usdt, eth_usdc),
            "same-chain asset changes can keep the refund address"
        );
        assert!(
            deposit_selection_clears_refund(eth_usdt, tron_usdt),
            "cross-chain changes must clear the refund address"
        );
    }

    #[test]
    fn refund_address_edit_error_is_quiet_for_blank_input_but_validates_nonblank() {
        assert_eq!(refund_address_edit_error(SideshiftNetwork::Tron, ""), None);
        assert_eq!(
            refund_address_edit_error(SideshiftNetwork::Tron, "   "),
            None
        );
        assert_eq!(
            refund_address_edit_error(SideshiftNetwork::Tron, "TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC"),
            None
        );

        let err = refund_address_edit_error(
            SideshiftNetwork::Tron,
            "0x71C7656EC7ab88b098defB751B7401B5f6d8976F",
        )
        .expect("wrong-chain addresses should be rejected");
        assert!(err.contains("Tron"));
        assert!(err.contains("lost"));
    }

    /// The affiliate id comes from *our* API (`/api/v1/config/sideshift`), not
    /// from SideShift. Blaming SideShift for a Coincube outage sends whoever
    /// debugs it to a third party that was never contacted, and tells the user
    /// someone else is at fault when it's us.
    #[test]
    fn a_backend_failure_is_not_blamed_on_sideshift() {
        let msg = nothing_sent_yet("Couldn't load swap settings from Coincube: 503");
        assert!(msg.contains("Coincube"));
        assert!(
            !msg.contains("SideShift"),
            "a Coincube failure must not name SideShift: {}",
            msg
        );
    }

    /// The reassurance is only sound because every failure it wraps happens
    /// before a deposit address exists. It's the difference between "retry
    /// freely" and "you may have money in flight".
    #[test]
    fn setup_failures_say_the_funds_are_untouched() {
        let msg = nothing_sent_yet("SideShift error 400: bad pair");
        assert!(msg.contains("Nothing has been sent"));
        // The underlying error survives verbatim — it's what support reads.
        assert!(msg.contains("SideShift error 400: bad pair"));
    }

    #[test]
    fn error_framing_does_not_produce_ragged_punctuation() {
        // Errors arrive with and without trailing periods; neither should end
        // up as ".." on screen.
        assert_eq!(
            nothing_sent_yet("it broke."),
            "it broke. Nothing has been sent — try again."
        );
        assert_eq!(
            nothing_sent_yet("it broke"),
            "it broke. Nothing has been sent — try again."
        );
    }

    #[test]
    fn a_bip21_uri_is_reduced_to_the_bare_settle_address() {
        // SideShift settles to whatever string we hand it. A URI here would be
        // a bad settle address, so this reduction is load-bearing.
        assert_eq!(
            bare_bitcoin_address("bitcoin:bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq?amount=0.1"),
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"
        );
        assert_eq!(
            bare_bitcoin_address("bitcoin:bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"),
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"
        );
        // Already bare — passed through untouched.
        assert_eq!(
            bare_bitcoin_address("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"),
            "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq"
        );
    }

    #[test]
    fn settle_amounts_are_parsed_exactly_as_bitcoin_not_floats() {
        assert_eq!(parse_btc_settle_amount_sats(Some("0.00000001")), Some(1));
        assert_eq!(
            parse_btc_settle_amount_sats(Some(" 1.23456789 ")),
            Some(123_456_789)
        );
        assert_eq!(parse_btc_settle_amount_sats(None), None);
        assert_eq!(parse_btc_settle_amount_sats(Some("")), None);
        assert_eq!(parse_btc_settle_amount_sats(Some("not btc")), None);
        assert_eq!(parse_btc_settle_amount_sats(Some("0.000000001")), None);
    }

    fn backend() -> Arc<SparkBackend> {
        Arc::new(SparkBackend::new(Arc::new(
            crate::app::breez_spark::SparkClient::disconnected_for_test(),
        )))
    }

    fn deposit_payment(
        method: &str,
        tx_id: Option<&str>,
        amount_sat: i64,
    ) -> coincube_spark_protocol::PaymentSummary {
        coincube_spark_protocol::PaymentSummary {
            id: "p".to_string(),
            amount_sat,
            fees_sat: 0,
            token_amount: None,
            token_decimals: None,
            token_ticker: None,
            timestamp: 1_700_000_000,
            status: "completed".to_string(),
            direction: "receive".to_string(),
            method: method.to_string(),
            description: None,
            details: coincube_spark_protocol::PaymentDetailSummary {
                tx_id: tx_id.map(str::to_string),
                ..Default::default()
            },
        }
    }

    /// A stored shift comes back live, on its own asset and refund address,
    /// and is stored again exactly as it was.
    #[test]
    fn a_restored_shift_is_live_and_records_itself_unchanged() {
        let record = PendingSparkShift::sample();
        let flow = SparkSideshiftReceiveFlow::restored(backend(), &record).unwrap();
        assert_eq!(flow.phase(), &SparkShiftPhase::Active);
        assert_eq!(flow.shift_status(), Some(&ShiftStatusKind::Waiting));
        assert_eq!(flow.refund_address(), record.refund_address);
        assert!(flow.asset_preselected());
        assert!(flow.qr_data().is_some());
        assert_eq!(flow.record(), Some(record));
    }

    #[test]
    fn a_stored_shift_for_an_asset_no_longer_offered_is_not_restored() {
        let mut record = PendingSparkShift::sample();
        record.deposit_option = "doge:dogecoin".to_string();
        assert!(SparkSideshiftReceiveFlow::restored(backend(), &record).is_none());
    }

    #[test]
    fn generate_stays_disabled_until_the_refund_address_fits_the_origin_chain() {
        let liquid_usdt = crate::services::sideshift::deposit_option_by_key("usdt:liquid")
            .expect("USDt on Liquid is offered");
        let mut flow = SparkSideshiftReceiveFlow::new_preselected(backend(), liquid_usdt);
        assert!(!flow.can_generate(), "blank refund address");

        // A Solana address for a Liquid deposit — the mistake that loses funds.
        let _ = flow.update(&Msg::RefundAddressEdited(
            "135GTBUiUTDFYLzvstzJq4CxAXDLwZiBmZr9k9f8KBQy".to_string(),
        ));
        assert!(flow.refund_error().is_some());
        assert!(!flow.can_generate(), "wrong-chain refund address");

        // And pressing Generate anyway (stale button) still creates nothing.
        let _ = flow.update(&Msg::Generate);
        assert_eq!(flow.phase(), &SparkShiftPhase::Setup);
    }

    #[test]
    fn no_record_before_a_shift_exists() {
        assert_eq!(SparkSideshiftReceiveFlow::new(backend()).record(), None);
    }

    /// A shift that settled while the app was closed is found by its settle
    /// txid among claimed deposits; nothing else counts as its arrival.
    #[test]
    fn a_settled_shift_s_arrival_is_found_by_its_settle_txid() {
        let payments = vec![
            deposit_payment("spark", Some("abc"), 99),
            deposit_payment("deposit", Some("other"), 50),
            deposit_payment("deposit", Some("ABC"), 27_961),
            deposit_payment("deposit", None, 10),
        ];
        assert_eq!(claimed_settle_amount(&payments, "abc"), Some(27_961));
        assert_eq!(claimed_settle_amount(&payments, "missing"), None);
        assert_eq!(claimed_settle_amount(&[], "abc"), None);
    }

    /// The bitcoin landing ends the shift for good, so its record goes too.
    #[test]
    fn an_arrival_on_a_restored_shift_finishes_it() {
        let mut flow =
            SparkSideshiftReceiveFlow::restored(backend(), &PendingSparkShift::sample()).unwrap();
        flow.arrival_unconfirmed = true;
        let _ = flow.update(&Msg::RestoredSettleChecked(RestoredSettle::Claimed(27_961)));
        assert_eq!(flow.phase(), &SparkShiftPhase::Arrived);
        assert_eq!(flow.arrived_amount_sat(), Some(27_961));
    }

    fn status(kind: &str, settle_hash: Option<&str>) -> Msg {
        Msg::StatusUpdated(Ok(crate::services::sideshift::ShiftStatus {
            id: "d97d3994cd95e54364b6".to_string(),
            status: kind.to_string(),
            deposit_address: None,
            deposit_amount: Some("20".to_string()),
            settle_amount: Some("0.00027961".to_string()),
            expires_at: None,
            settle_hash: settle_hash.map(str::to_string),
        }))
    }

    fn restored_flow() -> SparkSideshiftReceiveFlow {
        SparkSideshiftReceiveFlow::restored(backend(), &PendingSparkShift::sample()).unwrap()
    }

    /// A restored shift that turns out settled is not registered with Home
    /// until the wallet shows where its bitcoin is, with or without the hash.
    #[test]
    fn a_restored_settled_shift_waits_to_learn_where_its_bitcoin_is() {
        let mut with_hash = restored_flow();
        let _ = with_hash.update(&status("settled", Some("abc")));
        assert!(with_hash.arrival_unconfirmed);
        assert_eq!(with_hash.settle_hash(), Some("abc"));

        let mut without_hash = restored_flow();
        let _ = without_hash.update(&status("settled", None));
        assert!(without_hash.arrival_unconfirmed);
        // The hash arriving on a later poll doesn't end the wait by itself.
        let _ = without_hash.update(&status("settled", Some("abc")));
        assert!(without_hash.arrival_unconfirmed);
    }

    /// Not found yet keeps looking; found pending or claimed ends the wait.
    #[test]
    fn the_restored_arrival_check_keeps_looking_until_the_wallet_shows_it() {
        let mut flow = restored_flow();
        let _ = flow.update(&status("settled", Some("abc")));

        let _ = flow.update(&Msg::RestoredSettleChecked(RestoredSettle::Unknown));
        assert!(flow.arrival_unconfirmed);
        assert_eq!(flow.phase(), &SparkShiftPhase::Active);

        let _ = flow.update(&Msg::RestoredSettleChecked(RestoredSettle::StillPending));
        assert!(!flow.arrival_unconfirmed);
        assert_eq!(flow.phase(), &SparkShiftPhase::Active);

        // A late answer after the wait ended changes nothing.
        let _ = flow.update(&Msg::RestoredSettleChecked(RestoredSettle::Claimed(1)));
        assert_eq!(flow.phase(), &SparkShiftPhase::Active);
    }

    /// A shift restored while still moving settles the ordinary way.
    #[test]
    fn a_restored_shift_that_settles_later_is_not_held_back() {
        let mut flow = restored_flow();
        let _ = flow.update(&status("settling", None));
        let _ = flow.update(&status("settled", Some("abc")));
        assert!(!flow.arrival_unconfirmed);
    }

    #[test]
    fn a_deposit_window_closes_at_its_expiry_and_unknown_never_closes() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-13T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(deposit_window_closed(Some("2026-11-13T11:59:59.000Z"), now));
        assert!(deposit_window_closed(Some("2026-11-13T12:00:00Z"), now));
        assert!(!deposit_window_closed(Some("2026-11-13T12:00:01Z"), now));
        assert!(!deposit_window_closed(Some("not a time"), now));
        assert!(!deposit_window_closed(None, now));
    }

    /// An expired shift doesn't offer its deposit address while SideShift is
    /// unreachable, and SideShift confirming the expiry clears the record.
    #[test]
    fn a_restored_shift_past_its_expiry_starts_expired() {
        let mut record = PendingSparkShift::sample();
        record.shift.expires_at = Some("2020-01-01T00:00:00.000Z".to_string());
        let mut flow = SparkSideshiftReceiveFlow::restored(backend(), &record).unwrap();
        assert_eq!(flow.shift_status(), Some(&ShiftStatusKind::Expired));

        let _ = flow.update(&status("expired", None));
        assert_eq!(flow.shift_status(), Some(&ShiftStatusKind::Expired));
        assert!(!flow.restored);

        // A deposit sent in time: SideShift's answer wins over the clock.
        let mut late = SparkSideshiftReceiveFlow::restored(backend(), &record).unwrap();
        let _ = late.update(&status("processing", None));
        assert_eq!(late.shift_status(), Some(&ShiftStatusKind::Processing));
    }

    /// A restored shift keeps asking SideShift until it answers, even while
    /// it looks expired by the clock; after that, terminal stops the poll.
    #[test]
    fn a_restored_shift_polls_until_sideshift_answers_once() {
        assert!(polls_status(&SparkShiftPhase::Active, true, true));
        assert!(!polls_status(&SparkShiftPhase::Active, true, false));
        assert!(polls_status(&SparkShiftPhase::Active, false, false));
        assert!(!polls_status(&SparkShiftPhase::Arrived, false, true));

        let mut record = PendingSparkShift::sample();
        record.shift.expires_at = Some("2020-01-01T00:00:00.000Z".to_string());
        let mut flow = SparkSideshiftReceiveFlow::restored(backend(), &record).unwrap();
        // A failed poll leaves it waiting for an answer, still polling.
        let _ = flow.update(&Msg::StatusUpdated(Err("offline".to_string())));
        assert!(flow.restored);
        // "Get a new address" on the unconfirmed page keeps the shift's
        // record for the next restore; it is only the on-screen page reset.
        let _ = flow.update(&Msg::Reset);
        assert_eq!(flow.phase(), &SparkShiftPhase::Setup);
    }

    /// A poll answered after the page was reset (or for any other shift)
    /// leaves the current flow alone.
    #[test]
    fn a_status_for_another_shift_is_ignored() {
        let mut record = PendingSparkShift::sample();
        record.shift.expires_at = Some("2020-01-01T00:00:00.000Z".to_string());
        let mut flow = SparkSideshiftReceiveFlow::restored(backend(), &record).unwrap();
        let _ = flow.update(&Msg::Reset);
        assert_eq!(flow.shift(), None);

        // The old shift's poll lands after the reset.
        let _ = flow.update(&status("expired", None));
        assert_eq!(flow.shift_status(), None);
        assert_eq!(flow.phase(), &SparkShiftPhase::Setup);

        // On a live shift, an answer carrying another id changes nothing.
        let mut live = restored_flow();
        let _ = live.update(&Msg::StatusUpdated(Ok(
            crate::services::sideshift::ShiftStatus {
                id: "someone-else".to_string(),
                status: "expired".to_string(),
                deposit_address: None,
                deposit_amount: None,
                settle_amount: None,
                expires_at: None,
                settle_hash: None,
            },
        )));
        assert_eq!(live.shift_status(), Some(&ShiftStatusKind::Waiting));
        assert!(live.restored);
    }
}
