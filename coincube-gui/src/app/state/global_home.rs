/// Minimum amount (in sats) for any direction of the Transfer flow.
///
/// Composed with Breez's protocol minimums for swap-involving legs (see
/// `effective_min_sats` in this module for the per-direction rule). Spark and
/// Vault have no SDK-level minimums we need to respect, so for Vault↔Spark
/// this constant *is* the minimum.
pub const MIN_TRANSFER_SATS: u64 = 25_000;

/// Default feerate (sats/vbyte) shown in the Vault-sourced Transfer confirm
/// screen before the user picks a preset or edits the text input. Kept low so
/// that an accidental no-edit signing on regtest / testnet still confirms.
const DEFAULT_TRANSFER_FEERATE_SATS_VB: &str = "2";

/// Sum of external unconfirmed vault coins — the daemon's view of "sats
/// pending incoming" on the Vault. Used both to drive the Vault card's
/// "+ pending" badge and to reconcile the session-level
/// `pending_vault_receive_sats` counter (see `Message::Tick` handler).
fn cache_vault_pending_receive_sats(cache: &Cache) -> u64 {
    cache
        .coins()
        .iter()
        .filter(|coin| coin.spend_info.is_none() && !crate::daemon::model::coin_is_owned(coin))
        .fold(Amount::ZERO, |acc, coin| acc + coin.amount)
        .to_sat()
}

/// Construct a `form::Value<String>` at the default Transfer feerate. Used on
/// `GlobalHome::new`, and on every flow reset so a value (or stuck loading
/// flag) from a prior transfer doesn't leak into the next flow.
fn default_transfer_feerate() -> form::Value<String> {
    form::Value {
        value: DEFAULT_TRANSFER_FEERATE_SATS_VB.to_string(),
        valid: true,
        warning: None,
    }
}

/// Extract the Breez peg-out swap id from a successful `pay_onchain` response.
/// Mirrors `swap_id_for_bitcoin_send` in `app/mod.rs` for the SdkEvent path —
/// here we consume the `Payment` by value since the caller owns the response.
fn bitcoin_send_swap_id(payment: &breez_sdk_liquid::prelude::Payment) -> Option<String> {
    if matches!(payment.payment_type, PaymentType::Send) {
        match &payment.details {
            PaymentDetails::Bitcoin { swap_id, .. } => Some(swap_id.clone()),
            _ => None,
        }
    } else {
        None
    }
}

use std::collections::HashMap;
use std::convert::TryInto;
use std::str::FromStr;
use std::sync::Arc;

use breez_sdk_liquid::model::{
    PayOnchainRequest, PaymentDetails, PaymentType, PreparePayOnchainRequest,
    PreparePayOnchainResponse,
};
use coincube_core::miniscript::bitcoin::{bip32::ChildNumber, Address, Amount};

use crate::app::wallets::{DomainPaymentDetails, DomainPaymentStatus};
use coincube_ui::component::amount::BitcoinDisplayUnit;
use coincube_ui::component::form;
use coincube_ui::widget::*;
use iced::{Subscription, Task};

use super::vault::psbt::SignModal;
use super::{Cache, Menu, State};
use crate::app::state::vault::label::LabelsEdited;
use crate::app::state::vault::receive::ShowQrCodeModal;
use crate::app::view::global_home::{
    GlobalViewConfig, HomeView, PendingTransfer, PickerSide, TransferDirection, TransferStage,
    WalletKind,
};

/// Returns `(effective_min_sat, max_sat_opt)` for a given direction.
///
/// Swap-involving legs compose the Transfer-flow floor with Breez's own
/// minimums; pure on-chain legs (Vault↔Spark) use the floor alone and have no
/// upper bound beyond the source wallet's balance (checked elsewhere).
///
/// If the caller's limits argument is `None` on a swap-involving direction,
/// `effective_min_sat` returns `None` — the amount screen uses this as a
/// "limits still loading" signal.
pub(crate) fn effective_transfer_min_sat(
    direction: TransferDirection,
    onchain_send_limit: Option<(u64, u64)>,
    onchain_receive_limit: Option<(u64, u64)>,
) -> Option<u64> {
    match direction {
        // Liquid source → Breez peg-out. Min composes with Breez's send limit.
        TransferDirection::LiquidToVault | TransferDirection::LiquidToSpark => {
            onchain_send_limit.map(|(min, _)| MIN_TRANSFER_SATS.max(min))
        }
        // Liquid destination → Breez peg-in. Min composes with Breez's receive limit.
        TransferDirection::VaultToLiquid | TransferDirection::SparkToLiquid => {
            onchain_receive_limit.map(|(min, _)| MIN_TRANSFER_SATS.max(min))
        }
        // Pure on-chain: Vault↔Spark. No swap limits apply.
        TransferDirection::VaultToSpark | TransferDirection::SparkToVault => {
            Some(MIN_TRANSFER_SATS)
        }
    }
}

pub(crate) fn effective_transfer_max_sat(
    direction: TransferDirection,
    onchain_send_limit: Option<(u64, u64)>,
    onchain_receive_limit: Option<(u64, u64)>,
) -> Option<u64> {
    match direction {
        TransferDirection::LiquidToVault | TransferDirection::LiquidToSpark => {
            onchain_send_limit.map(|(_, max)| max)
        }
        TransferDirection::VaultToLiquid | TransferDirection::SparkToLiquid => {
            onchain_receive_limit.map(|(_, max)| max)
        }
        // Pure on-chain: cap is the source balance — the caller enforces it.
        TransferDirection::VaultToSpark | TransferDirection::SparkToVault => None,
    }
}
use crate::app::view::shared::feerate_picker::FeeratePreset;
use crate::app::view::HomeMessage;
use crate::app::wallets::{LiquidBackend, SparkBackend};
use crate::app::{message::Message, settings, view, wallet::Wallet};
use crate::daemon::model::{CreateSpendResult, LabelItem, Labelled, SpendTx};
use crate::daemon::Daemon;
use crate::dir::CoincubeDirectory;
use crate::services::feeestimation::fee_estimation::FeeEstimator;

#[derive(Default)]
pub enum Modal {
    ShowQrCode(ShowQrCodeModal),
    Sign(Box<SignModal>),
    #[default]
    None,
}

impl std::fmt::Debug for Modal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShowQrCode(m) => f.debug_tuple("ShowQrCode").field(m).finish(),
            Self::Sign(_) => f.debug_tuple("Sign").field(&"<SignModal>").finish(),
            Self::None => write!(f, "None"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReceiveAddressInfo {
    pub address: Address,
    pub index: ChildNumber,
    pub labels: HashMap<String, String>,
}

impl Labelled for ReceiveAddressInfo {
    fn labelled(&self) -> Vec<LabelItem> {
        vec![LabelItem::Address(self.address.clone())]
    }

    fn labels(&mut self) -> &mut HashMap<String, String> {
        &mut self.labels
    }
}

#[derive(Debug)]
pub struct GlobalHome {
    breez_client: Arc<LiquidBackend>,
    /// Optional Spark backend handle. `None` when the cube has no
    /// Spark signer or the bridge subprocess failed to spawn — the
    /// Home page simply hides the Spark card in that case.
    spark_backend: Option<Arc<SparkBackend>>,
    liquid_balance: Amount,
    /// Spark wallet balance in sats, refreshed by
    /// [`load_balance`]. `Amount::ZERO` while the first
    /// `get_info` RPC is in flight, or forever when no Spark
    /// backend is wired up for this cube.
    spark_balance: Amount,
    usdt_balance: u64,
    usdt_balance_error: bool,
    wallet: Option<Arc<Wallet>>,
    balance_masked: bool,
    /// Cached derivation of `(transfer_from, transfer_to)` via
    /// `TransferDirection::try_from_pair`. `None` when either side is unset or
    /// both sides point at the same wallet.
    transfer_direction: Option<TransferDirection>,
    transfer_from: Option<WalletKind>,
    transfer_to: Option<WalletKind>,
    /// `Some(side)` when the wallet-picker popup is open editing that side.
    wallet_picker: Option<PickerSide>,
    current_view: HomeView,
    entered_amount: form::Value<String>,
    receive_address_info: Option<ReceiveAddressInfo>,
    labels_edited: LabelsEdited,
    address_expanded: bool,
    modal: Modal,
    empty_labels: HashMap<String, String>,
    onchain_send_limit: Option<(u64, u64)>,
    onchain_receive_limit: Option<(u64, u64)>,
    prepare_onchain_send_response: Option<PreparePayOnchainResponse>,
    is_sending: bool,
    transfer_spend_tx: Option<SpendTx>,
    transfer_signed: bool,
    spend_tx_fees: Option<Amount>,
    /// Vault-sourced transfers let the user pick their own sats/vbyte feerate
    /// on the confirm screen (§Design section 9). Kept as a `form::Value` so
    /// parse/validate state mirrors the regular Vault spend flow.
    transfer_feerate: form::Value<String>,
    /// Which Fast/Normal/Slow preset is currently fetching a mempool estimate.
    /// While `Some`, the corresponding button renders non-pressable; the
    /// other two stay clickable so the user can swap targets mid-flight.
    transfer_feerate_loading: Option<crate::app::view::shared::feerate_picker::FeeratePreset>,
    pending_vault_incoming: Option<PendingTransfer>,
    pending_vault_incoming_swap_id: Option<String>,
    /// Session-level counter for non-swap inbound Vault transfers (SparkToVault).
    /// Mirrors `pending_liquid_receive_sats` but for the Vault card. Bumped on
    /// `TransferBroadcast` with a Vault destination and decremented on `Tick`
    /// as the vault daemon observes the incoming on-chain tx (so we don't
    /// double-count what `vault_pending_receive_sats` already surfaces from
    /// `cache.coins()`).
    pending_vault_receive_sats: u64,
    /// Snapshot of the cache-derived `vault_pending_receive_sats` at the moment
    /// we bumped `pending_vault_receive_sats`. Growth above this baseline is
    /// how we recognise our broadcast tx landing in the daemon's view.
    pending_vault_receive_baseline_sats: u64,
    /// Mirror of `pending_vault_incoming` for the Spark card. Set when a
    /// transfer into Spark has broadcast on-chain and the destination deposit
    /// is awaiting maturity + claim. Cleared by `SparkDepositsChanged` when a
    /// matching deposit matures and is claimed — or by `LiquidToVaultFailed`
    /// when the peg-out swap fails before a deposit ever arrives.
    pending_spark_incoming: Option<PendingTransfer>,
    /// Peg-out swap id for an in-flight LiquidToSpark transfer. Lets us
    /// match Breez's async `PaymentFailed` event back to the Spark pending
    /// indicator so a failed swap doesn't leave the badge stuck (no Spark
    /// deposit arrives in that case, so `SparkDepositsChanged` never fires).
    pending_spark_incoming_swap_id: Option<String>,
    /// Set once `SparkDepositsLoaded` has observed a non-empty deposit list
    /// while our LiquidToSpark transfer is pending. Required before we
    /// accept an empty-list signal as "our deposit was claimed" — otherwise
    /// an unrelated `SparkDepositsChanged` firing while our Bitcoin tx is
    /// still in-flight would clear the badge prematurely. Reset alongside
    /// `pending_spark_incoming` so each new transfer starts fresh.
    pending_spark_deposit_seen: bool,
    /// Prepared Spark send handle, populated on step 1→2 for Spark-sourced
    /// transfers (SparkToVault, SparkToLiquid). Consumed by `ConfirmSparkSend`.
    spark_send_handle: Option<String>,
    /// Spark-quoted on-chain fee for the prepared send. Rendered in the Fees
    /// row on the Transfer confirm screen (Spark-sourced directions only).
    spark_send_fee_sat: Option<u64>,
    /// `(txid, vout)` of an auto-claim currently in flight. Prevents the
    /// `SparkDepositsChanged` watcher from re-firing a second `claim_deposit`
    /// for the same deposit while the first one is still in flight.
    auto_claiming_spark_deposit: Option<(String, u32)>,
    pending_liquid_send_sats: u64,
    pending_usdt_send_sats: u64,
    pending_liquid_receive_sats: u64,
    pending_usdt_receive_sats: u64,
    datadir_path: CoincubeDirectory,
    network: coincube_core::miniscript::bitcoin::Network,
    cube_id: String,
}

impl GlobalHome {
    pub fn new(
        wallet: Arc<Wallet>,
        breez_client: Arc<LiquidBackend>,
        spark_backend: Option<Arc<SparkBackend>>,
        datadir_path: CoincubeDirectory,
        network: coincube_core::miniscript::bitcoin::Network,
        cube_id: String,
    ) -> Self {
        Self {
            wallet: Some(wallet),
            liquid_balance: Amount::ZERO,
            spark_balance: Amount::ZERO,
            usdt_balance: 0,
            usdt_balance_error: false,
            breez_client,
            spark_backend,
            balance_masked: false,
            transfer_direction: None,
            transfer_from: None,
            transfer_to: None,
            wallet_picker: None,
            current_view: HomeView::default(),
            entered_amount: form::Value::default(),
            receive_address_info: None,
            labels_edited: LabelsEdited::default(),
            address_expanded: false,
            modal: Modal::default(),
            empty_labels: HashMap::default(),
            onchain_send_limit: None,
            onchain_receive_limit: None,
            prepare_onchain_send_response: None,
            is_sending: false,
            transfer_spend_tx: None,
            transfer_signed: false,
            spend_tx_fees: None,
            transfer_feerate: default_transfer_feerate(),
            transfer_feerate_loading: None,
            pending_vault_incoming: None,
            pending_vault_incoming_swap_id: None,
            pending_vault_receive_sats: 0,
            pending_vault_receive_baseline_sats: 0,
            pending_spark_incoming: None,
            pending_spark_incoming_swap_id: None,
            pending_spark_deposit_seen: false,
            spark_send_handle: None,
            spark_send_fee_sat: None,
            auto_claiming_spark_deposit: None,
            pending_liquid_send_sats: 0,
            pending_usdt_send_sats: 0,
            pending_liquid_receive_sats: 0,
            pending_usdt_receive_sats: 0,
            datadir_path,
            network,
            cube_id,
        }
    }

    pub fn new_without_wallet(
        breez_client: Arc<LiquidBackend>,
        spark_backend: Option<Arc<SparkBackend>>,
        datadir_path: CoincubeDirectory,
        network: coincube_core::miniscript::bitcoin::Network,
        cube_id: String,
    ) -> Self {
        Self {
            wallet: None,
            liquid_balance: Amount::from_sat(0),
            spark_balance: Amount::ZERO,
            usdt_balance: 0,
            usdt_balance_error: false,
            breez_client,
            spark_backend,
            balance_masked: false,
            transfer_direction: None,
            transfer_from: None,
            transfer_to: None,
            wallet_picker: None,
            current_view: HomeView::default(),
            entered_amount: form::Value::default(),
            receive_address_info: None,
            labels_edited: LabelsEdited::default(),
            address_expanded: false,
            modal: Modal::default(),
            empty_labels: HashMap::default(),
            onchain_send_limit: None,
            onchain_receive_limit: None,
            prepare_onchain_send_response: None,
            is_sending: false,
            transfer_spend_tx: None,
            transfer_signed: false,
            spend_tx_fees: None,
            transfer_feerate: default_transfer_feerate(),
            transfer_feerate_loading: None,
            pending_vault_incoming: None,
            pending_vault_incoming_swap_id: None,
            pending_vault_receive_sats: 0,
            pending_vault_receive_baseline_sats: 0,
            pending_spark_incoming: None,
            pending_spark_incoming_swap_id: None,
            pending_spark_deposit_seen: false,
            spark_send_handle: None,
            spark_send_fee_sat: None,
            auto_claiming_spark_deposit: None,
            pending_liquid_send_sats: 0,
            pending_usdt_send_sats: 0,
            pending_liquid_receive_sats: 0,
            pending_usdt_receive_sats: 0,
            datadir_path,
            network,
            cube_id,
        }
    }
}

impl State for GlobalHome {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let vault_balance = cache
            .coins()
            .iter()
            .filter(|coin| coin.spend_info.is_none())
            .fold(Amount::from_sat(0), |acc, coin| acc + coin.amount);

        let cache_vault_pending_receive = cache_vault_pending_receive_sats(cache);

        // Add any remaining non-swap broadcast amount (SparkToVault) that the
        // daemon hasn't observed yet. The subtraction decays the counter as
        // `cache_derived` grows past the broadcast-time baseline so we don't
        // double-count once the tx lands in the daemon's mempool view.
        let still_pending_counter = self.pending_vault_receive_sats.saturating_sub(
            cache_vault_pending_receive.saturating_sub(self.pending_vault_receive_baseline_sats),
        );
        let vault_pending_receive_sats =
            cache_vault_pending_receive.saturating_add(still_pending_counter);

        let vault_pending_send_sats = cache
            .coins()
            .iter()
            .filter(|coin| {
                coin.spend_info
                    .as_ref()
                    .map(|si| si.height.is_none())
                    .unwrap_or(false)
            })
            .fold(Amount::ZERO, |acc, coin| acc + coin.amount)
            .to_sat();

        let liquid_balance = self.liquid_balance;
        let usdt_balance = self.usdt_balance;
        let usdt_balance_error = self.usdt_balance_error;

        // Fiat price is cube-level, not wallet-level, so get it directly from cache
        let fiat_converter: Option<view::FiatAmountConverter> =
            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

        let content = view::dashboard(
            menu,
            cache,
            view::global_home::global_home_view(GlobalViewConfig {
                liquid_balance,
                spark_balance: self.spark_balance,
                usdt_balance,
                usdt_balance_error,
                vault_balance,
                fiat_converter,
                balance_masked: self.balance_masked,
                has_vault: cache.has_vault,
                has_spark: self.spark_backend.is_some(),
                current_view: self.current_view,
                transfer_direction: self.transfer_direction,
                transfer_from: self.transfer_from,
                transfer_to: self.transfer_to,
                wallet_picker: self.wallet_picker,
                entered_amount: &self.entered_amount,
                receive_address: self.receive_address_info.as_ref().map(|info| &info.address),
                receive_index: self.receive_address_info.as_ref().map(|info| &info.index),
                labels: self
                    .receive_address_info
                    .as_ref()
                    .map_or(&self.empty_labels, |info| &info.labels),
                labels_editing: self.labels_edited.cache(),
                address_expanded: self.address_expanded,
                bitcoin_unit: cache.bitcoin_unit,
                onchain_send_limit: self.onchain_send_limit,
                onchain_receive_limit: self.onchain_receive_limit,
                is_sending: self.is_sending,
                is_tx_signed: self.transfer_signed,
                prepare_onchain_send_response: self.prepare_onchain_send_response.as_ref(),
                spend_tx_fees: self.spend_tx_fees,
                transfer_feerate: &self.transfer_feerate,
                transfer_feerate_loading: self.transfer_feerate_loading,
                spark_send_fee_sat: self.spark_send_fee_sat,
                pending_spark_incoming: self.pending_spark_incoming,
                pending_liquid_send_sats: self.pending_liquid_send_sats,
                pending_usdt_send_sats: self.pending_usdt_send_sats,
                pending_liquid_receive_sats: self.pending_liquid_receive_sats,
                pending_usdt_receive_sats: self.pending_usdt_receive_sats,
                vault_pending_send_sats,
                vault_pending_receive_sats,
                pending_vault_incoming: self.pending_vault_incoming,
                btc_usd_price: cache.btc_usd_price,
            }),
        );

        let overlay = match &self.modal {
            Modal::ShowQrCode(m) => m.view(),
            Modal::Sign(sign_modal) => {
                // Delegate to SignModal's view this will render the signing UI
                use crate::app::state::vault::psbt::Modal as PsbtModalTrait;
                if self.transfer_spend_tx.is_some() {
                    return sign_modal.view(content);
                } else {
                    return content;
                }
            }
            Modal::None => return content,
        };

        coincube_ui::widget::modal::Modal::new(content, overlay)
            .on_blur(Some(view::Message::Close))
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = Vec::new();

        if let Modal::Sign(sign_modal) = &self.modal {
            // To fetch hardware wallets
            use crate::app::state::vault::psbt::Modal as PsbtModalTrait;
            subscriptions.push(sign_modal.subscription());
        }

        Subscription::batch(subscriptions)
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Home(msg)) => {
                match msg {
                    HomeMessage::SendAsset(asset) => {
                        use crate::app::menu::LiquidSubMenu;
                        Task::batch(vec![
                            crate::app::state::redirect(Menu::Liquid(LiquidSubMenu::Send)),
                            Task::done(Message::View(view::Message::LiquidSend(
                                view::LiquidSendMessage::PresetAsset(asset),
                            ))),
                        ])
                    }
                    HomeMessage::ReceiveAsset(asset) => {
                        use crate::app::menu::LiquidSubMenu;
                        Task::batch(vec![
                            crate::app::state::redirect(Menu::Liquid(LiquidSubMenu::Receive)),
                            Task::done(Message::View(view::Message::LiquidReceive(
                                view::LiquidReceiveMessage::SetReceiveAsset(asset),
                            ))),
                        ])
                    }
                    HomeMessage::SendSparkBtc => {
                        use crate::app::menu::SparkSubMenu;
                        crate::app::state::redirect(Menu::Spark(SparkSubMenu::Send))
                    }
                    HomeMessage::ReceiveSparkBtc => {
                        use crate::app::menu::SparkSubMenu;
                        crate::app::state::redirect(Menu::Spark(SparkSubMenu::Receive))
                    }
                    HomeMessage::SparkBalanceUpdated(balance) => {
                        self.spark_balance = balance;
                        Task::none()
                    }
                    HomeMessage::ToggleBalanceMask => {
                        self.balance_masked = !self.balance_masked;
                        Task::none()
                    }
                    HomeMessage::NextStep => {
                        if let Some(daemon) = daemon {
                            // Step 0 → 1 (overview → amount entry): this is the Transfer
                            // button press. Default the direction and prefetch Breez's
                            // on-chain swap limits.
                            if self.current_view.step == 0 {
                                // Liquid is always present. Default the destination to
                                // Vault when available, otherwise Spark. `transfer_available`
                                // in the view already gates the button so at least one of
                                // the two non-Liquid wallets exists here.
                                let default_to = if cache.has_vault {
                                    WalletKind::Vault
                                } else if self.spark_backend.is_some() {
                                    WalletKind::Spark
                                } else {
                                    // Button shouldn't be reachable; bail rather than
                                    // advancing the step machine.
                                    return Task::none();
                                };
                                self.transfer_from = Some(WalletKind::Liquid);
                                self.transfer_to = Some(default_to);
                                self.transfer_direction = TransferDirection::try_from_pair(
                                    WalletKind::Liquid,
                                    default_to,
                                );
                                self.wallet_picker = None;
                                self.entered_amount = form::Value::default();
                                // Fresh flow: reset the Vault-sourced feerate
                                // picker so a value (or stuck loading flag)
                                // from a prior transfer doesn't leak in.
                                self.transfer_feerate = default_transfer_feerate();
                                self.transfer_feerate_loading = None;
                                self.current_view.next();
                                let breez_client = self.breez_client.clone();
                                return Task::perform(
                                    async move { breez_client.fetch_onchain_limits().await },
                                    |limit| match limit {
                                        Ok(limits) => Message::View(view::Message::Home(
                                            HomeMessage::OnChainLimitsFetched {
                                                send: (limits.send.min_sat, limits.send.max_sat),
                                                receive: (
                                                    limits.receive.min_sat,
                                                    limits.receive.max_sat,
                                                ),
                                            },
                                        )),
                                        Err(error) => Message::View(view::Message::Home(
                                            HomeMessage::Error(error.to_string()),
                                        )),
                                    },
                                );
                            }
                            // Step 1 → 2 (amount → confirm): fetch the destination
                            // receive address and, for the Liquid-source leg, ask Breez
                            // to prepare a pay_onchain so fees can render on the confirm
                            // screen.
                            if self.current_view.step == 1 {
                                let mut tasks = Vec::new();
                                if matches!(
                                    self.transfer_direction,
                                    Some(TransferDirection::LiquidToVault)
                                ) {
                                    self.current_view.next();
                                    tasks.push(Task::perform(
                                        async move {
                                            match daemon.get_new_address().await {
                                                Ok(res) => Ok((res.address, res.derivation_index)),
                                                Err(e) => Err(e.into()),
                                            }
                                        },
                                        Message::ReceiveAddress,
                                    ));
                                    if let Ok(amount) = Amount::from_str_in(
                                        &self.entered_amount.value,
                                        if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                            breez_sdk_liquid::bitcoin::Denomination::Bitcoin
                                        } else {
                                            breez_sdk_liquid::bitcoin::Denomination::Satoshi
                                        },
                                    ) {
                                        let breez_client = self.breez_client.clone();
                                        tasks.push(Task::perform(
                                            async move {
                                                breez_client
                                                    .prepare_pay_onchain(&PreparePayOnchainRequest {
                                                        fee_rate_sat_per_vbyte: None,
                                                        amount: breez_sdk_liquid::model::PayAmount::Bitcoin { receiver_amount_sat: amount.to_sat() },
                                                    })
                                                    .await
                                            },
                                            move |result| match result {
                                                Ok(response) => Message::View(view::Message::Home(HomeMessage::PrepareOnChainResponseReceived(response))),
                                                Err(error) => Message::View(view::Message::Home(
                                                    HomeMessage::Error(error.to_string()),
                                                )),
                                            },
                                        ))
                                    }
                                } else if matches!(
                                    self.transfer_direction,
                                    Some(TransferDirection::VaultToLiquid)
                                ) {
                                    self.current_view.next();
                                    let breez_client = self.breez_client.clone();
                                    tasks.push(Task::perform(
                                        async move {
                                            let result = breez_client.receive_onchain(None).await;
                                            result
                                        },
                                        |result| match result {
                                            Ok(response) => Message::View(view::Message::Home(
                                                HomeMessage::BreezOnchainAddress(
                                                    response.destination,
                                                ),
                                            )),
                                            Err(error) => Message::View(view::Message::Home(
                                                HomeMessage::Error(error.to_string()),
                                            )),
                                        },
                                    ));
                                } else if matches!(
                                    self.transfer_direction,
                                    Some(TransferDirection::VaultToSpark)
                                ) {
                                    // VaultToSpark: Spark issues a BTC deposit address;
                                    // the Vault signs a standard on-chain tx to that
                                    // address. The sign/broadcast path reuses the
                                    // existing `SignVaultToLiquidTx` handler (widened
                                    // below to accept this direction).
                                    if let Some(spark) = self.spark_backend.clone() {
                                        self.current_view.next();
                                        tasks.push(Task::perform(
                                            async move { spark.receive_onchain(None).await },
                                            |result| match result {
                                                // `payment_request` is a plain Bitcoin address for
                                                // on-chain receives (possibly wrapped in a BIP21
                                                // URI — the existing `BreezOnchainAddress` handler
                                                // already strips "bitcoin:" prefixes).
                                                Ok(response) => Message::View(view::Message::Home(
                                                    HomeMessage::BreezOnchainAddress(
                                                        response.payment_request,
                                                    ),
                                                )),
                                                Err(error) => Message::View(view::Message::Home(
                                                    HomeMessage::Error(error.to_string()),
                                                )),
                                            },
                                        ));
                                    } else {
                                        return Task::done(Message::View(view::Message::Home(
                                            HomeMessage::Error(
                                                "Spark backend unavailable".to_string(),
                                            ),
                                        )));
                                    }
                                } else if matches!(
                                    self.transfer_direction,
                                    Some(TransferDirection::LiquidToSpark)
                                ) {
                                    // LiquidToSpark: structurally identical to
                                    // LiquidToVault. Breez runs a peg-out swap to
                                    // the Spark-issued BTC deposit address.
                                    let Some(spark) = self.spark_backend.clone() else {
                                        return Task::done(Message::View(view::Message::Home(
                                            HomeMessage::Error(
                                                "Spark backend unavailable".to_string(),
                                            ),
                                        )));
                                    };
                                    self.current_view.next();
                                    tasks.push(Task::perform(
                                        async move { spark.receive_onchain(None).await },
                                        |result| match result {
                                            Ok(response) => Message::View(view::Message::Home(
                                                HomeMessage::BreezOnchainAddress(
                                                    response.payment_request,
                                                ),
                                            )),
                                            Err(error) => Message::View(view::Message::Home(
                                                HomeMessage::Error(error.to_string()),
                                            )),
                                        },
                                    ));
                                    if let Ok(amount) = Amount::from_str_in(
                                        &self.entered_amount.value,
                                        if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                            breez_sdk_liquid::bitcoin::Denomination::Bitcoin
                                        } else {
                                            breez_sdk_liquid::bitcoin::Denomination::Satoshi
                                        },
                                    ) {
                                        let breez_client = self.breez_client.clone();
                                        tasks.push(Task::perform(
                                            async move {
                                                breez_client
                                                    .prepare_pay_onchain(&PreparePayOnchainRequest {
                                                        fee_rate_sat_per_vbyte: None,
                                                        amount: breez_sdk_liquid::model::PayAmount::Bitcoin { receiver_amount_sat: amount.to_sat() },
                                                    })
                                                    .await
                                            },
                                            move |result| match result {
                                                Ok(response) => Message::View(view::Message::Home(HomeMessage::PrepareOnChainResponseReceived(response))),
                                                Err(error) => Message::View(view::Message::Home(
                                                    HomeMessage::Error(error.to_string()),
                                                )),
                                            },
                                        ));
                                    }
                                } else if matches!(
                                    self.transfer_direction,
                                    Some(TransferDirection::SparkToLiquid)
                                ) {
                                    // SparkToLiquid: fetch a Breez peg-in BTC address
                                    // and prepare a Spark send against it. Breez will
                                    // credit L-BTC once the on-chain tx confirms.
                                    let Ok(amount) = Amount::from_str_in(
                                        &self.entered_amount.value,
                                        if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                            coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                                        } else {
                                            coincube_core::miniscript::bitcoin::Denomination::Satoshi
                                        },
                                    ) else {
                                        return Task::none();
                                    };
                                    let Some(spark) = self.spark_backend.clone() else {
                                        return Task::done(Message::View(view::Message::Home(
                                            HomeMessage::Error(
                                                "Spark backend unavailable".to_string(),
                                            ),
                                        )));
                                    };
                                    self.current_view.next();
                                    let breez_client = self.breez_client.clone();
                                    let amount_sat = amount.to_sat();
                                    tasks.push(Task::perform(
                                        async move {
                                            let addr_res = breez_client
                                                .receive_onchain(None)
                                                .await
                                                .map_err(|e| {
                                                    format!("Breez receive_onchain failed: {e}")
                                                })?;
                                            let destination = addr_res.destination;
                                            let prep = spark
                                                .prepare_send(destination.clone(), Some(amount_sat))
                                                .await
                                                .map_err(|e| {
                                                    format!("Spark prepare_send failed: {e}")
                                                })?;
                                            Ok::<_, String>((
                                                destination,
                                                prep.handle,
                                                prep.fee_sat,
                                            ))
                                        },
                                        |result| match result {
                                            Ok((destination, prepare_handle, fee_sat)) => {
                                                Message::View(view::Message::Home(
                                                    HomeMessage::SparkPrepareSendReady {
                                                        destination,
                                                        prepare_handle,
                                                        fee_sat,
                                                    },
                                                ))
                                            }
                                            Err(error) => Message::View(view::Message::Home(
                                                HomeMessage::Error(error),
                                            )),
                                        },
                                    ));
                                } else if matches!(
                                    self.transfer_direction,
                                    Some(TransferDirection::SparkToVault)
                                ) {
                                    // SparkToVault: fetch a fresh Vault address, then
                                    // ask Spark to `prepare_send` to it — that call
                                    // yields a handle we broadcast at confirm time via
                                    // `ConfirmSparkSend`.
                                    let Ok(amount) = Amount::from_str_in(
                                        &self.entered_amount.value,
                                        if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                            coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                                        } else {
                                            coincube_core::miniscript::bitcoin::Denomination::Satoshi
                                        },
                                    ) else {
                                        return Task::none();
                                    };
                                    let Some(spark) = self.spark_backend.clone() else {
                                        return Task::done(Message::View(view::Message::Home(
                                            HomeMessage::Error(
                                                "Spark backend unavailable".to_string(),
                                            ),
                                        )));
                                    };
                                    self.current_view.next();
                                    let daemon_clone = daemon.clone();
                                    let amount_sat = amount.to_sat();
                                    tasks.push(Task::perform(
                                        async move {
                                            let addr_res = daemon_clone
                                                .get_new_address()
                                                .await
                                                .map_err(|e| {
                                                    format!("Failed to get Vault address: {e:?}")
                                                })?;
                                            let addr_str = addr_res.address.to_string();
                                            let prep = spark
                                                .prepare_send(addr_str.clone(), Some(amount_sat))
                                                .await
                                                .map_err(|e| {
                                                    format!("Spark prepare_send failed: {e}")
                                                })?;
                                            Ok::<_, String>((addr_str, prep.handle, prep.fee_sat))
                                        },
                                        |result| match result {
                                            Ok((destination, prepare_handle, fee_sat)) => {
                                                Message::View(view::Message::Home(
                                                    HomeMessage::SparkPrepareSendReady {
                                                        destination,
                                                        prepare_handle,
                                                        fee_sat,
                                                    },
                                                ))
                                            }
                                            Err(error) => Message::View(view::Message::Home(
                                                HomeMessage::Error(error),
                                            )),
                                        },
                                    ));
                                }
                                return Task::batch(tasks);
                            }
                            self.current_view.next();
                        }
                        Task::none()
                    }
                    HomeMessage::PreviousStep => {
                        self.current_view.previous();
                        Task::none()
                    }
                    HomeMessage::SparkPrepareSendReady {
                        destination,
                        prepare_handle,
                        fee_sat,
                    } => {
                        // Parse the destination as a checked Bitcoin address — if
                        // the string is a BIP21 URI (rare here but cheap to
                        // handle) strip the prefix first. Mirrors the logic in
                        // `BreezOnchainAddress`. Destination is sourced from our
                        // own trusted components (Breez peg-in / Vault daemon)
                        // and Spark has already prepared against it, so failure
                        // here indicates an upstream bug — refuse to proceed
                        // rather than leave the confirm view without a displayable
                        // destination.
                        let addr_str = destination
                            .strip_prefix("bitcoin:")
                            .unwrap_or(&destination)
                            .split('?')
                            .next()
                            .unwrap_or(&destination);
                        let checked = match Address::from_str(addr_str)
                            .ok()
                            .and_then(|p| p.require_network(cache.network).ok())
                        {
                            Some(a) => a,
                            None => {
                                log::error!(
                                    "Spark destination {addr_str} is not valid for network {:?}",
                                    cache.network
                                );
                                return Task::done(Message::View(view::Message::Home(
                                    HomeMessage::Error(format!(
                                        "Prepared Spark destination is not a valid address for this network: {addr_str}"
                                    )),
                                )));
                            }
                        };
                        self.receive_address_info = Some(ReceiveAddressInfo {
                            address: checked,
                            index: ChildNumber::Normal { index: 0 },
                            labels: HashMap::new(),
                        });
                        self.spark_send_handle = Some(prepare_handle);
                        self.spark_send_fee_sat = Some(fee_sat);
                        Task::none()
                    }
                    HomeMessage::ConfirmSparkSend => {
                        // Spark-sourced confirm: broadcast the prepared send. The
                        // destination address + prepare handle were populated at
                        // step 1→2 (see `SparkPrepareSendReady`).
                        let Some(spark) = self.spark_backend.clone() else {
                            return Task::done(Message::View(view::Message::Home(
                                HomeMessage::Error(
                                    "No prepared Spark send — retry from amount step".to_string(),
                                ),
                            )));
                        };
                        // Single-use handle: take it out of state before
                        // spawning the task so a retry click after a failed
                        // `send_payment` can't re-submit a handle the Spark
                        // backend has already consumed. Forces re-prepare from
                        // the amount step on failure; on success
                        // `TransferBroadcast` is the path that would have
                        // otherwise cleared it.
                        let Some(handle) = self.spark_send_handle.take() else {
                            return Task::done(Message::View(view::Message::Home(
                                HomeMessage::Error(
                                    "No prepared Spark send — retry from amount step".to_string(),
                                ),
                            )));
                        };
                        let Some(direction) = self.transfer_direction else {
                            return Task::none();
                        };
                        let Ok(transfer_amount) = Amount::from_str_in(
                            &self.entered_amount.value,
                            if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                            } else {
                                coincube_core::miniscript::bitcoin::Denomination::Satoshi
                            },
                        ) else {
                            return Task::none();
                        };
                        self.is_sending = true;
                        let amount_sats = transfer_amount.to_sat();
                        let to_kind = direction.to_kind();
                        Task::perform(
                            async move { spark.send_payment(handle).await },
                            move |result| match result {
                                Ok(_response) => Message::View(view::Message::Home(
                                    HomeMessage::TransferBroadcast {
                                        amount_sat: amount_sats,
                                        destination_kind: to_kind,
                                        swap_id: None,
                                    },
                                )),
                                Err(error) => Message::View(view::Message::Home(
                                    HomeMessage::Error(error.to_string()),
                                )),
                            },
                        )
                    }
                    HomeMessage::TransferBroadcast {
                        amount_sat,
                        destination_kind,
                        swap_id,
                    } => {
                        self.is_sending = false;
                        let pending = PendingTransfer {
                            amount: Amount::from_sat(amount_sat),
                            stage: TransferStage::PendingDeposit,
                        };
                        match destination_kind {
                            WalletKind::Vault => {
                                // SparkToVault: there's no swap lifecycle to
                                // drive stage transitions on the
                                // `pending_vault_incoming` indicator, so use
                                // the session-level counter (mirrors
                                // `pending_liquid_receive_sats`). Baseline
                                // snapshots the daemon's current view so the
                                // Tick reconciler can tell "our tx landed"
                                // from growth relative to this moment.
                                self.pending_vault_receive_baseline_sats =
                                    cache_vault_pending_receive_sats(cache);
                                self.pending_vault_receive_sats =
                                    self.pending_vault_receive_sats.saturating_add(amount_sat);
                            }
                            WalletKind::Liquid => {
                                // Liquid peg-in is pending — bump the existing
                                // "incoming peg-in" counter so the Liquid card
                                // reflects it (Phase 6b wires this through
                                // Breez's swap-completed event for clearing).
                                self.pending_liquid_receive_sats =
                                    self.pending_liquid_receive_sats.saturating_add(amount_sat);
                            }
                            WalletKind::Spark => {
                                // Spark destinations also land here on Spark-sourced
                                // flows only when the bridge routes internally — the
                                // normal transfer flow never has Spark as both source
                                // and destination. Treat defensively: still mark the
                                // Spark card as pending.
                                self.pending_spark_incoming = Some(pending);
                                // Only set for LiquidToSpark (Breez peg-out) — the
                                // Vault→Spark and Spark-internal paths don't produce
                                // a Breez swap_id, so we leave the tracker cleared
                                // and rely on `SparkDepositsChanged` for them.
                                self.pending_spark_incoming_swap_id = swap_id;
                                // Fresh transfer — we haven't observed the new
                                // deposit in Spark's unclaimed list yet.
                                self.pending_spark_deposit_seen = false;
                            }
                        }
                        self.current_view.next();
                        self.spark_send_handle = None;
                        Task::none()
                    }
                    HomeMessage::FetchTransferFeeratePreset(preset) => {
                        // Fire a mempool estimate for the requested target.
                        // The corresponding button is gated non-pressable while
                        // `transfer_feerate_loading == Some(preset)`.
                        self.transfer_feerate_loading = Some(preset);
                        Task::perform(
                            async move {
                                let estimator = FeeEstimator::new();
                                match preset {
                                    FeeratePreset::Fast => estimator.get_high_priority_rate().await,
                                    FeeratePreset::Normal => {
                                        estimator.get_mid_priority_rate().await
                                    }
                                    FeeratePreset::Slow => estimator.get_low_priority_rate().await,
                                }
                            },
                            move |result| {
                                Message::View(view::Message::Home(
                                    HomeMessage::TransferFeerateEstimated {
                                        preset,
                                        result: result.map(|r| r as u32).map_err(|e| e.to_string()),
                                    },
                                ))
                            },
                        )
                    }
                    HomeMessage::TransferFeerateEstimated { preset, result } => {
                        // If a different preset is now pending, ignore this stale
                        // response so a late Fast result doesn't clobber a more
                        // recent Slow click.
                        if self.transfer_feerate_loading != Some(preset) {
                            return Task::none();
                        }
                        self.transfer_feerate_loading = None;
                        match result {
                            Ok(sats_per_vb) => {
                                let clamped = sats_per_vb.clamp(1, 1000);
                                self.transfer_feerate = form::Value {
                                    value: clamped.to_string(),
                                    valid: true,
                                    warning: None,
                                };
                                // Re-preview the Vault-source PSBT at the new
                                // feerate (see `vault_transfer_preview_task`).
                                self.spend_tx_fees = None;
                                self.vault_transfer_preview_task(cache, &daemon)
                            }
                            Err(e) => {
                                log::warn!("Fee estimator failed for preset {preset:?}: {e}");
                                Task::done(Message::View(view::Message::ShowError(format!(
                                    "Couldn't fetch mempool fee rate: {e}"
                                ))))
                            }
                        }
                    }
                    HomeMessage::SetTransferFeerate(value) => {
                        let parsed = value.trim().parse::<u64>();
                        let (valid, warning) = match parsed {
                            Ok(v) if v > 0 && v <= 1000 => (true, None),
                            Ok(_) => (false, Some("Feerate must be 1..=1000 sats/vbyte")),
                            Err(_) if value.trim().is_empty() => {
                                (false, Some("Feerate is required"))
                            }
                            Err(_) => (false, Some("Feerate must be an integer")),
                        };
                        self.transfer_feerate = form::Value {
                            value,
                            valid,
                            warning,
                        };
                        // Feerate changed → any prior preview fee is now stale.
                        // Clear it immediately so the confirm screen doesn't
                        // show a number that no longer matches the input, then
                        // re-run the preview. The result handler gates on the
                        // feerate_vb it was dispatched with, so late results
                        // from the old feerate get dropped.
                        self.spend_tx_fees = None;
                        self.vault_transfer_preview_task(cache, &daemon)
                    }
                    HomeMessage::OpenWalletPicker(side) => {
                        self.wallet_picker = Some(side);
                        Task::none()
                    }
                    HomeMessage::CloseWalletPicker => {
                        self.wallet_picker = None;
                        Task::none()
                    }
                    HomeMessage::SelectWalletInPicker(kind) => {
                        // Apply the selection to the side being edited. The TO
                        // popup already filters out the current FROM, but the
                        // FROM popup shows all three — so picking a FROM that
                        // collides with the current TO is possible. Treat that
                        // as a swap: move the previous FROM into the TO slot.
                        // The TO popup's filter means the symmetric case
                        // doesn't occur there.
                        if let Some(side) = self.wallet_picker {
                            match side {
                                PickerSide::From => {
                                    if self.transfer_to == Some(kind) {
                                        self.transfer_to = self.transfer_from;
                                    }
                                    self.transfer_from = Some(kind);
                                }
                                PickerSide::To => self.transfer_to = Some(kind),
                            }
                            if let (Some(from), Some(to)) = (self.transfer_from, self.transfer_to) {
                                self.transfer_direction =
                                    TransferDirection::try_from_pair(from, to);
                            }
                            self.wallet_picker = None;
                            // Re-validate the entered amount against the new
                            // source wallet's balance and the new direction's
                            // min/max limits. Without this, switching from
                            // (e.g.) Liquid to Spark as the source could leave
                            // a now-too-large amount marked valid.
                            self.validate_entered_amount(cache);
                        }
                        Task::none()
                    }
                    HomeMessage::AmountEdited(amount) => {
                        self.entered_amount.value = amount;
                        self.validate_entered_amount(cache);
                        Task::none()
                    }
                    HomeMessage::SignVaultToLiquidTx => {
                        if let Some(transfer_direction) = self.transfer_direction {
                            // Shared signer path for both Vault-sourced directions —
                            // `receive_address_info` is the only thing that differs
                            // between them (populated from Breez for VaultToLiquid,
                            // from Spark's `receive_onchain` for VaultToSpark).
                            if matches!(
                                transfer_direction,
                                TransferDirection::VaultToLiquid | TransferDirection::VaultToSpark
                            ) {
                                if let Some(address_info) = self.receive_address_info.clone() {
                                    if let Some(daemon) = daemon {
                                        let denomination = if matches!(
                                            cache.bitcoin_unit,
                                            crate::app::settings::unit::BitcoinDisplayUnit::BTC
                                        ) {
                                            coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                                        } else {
                                            coincube_core::miniscript::bitcoin::Denomination::Satoshi
                                        };
                                        if let Ok(amount) = Amount::from_str_in(
                                            &self.entered_amount.value,
                                            denomination,
                                        ) {
                                            let amount_sat = amount.to_sat();
                                            let mut destinations = std::collections::HashMap::new();
                                            destinations.insert(
                                                address_info.address.as_unchecked().clone(),
                                                amount_sat,
                                            );

                                            let daemon_clone = daemon.clone();
                                            let wallet = self.wallet.clone();
                                            let cache_clone =
                                                (cache.datadir_path.clone(), cache.network);
                                            // User-picked feerate from the confirm-screen feerate
                                            // input (§Design section 9). Parsing is already gated
                                            // on `transfer_feerate.valid`, so a `parse` failure
                                            // here indicates a bypass — short-circuit instead of
                                            // silently falling back to an estimator.
                                            let feerate_vb = match self
                                                .transfer_feerate
                                                .value
                                                .trim()
                                                .parse::<u64>()
                                            {
                                                Ok(v) if v > 0 => v,
                                                _ => {
                                                    return Task::done(Message::View(
                                                        view::Message::Home(HomeMessage::Error(
                                                            "Invalid feerate".to_string(),
                                                        )),
                                                    ));
                                                }
                                            };
                                            self.is_sending = true;
                                            return Task::perform(
                                                async move {
                                                    let psbt = match daemon_clone
                                                        .create_spend_tx(
                                                            &[],
                                                            &destinations,
                                                            feerate_vb,
                                                            None,
                                                        )
                                                        .await
                                                    {
                                                        Ok(CreateSpendResult::Success {
                                                            psbt,
                                                            ..
                                                        }) => psbt,
                                                        Ok(
                                                            CreateSpendResult::InsufficientFunds {
                                                                missing,
                                                            },
                                                        ) => {
                                                            return Err(format!("Insufficient funds: {} sats missing", missing));
                                                        }
                                                        Err(e) => {
                                                            return Err(format!(
                                                                "Failed to create transaction: {}",
                                                                e
                                                            ));
                                                        }
                                                    };

                                                    daemon_clone
                                                        .update_spend_tx(&psbt)
                                                        .await
                                                        .map_err(|e| {
                                                            format!("Failed to save PSBT: {}", e)
                                                        })?;

                                                    Ok((psbt, wallet, cache_clone))
                                                },
                                                |result| {
                                                    Message::View(view::Message::Home(
                                                        HomeMessage::TransferPsbtReady(result),
                                                    ))
                                                },
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::ConfirmTransfer => {
                        if let Some(transfer_direction) = self.transfer_direction {
                            if matches!(
                                transfer_direction,
                                TransferDirection::LiquidToVault | TransferDirection::LiquidToSpark
                            ) {
                                // Liquid-sourced: direct Breez pay_onchain broadcast.
                                // Destination address was populated at step 1→2 (Vault
                                // BIP-32 for LiquidToVault, Spark BTC deposit for
                                // LiquidToSpark).
                                if let Some(address_info) = self.receive_address_info.clone() {
                                    if let Some(prepare_onchain_send_response) =
                                        self.prepare_onchain_send_response.clone()
                                    {
                                        let Ok(transfer_amount) = Amount::from_str_in(
                                            &self.entered_amount.value,
                                            if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC)
                                            {
                                                breez_sdk_liquid::bitcoin::Denomination::Bitcoin
                                            } else {
                                                breez_sdk_liquid::bitcoin::Denomination::Satoshi
                                            },
                                        ) else {
                                            self.entered_amount.valid = false;
                                            return Task::none();
                                        };
                                        let breez_client = self.breez_client.clone();
                                        self.is_sending = true;
                                        let destination_kind = transfer_direction.to_kind();
                                        return Task::perform(
                                            async move {
                                                breez_client
                                                    .pay_onchain(&PayOnchainRequest {
                                                        address: address_info.address.to_string(),
                                                        prepare_response:
                                                            prepare_onchain_send_response,
                                                    })
                                                    .await
                                            },
                                            move |result| match result {
                                                Ok(response) => {
                                                    // Both LiquidToVault and LiquidToSpark
                                                    // care about the Breez peg-out swap_id
                                                    // (for resume/persistence and for
                                                    // matching the async PaymentFailed back
                                                    // to the right pending indicator).
                                                    let swap_id =
                                                        bitcoin_send_swap_id(&response.payment);
                                                    match destination_kind {
                                                        // LiquidToVault keeps the richer path
                                                        // (swap_id persistence so we can
                                                        // resume a pending peg-out across
                                                        // restarts).
                                                        WalletKind::Vault => Message::View(
                                                            view::Message::Home(
                                                                HomeMessage::LiquidToVaultSubmitted {
                                                                    amount: transfer_amount,
                                                                    swap_id,
                                                                },
                                                            ),
                                                        ),
                                                        // LiquidToSpark: route through the
                                                        // generic success handler, but
                                                        // forward the swap_id so an async
                                                        // PaymentFailed can clear the Spark
                                                        // pending indicator (see
                                                        // `pending_spark_incoming_swap_id`).
                                                        WalletKind::Spark => Message::View(
                                                            view::Message::Home(
                                                                HomeMessage::TransferBroadcast {
                                                                    amount_sat: transfer_amount
                                                                        .to_sat(),
                                                                    destination_kind,
                                                                    swap_id,
                                                                },
                                                            ),
                                                        ),
                                                        // Liquid can't be its own destination.
                                                        WalletKind::Liquid => Message::View(
                                                            view::Message::Home(
                                                                HomeMessage::TransferSuccessful,
                                                            ),
                                                        ),
                                                    }
                                                }
                                                Err(error) => Message::View(view::Message::Home(
                                                    HomeMessage::Error(error.to_string()),
                                                )),
                                            },
                                        );
                                    }
                                }
                            } else if matches!(
                                transfer_direction,
                                TransferDirection::VaultToLiquid | TransferDirection::VaultToSpark
                            ) {
                                // Post-sign broadcast path for both Vault-sourced
                                // directions. The PSBT was built earlier in the same
                                // shared `SignVaultToLiquidTx` handler against the
                                // destination address stored in `receive_address_info`.
                                if self.transfer_signed {
                                    if let Some(spend_tx) = &self.transfer_spend_tx {
                                        if let Some(daemon) = daemon {
                                            let txid = spend_tx.psbt.unsigned_tx.compute_txid();
                                            let daemon_clone = daemon.clone();
                                            let to_kind = transfer_direction.to_kind();
                                            let Ok(amount_sat) = Amount::from_str_in(
                                                &self.entered_amount.value,
                                                if matches!(
                                                    cache.bitcoin_unit,
                                                    BitcoinDisplayUnit::BTC
                                                ) {
                                                    coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                                                } else {
                                                    coincube_core::miniscript::bitcoin::Denomination::Satoshi
                                                },
                                            )
                                            .map(|a| a.to_sat()) else {
                                                // Signed PSBT exists but the amount
                                                // field no longer parses — refuse to
                                                // broadcast rather than emit a
                                                // misleading zero-amount pending card.
                                                self.entered_amount.valid = false;
                                                return Task::done(Message::View(
                                                    view::Message::ShowError(
                                                        "Invalid amount; please re-enter before broadcasting.".to_string(),
                                                    ),
                                                ));
                                            };
                                            self.is_sending = true;

                                            return Task::perform(
                                                async move {
                                                    daemon_clone
                                                        .broadcast_spend_tx(&txid)
                                                        .await
                                                        .map_err(|e| {
                                                            format!("Failed to broadcast: {}", e)
                                                        })
                                                },
                                                move |result| match result {
                                                    Ok(()) => match to_kind {
                                                        // Vault→Liquid: route through
                                                        // `TransferBroadcast` so the Liquid
                                                        // card's pending-receive counter
                                                        // gets bumped at broadcast time.
                                                        // `LiquidPeginCompleted` decrements
                                                        // it once Breez finishes the
                                                        // peg-in swap. The PSBT is paying
                                                        // a Breez peg-in deposit address;
                                                        // we don't track a swap_id here
                                                        // because Breez correlates by
                                                        // tx-output, not by GUI state.
                                                        WalletKind::Liquid => {
                                                            Message::View(view::Message::Home(
                                                                HomeMessage::TransferBroadcast {
                                                                    amount_sat,
                                                                    destination_kind: to_kind,
                                                                    swap_id: None,
                                                                },
                                                            ))
                                                        }
                                                        // Vault→Spark: reuse the Spark-send
                                                        // success handler so both legs land
                                                        // in the same `PendingDeposit` state.
                                                        // No Breez swap is involved — this is
                                                        // a direct Vault PSBT broadcast — so
                                                        // there's no swap_id to track.
                                                        WalletKind::Spark => {
                                                            Message::View(view::Message::Home(
                                                                HomeMessage::TransferBroadcast {
                                                                    amount_sat,
                                                                    destination_kind: to_kind,
                                                                    swap_id: None,
                                                                },
                                                            ))
                                                        }
                                                        // Vault-sourced transfers never have
                                                        // Vault as the destination.
                                                        WalletKind::Vault => {
                                                            Message::View(view::Message::Home(
                                                                HomeMessage::TransferSuccessful,
                                                            ))
                                                        }
                                                    },
                                                    Err(e) => {
                                                        log::error!(
                                                            "Failed to broadcast transfer: {}",
                                                            e
                                                        );
                                                        Message::View(view::Message::Home(
                                                            HomeMessage::Error(e),
                                                        ))
                                                    }
                                                },
                                            );
                                        }
                                    }
                                } else {
                                    return Task::done(Message::View(view::Message::ShowError(
                                        "Please sign the transaction first".to_string(),
                                    )));
                                }
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::Error(err) => {
                        self.is_sending = false;
                        Task::done(Message::View(view::Message::ShowError(err)))
                    }
                    HomeMessage::PendingAmountsUpdated {
                        liquid_send_sats,
                        usdt_send_sats,
                        liquid_receive_sats,
                        usdt_receive_sats,
                    } => {
                        self.pending_liquid_send_sats = liquid_send_sats;
                        self.pending_usdt_send_sats = usdt_send_sats;
                        self.pending_liquid_receive_sats = liquid_receive_sats;
                        self.pending_usdt_receive_sats = usdt_receive_sats;
                        Task::none()
                    }
                    HomeMessage::SparkDepositsChanged => {
                        // The Spark bridge signalled a change to the unclaimed-deposit
                        // list — could be a new deposit appearing, one maturing, or one
                        // being claimed. Re-query the list; the `SparkDepositsLoaded`
                        // handler decides what to do with it.
                        let Some(spark) = self.spark_backend.clone() else {
                            return Task::none();
                        };
                        // Only act if we have a pending inbound Spark transfer to
                        // correlate against. Without one there's nothing for us to
                        // auto-claim or clear — the Spark Receive panel handles the
                        // user-facing pending-deposit card independently.
                        if self.pending_spark_incoming.is_none() {
                            return Task::none();
                        }
                        Task::perform(
                            async move { spark.list_unclaimed_deposits().await },
                            |result| match result {
                                Ok(ok) => Message::View(view::Message::Home(
                                    HomeMessage::SparkDepositsLoaded(ok.deposits),
                                )),
                                Err(e) => {
                                    log::warn!("list_unclaimed_deposits failed: {e:?}");
                                    Message::CacheUpdated
                                }
                            },
                        )
                    }
                    HomeMessage::SparkDepositsLoaded(deposits) => {
                        // No pending transfer → nothing to reconcile.
                        if self.pending_spark_incoming.is_none() {
                            return Task::none();
                        }
                        // Empty list → whatever was pending got claimed (either by
                        // us or by the user from the Spark Receive panel). For
                        // LiquidToSpark (swap_id tracked) we additionally require
                        // that we've previously observed our deposit in the list,
                        // because the Bitcoin peg-out tx doesn't show up until it
                        // lands on-chain: an unrelated `SparkDepositsChanged`
                        // firing during that window would otherwise clear the
                        // badge while the transfer is still in flight. For the
                        // general case (Vault→Spark, Spark-internal) keep the
                        // existing clear-on-empty behavior — a failure path would
                        // never produce an empty list anyway.
                        if deposits.is_empty() {
                            let is_liquid_to_spark = self.pending_spark_incoming_swap_id.is_some();
                            if is_liquid_to_spark && !self.pending_spark_deposit_seen {
                                return Task::none();
                            }
                            self.pending_spark_incoming = None;
                            self.pending_spark_incoming_swap_id = None;
                            self.pending_spark_deposit_seen = false;
                            self.auto_claiming_spark_deposit = None;
                            return Task::none();
                        }
                        // We've now observed at least one deposit in the list
                        // while our transfer is pending — remember this so a
                        // later empty list can be interpreted as "claimed"
                        // rather than "never arrived" (see above).
                        self.pending_spark_deposit_seen = true;
                        // One auto-claim at a time: if a claim is in flight,
                        // wait for its `DepositsChanged` follow-up before
                        // picking the next candidate. This covers both the
                        // "same deposit re-firing" and "different mature
                        // deposit arrived" cases in one check.
                        if self.auto_claiming_spark_deposit.is_some() {
                            return Task::none();
                        }
                        // Pick the first mature, error-free deposit and
                        // auto-claim it. The bridge fires `DepositsChanged`
                        // again on success, which re-enters this handler and
                        // clears the indicator.
                        let Some(candidate) = deposits
                            .iter()
                            .find(|d| d.is_mature && d.claim_error.is_none())
                        else {
                            return Task::none();
                        };
                        let Some(spark) = self.spark_backend.clone() else {
                            return Task::none();
                        };
                        let txid = candidate.txid.clone();
                        let vout = candidate.vout;
                        self.auto_claiming_spark_deposit = Some((txid.clone(), vout));
                        let txid_for_msg = txid.clone();
                        Task::perform(
                            async move { spark.claim_deposit(txid, vout).await },
                            move |result| {
                                Message::View(view::Message::Home(
                                    HomeMessage::AutoClaimSparkResult {
                                        txid: txid_for_msg.clone(),
                                        vout,
                                        result: match result {
                                            Ok(ok) => Ok(ok.amount_sat),
                                            Err(e) => Err(e.to_string()),
                                        },
                                    },
                                ))
                            },
                        )
                    }
                    HomeMessage::AutoClaimSparkResult {
                        txid: _,
                        vout: _,
                        result,
                    } => {
                        // Success: mark the indicator Completed so the view hides
                        // it immediately and `BackToHome` can reap it symmetric to
                        // `pending_vault_incoming`. The bridge's follow-up
                        // `DepositsChanged` will still re-enter the watcher and
                        // clear the field outright. Failure: log and surface so
                        // the user can retry from the Receive panel.
                        self.auto_claiming_spark_deposit = None;
                        match result {
                            Ok(_amount) => {
                                if let Some(mut pending) = self.pending_spark_incoming {
                                    pending.stage = TransferStage::Completed;
                                    self.pending_spark_incoming = Some(pending);
                                }
                                Task::none()
                            }
                            Err(e) => {
                                log::warn!("Auto-claim of Spark deposit failed: {e}");
                                Task::none()
                            }
                        }
                    }
                    HomeMessage::LiquidPeginCompleted { amount_sat } => {
                        // Decrement the pending-receive counter instantly so the
                        // Liquid card drops its "pending" badge without waiting for
                        // the next `load_pending_sends` sync. Then re-run the sync
                        // for full self-healing — if the counter was already too
                        // low (e.g. multiple peg-ins) the SDK-derived refresh
                        // corrects it.
                        self.pending_liquid_receive_sats =
                            self.pending_liquid_receive_sats.saturating_sub(amount_sat);
                        self.load_pending_sends()
                    }
                    HomeMessage::LiquidBalanceUpdated(liquid_balance) => {
                        self.liquid_balance = liquid_balance;
                        Task::none()
                    }
                    HomeMessage::UsdtBalanceUpdated(usdt_balance) => {
                        self.usdt_balance = usdt_balance;
                        self.usdt_balance_error = false;
                        Task::none()
                    }
                    HomeMessage::UsdtBalanceFetchFailed => {
                        self.usdt_balance_error = true;
                        Task::none()
                    }
                    HomeMessage::OnChainLimitsFetched { send, receive } => {
                        self.onchain_send_limit = Some(send);
                        self.onchain_receive_limit = Some(receive);
                        // Re-validate: if the user entered an amount before limits
                        // resolved, `validate_entered_amount` parked it with
                        // "Loading limits…" / valid=false. Now that the min/max
                        // are available, re-check so the Next button unlocks
                        // without requiring the user to retype.
                        self.validate_entered_amount(cache);
                        Task::none()
                    }
                    HomeMessage::PrepareOnChainResponseReceived(response) => {
                        self.prepare_onchain_send_response = Some(response);
                        Task::none()
                    }
                    HomeMessage::TransferSuccessful => {
                        self.current_view.next();
                        self.is_sending = false;
                        Task::none()
                    }
                    HomeMessage::LiquidToVaultSubmitted { amount, swap_id } => {
                        self.current_view.next();
                        self.is_sending = false;
                        self.pending_vault_incoming = Some(PendingTransfer {
                            amount,
                            stage: TransferStage::Initiated,
                        });
                        self.pending_vault_incoming_swap_id = swap_id.clone();
                        if let Some(swap_id) = swap_id {
                            return self.persist_pending_liquid_to_vault_transfer(
                                swap_id,
                                amount.to_sat(),
                            );
                        }
                        Task::none()
                    }
                    HomeMessage::LiquidToVaultPending(swap_id) => {
                        if self.is_matching_pending_swap(swap_id.as_deref()) {
                            if let Some(mut pending) = self.pending_vault_incoming {
                                pending.stage = TransferStage::SwappingLbtcToBtc;
                                self.pending_vault_incoming = Some(pending);
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::LiquidToVaultWaitingConfirmation(swap_id) => {
                        if self.is_matching_pending_swap(swap_id.as_deref()) {
                            if let Some(mut pending) = self.pending_vault_incoming {
                                pending.stage = TransferStage::SendingToVault;
                                self.pending_vault_incoming = Some(pending);
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::LiquidToVaultSucceeded(swap_id) => {
                        if self.is_matching_pending_swap(swap_id.as_deref()) {
                            if let Some(mut pending) = self.pending_vault_incoming {
                                pending.stage = TransferStage::Completed;
                                self.pending_vault_incoming = Some(pending);
                            }
                            self.pending_vault_incoming_swap_id = None;
                            return self.clear_pending_liquid_to_vault_transfer();
                        }
                        if self.is_matching_pending_spark_swap(swap_id.as_deref()) {
                            // Peg-out landed on-chain — leave the Spark pending
                            // badge in place (`SparkDepositsChanged` clears it
                            // once the deposit matures), but release the swap_id
                            // tracker so a future unrelated `PaymentFailed`
                            // can't accidentally match and clear it.
                            self.pending_spark_incoming_swap_id = None;
                        }
                        Task::none()
                    }
                    HomeMessage::LiquidToVaultFailed(swap_id) => {
                        // Dispatched from Breez `PaymentFailed` for any Liquid
                        // peg-out, so it covers both LiquidToVault and
                        // LiquidToSpark. Route to whichever pending indicator
                        // owns this swap_id — for LiquidToSpark there's no
                        // deposit to ever arrive, so `SparkDepositsChanged`
                        // won't clear the badge on our behalf.
                        if self.is_matching_pending_swap(swap_id.as_deref()) {
                            self.pending_vault_incoming = None;
                            self.pending_vault_incoming_swap_id = None;
                            return Task::batch(vec![
                                self.clear_pending_liquid_to_vault_transfer(),
                                Task::done(Message::View(view::Message::ShowError(
                                    "Liquid to Vault transfer failed. Please retry.".to_string(),
                                ))),
                            ]);
                        }
                        if self.is_matching_pending_spark_swap(swap_id.as_deref()) {
                            self.pending_spark_incoming = None;
                            self.pending_spark_incoming_swap_id = None;
                            self.pending_spark_deposit_seen = false;
                            return Task::done(Message::View(view::Message::ShowError(
                                "Liquid to Spark transfer failed. Please retry.".to_string(),
                            )));
                        }
                        Task::none()
                    }
                    HomeMessage::PendingTransferRestored {
                        amount_sat,
                        stage,
                        swap_id,
                    } => {
                        self.pending_vault_incoming = Some(PendingTransfer {
                            amount: Amount::from_sat(amount_sat),
                            stage,
                        });
                        self.pending_vault_incoming_swap_id = Some(swap_id);
                        Task::none()
                    }
                    HomeMessage::BackToHome => {
                        self.current_view.reset();
                        self.transfer_direction = None;
                        self.transfer_from = None;
                        self.transfer_to = None;
                        self.wallet_picker = None;
                        self.entered_amount = form::Value::default();
                        self.receive_address_info = None;
                        self.onchain_send_limit = None;
                        self.onchain_receive_limit = None;
                        self.prepare_onchain_send_response = None;
                        self.is_sending = false;
                        self.transfer_spend_tx = None;
                        self.transfer_signed = false;
                        self.spark_send_handle = None;
                        self.spark_send_fee_sat = None;
                        self.spend_tx_fees = None;
                        self.transfer_feerate = default_transfer_feerate();
                        self.transfer_feerate_loading = None;
                        if self
                            .pending_vault_incoming
                            .map(|pending| pending.stage == TransferStage::Completed)
                            .unwrap_or(false)
                        {
                            self.pending_vault_incoming = None;
                            self.pending_vault_incoming_swap_id = None;
                            return self.clear_pending_liquid_to_vault_transfer();
                        }
                        if self
                            .pending_spark_incoming
                            .map(|pending| pending.stage == TransferStage::Completed)
                            .unwrap_or(false)
                        {
                            self.pending_spark_incoming = None;
                            self.pending_spark_incoming_swap_id = None;
                            self.pending_spark_deposit_seen = false;
                        }
                        Task::none()
                    }
                    HomeMessage::BreezOnchainAddress(address) => {
                        // Parse BIP-21 URI format: bitcoin:address?params or plain address
                        let addr_str = address
                            .strip_prefix("bitcoin:")
                            .unwrap_or(&address)
                            .split('?')
                            .next()
                            .unwrap_or(&address);

                        if let Ok(parsed) = Address::from_str(addr_str) {
                            let network = cache.network;
                            match parsed.require_network(network) {
                                Ok(checked_address) => {
                                    self.receive_address_info = Some(ReceiveAddressInfo {
                                        address: checked_address,
                                        index: ChildNumber::Normal { index: 1 },
                                        labels: HashMap::new(),
                                    });
                                    // For Vault-sourced directions the Fees/Total
                                    // rows can't be populated until we can call
                                    // `create_spend_tx` — which needs this address.
                                    // Kick off the dry-run preview now that it's in
                                    // place. No-op for other directions.
                                    return self.vault_transfer_preview_task(cache, &daemon);
                                }
                                Err(_) => {
                                    log::error!(
                                        "Address {} is not valid for network {:?}",
                                        addr_str,
                                        network
                                    );
                                }
                            }
                        } else {
                            log::error!("Failed to parse Breez on-chain address: {}", addr_str);
                        }
                        Task::none()
                    }
                    HomeMessage::RefreshLiquidBalance => self.load_liquid_balance(),
                    HomeMessage::TransferPsbtReady(result) => {
                        self.is_sending = false;
                        match result {
                            Ok((psbt, wallet, (datadir_path, network))) => {
                                if let Some(wallet) = wallet {
                                    let sigs =
                                        match wallet.main_descriptor.partial_spend_info(&psbt) {
                                            Ok(info) => info,
                                            Err(e) => {
                                                let err_msg =
                                                    format!("Failed to get signature info: {}", e);
                                                return Task::done(Message::View(
                                                    view::Message::ShowError(err_msg),
                                                ));
                                            }
                                        };

                                    let spend_amount =
                                        psbt.unsigned_tx.output.iter().map(|out| out.value).sum();

                                    // Use primary path if no inputs are using a relative locktime
                                    let use_primary_path = !psbt
                                        .unsigned_tx
                                        .input
                                        .iter()
                                        .map(|txin| txin.sequence)
                                        .any(|seq| seq.is_relative_lock_time());
                                    let max_vbytes = wallet.main_descriptor.unsigned_tx_max_vbytes(
                                        &psbt.unsigned_tx,
                                        use_primary_path,
                                    );
                                    let fees = psbt.fee().expect("Fees should be present");
                                    self.spend_tx_fees = Some(fees);

                                    // Create minimal SpendTx
                                    let spend_tx = SpendTx {
                                        network,
                                        psbt: psbt.clone(),
                                        coins: std::collections::HashMap::new(), // Empty for transfer
                                        labels: HashMap::new(),
                                        sigs,
                                        change_indexes: vec![],
                                        spend_amount,
                                        fee_amount: None,
                                        max_vbytes,
                                        status: crate::daemon::model::SpendStatus::Pending,
                                        updated_at: Some(
                                            std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap()
                                                .as_secs()
                                                as u32,
                                        ),
                                        kind: crate::daemon::model::TransactionKind::SendToSelf,
                                    };

                                    self.transfer_spend_tx = Some(spend_tx);

                                    // Create the SignModal
                                    let sign_modal = SignModal::new(
                                        std::collections::HashSet::new(),
                                        wallet,
                                        datadir_path,
                                        network,
                                        true,
                                        None,
                                    );

                                    self.modal = Modal::Sign(Box::new(sign_modal));
                                } else {
                                    return Task::done(Message::View(view::Message::ShowError(
                                        "Wallet not available".to_string(),
                                    )));
                                }
                            }
                            Err(e) => {
                                return Task::done(Message::View(view::Message::ShowError(e)));
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::TransferSigningComplete => {
                        self.transfer_signed = true;
                        self.modal = Modal::None;
                        self.is_sending = false;
                        Task::none()
                    }
                    HomeMessage::TransferPsbtPreviewReady { feerate_vb, result } => {
                        // Drop late results: if the user has since edited the
                        // feerate, `self.transfer_feerate.value` won't parse to
                        // the feerate this preview was built against. Keeping
                        // only the latest-feerate result prevents the Fees row
                        // flickering to out-of-order values during fast typing.
                        let current_feerate =
                            self.transfer_feerate.value.trim().parse::<u64>().ok();
                        if current_feerate != Some(feerate_vb) {
                            return Task::none();
                        }
                        match result {
                            Ok(fees) => {
                                self.spend_tx_fees = Some(fees);
                            }
                            Err(e) => {
                                // Quiet failure: the user sees blank Fees/Total,
                                // and the real `SignVaultToLiquidTx` path will
                                // surface any error (insufficient funds etc.)
                                // if they click through. Logging only.
                                log::debug!(
                                    "Vault transfer PSBT preview failed at feerate {feerate_vb}: {e}"
                                );
                                self.spend_tx_fees = None;
                            }
                        }
                        Task::none()
                    }
                }
            }
            Message::ReceiveAddress(res) => match res {
                Ok((address, index)) => {
                    self.receive_address_info = Some(ReceiveAddressInfo {
                        address,
                        index,
                        labels: HashMap::new(),
                    });
                    Task::none()
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    self.receive_address_info = None;
                    Task::done(Message::View(view::Message::ShowError(err_msg)))
                }
            },
            Message::View(view::Message::SelectAddress(_addr)) => {
                self.address_expanded = !self.address_expanded;
                Task::none()
            }
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                if let Some(daemon) = daemon {
                    match self.labels_edited.update(
                        daemon,
                        message,
                        self.receive_address_info
                            .iter_mut()
                            .map(|info| info as &mut dyn crate::daemon::model::LabelsLoader),
                    ) {
                        Ok(cmd) => cmd,
                        Err(e) => {
                            let err_msg = e.to_string();
                            Task::done(Message::View(view::Message::ShowError(err_msg)))
                        }
                    }
                } else {
                    Task::none()
                }
            }
            Message::View(view::Message::ShowQrCode(_)) => {
                if let Some(info) = &self.receive_address_info {
                    if let Some(modal) = ShowQrCodeModal::new(&info.address, info.index) {
                        self.modal = Modal::ShowQrCode(modal);
                    }
                }
                Task::none()
            }
            Message::View(view::Message::Close) => {
                self.modal = Modal::None;
                self.transfer_spend_tx = None;
                Task::none()
            }
            Message::Updated(_) => {
                if let (Modal::Sign(ref mut sign_modal), Some(daemon)) = (&mut self.modal, daemon) {
                    if let Some(ref mut spend_tx) = self.transfer_spend_tx {
                        use crate::app::state::vault::psbt::Modal as PsbtModalTrait;
                        let task = sign_modal.update(daemon, message, spend_tx);

                        return Task::batch(vec![
                            task,
                            Task::perform(async {}, |_| {
                                Message::View(view::Message::Home(
                                    HomeMessage::TransferSigningComplete,
                                ))
                            }),
                        ]);
                    }
                }
                Task::none()
            }
            Message::Signed(_, _)
            | Message::HardwareWallets(_)
            | Message::View(view::Message::SelectHardwareWallet(_))
            | Message::View(view::Message::Spend(_)) => {
                if let (Modal::Sign(ref mut sign_modal), Some(daemon)) = (&mut self.modal, daemon) {
                    if let Some(ref mut spend_tx) = self.transfer_spend_tx {
                        use crate::app::state::vault::psbt::Modal as PsbtModalTrait;
                        return sign_modal.update(daemon, message, spend_tx);
                    }
                }
                Task::none()
            }
            Message::Tick => {
                // Reconcile `pending_vault_receive_sats` against the daemon's
                // view. Once cache-derived pending has grown by at least the
                // broadcast amount relative to our baseline, the tx has landed
                // and we latch the counter to zero so a later confirmation
                // (which drops the coin out of the "external unconfirmed" set)
                // doesn't resurrect the stale indicator.
                if self.pending_vault_receive_sats > 0 {
                    let current = cache_vault_pending_receive_sats(cache);
                    if current
                        >= self
                            .pending_vault_receive_baseline_sats
                            .saturating_add(self.pending_vault_receive_sats)
                    {
                        self.pending_vault_receive_sats = 0;
                        self.pending_vault_receive_baseline_sats = 0;
                    } else if current < self.pending_vault_receive_baseline_sats {
                        // Cache shrunk below the snapshot (e.g. a prior external
                        // unconfirmed coin confirmed). Rebase so growth is
                        // measured from the new floor.
                        self.pending_vault_receive_baseline_sats = current;
                    }
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.wallet = wallet;
        let mut tasks = vec![
            self.load_liquid_balance(),
            self.load_usdt_balance(),
            self.load_pending_sends(),
            self.restore_pending_liquid_to_vault_transfer(),
        ];
        if let Some(spark) = self.load_spark_balance() {
            tasks.push(spark);
        }
        Task::batch(tasks)
    }
}

impl GlobalHome {
    /// Re-parse `entered_amount.value` against the current `transfer_direction`
    /// and the source wallet's balance. Mutates `entered_amount.valid` and
    /// `entered_amount.warning`. Called from `AmountEdited` (obvious) and
    /// `SelectWalletInPicker` (after the direction changes — otherwise a
    /// stale `valid=true` would leave the Next button enabled on an amount
    /// that now exceeds the new source balance or the new direction's limits).
    fn validate_entered_amount(&mut self, cache: &Cache) {
        let amount_str = self.entered_amount.value.clone();
        if amount_str.is_empty() {
            self.entered_amount.valid = true;
            self.entered_amount.warning = None;
            return;
        }
        let denomination = if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
            coincube_core::miniscript::bitcoin::Denomination::Bitcoin
        } else {
            coincube_core::miniscript::bitcoin::Denomination::Satoshi
        };
        let Ok(entered_amt) = Amount::from_str_in(&amount_str, denomination) else {
            self.entered_amount.valid = false;
            self.entered_amount.warning = Some("Invalid amount format");
            return;
        };
        let entered_sat = entered_amt.to_sat();
        let mut valid = true;
        let mut warning = None;

        let vault_balance = cache
            .coins()
            .iter()
            .filter(|coin| coin.spend_info.is_none())
            .fold(Amount::from_sat(0), |acc, coin| acc + coin.amount);

        if let Some(direction) = self.transfer_direction {
            let source_cap = match direction.from_kind() {
                WalletKind::Liquid => self.liquid_balance,
                WalletKind::Spark => self.spark_balance,
                WalletKind::Vault => vault_balance,
            };
            if entered_amt > source_cap {
                valid = false;
                warning = Some(match direction.from_kind() {
                    WalletKind::Liquid => "Amount exceeds Liquid balance",
                    WalletKind::Spark => "Amount exceeds Spark balance",
                    WalletKind::Vault => "Amount exceeds Vault balance",
                });
            } else {
                let effective_min = effective_transfer_min_sat(
                    direction,
                    self.onchain_send_limit,
                    self.onchain_receive_limit,
                );
                let effective_max = effective_transfer_max_sat(
                    direction,
                    self.onchain_send_limit,
                    self.onchain_receive_limit,
                );
                if let Some(min_sat) = effective_min {
                    if entered_sat < min_sat {
                        valid = false;
                        warning = Some("Amount below minimum limits");
                    } else if let Some(max_sat) = effective_max {
                        if entered_sat > max_sat {
                            valid = false;
                            warning = Some("Amount above maximum limits");
                        }
                    }
                } else {
                    // Swap-involving direction and Breez limits haven't resolved
                    // yet; keep the input invalid until they do (Next disabled).
                    valid = false;
                    warning = Some("Loading limits…");
                }
            }
        }

        self.entered_amount.valid = valid;
        self.entered_amount.warning = warning;
    }

    fn is_matching_pending_swap(&self, incoming_swap_id: Option<&str>) -> bool {
        match (
            &self.pending_vault_incoming,
            &self.pending_vault_incoming_swap_id,
        ) {
            (Some(_), Some(expected_swap_id)) => incoming_swap_id
                .map(|swap_id| swap_id == expected_swap_id)
                .unwrap_or(false),
            (Some(_), None) => false,
            _ => false,
        }
    }

    fn is_matching_pending_spark_swap(&self, incoming_swap_id: Option<&str>) -> bool {
        match (
            &self.pending_spark_incoming,
            &self.pending_spark_incoming_swap_id,
        ) {
            (Some(_), Some(expected_swap_id)) => incoming_swap_id
                .map(|swap_id| swap_id == expected_swap_id)
                .unwrap_or(false),
            _ => false,
        }
    }

    /// Build a dry-run PSBT for Vault-sourced transfers to populate the
    /// Fees/Total rows on the confirm screen pre-sign. Liquid-sourced
    /// directions get this via `prepare_pay_onchain`; Vault-sourced needs a
    /// real `create_spend_tx` because the fee depends on coin selection
    /// + the user-picked feerate. Skipped unless direction is VaultToLiquid
    /// or VaultToSpark AND all required inputs (address, feerate, amount)
    /// are populated and valid. The result carries its source feerate so
    /// the handler can discard late-arriving results whose feerate has
    /// since been edited (prevents keystroke-race flicker).
    fn vault_transfer_preview_task(
        &self,
        cache: &Cache,
        daemon: &Option<Arc<dyn Daemon + Sync + Send>>,
    ) -> Task<Message> {
        let Some(direction) = self.transfer_direction else {
            return Task::none();
        };
        if !matches!(
            direction,
            TransferDirection::VaultToLiquid | TransferDirection::VaultToSpark
        ) {
            return Task::none();
        }
        let Some(address_info) = self.receive_address_info.clone() else {
            return Task::none();
        };
        let Some(daemon) = daemon.clone() else {
            return Task::none();
        };
        if !self.transfer_feerate.valid {
            return Task::none();
        }
        let Ok(feerate_vb) = self.transfer_feerate.value.trim().parse::<u64>() else {
            return Task::none();
        };
        if feerate_vb == 0 {
            return Task::none();
        }
        let denomination = if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
            coincube_core::miniscript::bitcoin::Denomination::Bitcoin
        } else {
            coincube_core::miniscript::bitcoin::Denomination::Satoshi
        };
        let Ok(amount) = Amount::from_str_in(&self.entered_amount.value, denomination) else {
            return Task::none();
        };
        if !self.entered_amount.valid {
            return Task::none();
        }
        let amount_sat = amount.to_sat();
        let mut destinations = HashMap::new();
        destinations.insert(address_info.address.as_unchecked().clone(), amount_sat);
        Task::perform(
            async move {
                match daemon
                    .create_spend_tx(&[], &destinations, feerate_vb, None)
                    .await
                {
                    Ok(CreateSpendResult::Success { psbt, .. }) => {
                        psbt.fee().map_err(|e| e.to_string())
                    }
                    Ok(CreateSpendResult::InsufficientFunds { missing }) => {
                        Err(format!("Insufficient funds: {missing} sats missing"))
                    }
                    Err(e) => Err(format!("Preview failed: {e}")),
                }
            },
            move |result| {
                Message::View(view::Message::Home(HomeMessage::TransferPsbtPreviewReady {
                    feerate_vb,
                    result,
                }))
            },
        )
    }

    /// Fetch the Spark wallet balance via `get_info` on the bridge.
    /// Returns `None` when no Spark backend is wired up for the cube
    /// so the caller can skip scheduling the task entirely.
    fn load_spark_balance(&self) -> Option<Task<Message>> {
        let backend = self.spark_backend.clone()?;
        Some(Task::perform(
            async move { backend.get_info().await },
            |result| match result {
                Ok(info) => Message::View(view::Message::Home(HomeMessage::SparkBalanceUpdated(
                    Amount::from_sat(info.balance_sats),
                ))),
                Err(e) => {
                    tracing::warn!("Home: spark get_info failed: {}", e);
                    // Soft-fail: leave the card showing whatever the
                    // last successful fetch returned by not emitting a
                    // balance update at all. CacheUpdated is a no-op
                    // ping that doesn't touch the Spark balance.
                    Message::CacheUpdated
                }
            },
        ))
    }

    fn load_liquid_balance(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();
        Task::perform(async move { breez_client.info().await }, |info| {
            if let Ok(info) = info {
                let balance = Amount::from_sat(
                    info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat,
                );
                Message::View(view::Message::Home(HomeMessage::LiquidBalanceUpdated(
                    balance,
                )))
            } else {
                Message::View(view::Message::Home(HomeMessage::Error(
                    "Couldn't fetch Liquid Wallet Balance".to_string(),
                )))
            }
        })
    }

    fn load_pending_sends(&self) -> Task<Message> {
        use crate::app::breez_liquid::assets::{asset_kind_for_id, AssetKind};
        let breez_client = self.breez_client.clone();
        let network = self.network;
        Task::perform(
            async move {
                match breez_client.list_payments(Some(20)).await {
                    Ok(payments) => {
                        let mut liquid_send_sats: u64 = 0;
                        let mut usdt_send_sats: u64 = 0;
                        let mut liquid_receive_sats: u64 = 0;
                        let mut usdt_receive_sats: u64 = 0;
                        for payment in &payments {
                            if !matches!(payment.status, DomainPaymentStatus::Pending) {
                                continue;
                            }
                            let is_send = !payment.is_incoming();
                            match &payment.details {
                                DomainPaymentDetails::LiquidAsset {
                                    asset_id,
                                    asset_info,
                                    ..
                                } => {
                                    if asset_kind_for_id(asset_id, network) == Some(AssetKind::Usdt)
                                    {
                                        let minor = asset_info
                                            .as_ref()
                                            .map(|ai| ai.amount_minor)
                                            .unwrap_or(payment.amount_sat);
                                        if is_send {
                                            usdt_send_sats = usdt_send_sats.saturating_add(minor);
                                        } else {
                                            usdt_receive_sats =
                                                usdt_receive_sats.saturating_add(minor);
                                        }
                                    } else if is_send {
                                        liquid_send_sats = liquid_send_sats
                                            .saturating_add(payment.amount_sat + payment.fees_sat);
                                    } else {
                                        liquid_receive_sats =
                                            liquid_receive_sats.saturating_add(payment.amount_sat);
                                    }
                                }
                                _ => {
                                    if is_send {
                                        liquid_send_sats = liquid_send_sats
                                            .saturating_add(payment.amount_sat + payment.fees_sat);
                                    } else {
                                        liquid_receive_sats =
                                            liquid_receive_sats.saturating_add(payment.amount_sat);
                                    }
                                }
                            }
                        }
                        (
                            liquid_send_sats,
                            usdt_send_sats,
                            liquid_receive_sats,
                            usdt_receive_sats,
                        )
                    }
                    Err(_) => (0, 0, 0, 0),
                }
            },
            |(liquid_send_sats, usdt_send_sats, liquid_receive_sats, usdt_receive_sats)| {
                Message::View(view::Message::Home(HomeMessage::PendingAmountsUpdated {
                    liquid_send_sats,
                    usdt_send_sats,
                    liquid_receive_sats,
                    usdt_receive_sats,
                }))
            },
        )
    }

    fn load_usdt_balance(&self) -> Task<Message> {
        use crate::app::breez_liquid::assets::{asset_kind_for_id, AssetKind};
        let breez_client = self.breez_client.clone();
        let network = self.network;
        Task::perform(
            async move {
                breez_client.info().await.map(|info| {
                    info.wallet_info
                        .asset_balances
                        .iter()
                        .find_map(|ab| {
                            if asset_kind_for_id(&ab.asset_id, network) == Some(AssetKind::Usdt) {
                                Some(ab.balance_sat)
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0)
                })
            },
            |result| match result {
                Ok(usdt_balance) => Message::View(view::Message::Home(
                    HomeMessage::UsdtBalanceUpdated(usdt_balance),
                )),
                Err(e) => {
                    tracing::error!("USDt balance fetch failed: {:?}", e);
                    Message::View(view::Message::Home(HomeMessage::UsdtBalanceFetchFailed))
                }
            },
        )
    }

    fn persist_pending_liquid_to_vault_transfer(
        &self,
        swap_id: String,
        amount_sat: u64,
    ) -> Task<Message> {
        let network_dir = self.datadir_path.network_directory(self.network);
        let cube_id = self.cube_id.clone();
        Task::perform(
            async move {
                settings::update_settings_file(&network_dir, move |mut current| {
                    if let Some(cube) = current.cubes.iter_mut().find(|c| c.id == cube_id) {
                        cube.pending_liquid_to_vault_transfer =
                            Some(settings::PendingLiquidToVaultTransfer {
                                swap_id,
                                amount_sat,
                            });
                    }
                    Some(current)
                })
                .await
            },
            |res| {
                if let Err(e) = res {
                    log::warn!("Failed to persist pending liquid->vault transfer: {}", e);
                }
                Message::Tick
            },
        )
    }

    fn clear_pending_liquid_to_vault_transfer(&self) -> Task<Message> {
        let network_dir = self.datadir_path.network_directory(self.network);
        let cube_id = self.cube_id.clone();
        Task::perform(
            async move {
                settings::update_settings_file(&network_dir, move |mut current| {
                    if let Some(cube) = current.cubes.iter_mut().find(|c| c.id == cube_id) {
                        cube.pending_liquid_to_vault_transfer = None;
                    }
                    Some(current)
                })
                .await
            },
            |res| {
                if let Err(e) = res {
                    log::warn!("Failed to clear pending liquid->vault transfer: {}", e);
                }
                Message::Tick
            },
        )
    }

    fn restore_pending_liquid_to_vault_transfer(&self) -> Task<Message> {
        let network_dir = self.datadir_path.network_directory(self.network);
        let cube_id = self.cube_id.clone();
        let breez_client = self.breez_client.clone();
        Task::perform(
            async move {
                let settings = settings::Settings::from_file(&network_dir).ok();
                let pending = settings
                    .as_ref()
                    .and_then(|s| s.cubes.iter().find(|c| c.id == cube_id))
                    .and_then(|cube| cube.pending_liquid_to_vault_transfer.clone())?;

                let mut stage = TransferStage::Initiated;
                let payments = breez_client.list_payments(None).await.ok();

                if let Some(payment) = payments.and_then(|ps| {
                    ps.into_iter().find(|payment| {
                        if payment.is_incoming() {
                            return false;
                        }
                        match &payment.details {
                            DomainPaymentDetails::OnChainBitcoin {
                                swap_id: Some(id), ..
                            } => id == &pending.swap_id,
                            _ => false,
                        }
                    })
                }) {
                    stage = match payment.status {
                        DomainPaymentStatus::Complete => {
                            let cube_id_for_clear = cube_id.clone();
                            let _ =
                                settings::update_settings_file(&network_dir, move |mut current| {
                                    if let Some(cube) =
                                        current.cubes.iter_mut().find(|c| c.id == cube_id_for_clear)
                                    {
                                        cube.pending_liquid_to_vault_transfer = None;
                                    }
                                    Some(current)
                                })
                                .await;
                            return None;
                        }
                        DomainPaymentStatus::Pending
                        | DomainPaymentStatus::WaitingFeeAcceptance => match payment.details {
                            DomainPaymentDetails::OnChainBitcoin {
                                claim_tx_id: Some(_),
                                ..
                            } => TransferStage::SendingToVault,
                            _ => TransferStage::SwappingLbtcToBtc,
                        },
                        DomainPaymentStatus::Created => TransferStage::Initiated,
                        DomainPaymentStatus::Failed
                        | DomainPaymentStatus::TimedOut
                        | DomainPaymentStatus::Refundable
                        | DomainPaymentStatus::RefundPending => {
                            let cube_id_for_clear = cube_id.clone();
                            let _ =
                                settings::update_settings_file(&network_dir, move |mut current| {
                                    if let Some(cube) =
                                        current.cubes.iter_mut().find(|c| c.id == cube_id_for_clear)
                                    {
                                        cube.pending_liquid_to_vault_transfer = None;
                                    }
                                    Some(current)
                                })
                                .await;
                            return None;
                        }
                    };
                }

                Some((pending.amount_sat, pending.swap_id, stage))
            },
            |restored| {
                if let Some((amount_sat, swap_id, stage)) = restored {
                    Message::View(view::Message::Home(HomeMessage::PendingTransferRestored {
                        amount_sat,
                        stage,
                        swap_id,
                    }))
                } else {
                    Message::Tick
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Breez-involving legs compose `MIN_TRANSFER_SATS` with the SDK's own
    /// minimum. Whichever is larger wins.
    #[test]
    fn liquid_sourced_min_composes_with_send_limit() {
        // Breez send-min below floor → floor wins.
        assert_eq!(
            effective_transfer_min_sat(
                TransferDirection::LiquidToVault,
                Some((1_000, 10_000_000)),
                None,
            ),
            Some(MIN_TRANSFER_SATS),
        );
        // Breez send-min above floor → Breez min wins.
        assert_eq!(
            effective_transfer_min_sat(
                TransferDirection::LiquidToSpark,
                Some((50_000, 10_000_000)),
                None,
            ),
            Some(50_000),
        );
    }

    #[test]
    fn liquid_destination_min_uses_receive_limit() {
        assert_eq!(
            effective_transfer_min_sat(
                TransferDirection::VaultToLiquid,
                None,
                Some((1_000, 5_000_000)),
            ),
            Some(MIN_TRANSFER_SATS),
        );
        assert_eq!(
            effective_transfer_min_sat(
                TransferDirection::SparkToLiquid,
                None,
                Some((75_000, 5_000_000)),
            ),
            Some(75_000),
        );
    }

    /// Pure on-chain legs (Vault↔Spark) don't involve Breez, so the floor
    /// is always the effective minimum and max is unconstrained (source
    /// balance is enforced elsewhere).
    #[test]
    fn pure_onchain_legs_ignore_breez_limits() {
        for direction in [
            TransferDirection::VaultToSpark,
            TransferDirection::SparkToVault,
        ] {
            assert_eq!(
                effective_transfer_min_sat(direction, None, None),
                Some(MIN_TRANSFER_SATS),
            );
            assert_eq!(
                effective_transfer_min_sat(
                    direction,
                    Some((9_999, 1_000_000)),
                    Some((9_999, 1_000_000)),
                ),
                Some(MIN_TRANSFER_SATS),
                "Breez limits should be ignored for pure on-chain legs"
            );
            assert_eq!(effective_transfer_max_sat(direction, None, None), None,);
        }
    }

    /// When the relevant Breez limit hasn't resolved yet, the helper
    /// returns `None` — the amount screen uses this as the
    /// "Loading limits…" signal.
    #[test]
    fn swap_legs_return_none_while_limits_load() {
        assert_eq!(
            effective_transfer_min_sat(TransferDirection::LiquidToVault, None, None),
            None,
        );
        assert_eq!(
            effective_transfer_max_sat(TransferDirection::VaultToLiquid, None, None),
            None,
        );
    }

    #[test]
    fn max_sat_tracks_breez_source_side() {
        // Liquid-sourced legs use `onchain_send_limit` for max.
        assert_eq!(
            effective_transfer_max_sat(
                TransferDirection::LiquidToVault,
                Some((25_000, 42_000_000)),
                Some((25_000, 999)),
            ),
            Some(42_000_000),
        );
        // Liquid-destination legs use `onchain_receive_limit` for max.
        assert_eq!(
            effective_transfer_max_sat(
                TransferDirection::SparkToLiquid,
                Some((25_000, 999)),
                Some((25_000, 42_000_000)),
            ),
            Some(42_000_000),
        );
    }
}
