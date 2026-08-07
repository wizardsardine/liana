use std::convert::TryInto;
use std::sync::Arc;
use std::time::Duration;

use breez_sdk_liquid::InputType;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{component::form, widget::*};
use iced::Task;

use super::sideshift_send::SideshiftSendFlow;

/// Map SDK prepare errors to user-friendly messages.
fn friendly_prepare_error(e: &impl std::fmt::Display) -> String {
    let msg = e.to_string();
    if msg.contains("not enough funds") || msg.contains("InsufficientFunds") {
        "Minimum spendable amount not met. Try adding more funds.".to_string()
    } else {
        format!("Failed to prepare payment: {}", msg)
    }
}
use crate::app::breez_liquid::assets::{
    asset_kind_for_id, format_usdt_display, lbtc_asset_id, parse_asset_to_minor_units,
    usdt_asset_id, AssetKind, USDT_PRECISION,
};
use crate::app::cache::Cache;
use crate::app::menu::{LiquidSubMenu, Menu};
use crate::app::settings::unit::BitcoinDisplayUnit;
use crate::app::state::{redirect, State};
use crate::app::view::SendPopupMessage;
use crate::app::wallets::{DomainPayment, DomainPaymentDetails, LiquidBackend};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::utils::format_time_ago;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAsset {
    Lbtc,
    Usdt,
}

/// Network/rail for the receiving side of a send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveNetwork {
    /// BTC via Lightning Network
    Lightning,
    /// L-BTC or USDt on Liquid
    Liquid,
    /// BTC on-chain
    Bitcoin,
    /// USDt on Ethereum (SideShift)
    Ethereum,
    /// USDt on Tron (SideShift)
    Tron,
    /// USDt on Binance Smart Chain (SideShift)
    Binance,
    /// USDt on Solana (SideShift)
    Solana,
}

impl ReceiveNetwork {
    /// Display name for the network badge.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Lightning => "Lightning",
            Self::Liquid => "Liquid",
            Self::Bitcoin => "Bitcoin",
            Self::Ethereum => "Ethereum",
            Self::Tron => "Tron",
            Self::Binance => "Binance",
            Self::Solana => "Solana",
        }
    }

    /// Whether this network requires SideShift.
    pub fn is_sideshift(&self) -> bool {
        matches!(
            self,
            Self::Ethereum | Self::Tron | Self::Binance | Self::Solana
        )
    }

    /// Convert to SideshiftNetwork for the SideShift API.
    pub fn to_sideshift_network(&self) -> Option<crate::services::sideshift::SideshiftNetwork> {
        match self {
            Self::Ethereum => Some(crate::services::sideshift::SideshiftNetwork::Ethereum),
            Self::Tron => Some(crate::services::sideshift::SideshiftNetwork::Tron),
            Self::Binance => Some(crate::services::sideshift::SideshiftNetwork::Binance),
            Self::Solana => Some(crate::services::sideshift::SideshiftNetwork::Solana),
            _ => None,
        }
    }

    /// Valid "They Receive" networks for a given "You Send" asset.
    pub fn options_for_send_asset(
        send_asset: SendAsset,
        cross_asset_supported: bool,
    ) -> Vec<(SendAsset, ReceiveNetwork)> {
        match send_asset {
            SendAsset::Lbtc => {
                let mut opts = vec![
                    (SendAsset::Lbtc, ReceiveNetwork::Lightning),
                    (SendAsset::Lbtc, ReceiveNetwork::Liquid),
                    (SendAsset::Lbtc, ReceiveNetwork::Bitcoin),
                ];
                if cross_asset_supported {
                    opts.push((SendAsset::Usdt, ReceiveNetwork::Liquid));
                }
                opts
            }
            SendAsset::Usdt => {
                let mut opts = vec![(SendAsset::Usdt, ReceiveNetwork::Liquid)];
                if cross_asset_supported {
                    opts.push((SendAsset::Lbtc, ReceiveNetwork::Lightning));
                    opts.push((SendAsset::Lbtc, ReceiveNetwork::Liquid));
                    opts.push((SendAsset::Lbtc, ReceiveNetwork::Bitcoin));
                }
                opts.extend([
                    (SendAsset::Usdt, ReceiveNetwork::Ethereum),
                    (SendAsset::Usdt, ReceiveNetwork::Tron),
                    (SendAsset::Usdt, ReceiveNetwork::Binance),
                    (SendAsset::Usdt, ReceiveNetwork::Solana),
                ]);
                opts
            }
        }
    }
}

#[derive(Debug)]
pub enum Modal {
    AmountInput,
    FiatInput {
        fiat_input: form::Value<String>,
        currencies: [crate::services::fiat::Currency; 4],
        selected_currency: crate::services::fiat::Currency,
        converters:
            std::collections::HashMap<crate::services::fiat::Currency, view::FiatAmountConverter>,
    },
    None,
}

/// Everything about a payment that is decided the moment the user presses Done,
/// captured before any `prepare_*` call is dispatched.
///
/// The screen's own fields (`amount`, `to_asset`, `comment`, `input_type`, …)
/// stay editable while a prepare is in flight and describe *the next* payment
/// the user is composing, not the one being prepared. Confirmation and
/// execution must both read this snapshot instead, or the FinalCheck screen can
/// end up describing one payment while `ConfirmSend` executes another.
#[derive(Debug, Clone, PartialEq)]
pub struct PrepareContext {
    /// Which prepare round this belongs to. See [`LiquidSend::begin_prepare`].
    pub generation: u64,
    pub amount: Amount,
    /// Formatted USDt amount as the user typed it; empty for L-BTC sends.
    pub usdt_amount_display: String,
    pub to_asset: SendAsset,
    pub from_asset: SendAsset,
    pub comment: Option<String>,
    pub description: Option<String>,
    /// Whether the user asked to pay fees in the asset. The SDK response may
    /// override this when the preferred method isn't offered.
    pub pay_fees_with_asset: bool,
}

/// The operation `ConfirmSend` will execute, and nothing else.
///
/// Replaces the two independent `Option<…Response>` fields this screen used to
/// carry. With two options, "which payment is prepared?" was answered by
/// checking one and falling back to the other — so a stale Lightning prepare
/// that was never cleared won the check against a freshly prepared on-chain
/// payment, and the wrong payment went out (audit: stale-prepare execution).
/// One value can only describe one payment.
#[derive(Debug, Clone)]
pub enum PreparedPayment {
    /// Liquid, Lightning or asset send, executed via `send_payment`.
    Regular {
        response: Box<breez_sdk_liquid::prelude::PrepareSendResponse>,
        /// Resolved once, at prepare time, from the snapshot plus what the SDK
        /// actually offered. Never recomputed from mutable screen state.
        use_asset_fees: bool,
    },
    /// BTC on-chain payout, executed via `pay_onchain`.
    ///
    /// Carries its own destination: reading it back off `input_type` at confirm
    /// time would take the address the user is editing now, not the one that
    /// was prepared.
    Onchain {
        response: Box<breez_sdk_liquid::prelude::PreparePayOnchainResponse>,
        address: String,
    },
}

/// A completed prepare: the immutable payment the FinalCheck screen displays
/// and the one `ConfirmSend` executes. The two cannot disagree because they are
/// the same value.
#[derive(Debug, Clone)]
pub struct PreparedIntent {
    context: PrepareContext,
    payment: PreparedPayment,
}

impl PreparedIntent {
    pub fn new(context: PrepareContext, payment: PreparedPayment) -> Self {
        Self { context, payment }
    }

    pub fn generation(&self) -> u64 {
        self.context.generation
    }

    pub fn context(&self) -> &PrepareContext {
        &self.context
    }

    pub fn payment(&self) -> &PreparedPayment {
        &self.payment
    }

    pub fn amount(&self) -> Amount {
        self.context.amount
    }

    pub fn to_asset(&self) -> SendAsset {
        self.context.to_asset
    }

    pub fn from_asset(&self) -> SendAsset {
        self.context.from_asset
    }

    pub fn usdt_amount_display(&self) -> &str {
        &self.context.usdt_amount_display
    }

    pub fn comment(&self) -> Option<&str> {
        self.context.comment.as_deref()
    }

    pub fn description(&self) -> Option<&str> {
        self.context.description.as_deref()
    }

    /// Fee paid in the asset (USDt), when the payment about to be executed
    /// actually pays in it. `None` means the fee is paid in L-BTC and
    /// [`Self::fees_sat`] carries it.
    ///
    /// Keyed on the `use_asset_fees` frozen into the payment — the same value
    /// `ConfirmSend` hands the SDK — not on the destination asset alone. The
    /// SDK quotes `estimated_asset_fees` alongside `fees_sat` whenever an asset
    /// fee is *available*, including on the two shapes that cannot use one: a
    /// cross-asset send (refused by the SDK) and a same-asset USDt send where
    /// the user declined asset fees. Reading the quote on its own therefore
    /// showed a USDt fee and a zero L-BTC fee for a payment that was about to
    /// deduct L-BTC, and `total_sat` under-reported by that fee.
    pub fn asset_fees(&self) -> Option<f64> {
        if self.context.to_asset != SendAsset::Usdt {
            return None;
        }
        match &self.payment {
            PreparedPayment::Regular {
                response,
                use_asset_fees,
            } => use_asset_fees
                .then_some(response.estimated_asset_fees)
                .flatten(),
            PreparedPayment::Onchain { .. } => None,
        }
    }

    /// L-BTC fee in sats. Zero when the fee is paid in the asset instead.
    pub fn fees_sat(&self) -> u64 {
        if self.asset_fees().is_some() {
            return 0;
        }
        match &self.payment {
            PreparedPayment::Regular { response, .. } => response.fees_sat.unwrap_or(0),
            PreparedPayment::Onchain { response, .. } => response.total_fees_sat,
        }
    }

    /// Amount + L-BTC fee. Saturating: an SDK-supplied fee must not be able to
    /// wrap the displayed total.
    pub fn total_sat(&self) -> u64 {
        self.context.amount.to_sat().saturating_add(self.fees_sat())
    }
}

#[derive(Debug)]
pub enum LiquidSendFlowState {
    Main {
        modal: Modal,
    },
    /// Holding the intent *in* the state is what makes the confirmation screen
    /// and the executed payment the same object. Leaving FinalCheck drops it,
    /// so no later screen can execute what this one displayed.
    FinalCheck(Box<PreparedIntent>),
    Sent,
}

/// LiquidSend manages the send interface for all Liquid wallet assets.
pub struct LiquidSend {
    breez_client: Arc<LiquidBackend>,
    sideshift_flow: Option<SideshiftSendFlow>,
    btc_balance: Amount,
    usdt_balance: u64,
    amount: Amount,
    amount_input: form::Value<String>,
    usdt_amount_input: form::Value<String>,
    /// The asset the recipient will receive.
    to_asset: SendAsset,
    /// The asset the user is paying with. Equals `to_asset` for same-asset sends;
    /// differs for cross-asset swaps (via SideSwap).
    from_asset: SendAsset,
    /// The wallet screen the user entered from. Set once when the send screen is
    /// opened and never mutated by cross-asset toggles. Used for guards and resets
    /// that need to know the user's original intent (replaces the old `usdt_only`
    /// invariant).
    home_asset: SendAsset,
    /// Network the recipient receives on (Lightning, Liquid, Bitcoin, Ethereum, etc.)
    receive_network: ReceiveNetwork,
    /// Whether the "You Send" picker modal is open.
    send_picker_open: bool,
    /// Whether the "They Receive" picker modal is open.
    receive_picker_open: bool,
    recent_transaction: Vec<view::liquid::RecentTransaction>,
    recent_payments: Vec<DomainPayment>,
    selected_payment: Option<DomainPayment>,
    input: form::Value<String>,
    input_type: Option<InputType>,
    lightning_limits: Option<(u64, u64)>, // (min_sats, max_sats)
    onchain_limits: Option<(u64, u64)>,   // (min_sats, max_sats)
    /// The asset requested by the URI (locked once detected from BIP21 asset_id).
    uri_asset: Option<AssetKind>,
    flow_state: LiquidSendFlowState,
    description: Option<String>,
    comment: Option<String>,
    error: Option<String>,
    /// Increments on every prepare, and on every event that abandons one.
    /// A prepare response stamped with an older generation is a response to a
    /// payment the user has already walked away from, and is dropped.
    prepare_generation: u64,
    is_sending: bool,
    /// User preference for paying fees in the asset (USDt) vs L-BTC.
    /// `true` = pay fees in USDt, `false` = pay fees in L-BTC.
    /// Only relevant for same-asset USDt sends.
    pay_fees_with_asset: bool,
    /// Whether a SendMax prepare call is in flight.
    max_loading: bool,
    /// Whether the current amount was set via "Send Max" for L-BTC (use Drain).
    is_drain: bool,
    /// Quote and image handle for the "Transaction complete" screen.
    sent_celebration_context: String,
    sent_amount_display: String,
    sent_quote: coincube_ui::component::quote_display::Quote,
    sent_image_handle: iced::widget::image::Handle,
}

impl LiquidSend {
    pub fn new(breez_client: Arc<LiquidBackend>) -> Self {
        Self {
            breez_client,
            sideshift_flow: None,
            btc_balance: Amount::from_sat(0),
            usdt_balance: 0,
            amount: Amount::from_sat(0),
            amount_input: form::Value::default(),
            usdt_amount_input: form::Value::default(),
            to_asset: SendAsset::Lbtc,
            from_asset: SendAsset::Lbtc,
            home_asset: SendAsset::Lbtc,
            receive_network: ReceiveNetwork::Lightning,
            send_picker_open: false,
            receive_picker_open: false,
            recent_transaction: Vec::new(),
            recent_payments: Vec::new(),
            selected_payment: None,
            input: form::Value::default(),
            uri_asset: None,
            error: None,
            flow_state: LiquidSendFlowState::Main { modal: Modal::None },
            input_type: None,
            lightning_limits: None,
            onchain_limits: None,
            comment: None,
            description: None,
            prepare_generation: 0,
            is_sending: false,
            pay_fees_with_asset: true,
            max_loading: false,
            is_drain: false,
            sent_celebration_context: "liquid-send".to_string(),
            sent_amount_display: String::new(),
            sent_quote: coincube_ui::component::quote_display::random_quote("liquid-send"),
            sent_image_handle: coincube_ui::component::quote_display::image_handle_for_context(
                "liquid-send",
            ),
        }
    }

    pub fn usdt_balance(&self) -> u64 {
        self.usdt_balance
    }

    pub fn btc_balance(&self) -> Amount {
        self.btc_balance
    }

    /// Open a new prepare round and return its generation token.
    ///
    /// Every prepare request carries the token it was dispatched under, and a
    /// response is only accepted while its token is still the current one. That
    /// covers both orderings the SDK can produce: a response that arrives after
    /// the user abandoned the payment, and two responses that come back out of
    /// order (the older one loses, whichever lands last).
    fn begin_prepare(&mut self) -> u64 {
        self.prepare_generation = self.prepare_generation.wrapping_add(1);
        self.prepare_generation
    }

    /// Snapshot the payment the user just confirmed on the amount screen and
    /// open a new prepare round for it.
    ///
    /// Taken once, at dispatch, so everything downstream — the confirmation
    /// screen and the executed payment alike — reads the same frozen values.
    fn open_prepare_context(&mut self) -> PrepareContext {
        PrepareContext {
            generation: self.begin_prepare(),
            amount: self.amount,
            usdt_amount_display: self.usdt_amount_input.value.trim().to_string(),
            to_asset: self.to_asset,
            from_asset: self.from_asset,
            comment: self.comment.clone(),
            description: self.description.clone(),
            pay_fees_with_asset: self.pay_fees_with_asset,
        }
    }

    /// Abandon any prepared or in-flight payment.
    ///
    /// Called wherever the user leaves the payment behind — closing the flow,
    /// going home, completing a send, or editing the destination. The intent
    /// itself lives in [`LiquidSendFlowState::FinalCheck`] and is dropped by the
    /// state transition; this bumps the token so a response still in flight for
    /// it cannot reopen FinalCheck afterwards.
    fn invalidate_prepare(&mut self) {
        self.prepare_generation = self.prepare_generation.wrapping_add(1);
    }

    /// The prepared payment currently displayed, if the flow is on FinalCheck.
    pub fn prepared_intent(&self) -> Option<&PreparedIntent> {
        match &self.flow_state {
            LiquidSendFlowState::FinalCheck(intent) => Some(intent),
            _ => None,
        }
    }

    /// The one intent `ConfirmSend` may execute, or `None` if nothing may be
    /// sent — the flow is not on FinalCheck, or what it is showing belongs to a
    /// prepare round that has since been superseded.
    ///
    /// `ConfirmSend` goes through this and has no other path to a payment, so
    /// "what would be sent" is answerable without dispatching anything.
    pub fn executable_intent(&self) -> Option<&PreparedIntent> {
        match &self.flow_state {
            LiquidSendFlowState::FinalCheck(intent)
                if intent.generation() == self.prepare_generation =>
            {
                Some(intent)
            }
            _ => None,
        }
    }

    pub fn pay_fees_with_asset(&self) -> bool {
        self.pay_fees_with_asset
    }

    pub fn max_loading(&self) -> bool {
        self.max_loading
    }

    pub fn send_asset(&self) -> SendAsset {
        self.from_asset
    }

    pub fn receive_asset(&self) -> SendAsset {
        self.to_asset
    }

    pub fn receive_network(&self) -> ReceiveNetwork {
        self.receive_network
    }

    pub fn send_picker_open(&self) -> bool {
        self.send_picker_open
    }

    pub fn receive_picker_open(&self) -> bool {
        self.receive_picker_open
    }

    pub fn cross_asset_supported(&self) -> bool {
        matches!(
            self.breez_client.network(),
            breez_sdk_liquid::bitcoin::Network::Bitcoin
        )
    }

    pub fn breez_client(&self) -> &Arc<LiquidBackend> {
        &self.breez_client
    }

    pub fn recent_transactions(&self) -> &[view::liquid::RecentTransaction] {
        &self.recent_transaction
    }

    fn load_balance(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();
        let usdt_only = self.home_asset == SendAsset::Usdt;

        Task::perform(
            async move {
                let info = breez_client.info().await;
                let payments = breez_client.list_payments(Some(20), None, None).await;

                let balance = info
                    .as_ref()
                    .map(|info| {
                        let balance =
                            info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat;
                        Amount::from_sat(balance)
                    })
                    .unwrap_or(Amount::ZERO);

                let usdt_id = usdt_asset_id(breez_client.network()).unwrap_or("");

                let usdt_balance = info
                    .as_ref()
                    .ok()
                    .and_then(|info| {
                        info.wallet_info.asset_balances.iter().find_map(|ab| {
                            if ab.asset_id == usdt_id {
                                Some(ab.balance_sat)
                            } else {
                                None
                            }
                        })
                    })
                    .unwrap_or(0);

                let error = match (&info, &payments) {
                    (Err(_), Err(_)) => Some("Couldn't fetch balance or transactions".to_string()),
                    (Err(_), _) => Some("Couldn't fetch account balance".to_string()),
                    (_, Err(_)) => Some("Couldn't fetch recent transactions".to_string()),
                    _ => None,
                };

                let all_payments = payments.unwrap_or_default();
                let payments: Vec<DomainPayment> = all_payments
                    .into_iter()
                    .filter(|p| {
                        let is_usdt = matches!(
                            &p.details,
                            DomainPaymentDetails::LiquidAsset { asset_id, .. }
                                if asset_id == usdt_id
                        );
                        if usdt_only {
                            is_usdt
                        } else {
                            !is_usdt
                        }
                    })
                    .take(5)
                    .collect();

                (balance, usdt_balance, payments, error)
            },
            |(balance, usdt_balance, recent_payment, error)| {
                if let Some(err) = error {
                    Message::View(view::Message::LiquidSend(view::LiquidSendMessage::Error(
                        err,
                    )))
                } else {
                    Message::View(view::Message::LiquidSend(
                        view::LiquidSendMessage::DataLoaded {
                            balance,
                            usdt_balance,
                            recent_payment,
                        },
                    ))
                }
            },
        )
    }
}

impl State for LiquidSend {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        // Delegate to SideShift flow when active
        if let Some(sideshift) = &self.sideshift_flow {
            let asset_id = usdt_asset_id(self.breez_client.network()).unwrap_or("");
            return sideshift.view(
                menu,
                cache,
                self.usdt_balance,
                &self.recent_transaction,
                asset_id,
            );
        }

        let fiat_converter = cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

        if let Some(payment) = &self.selected_payment {
            view::dashboard(
                menu,
                cache,
                view::liquid::transaction_detail_view(
                    payment,
                    fiat_converter,
                    cache.bitcoin_unit,
                    usdt_asset_id(self.breez_client.network()).unwrap_or(""),
                    &[],
                ),
            )
        } else {
            let comment = self.comment.clone().unwrap_or("".to_string());

            view::liquid_send_with_flow(view::LiquidSendFlowConfig {
                flow_state: &self.flow_state,
                btc_balance: self.btc_balance,
                usdt_balance: self.usdt_balance,
                fiat_converter,
                recent_transaction: &self.recent_transaction,
                input: &self.input,
                amount_input: &self.amount_input,
                usdt_amount_input: &self.usdt_amount_input,
                to_asset: self.to_asset,
                from_asset: self.from_asset,
                receive_network: self.receive_network,
                send_picker_open: self.send_picker_open,
                receive_picker_open: self.receive_picker_open,
                uri_asset: self.uri_asset,
                usdt_asset_id: usdt_asset_id(self.breez_client.network()).unwrap_or(""),
                comment,
                description: self.description.as_deref(),
                lightning_limits: self.lightning_limits,
                amount: self.amount,
                is_sending: self.is_sending,
                menu,
                cache,
                input_type: &self.input_type,
                onchain_limits: self.onchain_limits,
                bitcoin_unit: cache.bitcoin_unit,
                error: self.error.as_deref(),
                // Cross-asset swaps require SideSwap (mainnet only)
                cross_asset_supported: matches!(
                    self.breez_client.network(),
                    breez_sdk_liquid::bitcoin::Network::Bitcoin
                ),
                pay_fees_with_asset: self.pay_fees_with_asset,
                max_loading: self.max_loading,
                sent_celebration_context: &self.sent_celebration_context,
                sent_amount_display: &self.sent_amount_display,
                sent_quote: &self.sent_quote,
                sent_image_handle: &self.sent_image_handle,
            })
        }
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        // Handle SideShift send messages when flow is active
        if let Message::View(view::Message::SideshiftSend(ref msg)) = message {
            if let Some(sideshift) = &mut self.sideshift_flow {
                // Intercept Reset/Back to return to native send
                if matches!(
                    msg,
                    view::SideshiftSendMessage::Reset | view::SideshiftSendMessage::Back
                ) && matches!(
                    sideshift.phase(),
                    super::sideshift_send::SendPhase::Sent
                        | super::sideshift_send::SendPhase::Failed
                        | super::sideshift_send::SendPhase::AddressInput
                ) {
                    self.sideshift_flow = None;
                    return self.load_balance();
                }
                return sideshift.update(msg, &self.breez_client, self.usdt_balance);
            }
            return Task::none();
        }

        // When SideShift flow is active, only forward DataLoaded for balance updates
        if self.sideshift_flow.is_some() {
            if let Message::View(view::Message::LiquidSend(view::LiquidSendMessage::DataLoaded {
                ..
            })) = &message
            {
                // Fall through to handle DataLoaded below
            } else {
                return Task::none();
            }
        }

        if let Message::View(view::Message::LiquidSend(ref msg)) = message {
            match msg {
                view::LiquidSendMessage::PresetAsset(asset) => {
                    // Set both "You Send" and "They Receive" to the same asset
                    self.from_asset = *asset;
                    self.to_asset = *asset;
                    self.home_asset = *asset;
                    self.receive_network = match asset {
                        SendAsset::Lbtc => ReceiveNetwork::Lightning,
                        SendAsset::Usdt => ReceiveNetwork::Liquid,
                    };
                    self.amount = Amount::ZERO;
                    self.input = form::Value::default();
                    self.input_type = None;
                    self.uri_asset = None;
                    self.error = None;
                    self.sideshift_flow = None;
                    self.is_drain = false;
                    return self.load_balance();
                }
                view::LiquidSendMessage::InputEdited(value) => {
                    // The destination is changing, so any prepare for the old one
                    // is now for a payment that will never be sent.
                    self.invalidate_prepare();
                    self.input.value = value.clone();
                    self.error = None;
                    let breez = self.breez_client.clone();
                    let breez_clone = self.breez_client.clone();
                    let breez_client = self.breez_client.clone();
                    let value_owned = value.clone();
                    // TODO: Add some kind of debouncing mechanism here, so that we don't call breez
                    // API again and again
                    let value_for_callback = value.clone();
                    let validate_input = Task::perform(
                        async move { breez.validate_input(value_owned).await },
                        move |input| {
                            Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::InputValidated(
                                    value_for_callback.clone(),
                                    input,
                                ),
                            ))
                        },
                    );

                    // Fetch limits only if not already available
                    if self.lightning_limits.is_none() || self.onchain_limits.is_none() {
                        let fetch_lightning_limits = Task::perform(
                            async move { breez_clone.fetch_lightning_limits().await },
                            |limits| match limits {
                                Ok(limits) => Message::View(view::Message::LiquidSend(
                                    view::LiquidSendMessage::LightningLimitsFetched {
                                        min_sat: limits.send.min_sat,
                                        max_sat: limits.send.max_sat,
                                    },
                                )),
                                Err(e) => Message::View(view::Message::LiquidSend(
                                    view::LiquidSendMessage::Error(format!(
                                        "Couldn't fetch lightning limits: {}",
                                        e
                                    )),
                                )),
                            },
                        );

                        let fetch_onchain_limits = Task::perform(
                            async move { breez_client.fetch_onchain_limits().await },
                            |limits| match limits {
                                Ok(limits) => Message::View(view::Message::LiquidSend(
                                    view::LiquidSendMessage::OnChainLimitsFetched {
                                        min_sat: limits.send.min_sat,
                                        max_sat: limits.send.max_sat,
                                    },
                                )),
                                Err(e) => Message::View(view::Message::LiquidSend(
                                    view::LiquidSendMessage::Error(format!(
                                        "Couldn't fetch onchain limits: {}",
                                        e
                                    )),
                                )),
                            },
                        );
                        return Task::batch(vec![
                            validate_input,
                            fetch_lightning_limits,
                            fetch_onchain_limits,
                        ]);
                    }
                    return validate_input;
                }
                view::LiquidSendMessage::Send => {
                    // Route to SideShift for cross-chain sends
                    if self.receive_network.is_sideshift() {
                        let flow = SideshiftSendFlow::new();
                        // Pre-fill the address from the input and auto-select the network
                        let addr = self.input.value.trim().to_string();
                        if !addr.is_empty() {
                            // Dispatch address edit + network selection + Next
                            self.sideshift_flow = Some(flow);
                            let addr_msg = Message::View(view::Message::SideshiftSend(
                                view::SideshiftSendMessage::RecipientAddressEdited(addr),
                            ));
                            let network = self.receive_network.to_sideshift_network();
                            let mut tasks = vec![Task::done(addr_msg)];
                            if let Some(net) = network {
                                tasks.push(Task::done(Message::View(
                                    view::Message::SideshiftSend(
                                        view::SideshiftSendMessage::DisambiguateNetwork(net),
                                    ),
                                )));
                            }
                            tasks.push(Task::done(Message::View(view::Message::SideshiftSend(
                                view::SideshiftSendMessage::Next,
                            ))));
                            return Task::batch(tasks);
                        } else {
                            // No address yet — just open SideShift flow
                            self.sideshift_flow = Some(flow);
                            return Task::none();
                        }
                    }

                    let description = if let Some(input_type) = &self.input_type {
                        match input_type {
                            InputType::BitcoinAddress { address } => {
                                format!(
                                    "Sending money to {}",
                                    display_abbreviated(address.address.clone())
                                )
                            }
                            InputType::Bolt11 { invoice } => {
                                self.is_drain = false;
                                if let Some(amt) = invoice.amount_msat {
                                    if let Ok(amount) = Amount::from_str_in(
                                        &amt.to_string(),
                                        breez_sdk_liquid::bitcoin::Denomination::MilliSatoshi,
                                    ) {
                                        self.amount = amount;
                                        self.amount_input.valid = true;
                                        self.amount_input.value = if matches!(
                                            cache.bitcoin_unit,
                                            BitcoinDisplayUnit::BTC
                                        ) {
                                            amount.to_btc().to_string()
                                        } else {
                                            amount.to_sat().to_string()
                                        };
                                    }
                                }
                                if let Some(description) =
                                    invoice.description.as_deref().filter(|d| !d.is_empty())
                                {
                                    description.to_string()
                                } else {
                                    format!(
                                        "Sending money to {}",
                                        display_abbreviated(invoice.bolt11.clone())
                                    )
                                }
                            }
                            InputType::Bolt12Offer {
                                offer,
                                bip353_address,
                            } => {
                                let min_amount = offer.min_amount.clone().unwrap_or(
                                    breez_sdk_liquid::Amount::Bitcoin { amount_msat: 0 },
                                );

                                if let Some((min_limits, max_limits)) = self.lightning_limits {
                                    if let breez_sdk_liquid::Amount::Bitcoin { amount_msat } =
                                        min_amount
                                    {
                                        // convert from millisat to sat
                                        let amount_sat = amount_msat / 1000;
                                        self.lightning_limits = Some((
                                            std::cmp::max(min_limits, amount_sat),
                                            max_limits,
                                        ));
                                    }
                                }

                                if let Some(bip353_address) = bip353_address {
                                    format!("Sending money to {}", bip353_address.clone())
                                } else if let Some(description) = offer.description.clone() {
                                    description
                                } else {
                                    format!(
                                        "Sending money to {}",
                                        display_abbreviated(offer.offer.clone())
                                    )
                                }
                            }

                            InputType::LiquidAddress { address } => {
                                self.is_drain = false;
                                if self.to_asset == SendAsset::Usdt {
                                    if let Some(amount) = address.amount {
                                        let amount_str = format!("{}", amount);
                                        let base_units_opt = parse_asset_to_minor_units(
                                            amount_str.trim(),
                                            USDT_PRECISION,
                                        );
                                        match base_units_opt {
                                            Some(base_units) => {
                                                self.usdt_amount_input.value = amount_str;
                                                if base_units == 0 {
                                                    self.usdt_amount_input.valid = false;
                                                    self.usdt_amount_input.warning =
                                                        Some("Amount must be greater than zero");
                                                } else if base_units > self.usdt_balance {
                                                    self.usdt_amount_input.valid = false;
                                                    self.usdt_amount_input.warning =
                                                        Some("Insufficient USDt balance");
                                                } else {
                                                    self.usdt_amount_input.valid = true;
                                                    self.usdt_amount_input.warning = None;
                                                }
                                            }
                                            None => {
                                                self.usdt_amount_input.value = String::new();
                                                self.usdt_amount_input.valid = false;
                                                self.usdt_amount_input.warning =
                                                    Some("Invalid amount");
                                            }
                                        }
                                    }
                                } else if let Some(amount_sat) = address.amount_sat {
                                    let amount_str =
                                        if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                            Amount::from_sat(amount_sat).to_btc().to_string()
                                        } else {
                                            amount_sat.to_string()
                                        };
                                    let amount = Amount::from_sat(amount_sat);
                                    self.amount = amount;
                                    self.amount_input.value = amount_str;
                                    if amount > self.btc_balance {
                                        self.amount_input.valid = false;
                                        self.amount_input.warning = Some("Insufficient balance");
                                    } else if let Some((min_sat, max_sat)) = self.lightning_limits {
                                        if amount_sat < min_sat {
                                            self.amount_input.valid = false;
                                            self.amount_input.warning = Some("Below minimum limit");
                                        } else if amount_sat > max_sat {
                                            self.amount_input.valid = false;
                                            self.amount_input.warning =
                                                Some("Exceeds maximum limit");
                                        } else {
                                            self.amount_input.valid = true;
                                            self.amount_input.warning = None;
                                        }
                                    } else {
                                        self.amount_input.valid = true;
                                        self.amount_input.warning = None;
                                    }
                                }
                                format!(
                                    "Sending money to {}",
                                    display_abbreviated(address.address.clone())
                                )
                            }
                            _ => String::from("Send Payment"),
                        }
                    } else {
                        String::from("")
                    };

                    self.description = if description.is_empty() {
                        None
                    } else {
                        Some(description)
                    };
                    self.flow_state = LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    };
                }
                view::LiquidSendMessage::History => {
                    return redirect(Menu::Liquid(LiquidSubMenu::Transactions(None)));
                }
                view::LiquidSendMessage::SelectTransaction(idx) => {
                    if let Some(payment) = self.recent_payments.get(*idx).cloned() {
                        self.selected_payment = Some(payment.clone());
                        return Task::batch(vec![
                            redirect(Menu::Liquid(LiquidSubMenu::Transactions(None))),
                            Task::done(Message::View(view::Message::PreselectPayment(payment))),
                        ]);
                    }
                }
                view::LiquidSendMessage::DataLoaded {
                    balance,
                    usdt_balance,
                    recent_payment,
                } => {
                    self.btc_balance = *balance;
                    self.usdt_balance = *usdt_balance;
                    self.recent_payments = recent_payment.clone();

                    if !recent_payment.is_empty() {
                        let fiat_converter: Option<view::FiatAmountConverter> =
                            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
                        let usdt_id = usdt_asset_id(self.breez_client.network()).unwrap_or("");
                        let txns = recent_payment
                            .iter()
                            .map(|payment| {
                                let status = payment.status;
                                let time_ago = format_time_ago(payment.timestamp.into());
                                let usdt_amount_minor = match &payment.details {
                                    DomainPaymentDetails::LiquidAsset {
                                        asset_id,
                                        asset_info,
                                        ..
                                    } if !usdt_id.is_empty() && asset_id == usdt_id => {
                                        asset_info.as_ref().map(|i| i.amount_minor)
                                    }
                                    _ => None,
                                };
                                let is_usdt_payment = usdt_amount_minor.is_some();
                                let amount = match usdt_amount_minor {
                                    Some(minor) => Amount::from_sat(minor),
                                    None => Amount::from_sat(payment.amount_sat),
                                };
                                let fiat_amount = if is_usdt_payment {
                                    None
                                } else {
                                    fiat_converter
                                        .as_ref()
                                        .map(|c: &view::FiatAmountConverter| c.convert(amount))
                                };

                                // Description: prefer payer_note, then fall back to the
                                // invoice description. For empty USDt Liquid payments, show
                                // "USDt Transfer" as a friendly default.
                                let desc: String = match &payment.details {
                                    DomainPaymentDetails::LiquidAsset {
                                        payer_note,
                                        description,
                                        ..
                                    } if is_usdt_payment => {
                                        let fallback = if description.is_empty() {
                                            "USDt Transfer"
                                        } else {
                                            description.as_str()
                                        };
                                        payer_note
                                            .as_deref()
                                            .filter(|s| !s.is_empty())
                                            .unwrap_or(fallback)
                                            .to_owned()
                                    }
                                    _ => payment.details.description().to_owned(),
                                };

                                let is_incoming = payment.is_incoming();

                                let fees_sat = Amount::from_sat(payment.fees_sat);

                                let details = payment.details.clone();
                                let usdt_display = if is_usdt_payment {
                                    Some(format!("{} USDt", format_usdt_display(amount.to_sat())))
                                } else {
                                    None
                                };

                                view::liquid::RecentTransaction {
                                    description: desc,
                                    time_ago,
                                    amount,
                                    fiat_amount,
                                    is_incoming,
                                    status,
                                    details,
                                    fees_sat,
                                    usdt_display,
                                }
                            })
                            .collect();
                        self.recent_transaction = txns;
                    } else {
                        self.recent_transaction = Vec::new();
                    }
                }
                view::LiquidSendMessage::Error(err) => {
                    self.error = Some(err.to_string());
                    self.is_sending = false; // Reset sending flag on error
                                             // When a modal is open, the error toast renders inside the modal
                                             // overlay (above the backdrop). Otherwise use the global toast.
                    let modal_open = matches!(
                        self.flow_state,
                        LiquidSendFlowState::Main {
                            modal: Modal::AmountInput | Modal::FiatInput { .. }
                        }
                    );
                    if !modal_open {
                        return Task::done(Message::View(view::Message::ShowError(
                            err.to_string(),
                        )));
                    }
                }
                view::LiquidSendMessage::ClearError => {
                    self.error = None;
                }
                view::LiquidSendMessage::InputValidated(original_input, input_type) => {
                    // Discard stale async validation results — the user may have
                    // edited the input while validation was in-flight.
                    if *original_input != self.input.value {
                        return Task::none();
                    }
                    self.input.valid = input_type.is_some();
                    self.input_type = input_type.clone();

                    // Auto-detect asset from Liquid URI's asset_id
                    if let Some(InputType::LiquidAddress { address }) = &input_type {
                        let network = self.breez_client.network();
                        if let Some(ref uri_asset_id) = address.asset_id {
                            match asset_kind_for_id(uri_asset_id, network) {
                                Some(kind) => {
                                    self.uri_asset = Some(kind);
                                    let target_asset = match kind {
                                        AssetKind::Usdt => SendAsset::Usdt,
                                        AssetKind::Lbtc => SendAsset::Lbtc,
                                    };
                                    // On usdt_only screen with L-BTC URI: auto-enable
                                    // cross-asset (pay from USDt, receiver gets L-BTC).
                                    // Only on mainnet where SideSwap is available.
                                    let cross_asset_supported = matches!(
                                        network,
                                        breez_sdk_liquid::bitcoin::Network::Bitcoin
                                    );
                                    if self.home_asset == SendAsset::Usdt
                                        && target_asset == SendAsset::Lbtc
                                        && cross_asset_supported
                                    {
                                        self.to_asset = SendAsset::Lbtc;
                                        self.from_asset = SendAsset::Usdt;
                                    } else if self.home_asset == SendAsset::Usdt
                                        && target_asset != SendAsset::Usdt
                                    {
                                        // Non-mainnet: cross-asset not available, keep USDt
                                        self.to_asset = SendAsset::Usdt;
                                        self.from_asset = self.to_asset;
                                    } else {
                                        self.to_asset = target_asset;
                                        self.from_asset = self.to_asset;
                                    }
                                }
                                None => {
                                    // Unknown asset_id — only reset to_asset if we're
                                    // clearing a previously set URI lock. Otherwise preserve
                                    // the user's current asset selection.
                                    if self.uri_asset.is_some() {
                                        self.to_asset = if self.home_asset == SendAsset::Usdt {
                                            SendAsset::Usdt
                                        } else {
                                            SendAsset::Lbtc
                                        };
                                    }
                                    self.uri_asset = None;
                                    self.from_asset = self.to_asset;
                                }
                            }
                        } else {
                            // No asset_id in URI — only reset to_asset if we're
                            // clearing a previously set URI lock.
                            if self.uri_asset.is_some() {
                                self.to_asset = if self.home_asset == SendAsset::Usdt {
                                    SendAsset::Usdt
                                } else {
                                    SendAsset::Lbtc
                                };
                            }
                            self.uri_asset = None;
                            self.from_asset = self.to_asset;
                        }

                        // Pre-fill amount from URI if present, or clear stale values
                        if self.to_asset == SendAsset::Usdt {
                            if let Some(amount) = address.amount {
                                self.usdt_amount_input.value = amount.to_string();
                                self.usdt_amount_input.valid = amount > 0.0;
                            } else {
                                self.usdt_amount_input = form::Value::default();
                            }
                        }
                        if self.to_asset == SendAsset::Lbtc {
                            if let Some(amount_sat) = address.amount_sat {
                                self.amount = Amount::from_sat(amount_sat);
                                self.amount_input.value =
                                    if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                        Amount::from_sat(amount_sat).to_btc().to_string()
                                    } else {
                                        amount_sat.to_string()
                                    };
                                self.amount_input.valid = true;
                            } else {
                                self.amount = Amount::ZERO;
                                self.amount_input = form::Value::default();
                            }
                        }
                    } else {
                        // Not a LiquidAddress — clear URI asset state and restore default
                        self.uri_asset = None;
                        self.to_asset = if self.home_asset == SendAsset::Usdt {
                            SendAsset::Usdt
                        } else {
                            SendAsset::Lbtc
                        };
                        self.from_asset = self.to_asset;
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::AmountEdited(v)) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &mut self.flow_state
                    {
                        self.is_drain = false;
                        self.amount_input.value = v.clone();

                        if v.is_empty() {
                            self.amount_input.valid = true;
                            self.amount_input.warning = None;
                            self.amount = Amount::from_sat(0);
                        } else if let Ok(amount) = Amount::from_str_in(
                            v,
                            if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                            } else {
                                coincube_core::miniscript::bitcoin::Denomination::Satoshi
                            },
                        ) {
                            self.amount = amount;
                            let amount_sats = amount.to_sat();
                            let is_cross_asset = self.from_asset != self.to_asset;

                            // Skip balance check in cross-asset mode — the receiver amount
                            // is in a different denomination than the paying asset; the SDK
                            // validates actual balance during prepare.
                            if !is_cross_asset && amount > self.btc_balance {
                                self.amount_input.valid = false;
                                self.amount_input.warning = Some("Insufficient balance");
                            }
                            // Check limits if available
                            else if let Some((min_sat, max_sat)) = self.lightning_limits {
                                if amount_sats < min_sat {
                                    self.amount_input.valid = false;
                                    self.amount_input.warning = Some("Below minimum limit");
                                } else if amount_sats > max_sat {
                                    self.amount_input.valid = false;
                                    self.amount_input.warning = Some("Exceeds maximum limit");
                                } else {
                                    self.amount_input.valid = true;
                                    self.amount_input.warning = None;
                                }
                            } else {
                                self.amount_input.valid = true;
                                self.amount_input.warning = None;
                            }
                        } else {
                            self.amount_input.valid = false;
                            self.amount_input.warning = Some("Invalid amount format");
                        }
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::CommentEdited(comment)) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &mut self.flow_state
                    {
                        self.comment = Some(comment.clone());
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::FiatConvert) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &self.flow_state
                    {
                        // Determine default currencies
                        use crate::services::fiat::Currency;
                        let fiat_currency = cache
                            .fiat_price
                            .as_ref()
                            .and_then(|p| TryInto::<view::FiatAmountConverter>::try_into(p).ok())
                            .map(|c| c.currency())
                            .unwrap_or(Currency::USD);

                        let currencies = if fiat_currency == Currency::USD
                            || fiat_currency == Currency::EUR
                            || fiat_currency == Currency::GBP
                            || fiat_currency == Currency::JPY
                        {
                            [Currency::USD, Currency::EUR, Currency::GBP, Currency::JPY]
                        } else {
                            [fiat_currency, Currency::USD, Currency::EUR, Currency::GBP]
                        };

                        // Transition to Fiat Input with empty converters initially
                        self.flow_state = LiquidSendFlowState::Main {
                            modal: Modal::FiatInput {
                                fiat_input: form::Value::default(),
                                currencies,
                                selected_currency: fiat_currency,
                                converters: std::collections::HashMap::new(),
                            },
                        };

                        let price_source = cache
                            .fiat_price
                            .as_ref()
                            .map(|p| p.source())
                            .unwrap_or(crate::services::fiat::PriceSource::CoinGecko);

                        return Task::perform(
                            async move {
                                use crate::app::cache::FiatPriceRequest;

                                let mut tasks = vec![];
                                for currency in currencies.iter() {
                                    let request = FiatPriceRequest::new(price_source, *currency);
                                    tasks.push(async move {
                                        let price = request.send_default().await;
                                        (*currency, price)
                                    });
                                }

                                let mut converters = std::collections::HashMap::new();

                                for task in tasks {
                                    let (currency, price) = task.await;
                                    if let Ok(converter) =
                                        TryInto::<view::FiatAmountConverter>::try_into(&price)
                                    {
                                        converters.insert(currency, converter);
                                    }
                                }

                                converters
                            },
                            |converters| {
                                Message::View(view::Message::LiquidSend(
                                    view::LiquidSendMessage::PopupMessage(
                                        SendPopupMessage::FiatPricesLoaded(converters),
                                    ),
                                ))
                            },
                        );
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::FiatInputEdited(
                    fiat_input,
                )) => {
                    let is_cross_asset = self.from_asset != self.to_asset;
                    if let LiquidSendFlowState::Main {
                        modal:
                            Modal::FiatInput {
                                fiat_input: current_input,
                                selected_currency,
                                converters,
                                ..
                            },
                    } = &mut self.flow_state
                    {
                        current_input.value = fiat_input.clone();
                        current_input.warning = None;

                        // Validate numeric format
                        if fiat_input.is_empty() {
                            current_input.valid = true;
                        } else if fiat_input.parse::<f64>().is_ok() {
                            // Check if converted BTC amount exceeds limits
                            if let Some(converter) = converters.get(selected_currency) {
                                if let Ok(fiat_amount) = view::vault::fiat::FiatAmount::from_str_in(
                                    fiat_input,
                                    *selected_currency,
                                ) {
                                    if let Ok(btc_amount) = converter.convert_to_btc(&fiat_amount) {
                                        let amount_sats = btc_amount.to_sat();

                                        // Skip balance check in cross-asset mode — receiver
                                        // amount denomination differs from paying asset.
                                        if !is_cross_asset && btc_amount > self.btc_balance {
                                            current_input.valid = false;
                                            current_input.warning = Some("Insufficient balance");
                                        } else if let Some((min_sat, max_sat)) =
                                            self.lightning_limits
                                        {
                                            if amount_sats < min_sat {
                                                current_input.valid = false;
                                                current_input.warning = Some("Below minimum limit");
                                            } else if amount_sats > max_sat {
                                                current_input.valid = false;
                                                current_input.warning =
                                                    Some("Exceeds maximum limit");
                                            } else {
                                                current_input.valid = true;
                                            }
                                        } else {
                                            current_input.valid = true;
                                        }
                                    } else {
                                        // Conversion to BTC failed
                                        current_input.valid = false;
                                        current_input.warning = Some("Unable to convert to BTC");
                                    }
                                } else {
                                    // Invalid fiat amount format
                                    current_input.valid = false;
                                    current_input.warning = Some("Invalid fiat amount");
                                }
                            } else {
                                // Converter not available
                                current_input.valid = false;
                                current_input.warning = Some("Exchange rate unavailable");
                            }
                        } else {
                            current_input.valid = false;
                            current_input.warning = Some("Invalid number format");
                        }
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::FiatCurrencySelected(
                    currency,
                )) => {
                    if let LiquidSendFlowState::Main {
                        modal:
                            Modal::FiatInput {
                                selected_currency, ..
                            },
                    } = &mut self.flow_state
                    {
                        *selected_currency = *currency;
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::FiatPricesLoaded(
                    converters,
                )) => {
                    if let LiquidSendFlowState::Main {
                        modal:
                            Modal::FiatInput {
                                converters: modal_converters,
                                ..
                            },
                    } = &mut self.flow_state
                    {
                        *modal_converters = converters.clone();
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::FiatDone) => {
                    self.is_drain = false;
                    let is_cross_asset = self.from_asset != self.to_asset;
                    if let LiquidSendFlowState::Main {
                        modal:
                            Modal::FiatInput {
                                fiat_input,
                                selected_currency,
                                converters,
                                ..
                            },
                    } = &mut self.flow_state
                    {
                        if let Ok(_fiat_val) = fiat_input.value.parse::<f64>() {
                            // Check if converter is available
                            if let Some(converter) = converters.get(selected_currency) {
                                // Convert fiat to BTC using the converter for selected currency
                                if let Ok(fiat_amount) = view::vault::fiat::FiatAmount::from_str_in(
                                    &fiat_input.value,
                                    *selected_currency,
                                ) {
                                    if let Ok(btc_amount) = converter.convert_to_btc(&fiat_amount) {
                                        self.amount = btc_amount;
                                        let btc_str = if matches!(
                                            cache.bitcoin_unit,
                                            BitcoinDisplayUnit::BTC
                                        ) {
                                            btc_amount.to_btc().to_string()
                                        } else {
                                            btc_amount.to_sat().to_string()
                                        };
                                        let amount_sats = btc_amount.to_sat();

                                        // Validate the converted BTC amount.
                                        // Skip balance check in cross-asset mode — receiver
                                        // amount denomination differs from paying asset.
                                        let (valid, warning) = if !is_cross_asset
                                            && btc_amount > self.btc_balance
                                        {
                                            (false, Some("Amount exceeds available balance"))
                                        } else {
                                            let limits = if matches!(
                                                self.input_type,
                                                Some(InputType::BitcoinAddress { .. })
                                            ) {
                                                self.onchain_limits
                                            } else {
                                                self.lightning_limits
                                            };

                                            if let Some((min_sat, max_sat)) = limits {
                                                if amount_sats < min_sat {
                                                    (false, Some("Amount is below minimum limit"))
                                                } else if amount_sats > max_sat {
                                                    (false, Some("Amount exceeds maximum limit"))
                                                } else {
                                                    (true, None)
                                                }
                                            } else {
                                                (true, None)
                                            }
                                        };

                                        self.amount_input = form::Value {
                                            value: btc_str,
                                            valid,
                                            warning,
                                        };

                                        // Only close modal on successful conversion
                                        self.flow_state = LiquidSendFlowState::Main {
                                            modal: Modal::AmountInput,
                                        };
                                    } else {
                                        // Conversion to BTC failed - stay in fiat modal with error
                                        fiat_input.valid = false;
                                        fiat_input.warning = Some("Unable to convert to BTC");
                                    }
                                } else {
                                    // Invalid fiat amount - stay in fiat modal with error
                                    fiat_input.valid = false;
                                    fiat_input.warning = Some("Invalid fiat amount");
                                }
                            } else {
                                // Converter not available - stay in fiat modal with error
                                fiat_input.valid = false;
                                fiat_input.warning = Some("Exchange rate unavailable");
                            }
                        }
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::Done) => {
                    self.error = None;
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &self.flow_state
                    {
                        if let Some(input_type) = &self.input_type {
                            // USDt send path: Liquid address + USDt asset selected
                            if matches!(input_type, InputType::LiquidAddress { .. })
                                && self.to_asset == SendAsset::Usdt
                            {
                                let usdt_val_str = self.usdt_amount_input.value.trim().to_string();
                                let usdt_base =
                                    match parse_asset_to_minor_units(&usdt_val_str, USDT_PRECISION)
                                        .filter(|&v| v > 0)
                                    {
                                        Some(v) => v,
                                        None => {
                                            self.error = Some("Invalid USDt amount".to_string());
                                            return Task::none();
                                        }
                                    };
                                let network = self.breez_client.network();
                                let to_asset_id = match usdt_asset_id(network) {
                                    Some(id) => id.to_string(),
                                    None => {
                                        self.error =
                                            Some("USDt not available on this network".to_string());
                                        return Task::none();
                                    }
                                };
                                // Resolve from_asset for cross-asset swap
                                let from_asset_id: Option<String> =
                                    if self.from_asset != self.to_asset {
                                        let kind = match self.from_asset {
                                            SendAsset::Lbtc => AssetKind::Lbtc,
                                            SendAsset::Usdt => AssetKind::Usdt,
                                        };
                                        match kind.asset_id(network) {
                                            Some(id) => Some(id.to_string()),
                                            None => {
                                                self.error = Some(format!(
                                                    "{} not available on this network",
                                                    kind.ticker()
                                                ));
                                                return Task::none();
                                            }
                                        }
                                    } else {
                                        None
                                    };
                                let destination = match input_type {
                                    InputType::LiquidAddress { address } => address.address.clone(),
                                    _ => unreachable!(),
                                };
                                let breez_client = self.breez_client.clone();
                                let context = Box::new(self.open_prepare_context());
                                return Task::perform(
                                    async move {
                                        breez_client
                                            .prepare_send_asset(
                                                destination,
                                                &to_asset_id,
                                                usdt_base,
                                                USDT_PRECISION,
                                                from_asset_id.as_deref(),
                                            )
                                            .await
                                    },
                                    move |result| match result {
                                        Ok(prepare_response) => {
                                            Message::View(view::Message::LiquidSend(
                                                view::LiquidSendMessage::PrepareResponseReceived(
                                                    context.clone(),
                                                    prepare_response,
                                                ),
                                            ))
                                        }
                                        Err(e) => Message::View(view::Message::LiquidSend(
                                            view::LiquidSendMessage::Error(friendly_prepare_error(
                                                &e,
                                            )),
                                        )),
                                    },
                                );
                            }

                            let destination = match input_type {
                                InputType::Bolt11 { invoice } => invoice.bolt11.clone(),
                                InputType::Bolt12Offer { offer, .. } => offer.offer.clone(),
                                InputType::BitcoinAddress { address } => address.address.clone(),
                                InputType::LiquidAddress { address } => address.address.clone(),
                                _ => {
                                    self.error = Some("Unsupported payment type".to_string());
                                    return Task::none();
                                }
                            };

                            // Cross-asset swap: from_asset differs from to_asset
                            // Use PayAmount::Asset with the appropriate asset IDs
                            if self.from_asset != self.to_asset {
                                let network = self.breez_client.network();
                                let to_asset_id = match lbtc_asset_id(network) {
                                    Some(id) => id.to_string(),
                                    None => {
                                        self.error =
                                            Some("L-BTC not available on this network".to_string());
                                        return Task::none();
                                    }
                                };
                                let from_kind = match self.from_asset {
                                    SendAsset::Lbtc => AssetKind::Lbtc,
                                    SendAsset::Usdt => AssetKind::Usdt,
                                };
                                let from_asset_id = match from_kind.asset_id(network) {
                                    Some(id) => id.to_string(),
                                    None => {
                                        self.error = Some(format!(
                                            "{} not available on this network",
                                            from_kind.ticker()
                                        ));
                                        return Task::none();
                                    }
                                };
                                let amount_sat = self.amount.to_sat();
                                let breez_client = self.breez_client.clone();
                                let context = Box::new(self.open_prepare_context());
                                return Task::perform(
                                    async move {
                                        breez_client
                                            .prepare_send_asset(
                                                destination,
                                                &to_asset_id,
                                                amount_sat,
                                                crate::app::breez_liquid::assets::LBTC_PRECISION,
                                                Some(&from_asset_id),
                                            )
                                            .await
                                    },
                                    move |result| match result {
                                        Ok(prepare_response) => {
                                            Message::View(view::Message::LiquidSend(
                                                view::LiquidSendMessage::PrepareResponseReceived(
                                                    context.clone(),
                                                    prepare_response,
                                                ),
                                            ))
                                        }
                                        Err(e) => Message::View(view::Message::LiquidSend(
                                            view::LiquidSendMessage::Error(format!(
                                                "Failed to prepare cross-asset payment: {}",
                                                e
                                            )),
                                        )),
                                    },
                                );
                            }

                            let breez_client = self.breez_client.clone();
                            let amount_sat = self.amount.to_sat();
                            let is_drain = self.is_drain;
                            let is_onchain = matches!(input_type, InputType::BitcoinAddress { .. });
                            let context = Box::new(self.open_prepare_context());

                            // On-chain and Lightning are prepared by different SDK
                            // calls and executed by different ones, so exactly one
                            // is dispatched — and whichever it is, its response
                            // carries the address and snapshot it was built from.
                            if is_onchain {
                                return Task::perform(
                                    async move {
                                        let pay_amount = if is_drain {
                                            breez_sdk_liquid::prelude::PayAmount::Drain
                                        } else {
                                            breez_sdk_liquid::prelude::PayAmount::Bitcoin {
                                                receiver_amount_sat: amount_sat,
                                            }
                                        };
                                        breez_client
                                            .prepare_pay_onchain(
                                                &breez_sdk_liquid::prelude::PreparePayOnchainRequest {
                                                    amount: pay_amount,
                                                    fee_rate_sat_per_vbyte: None,
                                                },
                                            )
                                            .await
                                    },
                                    move |result| {
                                        match result {
                                        Ok(response) => {
                                            Message::View(view::Message::LiquidSend(
                                                view::LiquidSendMessage::PrepareOnChainResponseReceived {
                                                    context: context.clone(),
                                                    address: destination.clone(),
                                                    response,
                                                },
                                            ))
                                        }
                                        Err(e) => Message::View(view::Message::LiquidSend(
                                            view::LiquidSendMessage::Error(format!(
                                                "Failed to prepare payment: {}",
                                                e
                                            )),
                                        )),
                                    }
                                    },
                                );
                            }

                            return Task::perform(
                                async move {
                                    let pay_amount = if is_drain {
                                        breez_sdk_liquid::prelude::PayAmount::Drain
                                    } else {
                                        breez_sdk_liquid::prelude::PayAmount::Bitcoin {
                                            receiver_amount_sat: amount_sat,
                                        }
                                    };
                                    breez_client
                                        .prepare_send_payment(
                                            &breez_sdk_liquid::prelude::PrepareSendRequest {
                                                destination,
                                                amount: Some(pay_amount),
                                                disable_mrh: None,
                                                payment_timeout_sec: None,
                                            },
                                        )
                                        .await
                                },
                                move |result| match result {
                                    Ok(prepare_response) => {
                                        Message::View(view::Message::LiquidSend(
                                            view::LiquidSendMessage::PrepareResponseReceived(
                                                context.clone(),
                                                prepare_response,
                                            ),
                                        ))
                                    }
                                    Err(e) => Message::View(view::Message::LiquidSend(
                                        view::LiquidSendMessage::Error(format!(
                                            "Failed to prepare payment: {}",
                                            e
                                        )),
                                    )),
                                },
                            );
                        }
                    }
                }
                view::LiquidSendMessage::PrepareResponseReceived(context, prepare_response) => {
                    // A response for an abandoned or superseded prepare. Dropping
                    // it is the whole point of the generation token: accepting it
                    // would reopen FinalCheck with a payment the user has already
                    // walked away from.
                    if context.generation != self.prepare_generation {
                        tracing::debug!(
                            "ignoring stale Liquid prepare response (generation {} != {})",
                            context.generation,
                            self.prepare_generation
                        );
                        return Task::none();
                    }

                    // The fee-method choice exists only for a same-asset USDt
                    // send: a cross-asset swap cannot use asset fees (SDK
                    // constraint) and no other send is offered one. Both the
                    // fallback and the write-back are scoped to that shape.
                    //
                    // Unscoped, the fallback read every response — and an L-BTC
                    // send's response is always `fees_sat: Some` with no asset
                    // fee, which is exactly the second branch's "the asset fee
                    // is unavailable, switch them to L-BTC". So sending L-BTC
                    // silently cleared a `pay_fees_with_asset` the user had set
                    // for USDt, and the next USDt send opened with a preference
                    // they never changed (it also drives the SendMax branch).
                    let asset_fee_choice_applies = context.from_asset == context.to_asset
                        && matches!(context.to_asset, SendAsset::Usdt);

                    let mut pay_fees_with_asset = context.pay_fees_with_asset;
                    if asset_fee_choice_applies {
                        // If the preferred fee method is unavailable, fall back
                        // to the other one automatically.
                        if !pay_fees_with_asset
                            && prepare_response.fees_sat.is_none()
                            && prepare_response.estimated_asset_fees.is_some()
                        {
                            pay_fees_with_asset = true;
                        } else if pay_fees_with_asset
                            && prepare_response.estimated_asset_fees.is_none()
                            && prepare_response.fees_sat.is_some()
                        {
                            pay_fees_with_asset = false;
                        }
                        self.pay_fees_with_asset = pay_fees_with_asset;
                    }

                    // Resolved here, once, and carried by the intent — never
                    // recomputed at confirm time from fields that have moved on.
                    let use_asset_fees = asset_fee_choice_applies && pay_fees_with_asset;

                    let mut context = (**context).clone();
                    context.pay_fees_with_asset = pay_fees_with_asset;
                    self.flow_state =
                        LiquidSendFlowState::FinalCheck(Box::new(PreparedIntent::new(
                            context,
                            PreparedPayment::Regular {
                                response: Box::new(prepare_response.clone()),
                                use_asset_fees,
                            },
                        )));
                }
                view::LiquidSendMessage::PrepareOnChainResponseReceived {
                    context,
                    address,
                    response,
                } => {
                    if context.generation != self.prepare_generation {
                        tracing::debug!(
                            "ignoring stale Liquid on-chain prepare response (generation {} != {})",
                            context.generation,
                            self.prepare_generation
                        );
                        return Task::none();
                    }
                    self.flow_state =
                        LiquidSendFlowState::FinalCheck(Box::new(PreparedIntent::new(
                            (**context).clone(),
                            PreparedPayment::Onchain {
                                response: Box::new(response.clone()),
                                address: address.clone(),
                            },
                        )));
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::ToggleSendAsset) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &self.flow_state
                    {
                        // Cross-asset swaps require SideSwap (mainnet only)
                        let cross_asset_supported = matches!(
                            self.breez_client.network(),
                            breez_sdk_liquid::bitcoin::Network::Bitcoin
                        );
                        if self.uri_asset.is_some() && cross_asset_supported {
                            // URI locked the to_asset — toggle changes from_asset (cross-asset swap)
                            let opposite = match self.to_asset {
                                SendAsset::Lbtc => SendAsset::Usdt,
                                SendAsset::Usdt => SendAsset::Lbtc,
                            };
                            if self.from_asset != self.to_asset {
                                // Already in cross-asset mode — toggle back to same-asset.
                                // On usdt_only screen: if to_asset was forced to Lbtc by URI,
                                // we can't go back to same-asset Lbtc send — block the toggle.
                                if self.home_asset == SendAsset::Usdt
                                    && self.to_asset != SendAsset::Usdt
                                {
                                    // Can't disable cross-asset on usdt_only screen when URI
                                    // requires a non-USDt asset — ignore toggle
                                } else {
                                    self.from_asset = self.to_asset;
                                }
                            } else {
                                // Enable cross-asset: pay with the opposite asset
                                self.from_asset = opposite;
                            }

                            // Re-validate amount inputs after cross-asset mode change.
                            // Balance checks depend on is_cross_asset, which just changed.
                            let is_cross_asset = self.from_asset != self.to_asset;
                            match self.to_asset {
                                SendAsset::Lbtc => {
                                    if !self.amount_input.value.trim().is_empty() {
                                        if !is_cross_asset && self.amount > self.btc_balance {
                                            self.amount_input.valid = false;
                                            self.amount_input.warning =
                                                Some("Insufficient balance");
                                        } else if self.amount_input.warning
                                            == Some("Insufficient balance")
                                        {
                                            // Clear stale balance warning
                                            self.amount_input.valid = true;
                                            self.amount_input.warning = None;
                                        }
                                    }
                                }
                                SendAsset::Usdt => {
                                    let trimmed = self.usdt_amount_input.value.trim();
                                    if !trimmed.is_empty() {
                                        if let Some(base_units) =
                                            parse_asset_to_minor_units(trimmed, USDT_PRECISION)
                                        {
                                            if !is_cross_asset && base_units > self.usdt_balance {
                                                self.usdt_amount_input.valid = false;
                                                self.usdt_amount_input.warning =
                                                    Some("Insufficient USDt balance");
                                            } else if self.usdt_amount_input.warning
                                                == Some("Insufficient USDt balance")
                                            {
                                                // Clear stale balance warning
                                                self.usdt_amount_input.valid = true;
                                                self.usdt_amount_input.warning = None;
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            // No URI lock — legacy behavior: toggle to_asset directly
                            let next = match self.to_asset {
                                SendAsset::Lbtc => SendAsset::Usdt,
                                SendAsset::Usdt => SendAsset::Lbtc,
                            };
                            self.to_asset = next;
                            self.from_asset = self.to_asset;
                            self.amount = Amount::ZERO;
                            self.usdt_amount_input = form::Value::default();
                            self.amount_input = form::Value::default();
                            self.is_drain = false;
                        }
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::ToggleFeeAsset) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &self.flow_state
                    {
                        self.pay_fees_with_asset = !self.pay_fees_with_asset;
                        self.error = None;
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::SendMax) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &self.flow_state
                    {
                        self.error = None;
                        if self.from_asset != self.to_asset {
                            // Cross-asset swap: max depends on a SideSwap quote
                            // which we don't have yet. Skip SendMax.
                            self.error = Some(
                                "Max send is not available for cross-asset swaps. Please enter an amount manually."
                                    .to_string(),
                            );
                        } else if self.to_asset == SendAsset::Usdt {
                            if !self.pay_fees_with_asset {
                                // Fees paid in L-BTC — full USDt balance can be sent
                                let display = format_usdt_display(self.usdt_balance);
                                self.usdt_amount_input.value = display;
                                self.usdt_amount_input.valid = true;
                                self.usdt_amount_input.warning = None;
                            } else if let Some(InputType::LiquidAddress { address }) =
                                &self.input_type
                            {
                                // Fees paid in USDt — prepare with a small probe amount
                                // to learn the asset fee, then subtract it from balance.
                                let destination = address.address.clone();
                                let network = self.breez_client.network();
                                if let Some(to_asset_id) = usdt_asset_id(network) {
                                    let breez_client = self.breez_client.clone();
                                    let to_asset_id = to_asset_id.to_string();
                                    // Use a small probe amount (0.01 USDt = 1_000_000 base units)
                                    // just to discover the fee.
                                    let probe_amount = 1_000_000_u64;
                                    self.max_loading = true;
                                    return Task::perform(
                                        async move {
                                            breez_client
                                                .prepare_send_asset(
                                                    destination,
                                                    &to_asset_id,
                                                    probe_amount,
                                                    USDT_PRECISION,
                                                    None,
                                                )
                                                .await
                                                .map_err(|e| e.to_string())
                                        },
                                        |result| {
                                            Message::View(view::Message::LiquidSend(
                                                view::LiquidSendMessage::SendMaxPrepared(result),
                                            ))
                                        },
                                    );
                                }
                            }
                        } else if let Some(input_type) = &self.input_type {
                            // L-BTC send: use Drain to let SDK calculate max minus fees
                            if let InputType::BitcoinAddress { .. } = input_type {
                                // On-chain sends use prepare_pay_onchain (different swap path)
                                let breez_client = self.breez_client.clone();
                                let btc_balance = self.btc_balance;
                                self.max_loading = true;
                                return Task::perform(
                                    async move {
                                        let onchain_resp = breez_client
                                            .prepare_pay_onchain(
                                                &breez_sdk_liquid::prelude::PreparePayOnchainRequest {
                                                    amount: breez_sdk_liquid::prelude::PayAmount::Drain,
                                                    fee_rate_sat_per_vbyte: None,
                                                },
                                            )
                                            .await
                                            .map_err(|e| e.to_string())?;
                                        // Calculate max sendable: balance - total fees
                                        let max_sat = btc_balance
                                            .to_sat()
                                            .saturating_sub(onchain_resp.total_fees_sat);
                                        Ok::<u64, String>(max_sat)
                                    },
                                    |result| match result {
                                        Ok(max_sat) => Message::View(view::Message::LiquidSend(
                                            view::LiquidSendMessage::SendMaxOnChainResult(max_sat),
                                        )),
                                        Err(e) => Message::View(view::Message::LiquidSend(
                                            view::LiquidSendMessage::Error(format!(
                                                "Failed to estimate max: {e}"
                                            )),
                                        )),
                                    },
                                );
                            }

                            let destination = match input_type {
                                InputType::Bolt11 { invoice } => invoice.bolt11.clone(),
                                InputType::Bolt12Offer { offer, .. } => offer.offer.clone(),
                                InputType::LiquidAddress { address } => address.address.clone(),
                                _ => return Task::none(),
                            };
                            let breez_client = self.breez_client.clone();
                            self.max_loading = true;
                            return Task::perform(
                                async move {
                                    breez_client
                                        .prepare_send_payment(
                                            &breez_sdk_liquid::prelude::PrepareSendRequest {
                                                destination,
                                                amount: Some(
                                                    breez_sdk_liquid::prelude::PayAmount::Drain,
                                                ),
                                                disable_mrh: None,
                                                payment_timeout_sec: None,
                                            },
                                        )
                                        .await
                                        .map_err(|e| e.to_string())
                                },
                                |result| {
                                    Message::View(view::Message::LiquidSend(
                                        view::LiquidSendMessage::SendMaxPrepared(result),
                                    ))
                                },
                            );
                        }
                    }
                }
                view::LiquidSendMessage::SendMaxPrepared(result) => {
                    self.max_loading = false;
                    match result {
                        Ok(prepare_response) => {
                            if self.to_asset == SendAsset::Usdt {
                                // USDt with asset fees: subtract fee from balance
                                if let Some(asset_fee) = prepare_response.estimated_asset_fees {
                                    let fee_base =
                                        (asset_fee * 10_u64.pow(USDT_PRECISION as u32) as f64)
                                            .ceil() as u64;
                                    let max_amount = self.usdt_balance.saturating_sub(fee_base);
                                    if max_amount == 0 {
                                        self.error =
                                            Some("Balance too low to cover fees".to_string());
                                    } else {
                                        let display = format_usdt_display(max_amount);
                                        self.usdt_amount_input.value = display;
                                        self.usdt_amount_input.valid = true;
                                        self.usdt_amount_input.warning = None;
                                    }
                                } else {
                                    // No asset fee — use full balance
                                    let display = format_usdt_display(self.usdt_balance);
                                    self.usdt_amount_input.value = display;
                                    self.usdt_amount_input.valid = true;
                                    self.usdt_amount_input.warning = None;
                                }
                            } else {
                                // L-BTC drain: SDK returns the max sendable amount
                                // via fees_sat; calculate balance - fees
                                let fees = prepare_response.fees_sat.unwrap_or(0);
                                let max_sat = self.btc_balance.to_sat().saturating_sub(fees);
                                if max_sat == 0 {
                                    self.error = Some("Balance too low to cover fees".to_string());
                                } else {
                                    let max_amount = Amount::from_sat(max_sat);
                                    self.amount = max_amount;
                                    self.amount_input.value =
                                        if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                            max_amount.to_btc().to_string()
                                        } else {
                                            max_sat.to_string()
                                        };
                                    self.amount_input.valid = true;
                                    self.amount_input.warning = None;
                                    self.is_drain = true;
                                }
                            }
                        }
                        Err(e) => {
                            self.error = Some(format!("Failed to estimate max: {}", e));
                        }
                    }
                }
                view::LiquidSendMessage::SendMaxOnChainResult(max_sat) => {
                    self.max_loading = false;
                    let max_sat = *max_sat;
                    if max_sat == 0 {
                        self.error = Some("Balance too low to cover fees".to_string());
                    } else {
                        let max_amount = Amount::from_sat(max_sat);
                        self.amount = max_amount;
                        self.amount_input.value =
                            if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                max_amount.to_btc().to_string()
                            } else {
                                max_sat.to_string()
                            };
                        self.amount_input.valid = true;
                        self.amount_input.warning = None;
                        self.is_drain = true;
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::UsdtAmountEdited(v)) => {
                    if let LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &mut self.flow_state
                    {
                        self.usdt_amount_input.value = v.clone();
                        let trimmed = v.trim();
                        if trimmed.is_empty() {
                            self.usdt_amount_input.valid = true;
                            self.usdt_amount_input.warning = None;
                        } else if let Some(base_units) =
                            parse_asset_to_minor_units(trimmed, USDT_PRECISION)
                        {
                            let is_cross_asset = self.from_asset != self.to_asset;

                            if base_units == 0 {
                                self.usdt_amount_input.valid = false;
                                self.usdt_amount_input.warning =
                                    Some("Amount must be greater than zero");
                            } else if !is_cross_asset && base_units > self.usdt_balance {
                                // Skip balance check in cross-asset mode — receiver amount
                                // denomination differs from paying asset; SDK validates during prepare.
                                self.usdt_amount_input.valid = false;
                                self.usdt_amount_input.warning = Some("Insufficient USDt balance");
                            } else {
                                self.usdt_amount_input.valid = true;
                                self.usdt_amount_input.warning = None;
                            }
                        } else {
                            self.usdt_amount_input.valid = false;
                            self.usdt_amount_input.warning = Some("Invalid amount");
                        }
                    }
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::Close) => {
                    // Leaving the flow abandons the payment: the intent goes with
                    // the state transition, and the bump stops a prepare still in
                    // flight from bringing FinalCheck back up behind the user.
                    self.invalidate_prepare();
                    self.flow_state = LiquidSendFlowState::Main { modal: Modal::None };
                    self.error = None;
                    self.amount = Amount::ZERO;
                    self.is_drain = false;
                    self.lightning_limits = None;
                    self.description = None;
                    self.comment = None;
                    self.amount_input = form::Value::default();
                    self.usdt_amount_input = form::Value::default();
                    self.to_asset = if self.home_asset == SendAsset::Usdt {
                        SendAsset::Usdt
                    } else {
                        SendAsset::Lbtc
                    };
                    self.input = form::Value::default();
                    self.input_type = None;
                    self.uri_asset = None;
                    self.from_asset = self.to_asset;
                }
                view::LiquidSendMessage::ConfirmSend => {
                    if self.is_sending {
                        return Task::none();
                    }
                    // Only ever the intent this screen is displaying: it is read
                    // out of the flow state itself, so there is no second
                    // candidate to fall back to and nothing else that could win.
                    let Some(intent) = self.executable_intent().cloned() else {
                        // On FinalCheck but superseded: fail closed and say so
                        // rather than sending something the user didn't approve.
                        if matches!(self.flow_state, LiquidSendFlowState::FinalCheck(_)) {
                            self.flow_state = LiquidSendFlowState::Main { modal: Modal::None };
                            // Said through the global toast, not `self.error`.
                            // That field reaches the screen only via the
                            // AmountInput modal, and this branch lands on
                            // `Modal::None` — so the refusal rendered nowhere
                            // and the payment appeared to vanish on its own.
                            // Clearing it also stops the banner resurfacing over
                            // whatever the user composes next.
                            self.error = None;
                            return Task::done(Message::View(view::Message::ShowError(
                                "This payment is no longer current. Please start again."
                                    .to_string(),
                            )));
                        }
                        return Task::none();
                    };

                    self.is_sending = true;
                    let breez_client = self.breez_client.clone();

                    match intent.payment() {
                        PreparedPayment::Regular {
                            response,
                            use_asset_fees,
                        } => {
                            let prepare_response = (**response).clone();
                            let payer_note = intent.comment().map(str::to_string);
                            let use_asset_fees = *use_asset_fees;
                            return Task::perform(
                                async move {
                                    breez_client
                                        .send_payment(
                                            &breez_sdk_liquid::prelude::SendPaymentRequest {
                                                prepare_response,
                                                payer_note,
                                                use_asset_fees: Some(use_asset_fees),
                                            },
                                        )
                                        .await
                                },
                                |result| match result {
                                    Ok(_send_response) => Message::View(view::Message::LiquidSend(
                                        view::LiquidSendMessage::SendComplete,
                                    )),
                                    Err(e) => Message::View(view::Message::LiquidSend(
                                        view::LiquidSendMessage::Error(format!(
                                            "Failed to send payment: {}",
                                            e
                                        )),
                                    )),
                                },
                            );
                        }
                        PreparedPayment::Onchain { response, address } => {
                            let prepare_response = (**response).clone();
                            let address = address.clone();
                            return Task::perform(
                                async move {
                                    breez_client
                                        .pay_onchain(
                                            &breez_sdk_liquid::prelude::PayOnchainRequest {
                                                address,
                                                prepare_response,
                                            },
                                        )
                                        .await
                                },
                                |result| match result {
                                    Ok(_send_response) => Message::View(view::Message::LiquidSend(
                                        view::LiquidSendMessage::SendComplete,
                                    )),
                                    Err(e) => Message::View(view::Message::LiquidSend(
                                        view::LiquidSendMessage::Error(format!(
                                            "Failed to send payment: {}",
                                            e
                                        )),
                                    )),
                                },
                            );
                        }
                    }
                }
                view::LiquidSendMessage::SendComplete => {
                    // The intent has been executed; it must not be executable
                    // again, and no in-flight prepare may resurrect FinalCheck
                    // over the success screen.
                    self.invalidate_prepare();
                    self.flow_state = LiquidSendFlowState::Sent;
                    self.is_sending = false;
                    self.is_drain = false;
                    // Compute amount display for celebration screen.
                    {
                        use coincube_ui::component::amount::DisplayAmount;
                        self.sent_amount_display = if self.to_asset == SendAsset::Usdt
                            && !self.usdt_amount_input.value.trim().is_empty()
                        {
                            format!("{} USDt", self.usdt_amount_input.value.trim())
                        } else {
                            self.amount
                                .to_formatted_string_with_unit(cache.bitcoin_unit)
                        };
                    }
                    // Fresh quote for the success screen — pick the
                    // context based on the send method/asset. USDt is
                    // checked first so the "note" pose wins whenever
                    // USDt is involved, matching the receive-side
                    // priority in state/liquid/receive.rs.
                    let context = if self.to_asset == SendAsset::Usdt {
                        "note-send"
                    } else if self.receive_network == ReceiveNetwork::Lightning {
                        "lightning-send"
                    } else if self.receive_network == ReceiveNetwork::Bitcoin {
                        "bitcoin-send"
                    } else {
                        "liquid-send"
                    };
                    self.sent_celebration_context = context.to_string();
                    self.sent_quote = coincube_ui::component::quote_display::random_quote(context);
                    self.sent_image_handle =
                        coincube_ui::component::quote_display::image_handle_for_context(context);
                    let breez_client = self.breez_client.clone();
                    return Task::perform(async move { breez_client.sync().await }, |result| {
                        match result {
                            Ok(()) => Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::RefreshRequested,
                            )),
                            Err(err) => Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::Error(format!(
                                    "Failed to sync wallet: {}",
                                    err
                                )),
                            )),
                        }
                    });
                }
                view::LiquidSendMessage::BackToHome => {
                    self.invalidate_prepare();
                    self.input = form::Value::default();
                    self.amount = Amount::ZERO;
                    self.amount_input = form::Value::default();
                    self.usdt_amount_input = form::Value::default();
                    self.to_asset = if self.home_asset == SendAsset::Usdt {
                        SendAsset::Usdt
                    } else {
                        SendAsset::Lbtc
                    };
                    self.input_type = None;
                    self.uri_asset = None;
                    self.from_asset = self.to_asset;
                    self.description = None;
                    self.comment = None;
                    self.lightning_limits = None;
                    self.is_sending = false;
                    self.is_drain = false;
                    self.flow_state = LiquidSendFlowState::Main { modal: Modal::None };
                }
                view::LiquidSendMessage::LightningLimitsFetched { min_sat, max_sat } => {
                    self.lightning_limits = Some((*min_sat, *max_sat));
                }
                view::LiquidSendMessage::OnChainLimitsFetched { min_sat, max_sat } => {
                    self.onchain_limits = Some((*min_sat, *max_sat));
                }
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::FiatClose) => {
                    self.error = None;
                    self.flow_state = LiquidSendFlowState::Main {
                        modal: Modal::AmountInput,
                    }
                }
                view::LiquidSendMessage::RefreshRequested => {
                    return self.load_balance();
                }
                view::LiquidSendMessage::OpenSendPicker => {
                    self.send_picker_open = true;
                    self.receive_picker_open = false;
                    return Task::none();
                }
                view::LiquidSendMessage::OpenReceivePicker => {
                    self.receive_picker_open = true;
                    self.send_picker_open = false;
                    return Task::none();
                }
                view::LiquidSendMessage::ClosePicker => {
                    self.send_picker_open = false;
                    self.receive_picker_open = false;
                    return Task::none();
                }
                view::LiquidSendMessage::SetSendAsset(asset) => {
                    self.send_picker_open = false;
                    if self.from_asset != *asset {
                        self.from_asset = *asset;
                        self.to_asset = *asset;
                        self.home_asset = *asset;
                        // Reset receive network to default for the new asset
                        self.receive_network = match asset {
                            SendAsset::Lbtc => ReceiveNetwork::Lightning,
                            SendAsset::Usdt => ReceiveNetwork::Liquid,
                        };
                        // Reset input state
                        self.input = form::Value::default();
                        self.input_type = None;
                        self.uri_asset = None;
                        self.error = None;
                        self.sideshift_flow = None;
                        return self.load_balance();
                    }
                    return Task::none();
                }
                view::LiquidSendMessage::SetReceiveTarget(asset, network) => {
                    self.receive_picker_open = false;
                    self.to_asset = *asset;
                    self.receive_network = *network;
                    // If cross-asset, set from_asset differently
                    if *asset != self.from_asset {
                        // Cross-asset swap: from_asset stays, to_asset changes
                    } else {
                        self.from_asset = *asset;
                    }
                    // Reset input state for new target
                    self.input = form::Value::default();
                    self.input_type = None;
                    self.uri_asset = None;
                    self.error = None;
                    self.sideshift_flow = None;
                    return self.load_balance();
                }
            }
        }
        if let Message::View(view::Message::Close) | Message::View(view::Message::Reload) = message
        {
            self.selected_payment = None;
        }
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        if let Some(sideshift) = &self.sideshift_flow {
            return sideshift.subscription();
        }
        if self.is_sending {
            iced::time::every(Duration::from_millis(50)).map(|_| Message::Tick)
        } else {
            iced::Subscription::none()
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.selected_payment = None;
        self.sideshift_flow = None;
        self.load_balance()
    }
}

fn display_abbreviated(s: String) -> String {
    let formatted = if s.chars().count() > 30 {
        let first: String = s.chars().take(7).collect();
        let last: String = s
            .chars()
            .rev()
            .take(7)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        format!("{first}.....{last}")
    } else {
        s.to_string()
    };
    formatted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::State;
    use breez_sdk_liquid::prelude::{
        LiquidAddressData, PreparePayOnchainResponse, PrepareSendResponse, SendDestination,
    };
    use breez_sdk_liquid::BitcoinAddressData;

    const NETWORK: breez_sdk_liquid::bitcoin::Network = breez_sdk_liquid::bitcoin::Network::Bitcoin;

    /// A send panel wired to a disconnected client: enough to drive the state
    /// machine without an SDK. Returned `Task`s are never executed, so no
    /// network call is made — which is exactly the point, since these tests are
    /// about which payment *would* be executed.
    fn panel() -> LiquidSend {
        let client = Arc::new(crate::app::breez_liquid::BreezClient::disconnected(NETWORK));
        LiquidSend::new(Arc::new(LiquidBackend::new(client)))
    }

    fn cache() -> Cache {
        Cache::default()
    }

    fn send(panel: &mut LiquidSend, msg: view::LiquidSendMessage) {
        let _ = panel.update(
            None,
            &cache(),
            Message::View(view::Message::LiquidSend(msg)),
        );
    }

    /// `send`, but handing back the messages the update emitted. Anything the
    /// screen reports through the global toast rather than its own state is
    /// only observable here.
    fn send_emitting(panel: &mut LiquidSend, msg: view::LiquidSendMessage) -> Vec<Message> {
        use iced_runtime::futures::futures::StreamExt;

        let task = panel.update(
            None,
            &cache(),
            Message::View(view::Message::LiquidSend(msg)),
        );
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            return Vec::new();
        };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                stream
                    .filter_map(|action| async move {
                        match action {
                            iced_runtime::Action::Output(msg) => Some(msg),
                            _ => None,
                        }
                    })
                    .collect::<Vec<_>>()
                    .await
            })
    }

    fn liquid_input(address: &str) -> InputType {
        InputType::LiquidAddress {
            address: LiquidAddressData {
                address: address.to_string(),
                network: breez_sdk_liquid::prelude::Network::Bitcoin,
                asset_id: None,
                amount: None,
                amount_sat: None,
                label: None,
                message: None,
            },
        }
    }

    fn bitcoin_input(address: &str) -> InputType {
        InputType::BitcoinAddress {
            address: BitcoinAddressData {
                address: address.to_string(),
                network: breez_sdk_liquid::prelude::Network::Bitcoin,
                amount_sat: None,
                label: None,
                message: None,
            },
        }
    }

    fn regular_response(address: &str, fees_sat: u64) -> PrepareSendResponse {
        PrepareSendResponse {
            destination: SendDestination::LiquidAddress {
                address_data: match liquid_input(address) {
                    InputType::LiquidAddress { address } => address,
                    _ => unreachable!(),
                },
                bip353_address: None,
            },
            amount: None,
            fees_sat: Some(fees_sat),
            estimated_asset_fees: None,
            exchange_amount_sat: None,
            disable_mrh: None,
            payment_timeout_sec: None,
        }
    }

    fn onchain_response(total_fees_sat: u64) -> PreparePayOnchainResponse {
        PreparePayOnchainResponse {
            receiver_amount_sat: 50_000,
            claim_fees_sat: total_fees_sat / 2,
            total_fees_sat,
        }
    }

    /// Put the panel where the user is about to press Done for `input`.
    fn compose(panel: &mut LiquidSend, input: InputType, amount_sat: u64) {
        panel.flow_state = LiquidSendFlowState::Main {
            modal: Modal::AmountInput,
        };
        panel.input_type = Some(input);
        panel.amount = Amount::from_sat(amount_sat);
        panel.to_asset = SendAsset::Lbtc;
        panel.from_asset = SendAsset::Lbtc;
    }

    /// **The audited bug, as a test.** Prepare a Lightning/Liquid payment A,
    /// press Back, prepare an on-chain payment B, confirm. B must be what
    /// executes: the old code checked `prepare_response` first and would have
    /// executed A, which the user had walked away from, while the screen showed
    /// B's amount and fee.
    #[test]
    fn back_then_a_new_onchain_prepare_confirms_the_onchain_payment() {
        let mut p = panel();

        // Payment A: 1_000 sat to a Liquid address.
        compose(&mut p, liquid_input("lq1a"), 1_000);
        let ctx_a = Box::new(p.open_prepare_context());
        send(
            &mut p,
            view::LiquidSendMessage::PrepareResponseReceived(
                ctx_a.clone(),
                regular_response("lq1a", 11),
            ),
        );
        assert!(matches!(
            p.executable_intent().map(PreparedIntent::payment),
            Some(PreparedPayment::Regular { .. })
        ));

        // Back out of FinalCheck.
        send(
            &mut p,
            view::LiquidSendMessage::PopupMessage(SendPopupMessage::Close),
        );
        assert!(p.executable_intent().is_none(), "Back must abandon A");

        // Payment B: 50_000 sat to a Bitcoin address.
        compose(&mut p, bitcoin_input("bc1b"), 50_000);
        let ctx_b = Box::new(p.open_prepare_context());
        send(
            &mut p,
            view::LiquidSendMessage::PrepareOnChainResponseReceived {
                context: ctx_b,
                address: "bc1b".to_string(),
                response: onchain_response(400),
            },
        );

        // What the screen shows and what Confirm executes are the same value.
        let intent = p.executable_intent().expect("B is prepared").clone();
        match intent.payment() {
            PreparedPayment::Onchain { address, response } => {
                assert_eq!(address, "bc1b");
                assert_eq!(response.total_fees_sat, 400);
            }
            other => panic!("expected the on-chain payment, got {:?}", other),
        }
        assert_eq!(intent.amount(), Amount::from_sat(50_000));
        assert_eq!(intent.fees_sat(), 400, "the displayed fee is B's fee");
        assert_eq!(intent.total_sat(), 50_400);

        // Confirm accepts it, and A is unreachable: there is no second slot.
        send(&mut p, view::LiquidSendMessage::ConfirmSend);
        assert!(p.is_sending, "Confirm must have dispatched B");
        assert!(matches!(
            p.executable_intent().map(PreparedIntent::payment),
            Some(PreparedPayment::Onchain { .. })
        ));

        // And A, arriving late, still cannot come back.
        send(
            &mut p,
            view::LiquidSendMessage::PrepareResponseReceived(ctx_a, regular_response("lq1a", 11)),
        );
        assert!(matches!(
            p.executable_intent().map(PreparedIntent::payment),
            Some(PreparedPayment::Onchain { .. })
        ));
    }

    /// Closing while a prepare is in flight must not leave a response able to
    /// reopen FinalCheck behind the user.
    #[test]
    fn a_prepare_that_lands_after_close_is_ignored() {
        let mut p = panel();
        compose(&mut p, liquid_input("lq1a"), 1_000);
        let ctx = Box::new(p.open_prepare_context());

        send(
            &mut p,
            view::LiquidSendMessage::PopupMessage(SendPopupMessage::Close),
        );
        send(
            &mut p,
            view::LiquidSendMessage::PrepareResponseReceived(ctx, regular_response("lq1a", 11)),
        );

        assert!(
            matches!(p.flow_state, LiquidSendFlowState::Main { .. }),
            "a late prepare must not reopen FinalCheck"
        );
        assert!(p.executable_intent().is_none());

        // Confirm has nothing to execute and must stay silent.
        send(&mut p, view::LiquidSendMessage::ConfirmSend);
        assert!(!p.is_sending);
    }

    /// Two prepares in flight, responses arriving out of order: only the newest
    /// round may enter FinalCheck, whichever lands last.
    #[test]
    fn out_of_order_prepares_let_only_the_newest_through() {
        let mut p = panel();

        compose(&mut p, liquid_input("lq1a"), 1_000);
        let ctx_a = Box::new(p.open_prepare_context());
        compose(&mut p, liquid_input("lq1b"), 2_000);
        let ctx_b = Box::new(p.open_prepare_context());
        assert!(ctx_b.generation > ctx_a.generation);

        // Newest completes first.
        send(
            &mut p,
            view::LiquidSendMessage::PrepareResponseReceived(ctx_b, regular_response("lq1b", 22)),
        );
        assert_eq!(
            p.executable_intent().map(PreparedIntent::amount),
            Some(Amount::from_sat(2_000))
        );

        // The older one lands afterwards and must be discarded.
        send(
            &mut p,
            view::LiquidSendMessage::PrepareResponseReceived(ctx_a, regular_response("lq1a", 11)),
        );
        assert_eq!(
            p.executable_intent().map(PreparedIntent::amount),
            Some(Amount::from_sat(2_000)),
            "the superseded prepare must not replace the current one"
        );
        assert_eq!(
            p.executable_intent().map(PreparedIntent::fees_sat),
            Some(22)
        );
    }

    /// Every exit from the flow drops the prepared intent, for both variants.
    #[test]
    fn reset_and_send_complete_clear_every_prepared_variant() {
        for onchain in [false, true] {
            for reset in [
                view::LiquidSendMessage::SendComplete,
                view::LiquidSendMessage::BackToHome,
                view::LiquidSendMessage::PopupMessage(SendPopupMessage::Close),
            ] {
                let mut p = panel();
                if onchain {
                    compose(&mut p, bitcoin_input("bc1b"), 50_000);
                    let ctx = Box::new(p.open_prepare_context());
                    send(
                        &mut p,
                        view::LiquidSendMessage::PrepareOnChainResponseReceived {
                            context: ctx,
                            address: "bc1b".to_string(),
                            response: onchain_response(400),
                        },
                    );
                } else {
                    compose(&mut p, liquid_input("lq1a"), 1_000);
                    let ctx = Box::new(p.open_prepare_context());
                    send(
                        &mut p,
                        view::LiquidSendMessage::PrepareResponseReceived(
                            ctx,
                            regular_response("lq1a", 11),
                        ),
                    );
                }
                assert!(p.executable_intent().is_some());

                let label = format!("{:?} (onchain={})", reset, onchain);
                send(&mut p, reset);

                assert!(
                    p.prepared_intent().is_none(),
                    "{} must drop the prepared intent",
                    label
                );
                assert!(
                    p.executable_intent().is_none(),
                    "{} must leave nothing executable",
                    label
                );

                // And Confirm afterwards is a no-op, not a re-send.
                send(&mut p, view::LiquidSendMessage::ConfirmSend);
                assert!(!p.is_sending, "{} must not allow a re-send", label);
            }
        }
    }

    /// A superseded intent still on screen fails closed with a message, rather
    /// than executing something the user has not approved in its current form.
    #[test]
    fn confirm_refuses_an_intent_whose_round_has_been_superseded() {
        let mut p = panel();
        compose(&mut p, liquid_input("lq1a"), 1_000);
        let ctx = Box::new(p.open_prepare_context());
        send(
            &mut p,
            view::LiquidSendMessage::PrepareResponseReceived(ctx, regular_response("lq1a", 11)),
        );
        assert!(p.executable_intent().is_some());

        // Something abandons the round while FinalCheck is still displayed.
        p.invalidate_prepare();
        assert!(p.prepared_intent().is_some());
        assert!(p.executable_intent().is_none());

        let emitted = send_emitting(&mut p, view::LiquidSendMessage::ConfirmSend);
        assert!(!p.is_sending, "a superseded intent must not be sent");
        assert!(matches!(p.flow_state, LiquidSendFlowState::Main { .. }));

        // Told through the toast, which is the only channel that reaches the
        // user here: this branch lands on `Modal::None`, and `self.error` is
        // rendered by the AmountInput modal alone. Asserting on `self.error`
        // passed while the user saw nothing at all.
        assert!(
            emitted.iter().any(|m| matches!(
                m,
                Message::View(view::Message::ShowError(msg))
                    if msg.contains("no longer current")
            )),
            "the user must be told to start again, got: {:?}",
            emitted
        );
        assert!(
            p.error.is_none(),
            "a stale banner must not follow the user to the next payment"
        );
    }

    /// Editing the destination invalidates a prepare made for the old one.
    #[test]
    fn editing_the_destination_abandons_a_prepared_payment() {
        let mut p = panel();
        compose(&mut p, liquid_input("lq1a"), 1_000);
        let ctx = Box::new(p.open_prepare_context());
        let generation_before = ctx.generation;

        send(
            &mut p,
            view::LiquidSendMessage::InputEdited("lq1somewhere-else".to_string()),
        );
        assert!(p.prepare_generation > generation_before);

        send(
            &mut p,
            view::LiquidSendMessage::PrepareResponseReceived(ctx, regular_response("lq1a", 11)),
        );
        assert!(
            p.executable_intent().is_none(),
            "a prepare for the previous destination must not become executable"
        );
    }

    /// The fee shown is the prepared payment's own fee, and the asset-fee
    /// branch is preserved: a USDt send quoted in USDt shows no L-BTC fee.
    #[test]
    fn fee_display_follows_the_prepared_payment() {
        let onchain = PreparedIntent::new(
            PrepareContext {
                generation: 1,
                amount: Amount::from_sat(50_000),
                usdt_amount_display: String::new(),
                to_asset: SendAsset::Lbtc,
                from_asset: SendAsset::Lbtc,
                comment: None,
                description: None,
                pay_fees_with_asset: false,
            },
            PreparedPayment::Onchain {
                response: Box::new(onchain_response(400)),
                address: "bc1b".to_string(),
            },
        );
        assert_eq!(onchain.asset_fees(), None);
        assert_eq!(onchain.fees_sat(), 400);
        assert_eq!(onchain.total_sat(), 50_400);

        let mut asset_response = regular_response("lq1a", 11);
        asset_response.estimated_asset_fees = Some(0.5);
        let usdt = PreparedIntent::new(
            PrepareContext {
                generation: 2,
                amount: Amount::from_sat(0),
                usdt_amount_display: "12.34".to_string(),
                to_asset: SendAsset::Usdt,
                from_asset: SendAsset::Usdt,
                comment: None,
                description: None,
                pay_fees_with_asset: true,
            },
            PreparedPayment::Regular {
                response: Box::new(asset_response),
                use_asset_fees: true,
            },
        );
        assert_eq!(usdt.asset_fees(), Some(0.5));
        assert_eq!(usdt.fees_sat(), 0, "fees are paid in USDt, not L-BTC");
        assert_eq!(usdt.usdt_amount_display(), "12.34");
    }

    /// Cross-asset sends never use asset fees (SDK constraint), and the choice
    /// is frozen into the intent rather than recomputed at confirm time from
    /// fields the user may since have changed.
    #[test]
    fn asset_fee_choice_is_bound_at_prepare_time() {
        let mut p = panel();
        p.flow_state = LiquidSendFlowState::Main {
            modal: Modal::AmountInput,
        };
        p.input_type = Some(liquid_input("lq1a"));
        p.to_asset = SendAsset::Usdt;
        p.from_asset = SendAsset::Usdt;
        p.pay_fees_with_asset = true;
        p.usdt_amount_input.value = "12.34".to_string();

        let ctx = Box::new(p.open_prepare_context());
        let mut response = regular_response("lq1a", 11);
        response.estimated_asset_fees = Some(0.5);
        send(
            &mut p,
            view::LiquidSendMessage::PrepareResponseReceived(ctx, response),
        );

        let intent = p.executable_intent().expect("prepared").clone();
        assert!(matches!(
            intent.payment(),
            PreparedPayment::Regular {
                use_asset_fees: true,
                ..
            }
        ));

        // The user flips assets afterwards; the intent is unmoved.
        p.to_asset = SendAsset::Lbtc;
        p.pay_fees_with_asset = false;
        let intent = p.executable_intent().expect("still prepared");
        assert_eq!(intent.to_asset(), SendAsset::Usdt);
        assert_eq!(intent.usdt_amount_display(), "12.34");
        assert!(matches!(
            intent.payment(),
            PreparedPayment::Regular {
                use_asset_fees: true,
                ..
            }
        ));

        // Cross-asset: asset fees are refused regardless of the preference.
        let mut p = panel();
        p.flow_state = LiquidSendFlowState::Main {
            modal: Modal::AmountInput,
        };
        p.input_type = Some(liquid_input("lq1a"));
        p.to_asset = SendAsset::Usdt;
        p.from_asset = SendAsset::Lbtc;
        p.pay_fees_with_asset = true;
        let ctx = Box::new(p.open_prepare_context());
        let mut response = regular_response("lq1a", 11);
        response.estimated_asset_fees = Some(0.5);
        send(
            &mut p,
            view::LiquidSendMessage::PrepareResponseReceived(ctx, response),
        );
        assert!(matches!(
            p.executable_intent().map(PreparedIntent::payment),
            Some(PreparedPayment::Regular {
                use_asset_fees: false,
                ..
            })
        ));

        // And the fee the screen shows follows that refusal. The SDK quoted an
        // asset fee anyway; displaying it here would promise a USDt fee and a
        // zero L-BTC fee for a payment that pays 11 sats in L-BTC.
        let intent = p.executable_intent().expect("prepared");
        assert_eq!(
            intent.asset_fees(),
            None,
            "a cross-asset send does not pay fees in the asset, whatever the SDK quoted"
        );
        assert_eq!(intent.fees_sat(), 11, "the L-BTC fee must still be shown");
    }

    /// The fee-method fallback belongs to the same-asset USDt send and nothing
    /// else. An L-BTC send's prepare response is always `fees_sat: Some` with no
    /// asset fee — the exact shape the fallback reads as "asset fees are
    /// unavailable, switch them to L-BTC" — so before this was scoped, sending
    /// L-BTC cleared a preference the user had set for USDt, and the next USDt
    /// send opened with a fee method they never chose.
    #[test]
    fn an_lbtc_send_leaves_the_usdt_fee_preference_alone() {
        let mut p = panel();
        p.pay_fees_with_asset = true; // chosen earlier, on a USDt send
        compose(&mut p, liquid_input("lq1a"), 50_000);

        let ctx = Box::new(p.open_prepare_context());
        send(
            &mut p,
            view::LiquidSendMessage::PrepareResponseReceived(ctx, regular_response("lq1a", 11)),
        );

        assert!(
            p.pay_fees_with_asset(),
            "an L-BTC send changed the USDt fee preference"
        );
        // And the payment it prepared pays its fee in L-BTC regardless.
        let intent = p.executable_intent().expect("prepared");
        assert_eq!(intent.asset_fees(), None);
        assert_eq!(intent.fees_sat(), 11);
    }

    /// Same asset, USDt, but the user declined asset fees: the SDK still quotes
    /// one, and the payment still executes with `use_asset_fees: false`. The
    /// screen must show the L-BTC fee it is actually going to pay.
    #[test]
    fn declining_asset_fees_shows_the_lbtc_fee() {
        let mut asset_response = regular_response("lq1a", 11);
        asset_response.estimated_asset_fees = Some(0.5);
        let intent = PreparedIntent::new(
            PrepareContext {
                generation: 1,
                amount: Amount::from_sat(0),
                usdt_amount_display: "12.34".to_string(),
                to_asset: SendAsset::Usdt,
                from_asset: SendAsset::Usdt,
                comment: None,
                description: None,
                pay_fees_with_asset: false,
            },
            PreparedPayment::Regular {
                response: Box::new(asset_response),
                use_asset_fees: false,
            },
        );

        assert_eq!(intent.asset_fees(), None);
        assert_eq!(intent.fees_sat(), 11);
        assert_eq!(intent.total_sat(), 11);
    }
}
