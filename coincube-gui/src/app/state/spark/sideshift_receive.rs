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
use crate::app::view;
use crate::app::wallets::SparkBackend;
use crate::services::coincube::CoincubeClient;
use crate::services::sideshift::{
    deposit_options, validate_refund_address, DepositOption, ShiftResponse, ShiftStatusKind,
    SideshiftClient,
};

use view::SparkSideshiftReceiveMessage as Msg;

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
    /// Terminal failure before a shift existed.
    Failed,
}

pub struct SparkSideshiftReceiveFlow {
    spark: Arc<SparkBackend>,
    coincube_client: CoincubeClient,
    sideshift_client: SideshiftClient,

    phase: SparkShiftPhase,
    selected: DepositOption,
    refund_address: String,
    /// Validation error for the refund address, shown inline under the field.
    refund_error: Option<String>,

    affiliate_id: Option<String>,
    shift: Option<ShiftResponse>,
    shift_status: Option<ShiftStatusKind>,
    qr_data: Option<qr_code::Data>,

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
            refund_address: String::new(),
            refund_error: None,
            affiliate_id: None,
            shift: None,
            shift_status: None,
            qr_data: None,
            loading: false,
            error: None,
        }
    }

    pub fn phase(&self) -> &SparkShiftPhase {
        &self.phase
    }
    pub fn selected(&self) -> DepositOption {
        self.selected
    }
    pub fn refund_address(&self) -> &str {
        &self.refund_address
    }
    pub fn refund_error(&self) -> Option<&str> {
        self.refund_error.as_deref()
    }
    pub fn shift(&self) -> Option<&ShiftResponse> {
        self.shift.as_ref()
    }
    pub fn shift_status(&self) -> Option<&ShiftStatusKind> {
        self.shift_status.as_ref()
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

    fn reset(&mut self) {
        self.phase = SparkShiftPhase::Setup;
        self.selected = deposit_options()[0];
        self.refund_address.clear();
        self.refund_error = None;
        self.affiliate_id = None;
        self.shift = None;
        self.shift_status = None;
        self.qr_data = None;
        self.loading = false;
        self.error = None;
    }

    // ── Async steps ─────────────────────────────────────────────────────

    fn fetch_affiliate_id(&self) -> Task<Message> {
        let client = self.coincube_client.clone();
        Task::perform(
            async move { client.get_sideshift_affiliate_id().await },
            |result| Message::View(view::Message::SparkSideshiftReceive(Msg::AffiliateFetched(result))),
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
            |result| Message::View(view::Message::SparkSideshiftReceive(Msg::ShiftCreated(result))),
        )
    }

    fn poll_shift_status(&self) -> Task<Message> {
        let Some(shift) = &self.shift else {
            return Task::none();
        };
        let client = self.sideshift_client.clone();
        let shift_id = shift.id.clone();
        Task::perform(
            async move { client.get_shift_status(&shift_id).await },
            |result| Message::View(view::Message::SparkSideshiftReceive(Msg::StatusUpdated(result))),
        )
    }

    // ── View ────────────────────────────────────────────────────────────

    pub fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let body = view::spark::spark_sideshift_receive_view(self);
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
                    if option.network != self.selected.network {
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
                self.refund_error = if value.trim().is_empty() {
                    None
                } else {
                    validate_refund_address(self.selected.network, value).err()
                };
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
                        self.error = Some(format!("Couldn't reach SideShift: {e}"));
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
                    }
                    Err(e) => {
                        self.phase = SparkShiftPhase::Failed;
                        self.error = Some(format!("Couldn't start the shift: {e}"));
                    }
                }
                Task::none()
            }

            Msg::PollStatus => self.poll_shift_status(),

            Msg::StatusUpdated(result) => {
                if let Ok(status) = result {
                    let kind = ShiftStatusKind::from(status.status.as_str());
                    let settled = kind == ShiftStatusKind::Settled
                        && self.shift_status.as_ref() != Some(&ShiftStatusKind::Settled);
                    self.shift_status = Some(kind);
                    if settled {
                        // The bitcoin has landed at Spark's on-chain deposit
                        // address, which means it shows up as an *unclaimed
                        // deposit* — not as a payment in history. Kick the
                        // panel's deposit list so the user is offered the claim
                        // instead of being left staring at a "settled" badge
                        // with no money visible in the wallet.
                        return Task::done(Message::View(view::Message::SparkReceive(
                            view::SparkReceiveMessage::DepositsChanged,
                        )));
                    }
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

            Msg::Back | Msg::Reset => {
                self.reset();
                Task::none()
            }
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
        if self.phase == SparkShiftPhase::Active && !is_terminal {
            iced::time::every(Duration::from_secs(10))
                .map(|_| Message::View(view::Message::SparkSideshiftReceive(Msg::PollStatus)))
        } else {
            Subscription::none()
        }
    }
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
                "{s} must keep polling — the shift is still moving"
            );
        }
        for s in terminal {
            assert!(
                ShiftStatusKind::from(s).is_terminal(),
                "{s} must stop polling — the shift will not advance on its own"
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
}
