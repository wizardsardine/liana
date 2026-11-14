/// Minimum amount (in sats) for transfers that settle on-chain.
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

/// Cap on the fallback `get_info` poll cycles while waiting for the SDK
/// to emit `Synced`. Paired with the 3-second poll interval that means
/// the loading placeholder gives up after ~30s and shows whatever the
/// last response returned — long enough for genuine cold-start sync,
/// short enough that a truly broken event path doesn't leave the user
/// stuck on `…` forever.
const SPARK_LOAD_RETRY_CAP: u8 = 10;

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

/// The Vault coins a Max transfer sweeps, and their total: every unspent coin
/// the wallet owns, as the Vault spend screen's MAX selects them.
fn vault_sweep_coins(cache: &Cache) -> (Vec<OutPoint>, Amount) {
    cache
        .coins()
        .iter()
        .filter(|coin| coin.spend_info.is_none() && crate::daemon::model::coin_is_owned(coin))
        .fold(
            (Vec::new(), Amount::ZERO),
            |(mut outpoints, total), coin| {
                outpoints.push(coin.outpoint);
                (outpoints, total + coin.amount)
            },
        )
}

/// What `psbt` pays to `script`.
fn paid_to(psbt: &Psbt, script: &ScriptBuf) -> Amount {
    psbt.unsigned_tx
        .output
        .iter()
        .filter(|output| output.script_pubkey == *script)
        .fold(Amount::ZERO, |total, output| total + output.value)
}

/// `settings` with this Cube's stored Spark conversion set to `record`. A Cube
/// missing from the file leaves it unchanged: a settings updater that returns
/// `None` would delete the file (see `settings::update_settings_file`).
fn with_pending_spark_shift(
    mut settings: settings::Settings,
    cube_id: &str,
    record: Option<settings::PendingSparkShift>,
) -> settings::Settings {
    if let Some(cube) = settings.cubes.iter_mut().find(|c| c.id == cube_id) {
        cube.pending_spark_shift = record;
    }
    settings
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

fn transfer_signing_completion_message(
    persisted: bool,
    threshold_met: bool,
    request: Option<(u64, u64, TransferDirection)>,
) -> Option<HomeMessage> {
    if persisted && threshold_met {
        request.map(
            |(request_id, generation, direction)| HomeMessage::TransferSigningComplete {
                request_id,
                generation,
                direction,
            },
        )
    } else {
        None
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
use coincube_core::miniscript::bitcoin::{
    bip32::ChildNumber, Address, Amount, OutPoint, Psbt, ScriptBuf,
};

use crate::app::wallets::{DomainPaymentDetails, DomainPaymentStatus};
use coincube_ui::component::amount::BitcoinDisplayUnit;
use coincube_ui::component::form;
use coincube_ui::widget::Element;
use iced::{Subscription, Task};

use super::vault::psbt::SignModal;
use super::{Cache, Menu, State};
use crate::app::breez_spark::SparkClientError;
use crate::app::state::vault::label::LabelsEdited;
use crate::app::state::vault::receive::ShowQrCodeModal;
use crate::app::transfer_destination::{Source, WalletIdentity};
use crate::app::transfer_destination_loader::{
    other_cubes, CubeChoice, OtherReceiveAddress, Refusal as ScopeRefusal, ScopedDestination,
};
use crate::app::view::global_home::{
    GlobalViewConfig, HomeView, OtherCubeView, PendingTransfer, PickerSide, TransferDirection,
    TransferError, TransferPage, TransferStage, UnlockPhase, WalletKind, WalletTransferEntry,
    UNLOCK_CHECKS,
};

/// Which per-wallet balances the Total Balance aggregate is still waiting on.
#[derive(Clone, Copy, Debug)]
struct BalanceReadiness {
    has_spark: bool,
    spark_loaded: bool,
    has_liquid: bool,
    liquid_loaded: bool,
    usdt_loaded: bool,
    vault_pending: bool,
}

/// Whether the Vault's balance is still loading rather than known.
///
/// Before the daemon's first poll there is no tip. After it, an empty Vault whose
/// full scan is still running, or whose polls keep failing, has not loaded its
/// history: its zero is a placeholder, not a balance, and counting it would put a
/// restored Vault's funds into the Total Balance as nothing. Failing polls end their
/// scan between attempts, so the scan flag alone would flip the Vault in and out of
/// the total on every retry.
fn vault_balance_pending(
    blockheight: i32,
    history: &coincubed::commands::HistorySync,
    vault_balance: Amount,
) -> bool {
    let failing = history.last_failure.as_ref().is_some_and(|failure| {
        failure.consecutive >= crate::app::wallet::FAILED_POLLS_BEFORE_REPORTING
    });
    blockheight <= 0
        || ((history.full_scan_in_progress || failing) && vault_balance == Amount::ZERO)
}

/// Whether the Total Balance placeholder should still be spinning.
///
/// Extracted from `view` so the rule can be tested, because getting it wrong is
/// invisible until someone's dashboard hangs. The rule: a wallet gates the
/// aggregate only while it is *present and still outstanding*.
///
/// "Outstanding" is not "successful". A fetch that failed has to mark itself
/// loaded — see `HomeMessage::LiquidBalanceFetchFailed` — or one wallet's
/// network blip freezes the total for every wallet, including the ones that
/// answered. A gated-off Liquid wallet never loads at all, which is the same
/// trap from the other direction.
fn total_balance_pending(r: BalanceReadiness) -> bool {
    let spark_pending = r.has_spark && !r.spark_loaded;
    let liquid_pending = r.has_liquid && !r.liquid_loaded;
    let usdt_pending = r.has_liquid && !r.usdt_loaded;
    spark_pending || liquid_pending || usdt_pending || r.vault_pending
}

/// Prefer an available local pair in Liquid, Vault, Spark order. A mainnet
/// Cube with only Spark or Vault starts with Another Cube selected.
pub(crate) fn default_transfer_direction(
    has_liquid: bool,
    has_vault: bool,
    has_spark: bool,
    chain: crate::chain::ChainId,
) -> Option<TransferDirection> {
    let available: Vec<WalletKind> = [
        has_liquid.then_some(WalletKind::Liquid),
        has_vault.then_some(WalletKind::Vault),
        has_spark.then_some(WalletKind::Spark),
    ]
    .iter()
    .flatten()
    .copied()
    .collect();
    match available.as_slice() {
        [from, to, ..] => TransferDirection::try_from_pair(*from, *to),
        [from] => TransferDirection::to_other_cube(*from, chain),
        _ => None,
    }
}

/// The wallet page a transfer opened from returns to, and keeps selected in
/// the side menu meanwhile.
pub(crate) fn transfer_origin_menu(wallet: WalletKind) -> &'static Menu {
    use crate::app::menu::{LiquidSubMenu, SparkSubMenu, VaultSubMenu};
    static SPARK: Menu = Menu::Spark(SparkSubMenu::Overview);
    static VAULT: Menu = Menu::Vault(VaultSubMenu::Overview);
    static LIQUID: Menu = Menu::Liquid(LiquidSubMenu::Overview);
    match wallet {
        WalletKind::Spark => &SPARK,
        WalletKind::Vault => &VAULT,
        WalletKind::Liquid => &LIQUID,
    }
}

/// Whether some other local Cube on this network can receive a transfer from
/// a Spark or a Vault source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct OtherCubeReceivers {
    spark: bool,
    vault: bool,
}

impl OtherCubeReceivers {
    fn from(self, wallet: WalletKind) -> bool {
        match wallet {
            WalletKind::Spark => self.spark,
            WalletKind::Vault => self.vault,
            WalletKind::Liquid => false,
        }
    }
}

/// The direction a wallet page's Transfer opens with, `from` fixed: another
/// wallet in this Cube in Liquid, Vault, Spark order, else another Cube when
/// one can receive. `None` leaves the wallet page's Transfer disabled.
pub(crate) fn transfer_direction_from(
    from: WalletKind,
    has_liquid: bool,
    has_vault: bool,
    has_spark: bool,
    chain: crate::chain::ChainId,
    other_cube_receives: bool,
) -> Option<TransferDirection> {
    let has = |wallet| match wallet {
        WalletKind::Liquid => has_liquid,
        WalletKind::Vault => has_vault,
        WalletKind::Spark => has_spark,
    };
    if !has(from) {
        return None;
    }
    [WalletKind::Liquid, WalletKind::Vault, WalletKind::Spark]
        .iter()
        .copied()
        .filter(|to| *to != from && has(*to))
        .find_map(|to| TransferDirection::try_from_pair(from, to))
        .or_else(|| {
            other_cube_receives
                .then(|| TransferDirection::to_other_cube(from, chain))
                .flatten()
        })
}

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
        // Pure on-chain: Vault↔Spark, or Vault to another Cube's Vault. No
        // swap limits apply.
        TransferDirection::VaultToSpark
        | TransferDirection::SparkToVault
        | TransferDirection::SparkToOtherCube
        | TransferDirection::VaultToOtherSpark
        | TransferDirection::VaultToOtherCube => Some(MIN_TRANSFER_SATS),
        TransferDirection::SparkToOtherSpark => Some(1),
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
        TransferDirection::VaultToSpark
        | TransferDirection::SparkToVault
        | TransferDirection::SparkToOtherCube
        | TransferDirection::SparkToOtherSpark
        | TransferDirection::VaultToOtherSpark
        | TransferDirection::VaultToOtherCube => None,
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
    /// Only populated after the imported wallet's full discovery is verified.
    pub(crate) imported_balance: Option<Amount>,
    pub(crate) imported_installed: bool,
    breez_client: Option<Arc<LiquidBackend>>,
    /// Optional Spark backend handle. `None` when the cube has no
    /// Spark signer or the bridge subprocess failed to spawn — the
    /// Home page simply hides the Spark card in that case.
    spark_backend: Option<Arc<SparkBackend>>,
    spark_identity: Option<String>,
    liquid_balance: Amount,
    liquid_balance_loaded: bool,
    /// Set when a balance fetch failed, so the row can say so instead of
    /// rendering the untouched zero. Mirrors `usdt_balance_error`.
    liquid_balance_error: bool,
    /// Spark wallet balance in sats, refreshed by
    /// [`load_balance`]. `Amount::ZERO` while the first
    /// `get_info` RPC is in flight, or forever when no Spark
    /// backend is wired up for this cube.
    spark_balance: Amount,
    /// Raw Spark holdings retained solely for truthful landing presentation.
    spark_btc_balance: Amount,
    spark_usdb_balance: Option<coincube_spark_protocol::UsdbBalanceSnapshot>,
    spark_balance_error: bool,
    spark_balance_loaded: bool,
    /// Set once the bridge has forwarded `Event::Synced` from the
    /// SDK. Until then a `get_info` response may be the SDK's
    /// persisted-on-disk value from a previous session — payments
    /// received between sessions won't be reflected yet. Required
    /// before `spark_balance_loaded` can flip true.
    spark_synced_seen: bool,
    /// Bounds the fallback poll in `subscription()` that re-runs
    /// `get_info` while loading is still pending — `tokio` broadcast
    /// doesn't replay events, so a `Synced` firing before iced
    /// subscribes would otherwise leave us stuck.
    spark_load_retry_count: u8,
    /// Set while a `load_spark_balance` task is in flight. Prevents
    /// overlapping `get_info` RPCs — the 3-second poll and SparkEvent-
    /// driven refreshes both call `load_spark_balance`, and a slow
    /// initial sync can hold a get_info request open for the full
    /// 30-second timeout, during which several more callers could
    /// otherwise pile on. Cleared by `SparkBalanceUpdated` /
    /// `SparkLoadFailed` when the response (or the soft-fail) lands.
    spark_balance_loading: bool,
    /// Set when `SparkSyncedObserved` arrives while a `load_spark_balance`
    /// is in flight. The pending response was issued pre-Synced and
    /// would otherwise trip `spark_balance_loaded` on the SDK's stale
    /// persisted value; `SparkBalanceUpdated` checks this flag and
    /// discards the response in favor of a fresh fetch instead.
    spark_pending_resync: bool,
    /// Set the first time a non-discarded `SparkBalanceUpdated` lands
    /// (i.e. an actual `get_info` response was written into
    /// `spark_balance`). Required for the `SparkLoadRetry` cap-
    /// release path to flip `spark_balance_loaded`: without it, a
    /// completely broken bridge could leave `spark_balance` at
    /// `Amount::ZERO` and the cap-release would publish a false zero
    /// as the authoritative balance, which `validate_entered_amount`
    /// would then use as the Spark source cap for transfers.
    spark_balance_received: bool,
    usdt_balance: u64,
    usdt_balance_error: bool,
    usdt_balance_loaded: bool,
    wallet: Option<Arc<Wallet>>,
    /// Retained only for the legacy Cube settings constructor; device privacy owns display.
    _legacy_balance_masked: bool,
    /// Session-only Overview controls. A true global Hide overrides both.
    overview_total_hidden: bool,
    overview_wallets_hidden: bool,
    /// Cached derivation of `(transfer_from, transfer_to)` via
    /// `TransferDirection::try_from_pair`. `None` when either side is unset or
    /// both sides point at the same wallet.
    transfer_direction: Option<TransferDirection>,
    transfer_from: Option<WalletKind>,
    transfer_to: Option<WalletKind>,
    /// The wallet page whose Transfer button opened this flow. Leaving the
    /// flow returns there, and the side menu keeps it selected meanwhile.
    /// `None` for a transfer opened from the Overview.
    transfer_origin: Option<WalletKind>,
    /// Whether another local Cube could receive from each source wallet, read
    /// from the Cube catalog when a wallet page opens (not on every frame).
    other_cube_receivers: OtherCubeReceivers,
    /// `Some(side)` when the wallet-picker popup is open editing that side.
    wallet_picker: Option<PickerSide>,
    current_view: HomeView,
    entered_amount: form::Value<String>,
    receive_address_info: Option<ReceiveAddressInfo>,
    transfer_request_generation: u64,
    destination_ready: Option<(u64, TransferDirection)>,
    transfer_psbt_ready_for: Option<(u64, u64, TransferDirection)>,
    next_psbt_request_id: u64,
    active_psbt_request: Option<u64>,
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
    /// Which page step 1 of the transfer dialog shows.
    transfer_page: TransferPage,
    /// Unit the transfer amount is entered in, seeded from the Cube's
    /// display unit when the dialog opens.
    transfer_unit: BitcoinDisplayUnit,
    /// Max was chosen for this source wallet: the transfer drains it and the
    /// fee comes out of the amount. Inert once the source changes.
    transfer_max: Option<WalletKind>,
    /// "Another Cube…": the local catalog read when that route opens.
    other_cubes: Vec<CubeChoice>,
    other_cube_selected: Option<String>,
    /// The chosen Cube, unlocked in place for this transfer only. Dropping
    /// it revokes the unlock and stops any daemon it started.
    other_scope: Option<ScopedDestination>,
    other_pin: crate::pin_input::PinInput,
    other_phase: UnlockPhase,
    other_error: Option<String>,
    /// The other Vault's receive address, re-derived from its descriptor.
    other_address: Option<OtherReceiveAddress>,
    other_locked_again: bool,
    pending_vault_incoming: Option<PendingTransfer>,
    pending_vault_incoming_swap_id: Option<String>,
    /// Whether the Liquid wallet is shown ([`LiquidGate::show`]), kept in
    /// step by the App because `reload` has no cache. While it is off, a
    /// Liquid → Vault transfer persisted by an earlier session is neither
    /// restored nor shown; its settings entry stays as it is.
    ///
    /// [`LiquidGate::show`]: crate::app::features::LiquidGate::show
    liquid_shown: bool,
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
    /// True when the pending incoming Spark deposit came from a SideShift swap
    /// rather than a Vault/Liquid→Spark transfer. Swaps reuse the very same
    /// auto-claim watcher; this flag is what makes the successful claim
    /// additionally fire the global "bitcoin received" celebration. Set only by
    /// `SwapSettledAwaitingArrival`, and reset wherever `pending_spark_incoming`
    /// is cleared or replaced so it can never leak onto a transfer's claim.
    pending_spark_incoming_is_swap: bool,
    /// The Cube's stored Spark cross-network receive has been read back and
    /// handed to the Spark Receive panel this session. Once is enough: after
    /// that the panel holds the shift and keeps the record current.
    spark_shift_restore_requested: bool,
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
    /// An auto-claim was accepted before maturity (SDK 0.25.0+) and its
    /// transfer is settling asynchronously: the settled amount is unknown
    /// until the bridge's `PaymentSucceeded` reports it, so Completed and
    /// the "received" splash wait for `SparkPaymentSucceeded`.
    spark_claim_awaiting_settlement: bool,
    pending_liquid_send_sats: u64,
    pending_usdt_send_sats: u64,
    pending_liquid_receive_sats: u64,
    pending_usdt_receive_sats: u64,
    datadir_path: CoincubeDirectory,
    network: crate::chain::ChainId,
    cube_id: String,
}

impl GlobalHome {
    /// No Spark backend, or a synced balance. A failed fetch is not enough:
    /// one can land before the sync that a later fetch completes.
    pub(crate) fn spark_settled(&self) -> bool {
        self.spark_backend.is_none() || self.spark_balance_loaded
    }

    /// A loaded Spark or Liquid balance is not zero. A balance still
    /// loading, or one whose fetch failed, does not count.
    pub(crate) fn holds_loaded_funds(&self) -> bool {
        let spark = self.spark_balance_loaded
            && (self.spark_balance > Amount::ZERO
                || self.spark_btc_balance > Amount::ZERO
                || self
                    .spark_usdb_balance
                    .as_ref()
                    .is_some_and(|usdb| usdb.balance > 0));
        let liquid = self.liquid_balance_loaded && self.liquid_balance > Amount::ZERO;
        let usdt = self.usdt_balance_loaded && self.usdt_balance > 0;
        spark || liquid || usdt
    }

    #[cfg(test)]
    pub(crate) fn set_loaded_spark_balance_for_test(&mut self, sats: u64) {
        self.spark_btc_balance = Amount::from_sat(sats);
        self.spark_balance = Amount::from_sat(sats);
        self.spark_balance_loaded = true;
    }

    /// Fallback poll for the Spark balance while it is still loading.
    /// `tokio::sync::broadcast` doesn't replay events fired before iced
    /// subscribed, so a `Synced` arriving during the bridge-init /
    /// iced-subscribe gap would be lost — the poll guarantees we still
    /// re-fetch every few seconds until either a `Synced` lands or the
    /// retry cap is reached. Self-cancels the moment `spark_balance_loaded`
    /// flips true (iced re-evaluates subscriptions on each render). Uses
    /// `SparkLoadRetry` (not `RefreshSparkBalance`) so SDK-event-driven
    /// refreshes don't count against the retry cap.
    ///
    /// The App subscribes it whichever panel is current: the Cube opening
    /// waits on this load, and a Cube can open on another panel.
    pub(crate) fn spark_load_poll(&self) -> Option<Subscription<Message>> {
        (self.spark_backend.is_some()
            && !self.spark_balance_loaded
            && self.spark_load_retry_count < SPARK_LOAD_RETRY_CAP)
            .then(|| {
                iced::time::every(std::time::Duration::from_secs(3))
                    .map(|_| Message::View(view::Message::Home(HomeMessage::SparkLoadRetry)))
            })
    }

    /// Follow the Liquid gate. Hiding Liquid also hides a restored
    /// Liquid → Vault transfer at once (the view reads this flag).
    pub fn set_liquid_shown(&mut self, shown: bool) {
        self.liquid_shown = shown;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        wallet: Arc<Wallet>,
        breez_client: impl Into<Option<Arc<LiquidBackend>>>,
        spark_backend: Option<Arc<SparkBackend>>,
        datadir_path: CoincubeDirectory,
        network: impl Into<crate::chain::ChainId>,
        cube_id: String,
        balance_masked: bool,
    ) -> Self {
        Self {
            imported_balance: None,
            imported_installed: false,
            wallet: Some(wallet),
            liquid_balance: Amount::ZERO,
            liquid_balance_loaded: false,
            liquid_balance_error: false,
            spark_balance: Amount::ZERO,
            spark_btc_balance: Amount::ZERO,
            spark_usdb_balance: None,
            spark_balance_error: false,
            spark_balance_loaded: false,
            spark_synced_seen: false,
            spark_load_retry_count: 0,
            spark_balance_loading: false,
            spark_pending_resync: false,
            spark_balance_received: false,
            usdt_balance: 0,
            usdt_balance_error: false,
            usdt_balance_loaded: false,
            breez_client: breez_client.into(),
            spark_backend,
            spark_identity: None,
            _legacy_balance_masked: balance_masked,
            overview_total_hidden: false,
            overview_wallets_hidden: false,
            transfer_direction: None,
            transfer_from: None,
            transfer_to: None,
            transfer_origin: None,
            other_cube_receivers: OtherCubeReceivers::default(),
            wallet_picker: None,
            current_view: HomeView::default(),
            entered_amount: form::Value::default(),
            receive_address_info: None,
            transfer_request_generation: 0,
            destination_ready: None,
            transfer_psbt_ready_for: None,
            next_psbt_request_id: 0,
            active_psbt_request: None,
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
            transfer_page: TransferPage::Wallets,
            transfer_unit: BitcoinDisplayUnit::Sats,
            transfer_max: None,
            other_cubes: Vec::new(),
            other_cube_selected: None,
            other_scope: None,
            other_pin: crate::pin_input::PinInput::new(),
            other_phase: UnlockPhase::default(),
            other_error: None,
            other_address: None,
            other_locked_again: false,
            pending_vault_incoming: None,
            liquid_shown: false,
            pending_vault_incoming_swap_id: None,
            pending_vault_receive_sats: 0,
            pending_vault_receive_baseline_sats: 0,
            pending_spark_incoming: None,
            pending_spark_incoming_swap_id: None,
            pending_spark_deposit_seen: false,
            pending_spark_incoming_is_swap: false,
            spark_shift_restore_requested: false,
            spark_send_handle: None,
            spark_send_fee_sat: None,
            auto_claiming_spark_deposit: None,
            spark_claim_awaiting_settlement: false,
            pending_liquid_send_sats: 0,
            pending_usdt_send_sats: 0,
            pending_liquid_receive_sats: 0,
            pending_usdt_receive_sats: 0,
            datadir_path,
            network: network.into(),
            cube_id,
        }
    }

    pub fn new_without_wallet(
        breez_client: impl Into<Option<Arc<LiquidBackend>>>,
        spark_backend: Option<Arc<SparkBackend>>,
        datadir_path: CoincubeDirectory,
        network: impl Into<crate::chain::ChainId>,
        cube_id: String,
        balance_masked: bool,
    ) -> Self {
        Self {
            imported_balance: None,
            imported_installed: false,
            wallet: None,
            liquid_balance: Amount::from_sat(0),
            liquid_balance_loaded: false,
            liquid_balance_error: false,
            spark_balance: Amount::ZERO,
            spark_btc_balance: Amount::ZERO,
            spark_usdb_balance: None,
            spark_balance_error: false,
            spark_balance_loaded: false,
            spark_synced_seen: false,
            spark_load_retry_count: 0,
            spark_balance_loading: false,
            spark_pending_resync: false,
            spark_balance_received: false,
            usdt_balance: 0,
            usdt_balance_error: false,
            usdt_balance_loaded: false,
            breez_client: breez_client.into(),
            spark_backend,
            spark_identity: None,
            _legacy_balance_masked: balance_masked,
            overview_total_hidden: false,
            overview_wallets_hidden: false,
            transfer_direction: None,
            transfer_from: None,
            transfer_to: None,
            transfer_origin: None,
            other_cube_receivers: OtherCubeReceivers::default(),
            wallet_picker: None,
            current_view: HomeView::default(),
            entered_amount: form::Value::default(),
            receive_address_info: None,
            transfer_request_generation: 0,
            destination_ready: None,
            transfer_psbt_ready_for: None,
            next_psbt_request_id: 0,
            active_psbt_request: None,
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
            transfer_page: TransferPage::Wallets,
            transfer_unit: BitcoinDisplayUnit::Sats,
            transfer_max: None,
            other_cubes: Vec::new(),
            other_cube_selected: None,
            other_scope: None,
            other_pin: crate::pin_input::PinInput::new(),
            other_phase: UnlockPhase::default(),
            other_error: None,
            other_address: None,
            other_locked_again: false,
            pending_vault_incoming: None,
            liquid_shown: false,
            pending_vault_incoming_swap_id: None,
            pending_vault_receive_sats: 0,
            pending_vault_receive_baseline_sats: 0,
            pending_spark_incoming: None,
            pending_spark_incoming_swap_id: None,
            pending_spark_deposit_seen: false,
            pending_spark_incoming_is_swap: false,
            spark_shift_restore_requested: false,
            spark_send_handle: None,
            spark_send_fee_sat: None,
            auto_claiming_spark_deposit: None,
            spark_claim_awaiting_settlement: false,
            pending_liquid_send_sats: 0,
            pending_usdt_send_sats: 0,
            pending_liquid_receive_sats: 0,
            pending_usdt_receive_sats: 0,
            datadir_path,
            network: network.into(),
            cube_id,
        }
    }
}

impl GlobalHome {
    /// The Overview. App passes its "Recent activity", read from Activity
    /// (`view::activity::recent_activity`), to sit under the wallet cards.
    pub fn view_with_recent_activity<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
        recent_activity: Option<Element<'a, view::Message>>,
    ) -> Element<'a, view::Message> {
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

        // Total Balance is "loading" until every applicable wallet has
        // reported in. Absent wallets (no Spark / no Vault) don't gate.
        // Vault loads when the daemon has produced its first poll —
        // we use `blockheight > 0` rather than `last_poll_timestamp`
        // because the wallet DB's `last_poll_timestamp` can remain
        // `None` for cubes whose coins were imported/restored, leaving
        // the gate stuck even though the cache has real coins and a
        // current blockheight. `blockheight` is updated atomically
        // with coins on every `UpdateDaemonCache`, so any non-zero
        // value is a reliable "daemon has spoken" signal.
        let has_spark = self.spark_backend.is_some();
        // Sunset gate AND a Liquid backend on this network: on a testnet cube
        // the SDK never connects, so a card with live Send/Receive buttons
        // would be a wallet the user can't actually use.
        let has_liquid =
            crate::app::features::liquid_wallet_usable(cache.chain(), cache.liquid_gate);
        let vault_pending = cache.has_vault
            && vault_balance_pending(cache.blockheight(), cache.history_sync(), vault_balance);
        let total_balance_loading = self.total_balance_loading(cache);

        let config = GlobalViewConfig {
            cube_id: &self.cube_id,
            liquid_balance,
            spark_balance: self.spark_balance,
            spark_btc_balance: self.spark_btc_balance,
            spark_usdb_balance: self.spark_usdb_balance.as_ref(),
            spark_balance_error: self.spark_balance_error,
            usdt_balance,
            usdt_balance_error,
            vault_balance,
            imported_balance: self.imported_balance,
            imported_installed: self.imported_installed,
            fiat_converter,
            balance_masked: cache.balance_amounts_masked(),
            overview_total_hidden: self.overview_total_hidden,
            overview_wallets_hidden: self.overview_wallets_hidden,
            total_balance_loading,
            spark_balance_loaded: self.spark_balance_loaded,
            liquid_balance_loaded: self.liquid_balance_loaded,
            liquid_balance_error: self.liquid_balance_error,
            usdt_balance_loaded: self.usdt_balance_loaded,
            vault_loaded: !vault_pending,
            recovery_available: !vault_pending
                && self
                    .wallet
                    .as_ref()
                    .and_then(|wallet| wallet.recovery_timelock())
                    .and_then(|timelock| {
                        crate::daemon::model::next_recovery_sequence(
                            cache.coins(),
                            cache.blockheight().max(0) as u32,
                            timelock,
                        )
                    })
                    .is_some_and(|sequence| sequence <= 1),
            vault_stale: cache
                .vault_alert
                .as_ref()
                .map(|alert| match &alert.last_synced {
                    Some(at) => format!("Last known Vault balance · not updating since {at}"),
                    None => "Last known Vault balance · not updating".to_owned(),
                }),
            display_mode: cache.display_mode,
            has_vault: cache.has_vault,
            has_imported_bitcoin: cache.has_imported_bitcoin,
            has_spark,
            has_liquid,
            current_view: self.current_view,
            transfer_direction: self.transfer_direction,
            transfer_from: self.transfer_from,
            transfer_to: self.transfer_to,
            wallet_picker: self.wallet_picker,
            entered_amount: &self.entered_amount,
            receive_address: self.receive_address_info.as_ref().map(|info| &info.address),
            destination_ready: self.destination_is_ready(),
            transfer_action_ready: self.transfer_action_ready(),
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
            pending_vault_incoming: self.pending_vault_incoming.filter(|_| self.liquid_shown),
            btc_usd_price: cache.btc_usd_price,
            cube_name: &cache.cube_name,
            vault_alias: self
                .wallet
                .as_ref()
                .and_then(|wallet| wallet.alias.as_deref()),
            chain: cache.chain(),
            light_theme: matches!(
                cache.theme_mode,
                coincube_ui::theme::palette::ThemeMode::Light
            ),
            transfer_page: self.transfer_page,
            transfer_unit: self.transfer_unit,
            transfer_max: self.sends_max(),
            other_cube: OtherCubeView {
                choices: &self.other_cubes,
                selected: self.other_cube_selected.as_deref(),
                pin: Some(&self.other_pin),
                phase: self.other_phase,
                error: self.other_error.as_deref(),
                spark_address: match self.other_address.as_ref() {
                    Some(OtherReceiveAddress::Spark(address)) => Some(address),
                    _ => None,
                },
                unlocked: self.other_scope.is_some() && self.other_address.is_some(),
                locked_again: self.other_locked_again,
            },
            transfer_origin: self.transfer_origin,
        };
        let transfer_action = view::global_home::transfer_head_action(&config);
        let (overview, transfer) = view::global_home::global_home_view(config);
        // A transfer step is a page of its own, as Send is: its head has the
        // steps and Back to Overview, which cancels the flow (it isn't
        // offered while a send is in flight).
        let content = match transfer {
            // Opened from a wallet page, that wallet stays selected in the
            // side menu for the whole flow.
            Some(screen) => crate::app::view::dashboard_payment(
                match self.transfer_origin {
                    Some(wallet) => transfer_origin_menu(wallet),
                    None => menu,
                },
                cache,
                Some(view::global_home::transfer_page_head(cache, &screen)),
                screen.body,
            ),
            None => view::global_home::overview_page(
                menu,
                cache,
                transfer_action,
                overview,
                recent_activity,
            ),
        };

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
}

impl State for GlobalHome {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        self.view_with_recent_activity(menu, cache, None)
    }

    /// The side menu is leaving Transfer (it's a page now, not a modal that
    /// blocked the menu): lock another Cube opened for this transfer, so
    /// coming back asks for its PIN or passkey again, and its pick starts
    /// over in To.
    fn interrupt(&mut self) {
        self.wallet_picker = None;
        // Left through the side menu: the flow stays open on the Overview,
        // which now owns it, rather than returning to the wallet page.
        self.transfer_origin = None;
        if self.other_scope.is_none() && self.other_cube_selected.is_none() {
            return;
        }
        if self.is_sending {
            // A send or PSBT preparation in flight already has the checked
            // address and needs the Cube no more: lock it now, whatever the
            // outcome. If it fails, Confirm finds the Cube locked and starts
            // its unlock over; Done still says it is locked again.
            let was_open = self.other_scope.is_some();
            self.lock_other_cube();
            self.other_locked_again |= was_open;
            return;
        }
        self.reset_other_cube();
        if self
            .transfer_direction
            .is_some_and(|direction| direction.is_other_cube())
            && matches!(self.current_view.step, 1 | 2)
        {
            self.invalidate_transfer_destination();
            self.current_view.step = 1;
            self.transfer_page = TransferPage::Wallets;
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = Vec::new();

        if let Modal::Sign(sign_modal) = &self.modal {
            // To fetch hardware wallets
            use crate::app::state::vault::psbt::Modal as PsbtModalTrait;
            subscriptions.push(sign_modal.subscription());
        }

        // The other Cube's passkey ceremony is polled on the UI thread.
        if self.other_phase == (UnlockPhase::Credential { busy: true })
            && self
                .other_scope
                .as_ref()
                .is_some_and(|scope| scope.is_passkey())
        {
            subscriptions.push(
                iced::time::every(std::time::Duration::from_millis(200))
                    .map(|_| Message::View(view::Message::Home(HomeMessage::OtherCubePasskeyPoll))),
            );
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
                    HomeMessage::ToggleTotalBalance => {
                        self.overview_total_hidden = !self.overview_total_hidden;
                        Task::none()
                    }
                    HomeMessage::ToggleWalletBalances => {
                        self.overview_wallets_hidden = !self.overview_wallets_hidden;
                        Task::none()
                    }
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
                    HomeMessage::SparkBalanceUpdated {
                        btc,
                        usdb_balance,
                        identity_pubkey,
                    } => {
                        self.spark_balance_loading = false;
                        // The in-flight fetch was issued before
                        // `Synced` arrived — its value reflects the
                        // SDK's pre-sync state. Discard it and fire a
                        // fresh post-sync fetch; the next response
                        // will flip the gate.
                        if self.spark_pending_resync {
                            self.spark_pending_resync = false;
                            return self.load_spark_balance().unwrap_or_else(Task::none);
                        }
                        self.spark_btc_balance = btc;
                        self.spark_identity = Some(identity_pubkey);
                        self.spark_usdb_balance = usdb_balance.clone();
                        self.spark_balance_error = false;
                        // Fold any Stable Balance (USDB) holding into the shown
                        // Spark balance — auto-sweep means a BTC-only reading
                        // would understate a funded wallet. Shared with the
                        // Send/Receive cards via `unified_spark_balance_sats`.
                        self.spark_balance =
                            Amount::from_sat(crate::app::state::spark::unified_spark_balance_sats(
                                btc.to_sat(),
                                usdb_balance.as_ref(),
                                cache,
                            ));
                        self.spark_balance_received = true;
                        // Trust the response once the bridge has
                        // confirmed at least one `Synced` event — until
                        // then this could be the SDK's pre-sync persisted
                        // value (e.g. zero before this session's incoming
                        // interest payments landed). Until corroborated,
                        // the card renders a `…` placeholder.
                        //
                        // Also release the gate when the fallback poll
                        // has already exhausted its budget: the
                        // subscription stops firing once
                        // `spark_load_retry_count` reaches the cap, so
                        // any response that lands after that point has
                        // no other path to release the loading UI.
                        // Best-effort — better to show the last
                        // received value than stay stuck on `…`.
                        if self.spark_synced_seen
                            || self.spark_load_retry_count >= SPARK_LOAD_RETRY_CAP
                        {
                            self.spark_balance_loaded = true;
                        }
                        Task::none()
                    }
                    HomeMessage::NextStep => {
                        // Step 0 → 1 (overview → amount entry): this is the
                        // Transfer button press. Default the direction and
                        // prefetch Breez's on-chain swap limits. Only
                        // `breez_client` is needed here — Vault daemon is
                        // required only by the LiquidToVault / SparkToVault
                        // branches of step 1 → 2, which unwrap inline.
                        if self.current_view.step == 0 {
                            let has_liquid = crate::app::features::liquid_wallet_usable(
                                cache.network,
                                cache.liquid_gate,
                            );
                            let Some(direction) = default_transfer_direction(
                                has_liquid,
                                cache.has_vault,
                                self.spark_backend.is_some(),
                                cache.chain(),
                            ) else {
                                return Task::none();
                            };
                            return self.begin_transfer(cache, direction);
                        }
                        // Step 1 → 2 (amount → confirm): fetch the destination
                        // receive address and, for the Liquid-source leg, ask
                        // Breez to prepare a pay_onchain so fees can render on
                        // the confirm screen.
                        if self.current_view.step == 1 {
                            // Prepare lives on the Amount page only.
                            if self.transfer_page != TransferPage::Amount {
                                return Task::none();
                            }
                            if self
                                .transfer_direction
                                .is_some_and(Self::direction_uses_vault)
                                && !self.legacy_vault_available(cache)
                            {
                                return Task::none();
                            }
                            let Some(direction) = self.transfer_direction else {
                                return Task::none();
                            };
                            let generation = self.invalidate_transfer_destination();
                            // Another Cube: its Vault's address was fetched and
                            // re-derived from its descriptor during Unlock.
                            if direction.is_other_cube()
                                && direction.from_kind() == WalletKind::Vault
                            {
                                if direction == TransferDirection::VaultToOtherSpark {
                                    if let Err(refusal) = self.other_receive_address(cache) {
                                        return self.restart_other_cube_unlock(refusal);
                                    }
                                }
                                let Some(OtherReceiveAddress::Bitcoin(address)) =
                                    self.other_address.clone()
                                else {
                                    return Task::none();
                                };
                                self.current_view.next();
                                return Task::done(Message::View(view::Message::Home(
                                    HomeMessage::TransferDestinationReady {
                                        generation,
                                        direction,
                                        result: Ok((address.address.to_string(), address.index)),
                                    },
                                )));
                            }
                            let mut tasks = Vec::new();
                            if matches!(
                                self.transfer_direction,
                                Some(TransferDirection::LiquidToVault)
                            ) {
                                let Some(daemon) = daemon.clone() else {
                                    return Task::done(Message::View(view::Message::Home(
                                        HomeMessage::Error(TransferError::WalletUnavailable(
                                            WalletKind::Vault,
                                        )),
                                    )));
                                };
                                self.current_view.next();
                                tasks.push(Task::perform(
                                    async move {
                                        match daemon.get_new_address().await {
                                            Ok(res) => {
                                                Ok((res.address.to_string(), res.derivation_index))
                                            }
                                            Err(e) => Err(TransferError::NoDepositAddress {
                                                to: WalletKind::Vault,
                                                detail: e.to_string(),
                                            }),
                                        }
                                    },
                                    move |result| {
                                        Message::View(view::Message::Home(
                                            HomeMessage::TransferDestinationReady {
                                                generation,
                                                direction,
                                                result,
                                            },
                                        ))
                                    },
                                ));
                                if let Ok(amount) = Amount::from_str_in(
                                    &self.entered_amount.value,
                                    if matches!(self.transfer_unit, BitcoinDisplayUnit::BTC) {
                                        breez_sdk_liquid::bitcoin::Denomination::Bitcoin
                                    } else {
                                        breez_sdk_liquid::bitcoin::Denomination::Satoshi
                                    },
                                ) {
                                    let Some(breez_client) = self.breez_client.clone() else {
                                        return Task::none();
                                    };
                                    let pay_amount = if self.sends_max() {
                                        breez_sdk_liquid::model::PayAmount::Drain
                                    } else {
                                        breez_sdk_liquid::model::PayAmount::Bitcoin {
                                            receiver_amount_sat: amount.to_sat(),
                                        }
                                    };
                                    tasks.push(Task::perform(
                                        async move {
                                            breez_client
                                                .prepare_pay_onchain(&PreparePayOnchainRequest {
                                                    fee_rate_sat_per_vbyte: None,
                                                    amount: pay_amount,
                                                })
                                                .await
                                        },
                                        move |result| match result {
                                            Ok(response) => Message::View(view::Message::Home(
                                                HomeMessage::PrepareOnChainResponseReceived {
                                                    generation,
                                                    direction,
                                                    response,
                                                },
                                            )),
                                            Err(error) => Message::View(view::Message::Home(
                                                HomeMessage::TransferDestinationFailed {
                                                    generation,
                                                    direction,
                                                    error: TransferError::FeeEstimateFailed {
                                                        detail: error.to_string(),
                                                    },
                                                },
                                            )),
                                        },
                                    ))
                                }
                            } else if matches!(
                                self.transfer_direction,
                                Some(TransferDirection::VaultToLiquid)
                            ) {
                                self.current_view.next();
                                let Some(breez_client) = self.breez_client.clone() else {
                                    return Task::none();
                                };
                                tasks.push(Task::perform(
                                    async move {
                                        let result = breez_client.receive_onchain(None).await;
                                        result
                                    },
                                    move |result| {
                                        Message::View(view::Message::Home(
                                            HomeMessage::TransferDestinationReady {
                                                generation,
                                                direction,
                                                result: result
                                                    .map(|response| {
                                                        (
                                                            response.destination,
                                                            ChildNumber::Normal { index: 1 },
                                                        )
                                                    })
                                                    .map_err(|error| {
                                                        TransferError::NoDepositAddress {
                                                            to: WalletKind::Liquid,
                                                            detail: error.to_string(),
                                                        }
                                                    }),
                                            },
                                        ))
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
                                        move |result| match result {
                                            // `payment_request` is a plain Bitcoin address for
                                            // on-chain receives (possibly wrapped in a BIP21
                                            // URI — the `TransferDestinationReady` handler
                                            // already strips "bitcoin:" prefixes).
                                            Ok(response) => Message::View(view::Message::Home(
                                                HomeMessage::TransferDestinationReady {
                                                    generation,
                                                    direction,
                                                    result: Ok((
                                                        response.payment_request,
                                                        ChildNumber::Normal { index: 1 },
                                                    )),
                                                },
                                            )),
                                            Err(error) => Message::View(view::Message::Home(
                                                HomeMessage::TransferDestinationReady {
                                                    generation,
                                                    direction,
                                                    result: Err(TransferError::NoDepositAddress {
                                                        to: WalletKind::Spark,
                                                        detail: error.to_string(),
                                                    }),
                                                },
                                            )),
                                        },
                                    ));
                                } else {
                                    return Task::done(Message::View(view::Message::Home(
                                        HomeMessage::Error(TransferError::WalletUnavailable(
                                            WalletKind::Spark,
                                        )),
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
                                        HomeMessage::Error(TransferError::WalletUnavailable(
                                            WalletKind::Spark,
                                        )),
                                    )));
                                };
                                self.current_view.next();
                                tasks.push(Task::perform(
                                    async move { spark.receive_onchain(None).await },
                                    move |result| {
                                        Message::View(view::Message::Home(
                                            HomeMessage::TransferDestinationReady {
                                                generation,
                                                direction,
                                                result: result
                                                    .map(|response| {
                                                        (
                                                            response.payment_request,
                                                            ChildNumber::Normal { index: 1 },
                                                        )
                                                    })
                                                    .map_err(|error| {
                                                        TransferError::NoDepositAddress {
                                                            to: WalletKind::Spark,
                                                            detail: error.to_string(),
                                                        }
                                                    }),
                                            },
                                        ))
                                    },
                                ));
                                if let Ok(amount) = Amount::from_str_in(
                                    &self.entered_amount.value,
                                    if matches!(self.transfer_unit, BitcoinDisplayUnit::BTC) {
                                        breez_sdk_liquid::bitcoin::Denomination::Bitcoin
                                    } else {
                                        breez_sdk_liquid::bitcoin::Denomination::Satoshi
                                    },
                                ) {
                                    let Some(breez_client) = self.breez_client.clone() else {
                                        return Task::none();
                                    };
                                    let pay_amount = if self.sends_max() {
                                        breez_sdk_liquid::model::PayAmount::Drain
                                    } else {
                                        breez_sdk_liquid::model::PayAmount::Bitcoin {
                                            receiver_amount_sat: amount.to_sat(),
                                        }
                                    };
                                    tasks.push(Task::perform(
                                        async move {
                                            breez_client
                                                .prepare_pay_onchain(&PreparePayOnchainRequest {
                                                    fee_rate_sat_per_vbyte: None,
                                                    amount: pay_amount,
                                                })
                                                .await
                                        },
                                        move |result| match result {
                                            Ok(response) => Message::View(view::Message::Home(
                                                HomeMessage::PrepareOnChainResponseReceived {
                                                    generation,
                                                    direction,
                                                    response,
                                                },
                                            )),
                                            Err(error) => Message::View(view::Message::Home(
                                                HomeMessage::TransferDestinationFailed {
                                                    generation,
                                                    direction,
                                                    error: TransferError::FeeEstimateFailed {
                                                        detail: error.to_string(),
                                                    },
                                                },
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
                                    if matches!(self.transfer_unit, BitcoinDisplayUnit::BTC) {
                                        coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                                    } else {
                                        coincube_core::miniscript::bitcoin::Denomination::Satoshi
                                    },
                                ) else {
                                    return Task::none();
                                };
                                let Some(spark) = self.spark_backend.clone() else {
                                    return Task::done(Message::View(view::Message::Home(
                                        HomeMessage::Error(TransferError::WalletUnavailable(
                                            WalletKind::Spark,
                                        )),
                                    )));
                                };
                                self.current_view.next();
                                let Some(breez_client) = self.breez_client.clone() else {
                                    return Task::none();
                                };
                                let amount_sat = amount.to_sat();
                                let fees_included = self.sends_max();
                                tasks.push(Task::perform(
                                    async move {
                                        let addr_res = breez_client
                                            .receive_onchain(None)
                                            .await
                                            .map_err(|e| TransferError::NoDepositAddress {
                                                to: WalletKind::Liquid,
                                                detail: e.to_string(),
                                            })?;
                                        let destination = addr_res.destination;
                                        let prep = if fees_included {
                                            spark
                                                .prepare_send_fees_included(
                                                    destination.clone(),
                                                    amount_sat,
                                                )
                                                .await
                                        } else {
                                            spark
                                                .prepare_send(destination.clone(), Some(amount_sat))
                                                .await
                                        }
                                        .map_err(|e| TransferError::FeeEstimateFailed {
                                            detail: e.to_string(),
                                        })?;
                                        Ok::<_, TransferError>((
                                            destination,
                                            prep.handle,
                                            prep.fee_sat,
                                        ))
                                    },
                                    move |result| match result {
                                        Ok((destination, prepare_handle, fee_sat)) => {
                                            Message::View(view::Message::Home(
                                                HomeMessage::SparkPrepareSendReady {
                                                    generation,
                                                    direction,
                                                    destination,
                                                    prepare_handle,
                                                    fee_sat,
                                                },
                                            ))
                                        }
                                        Err(error) => Message::View(view::Message::Home(
                                            HomeMessage::TransferDestinationFailed {
                                                generation,
                                                direction,
                                                error,
                                            },
                                        )),
                                    },
                                ));
                            } else if matches!(
                                direction,
                                TransferDirection::SparkToVault
                                    | TransferDirection::SparkToOtherCube
                                    | TransferDirection::SparkToOtherSpark
                            ) {
                                // SparkToVault: fetch a fresh Vault address, then
                                // ask Spark to `prepare_send` to it — that call
                                // yields a handle we broadcast at confirm time via
                                // `ConfirmSparkSend`.
                                let Ok(amount) = Amount::from_str_in(
                                    &self.entered_amount.value,
                                    if matches!(self.transfer_unit, BitcoinDisplayUnit::BTC) {
                                        coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                                    } else {
                                        coincube_core::miniscript::bitcoin::Denomination::Satoshi
                                    },
                                ) else {
                                    return Task::none();
                                };
                                let Some(spark) = self.spark_backend.clone() else {
                                    return Task::done(Message::View(view::Message::Home(
                                        HomeMessage::Error(TransferError::WalletUnavailable(
                                            WalletKind::Spark,
                                        )),
                                    )));
                                };
                                let other_address = if direction.is_other_cube() {
                                    self.validate_entered_amount(cache);
                                    if !self.entered_amount.valid {
                                        return Task::none();
                                    }
                                    match self.other_receive_address(cache) {
                                        Ok(OtherReceiveAddress::Bitcoin(address)) => {
                                            Some(address.address.to_string())
                                        }
                                        Ok(OtherReceiveAddress::Spark(address)) => {
                                            Some(address.clone())
                                        }
                                        Err(refusal) => {
                                            return self.restart_other_cube_unlock(refusal)
                                        }
                                    }
                                } else {
                                    None
                                };
                                let daemon = daemon.clone();
                                self.current_view.next();
                                let amount_sat = amount.to_sat();
                                let fees_included = self.sends_max();
                                tasks.push(Task::perform(
                                    async move {
                                        let addr_str = match other_address {
                                            Some(address) => address,
                                            None => daemon
                                                .ok_or(TransferError::WalletUnavailable(
                                                    WalletKind::Vault,
                                                ))?
                                                .get_new_address()
                                                .await
                                                .map_err(|e| TransferError::NoDepositAddress {
                                                    to: WalletKind::Vault,
                                                    detail: e.to_string(),
                                                })?
                                                .address
                                                .to_string(),
                                        };
                                        let prep = if fees_included {
                                            spark
                                                .prepare_send_fees_included(
                                                    addr_str.clone(),
                                                    amount_sat,
                                                )
                                                .await
                                        } else {
                                            spark
                                                .prepare_send(addr_str.clone(), Some(amount_sat))
                                                .await
                                        }
                                        .map_err(|e| TransferError::FeeEstimateFailed {
                                            detail: e.to_string(),
                                        })?;
                                        let method =
                                            if direction == TransferDirection::SparkToOtherSpark {
                                                "SparkAddress"
                                            } else {
                                                "BitcoinAddress"
                                            };
                                        if direction.is_other_cube()
                                            && (prep.method != method
                                                || prep.amount_sat != amount_sat)
                                        {
                                            return Err(TransferError::DestinationRejected {
                                                detail: coincube_ui::locale::copy(
                                                    "Spark quoted a different rail or amount",
                                                )
                                                .into(),
                                            });
                                        }
                                        Ok::<_, TransferError>((
                                            addr_str,
                                            prep.handle,
                                            prep.fee_sat,
                                        ))
                                    },
                                    move |result| match result {
                                        Ok((destination, prepare_handle, fee_sat)) => {
                                            Message::View(view::Message::Home(
                                                HomeMessage::SparkPrepareSendReady {
                                                    generation,
                                                    direction,
                                                    destination,
                                                    prepare_handle,
                                                    fee_sat,
                                                },
                                            ))
                                        }
                                        Err(error) => Message::View(view::Message::Home(
                                            HomeMessage::TransferDestinationFailed {
                                                generation,
                                                direction,
                                                error,
                                            },
                                        )),
                                    },
                                ));
                            }
                            return Task::batch(tasks);
                        }
                        Task::none()
                    }
                    HomeMessage::PreviousStep
                        if self.current_view.step == 1
                            && self.transfer_page != TransferPage::Wallets =>
                    {
                        let other = self
                            .transfer_direction
                            .is_some_and(|direction| direction.is_other_cube());
                        self.transfer_page = match self.transfer_page {
                            TransferPage::OtherCube => {
                                self.reset_other_cube();
                                TransferPage::Wallets
                            }
                            TransferPage::Unlock => {
                                self.lock_other_cube();
                                TransferPage::OtherCube
                            }
                            TransferPage::Amount if other => TransferPage::Unlock,
                            _ => TransferPage::Wallets,
                        };
                        Task::none()
                    }
                    HomeMessage::PreviousStep => {
                        if self.is_sending
                            && self
                                .transfer_direction
                                .is_some_and(|direction| direction.is_other_cube())
                        {
                            return Task::none();
                        }
                        // Any prepared/signed artifact from the confirm screen
                        // is bound to the amount + destination that were in
                        // effect when it was built. Stepping back allows the
                        // user to edit those, so invalidate here — otherwise a
                        // returning user would land on the confirm screen with
                        // a stale "Confirm & Broadcast" button and broadcast
                        // the previous PSBT against a changed amount.
                        self.invalidate_transfer_destination();
                        // Mirror `BackToHome`'s reset: if we left the confirm
                        // screen with a stuck `is_sending` (e.g. an error path
                        // that didn't clear it), leaving it set would disable
                        // all confirm buttons on re-entry. The Previous button
                        // is also view-level gated on `!is_sending` so a
                        // successful in-flight broadcast can't race this
                        // handler into advancing the step machine.
                        self.is_sending = false;
                        self.current_view.previous();
                        // Back on Amount, Max shows the whole balance again.
                        if let Some(source) = self.transfer_max.filter(|_| self.sends_max()) {
                            self.set_entered_amount(self.max_transfer_amount(source, cache));
                        }
                        Task::none()
                    }
                    HomeMessage::SparkPrepareSendReady {
                        generation,
                        direction,
                        destination,
                        prepare_handle,
                        fee_sat,
                    } => {
                        if !self.destination_response_is_current(generation, direction) {
                            return Task::none();
                        }
                        if direction == TransferDirection::SparkToOtherSpark {
                            match self.other_receive_address(cache) {
                                Ok(OtherReceiveAddress::Spark(address))
                                    if *address == destination => {}
                                Ok(_) => {
                                    return self
                                        .restart_other_cube_unlock(ScopeRefusal::IdentityChanged)
                                }
                                Err(refusal) => return self.restart_other_cube_unlock(refusal),
                            }
                            self.destination_ready = Some((generation, direction));
                            self.spark_send_handle = Some(prepare_handle);
                            self.show_max_after_fee(fee_sat);
                            return Task::none();
                        }
                        // Parse the destination as a checked Bitcoin address — if
                        // the string is a BIP21 URI (rare here but cheap to
                        // handle) strip the prefix first. Mirrors the logic in
                        // `TransferDestinationReady`. Destination is sourced from our
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
                                return Task::done(Message::View(view::Message::Home(
                                    HomeMessage::Error(TransferError::DestinationRejected {
                                        detail: format!(
                                            "Spark destination {addr_str} is not valid for network {:?}",
                                            cache.network
                                        ),
                                    }),
                                )));
                            }
                        };
                        let index = if direction.is_other_cube() {
                            match self.other_receive_address(cache) {
                                Ok(OtherReceiveAddress::Bitcoin(address))
                                    if address.address == checked =>
                                {
                                    address.index
                                }
                                Ok(_) => {
                                    return self
                                        .restart_other_cube_unlock(ScopeRefusal::IdentityChanged)
                                }
                                Err(refusal) => return self.restart_other_cube_unlock(refusal),
                            }
                        } else {
                            ChildNumber::Normal { index: 0 }
                        };
                        self.receive_address_info = Some(ReceiveAddressInfo {
                            address: checked,
                            index,
                            labels: HashMap::new(),
                        });
                        self.destination_ready = Some((generation, direction));
                        self.spark_send_handle = Some(prepare_handle);
                        self.show_max_after_fee(fee_sat);
                        Task::none()
                    }
                    HomeMessage::ConfirmSparkSend => {
                        let Some(direction) = self.transfer_direction else {
                            return Task::none();
                        };
                        if self.is_sending || !self.destination_is_ready() {
                            return Task::none();
                        }
                        if direction == TransferDirection::SparkToVault
                            && !self.legacy_vault_available(cache)
                        {
                            return Task::none();
                        }
                        if direction.is_other_cube() {
                            if let Err(refusal) = self.check_other_review_address(cache) {
                                return self.restart_other_cube_unlock(refusal);
                            }
                        }
                        // Spark-sourced confirm: broadcast the prepared send. The
                        // destination address + prepare handle were populated at
                        // step 1→2 (see `SparkPrepareSendReady`).
                        let Some(spark) = self.spark_backend.clone() else {
                            // Not an expired preparation — the wallet itself is
                            // gone, and "go back and re-enter the amount" would
                            // send the user in a circle.
                            return Task::done(Message::View(view::Message::Home(
                                HomeMessage::Error(TransferError::WalletUnavailable(
                                    WalletKind::Spark,
                                )),
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
                                HomeMessage::Error(TransferError::PreparationExpired),
                            )));
                        };
                        let Ok(transfer_amount) = Amount::from_str_in(
                            &self.entered_amount.value,
                            if matches!(self.transfer_unit, BitcoinDisplayUnit::BTC) {
                                coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                            } else {
                                coincube_core::miniscript::bitcoin::Denomination::Satoshi
                            },
                        ) else {
                            return Task::none();
                        };
                        self.is_sending = true;
                        let generation = self.transfer_request_generation;
                        Task::perform(
                            async move { spark.send_payment(handle, None).await },
                            move |result| {
                                let result = result.map(|_| ()).map_err(|error| match error {
                                    SparkClientError::OutcomeUnknown { .. }
                                    | SparkClientError::Protocol(_) => {
                                        TransferError::BroadcastOutcomeUnknown {
                                            detail: error.to_string(),
                                        }
                                    }
                                    _ => TransferError::BroadcastFailed {
                                        from: WalletKind::Spark,
                                        detail: error.to_string(),
                                    },
                                });
                                Message::View(view::Message::Home(
                                    HomeMessage::SparkTransferFinished {
                                        generation,
                                        direction,
                                        amount_sat: transfer_amount.to_sat(),
                                        result,
                                    },
                                ))
                            },
                        )
                    }
                    HomeMessage::SparkTransferFinished {
                        generation,
                        direction,
                        amount_sat,
                        result,
                    } => {
                        if !self.destination_response_is_current(generation, direction) {
                            return Task::none();
                        }
                        let message = match result {
                            Ok(()) => HomeMessage::TransferBroadcast {
                                amount_sat,
                                destination_kind: direction.to_kind(),
                                swap_id: None,
                            },
                            Err(error) => {
                                if direction.is_other_cube()
                                    && matches!(
                                        error,
                                        TransferError::BroadcastOutcomeUnknown { .. }
                                    )
                                {
                                    let reset = self.update(
                                        None,
                                        cache,
                                        Message::View(view::Message::Home(HomeMessage::BackToHome)),
                                    );
                                    let show_error = self.update(
                                        None,
                                        cache,
                                        Message::View(view::Message::Home(HomeMessage::Error(
                                            error,
                                        ))),
                                    );
                                    return Task::batch([reset, show_error]);
                                }
                                HomeMessage::Error(error)
                            }
                        };
                        self.update(daemon, cache, Message::View(view::Message::Home(message)))
                    }
                    HomeMessage::TransferBroadcast {
                        amount_sat,
                        destination_kind,
                        swap_id,
                    } => {
                        self.is_sending = false;
                        // Another Cube's Vault receives this one: nothing is
                        // pending in this Cube. Its unlock ends here.
                        if self
                            .transfer_direction
                            .is_some_and(|direction| direction.is_other_cube())
                        {
                            let _ = (amount_sat, destination_kind, swap_id);
                            self.current_view.next();
                            self.finish_other_cube();
                            return Task::none();
                        }
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
                                // from growth relative to this moment — but
                                // only on a fresh batch. If a prior broadcast
                                // is still unreconciled, its amount may
                                // already sit in `cache_vault_pending_receive_sats`;
                                // re-snapshotting here would fold it into the
                                // baseline while `pending_vault_receive_sats`
                                // still includes it, making the reconciliation
                                // bar (`baseline + pending`) permanently
                                // unreachable.
                                if self.pending_vault_receive_sats == 0 {
                                    self.pending_vault_receive_baseline_sats =
                                        cache_vault_pending_receive_sats(cache);
                                }
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
                                // A transfer, not a swap: clear any stale swap flag
                                // so it can't fire a celebration on this claim.
                                self.pending_spark_incoming_is_swap = false;
                                // Drop any in-flight auto-claim tracker from a
                                // prior transfer: its result would otherwise
                                // land with the new pending set and stamp this
                                // (unrelated) transfer as Completed. The
                                // `AutoClaimSparkResult` handler also gates on
                                // this field for defense in depth.
                                self.auto_claiming_spark_deposit = None;
                                self.spark_claim_awaiting_settlement = false;
                            }
                        }
                        self.current_view.next();
                        self.spark_send_handle = None;
                        Task::none()
                    }
                    HomeMessage::FetchTransferFeeratePreset(preset) => {
                        if self.is_sending {
                            return Task::none();
                        }
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
                        if self.is_sending {
                            self.transfer_feerate_loading = None;
                            return Task::none();
                        }
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
                                Task::done(Message::View(view::Message::ShowError(
                                    crate::user_error::UserError::logged(
                                        "Couldn't fetch the current fee rate",
                                        "Check your internet connection and try again, or enter a fee rate yourself.",
                                        crate::user_error::CC_NET_UNKNOWN,
                                        true,
                                        format!("fee estimator, preset {preset:?}: {e}"),
                                    )
                                    .toast(),
                                )))
                            }
                        }
                    }
                    HomeMessage::SetTransferFeerate(value) => {
                        if self.is_sending {
                            return Task::none();
                        }
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
                        if self.wallet_picker == Some(side) {
                            self.wallet_picker = None;
                            return Task::none();
                        }
                        self.wallet_picker = Some(side);
                        // The To menu lists other local Cubes the source can
                        // reach; read the catalog (names only, every Cube
                        // locked) as it opens.
                        if side == PickerSide::To && self.current_view.step == 1 {
                            if let Some(source) = self.other_cube_source(cache) {
                                self.other_cubes = other_cubes(&cache.datadir_path, &source);
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::CloseWalletPicker => {
                        self.wallet_picker = None;
                        Task::none()
                    }
                    selection @ (HomeMessage::SelectWalletInPicker(kind)
                    | HomeMessage::SelectTransferWallet(_, kind)) => {
                        if (kind == WalletKind::Spark && self.spark_backend.is_none())
                            || (kind == WalletKind::Liquid
                                && !crate::app::features::liquid_wallet_usable(
                                    cache.network,
                                    cache.liquid_gate,
                                ))
                        {
                            return Task::none();
                        }
                        if !self.legacy_vault_available(cache)
                            && (kind == WalletKind::Vault
                                || self.transfer_from == Some(WalletKind::Vault)
                                || self.transfer_to == Some(WalletKind::Vault))
                        {
                            return Task::none();
                        }
                        // Apply either the legacy popup or inline-row choice.
                        // The To list excludes the current source. A From
                        // choice matching To swaps the existing pair.
                        let side = match selection {
                            HomeMessage::SelectTransferWallet(side, _) => Some(side),
                            _ => self.wallet_picker,
                        };
                        if let Some(side) = side {
                            if side == PickerSide::To && self.transfer_from == Some(kind) {
                                return Task::none();
                            }
                            if side == PickerSide::From && self.transfer_from == Some(kind) {
                                self.wallet_picker = None;
                                return Task::none();
                            }
                            let old_pair = (self.transfer_from, self.transfer_to);
                            let was_other = self
                                .transfer_direction
                                .is_some_and(|direction| direction.is_other_cube());
                            match side {
                                // A new source keeps a same-Cube pair: the
                                // old source becomes the destination when it
                                // was the destination, or when another Cube
                                // was (that Cube was bound to the old source).
                                PickerSide::From => {
                                    if self.transfer_to == Some(kind) || self.transfer_to.is_none()
                                    {
                                        self.transfer_to = self.transfer_from;
                                    }
                                    self.transfer_from = Some(kind);
                                }
                                PickerSide::To => self.transfer_to = Some(kind),
                            }
                            self.transfer_direction =
                                self.transfer_from.and_then(|from| match self.transfer_to {
                                    Some(to) => TransferDirection::try_from_pair(from, to),
                                    None => TransferDirection::to_other_cube(from, cache.chain()),
                                });
                            if old_pair != (self.transfer_from, self.transfer_to) {
                                self.invalidate_transfer_destination();
                            }
                            // Leaving another Cube locks it again; a same-Cube
                            // pair takes its amount on the same card.
                            if self
                                .transfer_direction
                                .is_some_and(|direction| !direction.is_other_cube())
                            {
                                if was_other {
                                    self.reset_other_cube();
                                }
                                self.transfer_page = TransferPage::Amount;
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
                    HomeMessage::SelectAnotherCube => {
                        if self.current_view.step != 1
                            || self.transfer_page != TransferPage::Wallets
                        {
                            return Task::none();
                        }
                        let Some(direction) = self
                            .transfer_from
                            .and_then(|from| TransferDirection::to_other_cube(from, cache.chain()))
                        else {
                            return Task::none();
                        };
                        if self.transfer_source_wallet(direction, cache).is_none() {
                            return Task::none();
                        }
                        if !self
                            .transfer_direction
                            .is_some_and(|direction| direction.is_other_cube())
                        {
                            self.transfer_to = None;
                            self.transfer_direction = Some(direction);
                            self.invalidate_transfer_destination();
                            self.validate_entered_amount(cache);
                        }
                        Task::none()
                    }
                    HomeMessage::ContinueFromWallets => {
                        if self.current_view.step != 1
                            || self.transfer_page != TransferPage::Wallets
                        {
                            return Task::none();
                        }
                        let Some(direction) = self.transfer_direction else {
                            return Task::none();
                        };
                        if direction.is_other_cube() {
                            let Some(source) = self.transfer_source(cache) else {
                                return Task::none();
                            };
                            self.reset_other_cube();
                            self.other_cubes = other_cubes(&cache.datadir_path, &source);
                            let selected = self
                                .other_cubes
                                .iter()
                                .find(|choice| choice.refusal.is_none())
                                .map(|choice| choice.cube_id.clone());
                            self.transfer_page = TransferPage::OtherCube;
                            if let Some(id) = selected {
                                self.select_other_cube(id, cache);
                            }
                        } else {
                            self.transfer_page = TransferPage::Amount;
                        }
                        Task::none()
                    }
                    HomeMessage::SelectOtherCube(cube_id) => {
                        if self.transfer_page == TransferPage::OtherCube {
                            self.select_other_cube(cube_id, cache);
                        }
                        Task::none()
                    }
                    HomeMessage::SelectOtherCubeWallet(kind) => {
                        if self.transfer_page == TransferPage::OtherCube
                            && self.other_cubes.iter().any(|choice| {
                                Some(choice.cube_id.as_str()) == self.other_cube_selected.as_deref()
                                    && choice.can_receive(kind)
                            })
                        {
                            self.select_other_wallet(kind, cache);
                        }
                        Task::none()
                    }
                    HomeMessage::ContinueWithCube => {
                        if self.transfer_page != TransferPage::OtherCube {
                            return Task::none();
                        }
                        let Some(source) = self.transfer_source(cache) else {
                            return Task::none();
                        };
                        let Some(kind) =
                            self.transfer_direction.map(|direction| direction.to_kind())
                        else {
                            return Task::none();
                        };
                        let Some((cube_id, wallet_id)) = self
                            .other_cube_selected
                            .as_ref()
                            .and_then(|id| {
                                self.other_cubes.iter().find(|choice| {
                                    &choice.cube_id == id && choice.can_receive(kind)
                                })
                            })
                            .map(|choice| {
                                let wallet = if kind == WalletKind::Vault {
                                    choice.vault_wallet_id.clone()
                                } else {
                                    None
                                };
                                (choice.cube_id.clone(), wallet)
                            })
                        else {
                            return Task::none();
                        };
                        self.lock_other_cube();
                        match ScopedDestination::new(
                            cache.datadir_path.clone(),
                            source,
                            &cube_id,
                            wallet_id,
                        ) {
                            Ok(scope) => {
                                self.other_scope = Some(scope);
                                self.transfer_page = TransferPage::Unlock;
                                return iced::widget::operation::focus(
                                    crate::pin_input::first_digit_id(),
                                );
                            }
                            Err(refusal) => self.other_error = Some(refusal.reason()),
                        }
                        Task::none()
                    }
                    HomeMessage::PickOtherCubeWallet(cube_id, kind) => {
                        self.wallet_picker = None;
                        if self.current_view.step != 1 {
                            return Task::none();
                        }
                        let Some(direction) = self.transfer_from.and_then(|from| {
                            TransferDirection::to_other_wallet(from, kind, cache.chain())
                        }) else {
                            return Task::none();
                        };
                        // The same Cube and wallet again: keep its unlock.
                        if self.other_cube_selected.as_deref() == Some(cube_id.as_str())
                            && self.transfer_direction == Some(direction)
                            && self.other_scope.is_some()
                        {
                            return Task::none();
                        }
                        let Some(wallet) = self.transfer_source_wallet(direction, cache) else {
                            return Task::none();
                        };
                        let source = Source {
                            cube_id: self.cube_id.clone(),
                            chain: cache.chain(),
                            wallet,
                            app: cache.app_generation,
                        };
                        self.reset_other_cube();
                        self.transfer_to = None;
                        self.transfer_direction = Some(direction);
                        self.invalidate_transfer_destination();
                        self.validate_entered_amount(cache);
                        self.other_cubes = other_cubes(&cache.datadir_path, &source);
                        if !self
                            .other_cubes
                            .iter()
                            .any(|choice| choice.cube_id == cube_id && choice.can_receive(kind))
                        {
                            self.transfer_page = TransferPage::Wallets;
                            return Task::none();
                        }
                        self.transfer_page = TransferPage::OtherCube;
                        self.other_cube_selected = Some(cube_id);
                        self.select_other_wallet(kind, cache);
                        self.update(
                            daemon.clone(),
                            cache,
                            Message::View(view::Message::Home(HomeMessage::ContinueWithCube)),
                        )
                    }
                    HomeMessage::OtherCubePin(message) => {
                        if self.transfer_page != TransferPage::Unlock
                            || self.other_phase != (UnlockPhase::Credential { busy: false })
                        {
                            return Task::none();
                        }
                        if matches!(message, crate::pin_input::Message::Submit) {
                            return Task::done(Message::View(view::Message::Home(
                                HomeMessage::UnlockOtherCube,
                            )));
                        }
                        self.other_pin.update(message).map(|message| {
                            Message::View(view::Message::Home(HomeMessage::OtherCubePin(message)))
                        })
                    }
                    HomeMessage::UnlockOtherCube => {
                        if self.transfer_page != TransferPage::Unlock
                            || self.other_phase != (UnlockPhase::Credential { busy: false })
                            || !self.other_pin.is_complete()
                        {
                            return Task::none();
                        }
                        let Some(source) = self.transfer_source(cache) else {
                            return Task::none();
                        };
                        let pin = self.other_pin.value();
                        self.other_pin.clear();
                        let Some(scope) = self.other_scope.as_mut() else {
                            return Task::none();
                        };
                        match scope.begin_pin(&source, pin) {
                            Ok(request) => {
                                self.other_phase = UnlockPhase::Credential { busy: true };
                                self.other_error = None;
                                Task::perform(request.run_blocking(), |check| {
                                    Message::View(view::Message::Home(
                                        HomeMessage::OtherCubePinChecked(check),
                                    ))
                                })
                            }
                            Err(refusal) => {
                                self.other_error = Some(refusal.reason());
                                Task::none()
                            }
                        }
                    }
                    HomeMessage::OtherCubePinChecked(check) => {
                        // A duress PIN is the same emergency here as at the
                        // Launcher, whatever changed meanwhile: lock the other
                        // Cube, leave the flow and hand the one-use proof to
                        // the tab, which runs the existing duress activation.
                        if let Some(proof) = check.duress_proof() {
                            let reset = self.update(
                                daemon.clone(),
                                cache,
                                Message::View(view::Message::Home(HomeMessage::BackToHome)),
                            );
                            return Task::batch([
                                reset,
                                Task::done(Message::View(view::Message::DuressActivateLocal(
                                    proof,
                                ))),
                            ]);
                        }
                        let Some(source) = self.transfer_source(cache) else {
                            return Task::none();
                        };
                        let Some(scope) = self.other_scope.as_mut() else {
                            return Task::none();
                        };
                        let result = scope.accept_pin(&source, check);
                        self.after_other_credential(&source, result)
                    }
                    HomeMessage::UnlockOtherCubePasskey => {
                        if self.transfer_page != TransferPage::Unlock
                            || self.other_phase != (UnlockPhase::Credential { busy: false })
                        {
                            return Task::none();
                        }
                        #[cfg(target_os = "macos")]
                        {
                            let Some(source) = self.transfer_source(cache) else {
                                return Task::none();
                            };
                            let Some(scope) = self.other_scope.as_mut() else {
                                return Task::none();
                            };
                            match scope.begin_passkey(&source) {
                                Ok(()) => {
                                    self.other_phase = UnlockPhase::Credential { busy: true };
                                    self.other_error = None;
                                }
                                Err(refusal) => self.other_error = Some(refusal.reason()),
                            }
                        }
                        #[cfg(not(target_os = "macos"))]
                        {
                            self.other_error = Some(
                                coincube_ui::locale::copy(
                                    "Passkey unlock is available on macOS only.",
                                )
                                .into(),
                            );
                        }
                        Task::none()
                    }
                    HomeMessage::OtherCubePasskeyPoll => {
                        #[cfg(target_os = "macos")]
                        {
                            if self.other_phase != (UnlockPhase::Credential { busy: true }) {
                                return Task::none();
                            }
                            let Some(source) = self.transfer_source(cache) else {
                                return Task::none();
                            };
                            let Some(scope) = self.other_scope.as_mut() else {
                                return Task::none();
                            };
                            if let Some(result) = scope.poll_passkey(&source) {
                                return self.after_other_credential(&source, result);
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::OtherCubeBackendBound(reply) => {
                        if self.other_phase != UnlockPhase::Checks(1) {
                            return Task::none();
                        }
                        let Some(source) = self.transfer_source(cache) else {
                            return Task::none();
                        };
                        let Some(scope) = self.other_scope.as_mut() else {
                            return Task::none();
                        };
                        match scope
                            .accept_backend_reply(&source, reply)
                            .and_then(|()| scope.request_address(&source))
                        {
                            Ok(request) => {
                                self.other_phase = UnlockPhase::Checks(2);
                                Task::perform(request.run(), |reply| {
                                    Message::View(view::Message::Home(
                                        HomeMessage::OtherCubeAddress(reply),
                                    ))
                                })
                            }
                            Err(refusal) => {
                                self.other_error = Some(refusal.reason());
                                Task::none()
                            }
                        }
                    }
                    HomeMessage::OtherCubeAddress(reply) => {
                        if self.other_phase != UnlockPhase::Checks(2) {
                            return Task::none();
                        }
                        let Some(source) = self.transfer_source(cache) else {
                            return Task::none();
                        };
                        let Some(scope) = self.other_scope.as_ref() else {
                            return Task::none();
                        };
                        match scope.accept_address(&source, reply) {
                            Ok(address) => {
                                self.other_address = Some(OtherReceiveAddress::Bitcoin(address));
                                self.other_phase = UnlockPhase::Checks(UNLOCK_CHECKS);
                                self.other_error = None;
                                if self.transfer_page == TransferPage::Unlock {
                                    self.transfer_page = TransferPage::Amount;
                                }
                            }
                            Err(refusal) => self.other_error = Some(refusal.reason()),
                        }
                        Task::none()
                    }
                    HomeMessage::OtherCubeSparkAddress(reply) => {
                        if self.other_phase != UnlockPhase::Checks(1) {
                            return Task::none();
                        }
                        let Some(source) = self.transfer_source(cache) else {
                            return Task::none();
                        };
                        let Some(scope) = self.other_scope.as_ref() else {
                            return Task::none();
                        };
                        match scope.accept_spark_address(&source, reply) {
                            Ok(address) => {
                                self.other_address = Some(address);
                                self.other_phase = UnlockPhase::Checks(UNLOCK_CHECKS);
                                self.other_error = None;
                                if self.transfer_page == TransferPage::Unlock {
                                    self.transfer_page = TransferPage::Amount;
                                }
                            }
                            Err(ScopeRefusal::Stale) => {}
                            Err(refusal) => self.other_error = Some(refusal.reason()),
                        }
                        Task::none()
                    }
                    HomeMessage::ContinueFromUnlock => {
                        if self.transfer_page == TransferPage::Unlock
                            && self.other_phase == UnlockPhase::Checks(UNLOCK_CHECKS)
                            && self.other_address.is_some()
                        {
                            self.transfer_page = TransferPage::Amount;
                        }
                        Task::none()
                    }
                    HomeMessage::LeaveTransfer => {
                        let origin = self.transfer_origin;
                        let reset = self.update(
                            daemon.clone(),
                            cache,
                            Message::View(view::Message::Home(HomeMessage::BackToHome)),
                        );
                        match origin {
                            Some(wallet) => Task::batch([
                                reset,
                                crate::app::state::redirect(transfer_origin_menu(wallet).clone()),
                            ]),
                            None => reset,
                        }
                    }
                    // A wallet page's Transfer is routed by the App, which
                    // switches to the Overview first (`start_transfer_from`).
                    HomeMessage::TransferFromWallet(_) => Task::none(),
                    HomeMessage::OpenTransferActivity => {
                        let reset = self.update(
                            daemon.clone(),
                            cache,
                            Message::View(view::Message::Home(HomeMessage::BackToHome)),
                        );
                        Task::batch([
                            reset,
                            Task::done(Message::View(view::Message::Menu(Menu::Cube(
                                crate::app::menu::CubeSubMenu::Activity,
                            )))),
                        ])
                    }
                    HomeMessage::RestartTransfer => {
                        // Opened from a wallet page, the new transfer starts
                        // from that wallet again and still returns there.
                        if let Some(origin) = self.transfer_origin {
                            return self.start_transfer_from(daemon.clone(), cache, origin);
                        }
                        let reset = self.update(
                            daemon.clone(),
                            cache,
                            Message::View(view::Message::Home(HomeMessage::BackToHome)),
                        );
                        let open = self.update(
                            daemon.clone(),
                            cache,
                            Message::View(view::Message::Home(HomeMessage::NextStep)),
                        );
                        Task::batch([reset, open])
                    }
                    HomeMessage::AmountEdited(amount) => {
                        if self
                            .transfer_direction
                            .is_some_and(|direction| direction.is_other_cube())
                            && (self.current_view.step != 1
                                || self.transfer_page != TransferPage::Amount)
                        {
                            return Task::none();
                        }
                        self.entered_amount.value = amount;
                        self.transfer_max = None;
                        self.validate_entered_amount(cache);
                        Task::none()
                    }
                    HomeMessage::TransferMax => {
                        if self.current_view.step != 1 || self.transfer_page != TransferPage::Amount
                        {
                            return Task::none();
                        }
                        let Some(source) = self.transfer_direction.map(|d| d.from_kind()) else {
                            return Task::none();
                        };
                        self.set_entered_amount(self.max_transfer_amount(source, cache));
                        self.transfer_max = Some(source);
                        self.validate_entered_amount(cache);
                        Task::none()
                    }
                    HomeMessage::SignVaultToLiquidTx => {
                        if !self.legacy_vault_available(cache) {
                            return Task::none();
                        }
                        if !self.destination_is_ready() {
                            return Task::none();
                        }
                        let generation = self.transfer_request_generation;
                        if let Some(transfer_direction) = self.transfer_direction {
                            // Shared signer path for both Vault-sourced directions —
                            // `receive_address_info` is the only thing that differs
                            // between them (populated from Breez for VaultToLiquid,
                            // from Spark's `receive_onchain` for VaultToSpark).
                            if matches!(
                                transfer_direction,
                                TransferDirection::VaultToLiquid
                                    | TransferDirection::VaultToSpark
                                    | TransferDirection::VaultToOtherSpark
                                    | TransferDirection::VaultToOtherCube
                            ) {
                                // Another Cube must still be unlocked for this
                                // transfer (leaving the page locks it), whichever
                                // of its wallets receives.
                                if transfer_direction.is_other_cube() {
                                    if let Err(refusal) = self.check_other_review_address(cache) {
                                        return self.restart_other_cube_unlock(refusal);
                                    }
                                }
                                if let Some(address_info) = self.receive_address_info.clone() {
                                    if let Some(daemon) = daemon {
                                        let denomination = if matches!(
                                            self.transfer_unit,
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
                                            let destination = address_info.address.to_string();
                                            let mut destinations = std::collections::HashMap::new();
                                            destinations.insert(
                                                address_info.address.as_unchecked().clone(),
                                                amount_sat,
                                            );

                                            // Max sweeps every owned coin to the
                                            // destination, as the preview priced it.
                                            let sweep = self
                                                .sends_max()
                                                .then(|| vault_sweep_coins(cache).0);
                                            let script = address_info.address.script_pubkey();
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
                                                            TransferError::InvalidFeerate,
                                                        )),
                                                    ));
                                                }
                                            };
                                            self.is_sending = true;
                                            self.transfer_feerate_loading = None;
                                            self.next_psbt_request_id =
                                                self.next_psbt_request_id.wrapping_add(1);
                                            let request_id = self.next_psbt_request_id;
                                            self.active_psbt_request = Some(request_id);
                                            self.transfer_spend_tx = None;
                                            self.transfer_psbt_ready_for = None;
                                            self.transfer_signed = false;
                                            return Task::perform(
                                                async move {
                                                    let built = match &sweep {
                                                        Some(coins) => {
                                                            daemon_clone
                                                                .create_spend_tx(
                                                                    coins,
                                                                    &HashMap::new(),
                                                                    feerate_vb,
                                                                    destinations
                                                                        .keys()
                                                                        .next()
                                                                        .cloned(),
                                                                )
                                                                .await
                                                        }
                                                        None => {
                                                            daemon_clone
                                                                .create_spend_tx(
                                                                    &[],
                                                                    &destinations,
                                                                    feerate_vb,
                                                                    None,
                                                                )
                                                                .await
                                                        }
                                                    };
                                                    let psbt = match built {
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

                                                    if sweep.is_some()
                                                        && paid_to(&psbt, &script).to_sat()
                                                            != amount_sat
                                                    {
                                                        return Err(coincube_ui::locale::copy("The Vault balance changed. Go back and choose Max again.").to_string());
                                                    }

                                                    daemon_clone
                                                        .update_spend_tx(&psbt)
                                                        .await
                                                        .map_err(|e| {
                                                            format!("Failed to save PSBT: {}", e)
                                                        })?;

                                                    Ok((psbt, wallet, cache_clone))
                                                },
                                                move |result| {
                                                    Message::View(view::Message::Home(
                                                        HomeMessage::TransferPsbtReady {
                                                            request_id,
                                                            generation,
                                                            direction: transfer_direction,
                                                            destination,
                                                            amount_sat,
                                                            feerate_vb,
                                                            result,
                                                        },
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
                        if !self.destination_is_ready() {
                            return Task::none();
                        }
                        if self
                            .transfer_direction
                            .is_some_and(Self::direction_uses_vault)
                            && !self.legacy_vault_available(cache)
                        {
                            return Task::none();
                        }
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
                                            if matches!(self.transfer_unit, BitcoinDisplayUnit::BTC)
                                            {
                                                breez_sdk_liquid::bitcoin::Denomination::Bitcoin
                                            } else {
                                                breez_sdk_liquid::bitcoin::Denomination::Satoshi
                                            },
                                        ) else {
                                            self.entered_amount.valid = false;
                                            return Task::none();
                                        };
                                        let Some(breez_client) = self.breez_client.clone() else {
                                            return Task::none();
                                        };
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
                                                    HomeMessage::Error(
                                                        TransferError::BroadcastFailed {
                                                            from: WalletKind::Liquid,
                                                            detail: error.to_string(),
                                                        },
                                                    ),
                                                )),
                                            },
                                        );
                                    }
                                }
                            } else if matches!(
                                transfer_direction,
                                TransferDirection::VaultToLiquid
                                    | TransferDirection::VaultToSpark
                                    | TransferDirection::VaultToOtherSpark
                                    | TransferDirection::VaultToOtherCube
                            ) {
                                // Another Cube must still be unlocked for this
                                // transfer (leaving the page locks it), whichever
                                // of its wallets receives.
                                if transfer_direction.is_other_cube() {
                                    if let Err(refusal) = self.check_other_review_address(cache) {
                                        return self.restart_other_cube_unlock(refusal);
                                    }
                                }
                                // Post-sign broadcast path for both Vault-sourced
                                // directions. The PSBT was built earlier in the same
                                // shared `SignVaultToLiquidTx` handler against the
                                // destination address stored in `receive_address_info`.
                                if self.transfer_psbt_ready_for
                                    != Some((
                                        self.next_psbt_request_id,
                                        self.transfer_request_generation,
                                        transfer_direction,
                                    ))
                                {
                                    return Task::none();
                                }
                                if self.transfer_signed {
                                    if let Some(spend_tx) = &self.transfer_spend_tx {
                                        if let Some(daemon) = daemon {
                                            let txid = spend_tx.psbt.unsigned_tx.compute_txid();
                                            let daemon_clone = daemon.clone();
                                            let to_kind = transfer_direction.to_kind();
                                            let Ok(amount_sat) = Amount::from_str_in(
                                                &self.entered_amount.value,
                                                if matches!(
                                                    self.transfer_unit,
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
                                                        // Vault to Vault is only ever
                                                        // another Cube's Vault: nothing is
                                                        // pending in this Cube.
                                                        WalletKind::Vault => {
                                                            Message::View(view::Message::Home(
                                                                HomeMessage::TransferSuccessful,
                                                            ))
                                                        }
                                                    },
                                                    Err(e) => Message::View(view::Message::Home(
                                                        HomeMessage::Error(
                                                            TransferError::BroadcastFailed {
                                                                from: WalletKind::Vault,
                                                                detail: e,
                                                            },
                                                        ),
                                                    )),
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
                        // `TransferError` already knows which wallet is unwell
                        // and whether anything was sent; `report_transfer` turns
                        // that into copy and logs the detail once.
                        self.is_sending = false;
                        Task::done(Message::View(view::Message::ShowError(
                            crate::user_error::report_transfer(&err),
                        )))
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
                    HomeMessage::StoreSparkShift(record) => {
                        self.store_pending_spark_shift(record.as_deref().cloned())
                    }
                    HomeMessage::SwapSettledAwaitingArrival {
                        amount_sat,
                        deposit_seen,
                    } => {
                        // A SideShift swap just settled: bitcoin is en route to the
                        // Spark wallet's static on-chain deposit address.
                        // Structurally that's identical to a Vault→Spark transfer
                        // arrival — an on-chain deposit that matures and must be
                        // claimed — so register it exactly like one and let the
                        // watcher below auto-claim it. `is_swap` makes the eventual
                        // claim additionally fire the "bitcoin received" celebration.
                        //
                        // Don't clobber an in-flight transfer: if we're already
                        // tracking a pending incoming deposit, skip global tracking
                        // (the swap stays claimable from the Receive panel's
                        // pending-deposits card) rather than corrupting the
                        // transfer's reconciliation.
                        if self.pending_spark_incoming.is_some() {
                            return Task::none();
                        }
                        self.pending_spark_incoming = Some(PendingTransfer {
                            amount: Amount::from_sat(amount_sat),
                            stage: TransferStage::PendingDeposit,
                        });
                        // No Breez swap id — swaps use the generic deposit-based
                        // path (same as Vault→Spark), not the peg-out correlation.
                        self.pending_spark_incoming_swap_id = None;
                        // A restored shift saw its deposit unclaimed just now; if
                        // the SDK claims it before the list below is read, an
                        // empty list must still read as claimed.
                        self.pending_spark_deposit_seen = deposit_seen;
                        self.pending_spark_incoming_is_swap = true;
                        self.auto_claiming_spark_deposit = None;
                        // Reconcile immediately — the deposit may already be in the
                        // unclaimed list (it can land before we register here), and
                        // otherwise we'd wait for the next bridge event.
                        Task::done(Message::View(view::Message::Home(
                            HomeMessage::SparkDepositsChanged,
                        )))
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
                        // us or by the user from the Spark Receive panel). Only
                        // interpret it that way once we've previously observed
                        // our deposit in the list: the inbound tx (Breez peg-out
                        // for LiquidToSpark, Bitcoin broadcast for VaultToSpark)
                        // doesn't appear until it lands on-chain, so an unrelated
                        // `SparkDepositsChanged` firing during that window (e.g.
                        // a prior deposit being claimed from the Receive panel)
                        // would otherwise clear the badge while our transfer is
                        // still in flight.
                        if deposits.is_empty() {
                            if !self.pending_spark_deposit_seen {
                                return Task::none();
                            }
                            // The deposit was claimed by someone other than our own
                            // auto-claim below — the Spark SDK auto-claims static
                            // deposits, and the user can claim from the Receive
                            // panel — so `AutoClaimSparkResult` (which advances the
                            // cross-network Receive flow off "Bitcoin arriving")
                            // never ran. Nudge the flow to its arrived state here so
                            // it doesn't hang. This is the *only* signal for an
                            // SDK-silent claim; it's idempotent (the flow transitions
                            // once and ignores it otherwise), so it can't double up
                            // with a claim path that already fired. Non-swap
                            // transfers (Vault/Liquid→Spark) don't touch it.
                            let was_swap = self.pending_spark_incoming_is_swap;
                            let amount_sat = self
                                .pending_spark_incoming
                                .as_ref()
                                .map(|p| p.amount.to_sat())
                                .unwrap_or(0);
                            self.pending_spark_incoming = None;
                            self.pending_spark_incoming_swap_id = None;
                            self.pending_spark_deposit_seen = false;
                            self.pending_spark_incoming_is_swap = false;
                            self.auto_claiming_spark_deposit = None;
                            self.spark_claim_awaiting_settlement = false;
                            if was_swap {
                                return Task::done(Message::View(
                                    view::Message::SparkSideshiftReceive(
                                        view::SparkSideshiftReceiveMessage::Arrived { amount_sat },
                                    ),
                                ));
                            }
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
                                        // `amount_sat` is `None` for a pre-maturity
                                        // claim (SDK 0.25.0+): the transfer settles
                                        // asynchronously and only `PaymentSucceeded`
                                        // knows what actually landed. Keep it unknown
                                        // rather than showing the pre-fee deposit.
                                        // A deferral (SDK 0.28.0+) claimed nothing:
                                        // logged like a failure, and the indicator
                                        // stays pending until the SDK's own claim
                                        // fires `DepositsChanged`.
                                        result: match result {
                                            Ok(ok) => match &ok.deferred {
                                                Some(deferral) => Err(
                                                    crate::app::state::spark::receive::claim_deferral_notice(
                                                        deferral,
                                                    ),
                                                ),
                                                None => Ok(ok.amount_sat),
                                            },
                                            Err(e) => Err(e.to_string()),
                                        },
                                    },
                                ))
                            },
                        )
                    }
                    HomeMessage::AutoClaimSparkResult { txid, vout, result } => {
                        // Stale result guard: if the tracker no longer matches
                        // this (txid, vout) it belongs to a previous transfer —
                        // e.g. a second Spark-destined transfer broadcast while
                        // this claim was in flight. Stamping Completed on the
                        // now-current `pending_spark_incoming` would prematurely
                        // hide the new badge.
                        if self.auto_claiming_spark_deposit.as_ref() != Some(&(txid, vout)) {
                            return Task::none();
                        }
                        // Success: mark the indicator Completed so the view hides
                        // it immediately and `BackToHome` can reap it symmetric to
                        // `pending_vault_incoming`. The bridge's follow-up
                        // `DepositsChanged` will still re-enter the watcher and
                        // clear the field outright. Failure: log and surface so
                        // the user can retry from the Receive panel.
                        self.auto_claiming_spark_deposit = None;
                        match result {
                            Ok(None) => {
                                // Accepted before maturity: nothing has landed
                                // yet and the settled amount is unknown. Leave
                                // the indicator pending and let
                                // `SparkPaymentSucceeded` finish the job.
                                self.spark_claim_awaiting_settlement = true;
                                Task::none()
                            }
                            Ok(Some(amount)) => self.complete_spark_auto_claim(amount),
                            Err(e) => {
                                log::warn!("Auto-claim of Spark deposit failed: {e}");
                                Task::none()
                            }
                        }
                    }
                    HomeMessage::SparkPaymentSucceeded { amount_sat } => {
                        if !self.spark_claim_awaiting_settlement {
                            return Task::none();
                        }
                        self.spark_claim_awaiting_settlement = false;
                        self.complete_spark_auto_claim(amount_sat)
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
                        self.liquid_balance_error = false;
                        self.liquid_balance_loaded = true;
                        Task::none()
                    }
                    HomeMessage::UsdtBalanceUpdated(usdt_balance) => {
                        self.usdt_balance = usdt_balance;
                        self.usdt_balance_error = false;
                        self.usdt_balance_loaded = true;
                        Task::none()
                    }
                    HomeMessage::LiquidBalanceFetchFailed(detail) => {
                        // `loaded` means "we are no longer waiting on this", not
                        // "we have a number". Without it `liquid_pending` stays
                        // true and the *aggregate* Total Balance spins forever —
                        // taking the Vault and Spark rows down with it, neither
                        // of which failed. `UsdtBalanceFetchFailed` below makes
                        // the same bargain.
                        //
                        // `loaded` is never reset to false once set, so reading
                        // it *before* we set it answers the only question that
                        // matters here: have we ever had a real number?
                        let had_a_balance = self.liquid_balance_loaded;
                        self.liquid_balance_loaded = true;
                        // First load: there is nothing behind the failure but
                        // the untouched `Amount::ZERO`, and rendering that as a
                        // balance would not be stale, it would be wrong.
                        //
                        // Later refresh: `liquid_balance` still holds the last
                        // good value and the Total still counts it, so blanking
                        // the row to "Balance unavailable" would both lose
                        // information the user had and disagree with the total
                        // right above it. A transient blip keeps the number.
                        self.liquid_balance_error = !had_a_balance;
                        // A fetch started before Liquid was hidden can still
                        // fail late. Its wallet isn't on screen, so neither is
                        // the error.
                        if !self.liquid_shown {
                            return Task::none();
                        }
                        let (title, guidance) = if had_a_balance {
                            (
                                "Couldn't refresh your Liquid balance",
                                "The amount shown may be out of date. Check your internet connection.",
                            )
                        } else {
                            (
                                "Couldn't load your Liquid balance",
                                "Check your internet connection and try again.",
                            )
                        };
                        Task::done(Message::View(view::Message::ShowError(
                            crate::user_error::UserError::logged(
                                title,
                                guidance,
                                crate::user_error::CC_LQD_CONN,
                                true,
                                detail,
                            )
                            .toast(),
                        )))
                    }
                    HomeMessage::UsdtBalanceFetchFailed => {
                        // Same rule as its L-BTC twin above: only blank the row
                        // when there is no successful value behind the failure.
                        self.usdt_balance_error = !self.usdt_balance_loaded;
                        self.usdt_balance_loaded = true;
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
                    HomeMessage::PrepareOnChainResponseReceived {
                        generation,
                        direction,
                        response,
                    } => {
                        if !self.destination_response_is_current(generation, direction) {
                            return Task::none();
                        }
                        // A drain prepares the whole balance; show what arrives.
                        if self.sends_max() {
                            self.set_entered_amount(Amount::from_sat(response.receiver_amount_sat));
                        }
                        self.prepare_onchain_send_response = Some(response);
                        Task::none()
                    }
                    HomeMessage::TransferSuccessful => {
                        self.current_view.next();
                        self.is_sending = false;
                        // A Vault-to-Vault transfer goes to another Cube; it
                        // locks again now that the transaction is broadcast.
                        if self
                            .transfer_direction
                            .is_some_and(|direction| direction.is_other_cube())
                        {
                            self.finish_other_cube();
                        }
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
                            self.pending_spark_incoming_is_swap = false;
                            return Task::done(Message::View(view::Message::ShowError(
                                "Liquid to Spark transfer failed. Please retry.".to_string(),
                            )));
                        }
                        Task::none()
                    }
                    HomeMessage::PendingTransferRestored { .. } if !self.liquid_shown => {
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
                        self.transfer_origin = None;
                        self.transfer_page = TransferPage::Wallets;
                        self.reset_other_cube();
                        self.transfer_direction = None;
                        self.transfer_from = None;
                        self.transfer_to = None;
                        self.wallet_picker = None;
                        self.entered_amount = form::Value::default();
                        self.transfer_max = None;
                        self.invalidate_transfer_destination();
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
                            self.pending_spark_incoming_is_swap = false;
                        }
                        Task::none()
                    }
                    HomeMessage::TransferDestinationReady {
                        generation,
                        direction,
                        result,
                    } => {
                        if !self.destination_response_is_current(generation, direction) {
                            return Task::none();
                        }
                        let (address, index) = match result {
                            Ok(address) => address,
                            Err(error) => {
                                return Task::done(Message::View(view::Message::Home(
                                    HomeMessage::Error(error),
                                )));
                            }
                        };
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
                                        index,
                                        labels: HashMap::new(),
                                    });
                                    self.destination_ready = Some((generation, direction));
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
                    HomeMessage::TransferDestinationFailed {
                        generation,
                        direction,
                        error,
                    } => {
                        if !self.destination_response_is_current(generation, direction) {
                            return Task::none();
                        }
                        Task::done(Message::View(view::Message::Home(HomeMessage::Error(
                            error,
                        ))))
                    }
                    HomeMessage::RefreshLiquidBalance => self.load_liquid_balance(),
                    HomeMessage::RefreshSparkBalance => {
                        self.load_spark_balance().unwrap_or_else(Task::none)
                    }
                    HomeMessage::SparkLoadRetry => {
                        // Only the fallback poll bumps the retry
                        // counter — SDK-event-driven refreshes
                        // (`RefreshSparkBalance`) must not, or a burst
                        // of payment / deposit events on cold start
                        // could blow through the cap before `Synced`
                        // lands and prematurely flip the gate.
                        //
                        // The counter advances on every tick (wall-
                        // clock semantics) so a single slow get_info
                        // can't postpone the cap indefinitely. The
                        // single-flight guard lives inside
                        // `load_spark_balance`, which returns `None`
                        // when a previous RPC is still in flight — so
                        // we don't start an overlapping fetch even
                        // though the tick still counts.
                        self.spark_load_retry_count = self.spark_load_retry_count.saturating_add(1);
                        // Final-attempt graceful exit: if the fallback
                        // poll has hit its cap without ever seeing a
                        // `Synced`, release the loading UI rather than
                        // leave the user staring at `…` indefinitely.
                        // Shows whatever the last `get_info` returned —
                        // best-effort fallback when the event path is
                        // genuinely broken. Skipped when no response
                        // ever landed (`spark_balance` is still the
                        // `Amount::ZERO` default): publishing that zero
                        // would mislead the user and become a false
                        // source cap in `validate_entered_amount`. In
                        // that scenario we deliberately stay in the
                        // loading state — Transfer remains disabled
                        // and the placeholder honestly reflects that
                        // we don't know the balance.
                        if self.spark_load_retry_count >= SPARK_LOAD_RETRY_CAP
                            && !self.spark_balance_loaded
                            && self.spark_balance_received
                        {
                            self.spark_balance_loaded = true;
                        }
                        self.load_spark_balance().unwrap_or_else(Task::none)
                    }
                    HomeMessage::SparkSyncedObserved => {
                        // Flip the synced gate, but do NOT flip
                        // `spark_balance_loaded` here — the value
                        // currently in `spark_balance` may still be
                        // the SDK's pre-sync persisted snapshot.
                        // `mod.rs` dispatches `RefreshSparkBalance`
                        // alongside this message on every `Synced`;
                        // the fresh `SparkBalanceUpdated` it triggers
                        // is what flips the gate (its handler checks
                        // `spark_synced_seen`), so the placeholder
                        // only goes away once the post-sync value
                        // has actually landed.
                        self.spark_synced_seen = true;
                        // If a get_info is already in flight, the
                        // companion `RefreshSparkBalance` will be
                        // skipped by the in-flight guard. Mark the
                        // pending response as stale-on-arrival so
                        // `SparkBalanceUpdated` discards it instead
                        // of flipping the gate on a pre-Synced value.
                        if self.spark_balance_loading {
                            self.spark_pending_resync = true;
                        }
                        Task::none()
                    }
                    HomeMessage::SparkLoadFailed => {
                        // Soft-fail terminal: release the in-flight
                        // guard so the next poll tick or SparkEvent
                        // can fire a fresh `get_info`. If a `Synced`
                        // arrived during the failed fetch, the
                        // companion `RefreshSparkBalance` was skipped
                        // by the in-flight guard — fire a fresh one
                        // now so the placeholder can converge.
                        self.spark_balance_loading = false;
                        self.spark_balance_error = true;
                        if self.spark_pending_resync {
                            self.spark_pending_resync = false;
                            return self.load_spark_balance().unwrap_or_else(Task::none);
                        }
                        Task::none()
                    }
                    HomeMessage::TransferPsbtReady {
                        request_id,
                        generation,
                        direction,
                        destination,
                        amount_sat,
                        feerate_vb,
                        result,
                    } => {
                        if self.active_psbt_request != Some(request_id) {
                            return Task::none();
                        }
                        // This result owns the busy state. Release it even if
                        // the fee or amount changed while the PSBT was built;
                        // otherwise a rejected result strands Back and Sign.
                        self.active_psbt_request = None;
                        self.is_sending = false;
                        let denomination = if matches!(self.transfer_unit, BitcoinDisplayUnit::BTC)
                        {
                            coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                        } else {
                            coincube_core::miniscript::bitcoin::Denomination::Satoshi
                        };
                        if !self.legacy_vault_available(cache)
                            || !self.destination_response_is_current(generation, direction)
                            || !self.destination_is_ready()
                            || self
                                .receive_address_info
                                .as_ref()
                                .map(|info| info.address.to_string())
                                != Some(destination)
                            || Amount::from_str_in(&self.entered_amount.value, denomination)
                                .ok()
                                .map(|amount| amount.to_sat())
                                != Some(amount_sat)
                            || self.transfer_feerate.value.trim().parse::<u64>().ok()
                                != Some(feerate_vb)
                        {
                            return Task::none();
                        }
                        // Leaving the page while this PSBT was built locked the
                        // other Cube: open no signer; its unlock starts over.
                        if direction.is_other_cube() {
                            if let Err(refusal) = self.check_other_review_address(cache) {
                                return self.restart_other_cube_unlock(refusal);
                            }
                        }
                        match result {
                            Ok((psbt, wallet, (datadir_path, network))) => {
                                if let Some(wallet) = wallet {
                                    let Some(descriptor) = wallet.inheritance_descriptor() else {
                                        return Task::done(Message::View(
                                            view::Message::ShowError(
                                                "This transfer requires a Cube Vault.".into(),
                                            ),
                                        ));
                                    };
                                    let sigs = match descriptor.partial_spend_info(&psbt) {
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
                                    let max_vbytes = descriptor.unsigned_tx_max_vbytes(
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
                                    self.transfer_psbt_ready_for =
                                        Some((request_id, generation, direction));

                                    // Create the SignModal. This send-to-self
                                    // transfer signs locally only — no nested
                                    // Keychain flow.
                                    let sign_modal = SignModal::new(
                                        std::collections::HashSet::new(),
                                        wallet,
                                        datadir_path,
                                        network,
                                        true,
                                        None,
                                        None,
                                        false,
                                    );

                                    self.modal = Modal::Sign(Box::new(sign_modal));
                                } else {
                                    return Task::done(Message::View(view::Message::ShowError(
                                        "Wallet not available".to_string(),
                                    )));
                                }
                            }
                            Err(e) => {
                                return Task::done(Message::View(view::Message::ShowError(
                                    crate::user_error::UserError::logged(
                                        "Couldn't prepare that transfer for signing",
                                        "Try again. If it keeps failing, contact support and quote the reference below.",
                                        crate::user_error::CC_SPEND,
                                        true,
                                        e,
                                    )
                                    .toast(),
                                )));
                            }
                        }
                        Task::none()
                    }
                    HomeMessage::TransferSigningComplete {
                        request_id,
                        generation,
                        direction,
                    } => {
                        if !self.destination_response_is_current(generation, direction)
                            || !self.destination_is_ready()
                            || self.transfer_psbt_ready_for
                                != Some((request_id, generation, direction))
                            || self.transfer_spend_tx.is_none()
                            || !matches!(self.modal, Modal::Sign(_))
                        {
                            return Task::none();
                        }
                        self.transfer_signed = true;
                        self.modal = Modal::None;
                        self.is_sending = false;
                        Task::none()
                    }
                    HomeMessage::TransferPsbtPreviewReady {
                        generation,
                        direction,
                        feerate_vb,
                        result,
                    } => {
                        if !self.legacy_vault_available(cache) {
                            return Task::none();
                        }
                        if !self.destination_response_is_current(generation, direction)
                            || !self.destination_is_ready()
                        {
                            return Task::none();
                        }
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
                            Ok((fees, received)) => {
                                self.spend_tx_fees = Some(fees);
                                // A sweep's output is the balance less this fee.
                                if self.sends_max() {
                                    self.set_entered_amount(received);
                                }
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
            // Transfer destination fetches use TransferDestinationReady with a
            // request generation. An untagged result from another panel must
            // never become the destination of this transfer.
            Message::ReceiveAddress(_) => Task::none(),
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
                            let err_msg = crate::user_error::report(&e);
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
                // Only leaving the signer abandons the spend. Closing the
                // QR card over Review keeps a signed transfer signed.
                if matches!(self.modal, Modal::Sign(_)) {
                    self.transfer_spend_tx = None;
                    self.transfer_psbt_ready_for = None;
                    self.transfer_signed = false;
                }
                self.modal = Modal::None;
                Task::none()
            }
            Message::Updated(result) => {
                let persisted = result.is_ok();
                let signing_request = self.transfer_psbt_ready_for;
                let wallet = self.wallet.clone();
                if let (Modal::Sign(ref mut sign_modal), Some(daemon)) = (&mut self.modal, daemon) {
                    if let Some(ref mut spend_tx) = self.transfer_spend_tx {
                        use crate::app::state::vault::psbt::Modal as PsbtModalTrait;
                        let task = sign_modal.update(daemon, Message::Updated(result), spend_tx);
                        let threshold_met = wallet
                            .as_ref()
                            .and_then(|wallet| {
                                wallet.inheritance_descriptor().and_then(|descriptor| {
                                    crate::app::state::vault::replay::spend_info_for_chain(
                                        wallet.chain,
                                        descriptor,
                                        &spend_tx.psbt,
                                    )
                                    .ok()
                                })
                            })
                            .is_some_and(|info| {
                                let primary = info.primary_path();
                                primary.sigs_count >= primary.threshold
                                    || info
                                        .recovery_paths()
                                        .values()
                                        .any(|path| path.sigs_count >= path.threshold)
                            });
                        if let Some(completion) = transfer_signing_completion_message(
                            persisted,
                            threshold_met,
                            signing_request,
                        ) {
                            return Task::batch(vec![
                                task,
                                Task::done(Message::View(view::Message::Home(completion))),
                            ]);
                        }
                        return task;
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
        let mut tasks = Vec::new();
        // Hidden Liquid settles as a network with no Liquid backend does:
        // nothing is fetched, so a late failure can't raise an error.
        if self.liquid_shown
            && matches!(
                self.network,
                crate::chain::ChainId::Bitcoin | crate::chain::ChainId::Regtest
            )
        {
            tasks.extend([
                self.load_liquid_balance(),
                self.load_usdt_balance(),
                self.load_pending_sends(),
                self.restore_pending_liquid_to_vault_transfer(),
            ]);
        } else {
            self.liquid_balance = Amount::ZERO;
            self.usdt_balance = 0;
            self.liquid_balance_loaded = true;
            self.liquid_balance_error = false;
            self.usdt_balance_loaded = true;
            self.usdt_balance_error = false;
            self.pending_liquid_send_sats = 0;
            self.pending_liquid_receive_sats = 0;
            self.pending_usdt_send_sats = 0;
            self.pending_usdt_receive_sats = 0;
        }
        if let Some(spark) = self.load_spark_balance() {
            tasks.push(spark);
        }
        tasks.push(self.restore_pending_spark_shift());
        Task::batch(tasks)
    }
}

impl GlobalHome {
    /// Open the transfer's Details step with `direction`, fresh: the Overview's
    /// Transfer, a wallet page's Transfer and Done's New transfer all start
    /// here. Prefetches Breez's on-chain swap limits for the Liquid legs.
    fn begin_transfer(&mut self, cache: &Cache, direction: TransferDirection) -> Task<Message> {
        let has_liquid =
            crate::app::features::liquid_wallet_usable(cache.network, cache.liquid_gate);
        self.transfer_from = Some(direction.from_kind());
        self.transfer_to = (!direction.is_other_cube()).then(|| direction.to_kind());
        self.transfer_direction = Some(direction);
        self.invalidate_transfer_destination();
        self.wallet_picker = None;
        self.entered_amount = form::Value::default();
        self.transfer_max = None;
        // Fresh flow: reset the Vault-sourced feerate picker so a value (or
        // stuck loading flag) from a prior transfer doesn't leak in.
        self.transfer_feerate = default_transfer_feerate();
        self.transfer_feerate_loading = None;
        // One Details card: a same-Cube pair takes an amount at once;
        // another Cube waits for its pick.
        self.transfer_page = if direction.is_other_cube() {
            TransferPage::Wallets
        } else {
            TransferPage::Amount
        };
        self.transfer_unit = cache.bitcoin_unit;
        self.reset_other_cube();
        self.current_view.next();
        // Breez's swap limits bound only the Liquid legs (see
        // `effective_transfer_min_sat`). Without a usable Liquid wallet the
        // SDK isn't connected, so the fetch would do nothing but raise an
        // error.
        if !has_liquid {
            return Task::none();
        }
        let Some(breez_client) = self.breez_client.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { breez_client.fetch_onchain_limits().await },
            |limit| match limit {
                Ok(limits) => {
                    Message::View(view::Message::Home(HomeMessage::OnChainLimitsFetched {
                        send: (limits.send.min_sat, limits.send.max_sat),
                        receive: (limits.receive.min_sat, limits.receive.max_sat),
                    }))
                }
                Err(error) => Message::View(view::Message::Home(HomeMessage::Error(
                    TransferError::FeeEstimateFailed {
                        detail: error.to_string(),
                    },
                ))),
            },
        )
    }

    /// Whether the Total Balance is still waiting on a wallet (see
    /// [`total_balance_pending`]); the Transfer buttons wait on it too.
    fn total_balance_loading(&self, cache: &Cache) -> bool {
        let vault_balance = cache
            .coins()
            .iter()
            .filter(|coin| coin.spend_info.is_none())
            .fold(Amount::from_sat(0), |acc, coin| acc + coin.amount);
        let vault_pending = cache.has_vault
            && vault_balance_pending(cache.blockheight(), cache.history_sync(), vault_balance);
        (cache.has_imported_bitcoin && self.imported_balance.is_none())
            || total_balance_pending(BalanceReadiness {
                has_spark: self.spark_backend.is_some(),
                spark_loaded: self.spark_balance_loaded || self.spark_balance_error,
                has_liquid: crate::app::features::liquid_wallet_usable(
                    cache.chain(),
                    cache.liquid_gate,
                ),
                liquid_loaded: self.liquid_balance_loaded,
                usdt_loaded: self.usdt_balance_loaded,
                vault_pending,
            })
    }

    /// A wallet page's Transfer: open the flow from `from`, remembering the
    /// page so leaving returns there. Any transfer left open on the Overview
    /// starts over. The caller has already switched to the Overview route.
    pub fn start_transfer_from(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        from: WalletKind,
    ) -> Task<Message> {
        if self.wallet_transfer_entry(cache, from) != WalletTransferEntry::Ready {
            return Task::none();
        }
        let Some(direction) = self.wallet_transfer_direction(cache, from) else {
            return Task::none();
        };
        let reset = if self.current_view.step == 0 {
            Task::none()
        } else {
            self.update(
                daemon,
                cache,
                Message::View(view::Message::Home(HomeMessage::BackToHome)),
            )
        };
        self.transfer_origin = Some(from);
        Task::batch([reset, self.begin_transfer(cache, direction)])
    }

    /// The direction a wallet page's Transfer would open with.
    fn wallet_transfer_direction(
        &self,
        cache: &Cache,
        from: WalletKind,
    ) -> Option<TransferDirection> {
        transfer_direction_from(
            from,
            crate::app::features::liquid_wallet_usable(cache.chain(), cache.liquid_gate),
            cache.has_vault,
            self.spark_backend.is_some(),
            cache.chain(),
            self.other_cube_receivers.from(from),
        )
    }

    /// The state of a wallet page's Transfer button: disabled without a
    /// destination, while the balances the form caps against load, and while
    /// a transfer already open on the Overview is sending (starting another
    /// would reset it mid-send).
    pub fn wallet_transfer_entry(&self, cache: &Cache, from: WalletKind) -> WalletTransferEntry {
        if self.wallet_transfer_direction(cache, from).is_none() {
            WalletTransferEntry::NoDestination
        } else if !self.is_sending
            && view::global_home::transfer_balance_ready(
                self.total_balance_loading(cache),
                self.spark_backend.is_some(),
                self.spark_balance_loaded,
            )
        {
            WalletTransferEntry::Ready
        } else {
            WalletTransferEntry::Loading
        }
    }

    /// Re-read whether another local Cube can receive from this Cube's Spark
    /// and Vault. One catalog read, done when a wallet page opens.
    pub fn refresh_other_cube_receivers(&mut self, cache: &Cache) {
        let chain = cache.chain();
        let receives = |wallet: Option<WalletIdentity>| {
            wallet.is_some_and(|wallet| {
                other_cubes(
                    &cache.datadir_path,
                    &Source {
                        cube_id: self.cube_id.clone(),
                        chain,
                        wallet,
                        app: cache.app_generation,
                    },
                )
                .iter()
                .any(|cube| cube.refusal.is_none())
            })
        };
        let on_mainnet = chain == crate::chain::ChainId::Bitcoin;
        // The Spark identity only names the source; whether another Cube can
        // receive doesn't depend on it, so a not-yet-loaded one is fine here.
        let spark = (on_mainnet && self.spark_backend.is_some())
            .then(|| WalletIdentity::Spark(self.spark_identity.clone().unwrap_or_default()));
        let vault = (on_mainnet && self.legacy_vault_available(cache))
            .then(|| {
                self.wallet
                    .as_ref()
                    .map(|wallet| WalletIdentity::Vault(wallet.id()))
            })
            .flatten();
        self.other_cube_receivers = OtherCubeReceivers {
            spark: receives(spark),
            vault: receives(vault),
        };
    }

    fn transfer_source_wallet(
        &self,
        direction: TransferDirection,
        cache: &Cache,
    ) -> Option<WalletIdentity> {
        if !direction.is_other_cube() {
            return None;
        }
        match direction.from_kind() {
            WalletKind::Vault if self.legacy_vault_available(cache) => {
                Some(WalletIdentity::Vault(self.wallet.as_ref()?.id()))
            }
            WalletKind::Spark if self.spark_backend.is_some() => self
                .spark_identity
                .as_ref()
                .filter(|key| !key.is_empty())
                .map(|key| WalletIdentity::Spark(key.clone())),
            _ => None,
        }
    }

    /// Bind the destination unlock to this Cube, wallet and app session.
    /// The source as another Cube would see it, whatever the current
    /// destination: `None` when the source can't reach another Cube.
    fn other_cube_source(&self, cache: &Cache) -> Option<Source> {
        let direction = TransferDirection::to_other_cube(self.transfer_from?, cache.chain())?;
        let wallet = self.transfer_source_wallet(direction, cache)?;
        Some(Source {
            cube_id: self.cube_id.clone(),
            chain: cache.chain(),
            wallet,
            app: cache.app_generation,
        })
    }

    fn transfer_source(&self, cache: &Cache) -> Option<Source> {
        let wallet = self.transfer_source_wallet(self.transfer_direction?, cache)?;
        Some(Source {
            cube_id: self.cube_id.clone(),
            chain: cache.chain(),
            wallet,
            app: cache.app_generation,
        })
    }

    fn other_receive_address(&self, cache: &Cache) -> Result<&OtherReceiveAddress, ScopeRefusal> {
        let source = self.transfer_source(cache).ok_or(ScopeRefusal::Stale)?;
        let scope = self.other_scope.as_ref().ok_or(ScopeRefusal::Stale)?;
        let receives_spark = self
            .transfer_direction
            .is_some_and(|direction| direction.to_kind() == WalletKind::Spark);
        if scope.receives_spark() != receives_spark {
            return Err(ScopeRefusal::IdentityChanged);
        }
        scope.current(&source)?;
        self.other_address.as_ref().ok_or(ScopeRefusal::Stale)
    }

    fn check_other_review_address(&self, cache: &Cache) -> Result<(), ScopeRefusal> {
        match self.other_receive_address(cache)? {
            OtherReceiveAddress::Bitcoin(address)
                if self
                    .receive_address_info
                    .as_ref()
                    .is_some_and(|info| info.address == address.address) =>
            {
                Ok(())
            }
            OtherReceiveAddress::Spark(_)
                if self.transfer_direction == Some(TransferDirection::SparkToOtherSpark) =>
            {
                Ok(())
            }
            _ => Err(ScopeRefusal::IdentityChanged),
        }
    }

    fn select_other_cube(&mut self, id: String, cache: &Cache) {
        let Some(choice) = self.other_cubes.iter().find(|choice| choice.cube_id == id) else {
            return;
        };
        let preferred = self
            .transfer_direction
            .map_or(WalletKind::Vault, |direction| direction.to_kind());
        let Some(wallet) = [preferred, WalletKind::Vault, WalletKind::Spark]
            .iter()
            .copied()
            .find(|&wallet| choice.can_receive(wallet))
        else {
            return;
        };
        self.other_cube_selected = Some(id);
        self.other_error = None;
        self.select_other_wallet(wallet, cache);
    }

    fn select_other_wallet(&mut self, wallet: WalletKind, cache: &Cache) {
        let Some(direction) = self
            .transfer_from
            .and_then(|from| TransferDirection::to_other_wallet(from, wallet, cache.chain()))
        else {
            return;
        };
        self.transfer_direction = Some(direction);
        self.invalidate_transfer_destination();
        self.validate_entered_amount(cache);
    }

    fn restart_other_cube_unlock(&mut self, refusal: ScopeRefusal) -> Task<Message> {
        self.reset_other_cube();
        self.invalidate_transfer_destination();
        self.current_view.step = 1;
        self.transfer_page = TransferPage::Wallets;
        Task::done(Message::View(view::Message::ShowError(refusal.reason())))
    }

    /// Locks the other Cube again: drops its scoped unlock (and any daemon
    /// it started), the entered PIN and its checked address.
    fn lock_other_cube(&mut self) {
        if let Some(mut scope) = self.other_scope.take() {
            scope.revoke();
        }
        self.other_pin.clear();
        self.other_phase = UnlockPhase::default();
        self.other_error = None;
        self.other_address = None;
    }

    fn reset_other_cube(&mut self) {
        self.lock_other_cube();
        self.other_cubes.clear();
        self.other_cube_selected = None;
        self.other_locked_again = false;
    }

    /// The transfer was sent: the other Cube locks again and Done says so.
    fn finish_other_cube(&mut self) {
        let was_open = self.other_scope.is_some();
        self.lock_other_cube();
        // Leaving mid-send may have locked it already.
        self.other_locked_again |= was_open;
    }

    /// The other Cube's credential was checked. Open only its selected receiving wallet.
    fn after_other_credential(
        &mut self,
        source: &Source,
        result: Result<(), ScopeRefusal>,
    ) -> Task<Message> {
        if let Err(refusal) = result {
            self.other_phase = UnlockPhase::Credential { busy: false };
            self.other_error = Some(refusal.reason());
            return Task::none();
        }
        self.other_phase = UnlockPhase::Checks(1);
        self.other_error = None;
        let Some(scope) = self.other_scope.as_mut() else {
            return Task::none();
        };
        if scope.receives_spark() {
            return match scope.request_spark_address(source) {
                Ok(request) => Task::perform(request.run(), |reply| {
                    Message::View(view::Message::Home(HomeMessage::OtherCubeSparkAddress(
                        reply,
                    )))
                }),
                Err(refusal) => {
                    self.other_error = Some(refusal.reason());
                    Task::none()
                }
            };
        }
        match scope.request_backend(source) {
            Ok(request) => Task::perform(request.run_reply(), |reply| {
                Message::View(view::Message::Home(HomeMessage::OtherCubeBackendBound(
                    reply,
                )))
            }),
            Err(refusal) => {
                self.other_error = Some(refusal.reason());
                Task::none()
            }
        }
    }

    /// The destination is current and the direction's prepared response or
    /// handle is in, so Review can confirm.
    fn transfer_action_ready(&self) -> bool {
        self.destination_is_ready()
            && self
                .transfer_direction
                .is_some_and(|direction| match direction.from_kind() {
                    // Max shows what arrives only once the sweep is priced.
                    WalletKind::Vault => !self.sends_max() || self.spend_tx_fees.is_some(),
                    WalletKind::Liquid => self.prepare_onchain_send_response.is_some(),
                    WalletKind::Spark => self.spark_send_handle.is_some(),
                })
    }

    /// Max is on for the current source wallet.
    fn sends_max(&self) -> bool {
        self.transfer_max.is_some()
            && self.transfer_max == self.transfer_direction.map(|d| d.from_kind())
    }

    /// What Max fills in for `source`, before fees.
    fn max_transfer_amount(&self, source: WalletKind, cache: &Cache) -> Amount {
        match source {
            WalletKind::Liquid => self.liquid_balance,
            WalletKind::Spark => self.spark_balance,
            WalletKind::Vault => vault_sweep_coins(cache).1,
        }
    }

    /// Writes `amount` into the amount field in the transfer's unit.
    fn set_entered_amount(&mut self, amount: Amount) {
        self.entered_amount.value = match self.transfer_unit {
            BitcoinDisplayUnit::Sats => amount.to_sat().to_string(),
            BitcoinDisplayUnit::BTC => {
                amount.to_string_in(coincube_core::miniscript::bitcoin::Denomination::Bitcoin)
            }
        };
    }

    /// Stores Spark's quoted fee. With Max the prepared amount is the whole
    /// balance with the fee inside it, so the field shows what arrives; the
    /// fee is taken off once per preparation.
    fn show_max_after_fee(&mut self, fee_sat: u64) {
        if self.sends_max() && self.spark_send_fee_sat.is_none() {
            let denomination = if matches!(self.transfer_unit, BitcoinDisplayUnit::BTC) {
                coincube_core::miniscript::bitcoin::Denomination::Bitcoin
            } else {
                coincube_core::miniscript::bitcoin::Denomination::Satoshi
            };
            if let Ok(total) = Amount::from_str_in(&self.entered_amount.value, denomination) {
                self.set_entered_amount(
                    total
                        .checked_sub(Amount::from_sat(fee_sat))
                        .unwrap_or(Amount::ZERO),
                );
            }
        }
        self.spark_send_fee_sat = Some(fee_sat);
    }

    fn invalidate_transfer_destination(&mut self) -> u64 {
        self.transfer_request_generation = self.transfer_request_generation.wrapping_add(1);
        self.destination_ready = None;
        self.receive_address_info = None;
        self.prepare_onchain_send_response = None;
        self.transfer_spend_tx = None;
        self.transfer_psbt_ready_for = None;
        self.active_psbt_request = None;
        self.transfer_signed = false;
        self.spend_tx_fees = None;
        self.spark_send_handle = None;
        self.spark_send_fee_sat = None;
        self.transfer_feerate_loading = None;
        if matches!(self.modal, Modal::Sign(_)) {
            self.modal = Modal::None;
        }
        self.transfer_request_generation
    }

    fn destination_response_is_current(
        &self,
        generation: u64,
        direction: TransferDirection,
    ) -> bool {
        self.transfer_request_generation == generation
            && self.transfer_direction == Some(direction)
            && self.current_view.step == 2
    }

    fn destination_is_ready(&self) -> bool {
        self.current_view.step == 2
            && (self.receive_address_info.is_some()
                || (self.transfer_direction == Some(TransferDirection::SparkToOtherSpark)
                    && matches!(self.other_address, Some(OtherReceiveAddress::Spark(_)))))
            && self.destination_ready
                == self
                    .transfer_direction
                    .map(|direction| (self.transfer_request_generation, direction))
    }

    fn legacy_vault_available(&self, cache: &Cache) -> bool {
        cache.has_vault
            && self
                .wallet
                .as_ref()
                .is_some_and(|wallet| wallet.supports_inheritance())
    }

    fn direction_uses_vault(direction: TransferDirection) -> bool {
        let (from, to) = direction.pair();
        from == WalletKind::Vault || (to == WalletKind::Vault && !direction.is_other_cube())
    }

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
        let denomination = if matches!(self.transfer_unit, BitcoinDisplayUnit::BTC) {
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

    /// The auto-claimed deposit has landed for `amount_sat`: mark the
    /// indicator Completed so the view hides it immediately and `BackToHome`
    /// can reap it symmetric to `pending_vault_incoming` (the bridge's
    /// follow-up `DepositsChanged` will still clear the field outright), and
    /// fire the global "received" splash if this was a swap.
    fn complete_spark_auto_claim(&mut self, amount_sat: u64) -> Task<Message> {
        // Capture before the block below may clear it.
        let celebrate_swap = self.pending_spark_incoming_is_swap;
        if let Some(mut pending) = self.pending_spark_incoming {
            pending.stage = TransferStage::Completed;
            self.pending_spark_incoming = Some(pending);
        }
        if celebrate_swap {
            // Reset the flag first so the follow-up DepositsChanged (which
            // clears pending_spark_incoming) can't re-fire it.
            self.pending_spark_incoming_is_swap = false;
            return Task::done(Message::ShowReceivedCelebration {
                context: "spark-receive".to_string(),
                amount_sat,
            });
        }
        Task::none()
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
    ///   or VaultToSpark AND all required inputs (address, feerate, amount)
    ///   are populated and valid. The result carries its source feerate so
    ///   the handler can discard late-arriving results whose feerate has
    ///   since been edited (prevents keystroke-race flicker).
    fn vault_transfer_preview_task(
        &self,
        cache: &Cache,
        daemon: &Option<Arc<dyn Daemon + Sync + Send>>,
    ) -> Task<Message> {
        if !self.legacy_vault_available(cache) {
            return Task::none();
        }
        let Some(direction) = self.transfer_direction else {
            return Task::none();
        };
        if !matches!(
            direction,
            TransferDirection::VaultToLiquid
                | TransferDirection::VaultToSpark
                | TransferDirection::VaultToOtherSpark
                | TransferDirection::VaultToOtherCube
        ) {
            return Task::none();
        }
        if !self.destination_is_ready() {
            return Task::none();
        }
        let generation = self.transfer_request_generation;
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
        let destination = address_info.address.as_unchecked().clone();
        let script = address_info.address.script_pubkey();
        // Max sweeps every owned coin with the destination as the change
        // address, so it gets the balance less the fee. Otherwise this PSBT
        // only answers `fee()`: a fresh change address (`None`) would make the
        // daemon reserve a change index durably on every valid keystroke of
        // the feerate field; the wallet's fixed preview address sizes the
        // change output identically and reserves nothing. The real transfer in
        // `SignVaultToLiquidTx` still passes `None`.
        let (outpoints, destinations, change) = if self.sends_max() {
            (vault_sweep_coins(cache).0, HashMap::new(), destination)
        } else {
            let denomination = if matches!(self.transfer_unit, BitcoinDisplayUnit::BTC) {
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
            let Some(preview_change) = self.wallet.as_ref().map(|w| w.preview_change_address())
            else {
                return Task::none();
            };
            (
                Vec::new(),
                HashMap::from([(destination, amount.to_sat())]),
                preview_change,
            )
        };
        Task::perform(
            async move {
                match daemon
                    .create_spend_tx(&outpoints, &destinations, feerate_vb, Some(change))
                    .await
                {
                    Ok(CreateSpendResult::Success { psbt, .. }) => {
                        let fee = psbt.fee().map_err(|e| e.to_string())?;
                        Ok((fee, paid_to(&psbt, &script)))
                    }
                    Ok(CreateSpendResult::InsufficientFunds { missing }) => {
                        Err(format!("Insufficient funds: {missing} sats missing"))
                    }
                    Err(e) => Err(format!("Preview failed: {e}")),
                }
            },
            move |result| {
                Message::View(view::Message::Home(HomeMessage::TransferPsbtPreviewReady {
                    generation,
                    direction,
                    feerate_vb,
                    result,
                }))
            },
        )
    }

    /// Fetch the Spark wallet balance via `get_info` on the bridge.
    /// Returns `None` when no Spark backend is wired up for the cube
    /// (so the caller can skip scheduling the task entirely) or when
    /// a previous `get_info` is still in flight (so we don't pile up
    /// concurrent RPCs during slow initial sync or SparkEvent bursts).
    /// Sets `spark_balance_loading` for the duration of the task; the
    /// `SparkBalanceUpdated` / `SparkLoadFailed` handlers clear it.
    fn load_spark_balance(&mut self) -> Option<Task<Message>> {
        let backend = self.spark_backend.clone()?;
        if self.spark_balance_loading {
            return None;
        }
        self.spark_balance_loading = true;
        Some(Task::perform(
            async move { backend.get_info().await },
            |result| match result {
                Ok(info) => Message::View(view::Message::Home(HomeMessage::SparkBalanceUpdated {
                    btc: Amount::from_sat(info.balance_sats),
                    usdb_balance: crate::app::breez_spark::assets::usdb_holding(&info),
                    identity_pubkey: info.identity_pubkey,
                })),
                Err(e) => {
                    tracing::warn!("Home: spark get_info failed: {}", e);
                    // Soft-fail: leave the card showing whatever the
                    // last successful fetch returned. The Home handler
                    // just clears the in-flight guard so the next
                    // refresh / retry can fire.
                    Message::View(view::Message::Home(HomeMessage::SparkLoadFailed))
                }
            },
        ))
    }

    fn load_liquid_balance(&self) -> Task<Message> {
        // Networks without a Breez backend (Signet/Testnet) only ever return
        // `NetworkNotSupported` here — skip the RPC entirely to keep logs clean.
        // Hidden Liquid fetches nothing either (`RefreshLiquidBalance` still
        // arrives from the SDK's events).
        if !self.liquid_shown
            || !self
                .breez_client
                .as_ref()
                .is_some_and(|client| client.is_supported())
        {
            return Task::none();
        }
        let Some(breez_client) = self.breez_client.clone() else {
            return Task::none();
        };
        Task::perform(async move { breez_client.info().await }, |info| {
            if let Ok(info) = info {
                let balance = Amount::from_sat(
                    info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat,
                );
                Message::View(view::Message::Home(HomeMessage::LiquidBalanceUpdated(
                    balance,
                )))
            } else {
                Message::View(view::Message::Home(HomeMessage::LiquidBalanceFetchFailed(
                    info.err().map(|e| e.to_string()).unwrap_or_default(),
                )))
            }
        })
    }

    fn load_pending_sends(&self) -> Task<Message> {
        use crate::app::breez_liquid::assets::{asset_kind_for_id, AssetKind};
        if !self.liquid_shown
            || !self
                .breez_client
                .as_ref()
                .is_some_and(|client| client.is_supported())
        {
            return Task::none();
        }
        let Some(breez_client) = self.breez_client.clone() else {
            return Task::none();
        };
        let network = self.network.bitcoin_network();
        Task::perform(
            async move {
                match breez_client.list_payments(Some(20), None, None).await {
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
        if !self.liquid_shown
            || !self
                .breez_client
                .as_ref()
                .is_some_and(|client| client.is_supported())
        {
            return Task::none();
        }
        let Some(breez_client) = self.breez_client.clone() else {
            return Task::none();
        };
        let network = self.network.bitcoin_network();
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

    /// Store or clear the Cube's live Spark cross-network receive
    /// ([`settings::CubeSettings::pending_spark_shift`]).
    fn store_pending_spark_shift(
        &self,
        record: Option<settings::PendingSparkShift>,
    ) -> Task<Message> {
        let network_dir = self.datadir_path.network_directory(self.network);
        let cube_id = self.cube_id.clone();
        Task::perform(
            async move {
                settings::update_settings_file(&network_dir, move |current| {
                    Some(with_pending_spark_shift(current, &cube_id, record))
                })
                .await
            },
            |res| {
                if let Err(e) = res {
                    log::warn!("Failed to store the pending Spark conversion: {}", e);
                }
                Message::Tick
            },
        )
    }

    /// Read the Cube's stored Spark cross-network receive, once a session, and
    /// hand it to the Spark Receive panel. Mainnet only, as conversions are,
    /// and only with a Spark wallet to receive into; elsewhere the record is
    /// left alone for a build or network that can use it.
    fn restore_pending_spark_shift(&mut self) -> Task<Message> {
        if self.spark_shift_restore_requested
            || self.spark_backend.is_none()
            || !matches!(self.network, crate::chain::ChainId::Bitcoin)
        {
            return Task::none();
        }
        self.spark_shift_restore_requested = true;
        let network_dir = self.datadir_path.network_directory(self.network);
        let cube_id = self.cube_id.clone();
        Task::perform(
            async move {
                settings::Settings::from_file(&network_dir)
                    .ok()?
                    .cubes
                    .into_iter()
                    .find(|c| c.id == cube_id)?
                    .pending_spark_shift
            },
            |record| match record {
                Some(record) => Message::View(view::Message::SparkSideshiftReceive(
                    view::SparkSideshiftReceiveMessage::Restore(Box::new(record)),
                )),
                None => Message::Tick,
            },
        )
    }

    fn restore_pending_liquid_to_vault_transfer(&self) -> Task<Message> {
        // Hidden Liquid restores nothing: no banner, and the persisted entry
        // is left for a build that shows Liquid again.
        if !self.liquid_shown {
            return Task::none();
        }
        // Same guard as the balance/pending loaders above: without a connected
        // SDK (`features::liquid` is mainnet-only, so testnet/signet/regtest all
        // get a disconnected client) `list_payments` can only fail. The failure
        // is silent — `.ok()` swallows it and the swap falls through as
        // `Initiated` — so an unguarded restore would resurrect a stale settings
        // entry as a pending-transfer banner that can never progress.
        if !self
            .breez_client
            .as_ref()
            .is_some_and(|client| client.is_supported())
        {
            return Task::none();
        }
        let network_dir = self.datadir_path.network_directory(self.network);
        let cube_id = self.cube_id.clone();
        let Some(breez_client) = self.breez_client.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let settings = settings::Settings::from_file(&network_dir).ok();
                let pending = settings
                    .as_ref()
                    .and_then(|s| s.cubes.iter().find(|c| c.id == cube_id))
                    .and_then(|cube| cube.pending_liquid_to_vault_transfer.clone())?;

                let mut stage = TransferStage::Initiated;
                let payments = breez_client.list_payments(None, None, None).await.ok();

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

/// Inert adapter output used to exercise the real Hub Transfer presenter.
/// `has_spark` is a synthetic installed-wallet flag: constructing a real
/// Spark backend would start a bridge and is intentionally excluded here.
#[cfg(test)]
pub(crate) struct R06ATransferSnapshot {
    pub name: &'static str,
    pub has_spark: bool,
    pub total_balance_loading: bool,
    pub spark_balance_loaded: bool,
    pub spark_balance_error: bool,
    pub spark_balance: Amount,
}

#[cfg(test)]
impl GlobalHome {
    pub(crate) fn r06a_transfer_snapshots() -> Vec<R06ATransferSnapshot> {
        fn inert_home() -> GlobalHome {
            GlobalHome::new_without_wallet(
                None::<Arc<LiquidBackend>>,
                None,
                CoincubeDirectory::new(
                    std::env::temp_dir().join(format!("r06a-transfer-{}", std::process::id())),
                ),
                coincube_core::miniscript::bitcoin::Network::Bitcoin,
                "synthetic-cube".into(),
                false,
            )
        }

        fn send(home: &mut GlobalHome, msg: HomeMessage) {
            // Never poll the returned task: no bridge, network, or persistence.
            let _ = home.update(
                None,
                &Cache::default(),
                Message::View(view::Message::Home(msg)),
            );
        }

        fn snapshot(
            name: &'static str,
            home: &GlobalHome,
            has_spark: bool,
        ) -> R06ATransferSnapshot {
            let total_balance_loading = total_balance_pending(BalanceReadiness {
                has_spark,
                // The real aggregate intentionally settles after a first error.
                spark_loaded: home.spark_balance_loaded || home.spark_balance_error,
                has_liquid: false,
                liquid_loaded: false,
                usdt_loaded: false,
                vault_pending: false,
            });
            R06ATransferSnapshot {
                name,
                has_spark,
                total_balance_loading,
                spark_balance_loaded: home.spark_balance_loaded,
                spark_balance_error: home.spark_balance_error,
                spark_balance: home.spark_balance,
            }
        }

        let mut home = inert_home();
        send(
            &mut home,
            HomeMessage::SparkBalanceUpdated {
                btc: Amount::from_sat(10_000),
                usdb_balance: None,
                identity_pubkey: "spark-identity".into(),
            },
        );
        let presync = snapshot("presync-nonzero", &home, true);
        send(&mut home, HomeMessage::SparkLoadFailed);
        let first_failure = snapshot("first-failure-retained-raw", &home, true);

        let mut no_snapshot = inert_home();
        send(&mut no_snapshot, HomeMessage::SparkLoadFailed);
        let empty_failure = snapshot("first-failure-no-snapshot", &no_snapshot, true);

        send(&mut home, HomeMessage::SparkSyncedObserved);
        send(
            &mut home,
            HomeMessage::SparkBalanceUpdated {
                btc: Amount::from_sat(12_000),
                usdb_balance: None,
                identity_pubkey: "spark-identity".into(),
            },
        );
        let trusted = snapshot("trusted-postsync", &home, true);
        send(&mut home, HomeMessage::SparkLoadFailed);
        let retained = snapshot("trusted-refresh-error", &home, true);

        vec![
            presync,
            first_failure,
            empty_failure,
            trusted,
            retained,
            snapshot("no-spark", &inert_home(), false),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::{OutPoint, Txid};
    use std::str::FromStr;

    const MAINNET_ADDR: &str = "bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg";

    #[test]
    fn transfer_signing_completion_requires_persisted_threshold() {
        let request = Some((3, 7, TransferDirection::VaultToSpark));
        assert!(transfer_signing_completion_message(false, true, request).is_none());
        assert!(transfer_signing_completion_message(true, false, request).is_none());
        assert!(transfer_signing_completion_message(true, true, None).is_none());
        assert!(matches!(
            transfer_signing_completion_message(true, true, request),
            Some(HomeMessage::TransferSigningComplete {
                request_id: 3,
                generation: 7,
                direction: TransferDirection::VaultToSpark,
            })
        ));
    }

    /// A `GlobalHome` with no wallet and a disconnected Liquid backend — enough
    /// to drive the balance-message handlers, which touch nothing else.
    fn panel() -> GlobalHome {
        let network = coincube_core::miniscript::bitcoin::Network::Bitcoin;
        GlobalHome::new_without_wallet(
            Arc::new(LiquidBackend::new(Arc::new(
                crate::app::breez_liquid::BreezClient::disconnected(network),
            ))),
            None,
            CoincubeDirectory::new(std::path::PathBuf::from("/nonexistent")),
            network,
            "cube".to_string(),
            false,
        )
    }

    /// A wallet page's Transfer prefers another wallet in this Cube, falls
    /// back to another Cube (mainnet only) and is otherwise unavailable.
    #[test]
    fn a_wallet_page_transfer_picks_a_destination_for_its_source() {
        use crate::chain::ChainId::{Bitcoin, Regtest};
        use WalletKind::{Liquid, Spark, Vault};
        let from = |wallet, (liquid, vault, spark), chain, other| {
            transfer_direction_from(wallet, liquid, vault, spark, chain, other)
        };
        assert_eq!(
            from(Spark, (false, true, true), Bitcoin, false),
            Some(TransferDirection::SparkToVault)
        );
        assert_eq!(
            from(Vault, (true, true, true), Bitcoin, false),
            Some(TransferDirection::VaultToLiquid)
        );
        // Alone in its Cube: another Cube, when one can receive.
        assert_eq!(from(Spark, (false, false, true), Bitcoin, false), None);
        assert_eq!(
            from(Spark, (false, false, true), Bitcoin, true),
            Some(TransferDirection::SparkToOtherCube)
        );
        assert_eq!(
            from(Vault, (false, true, false), Bitcoin, true),
            Some(TransferDirection::VaultToOtherCube)
        );
        // Other Cubes receive on mainnet only.
        assert_eq!(from(Vault, (false, true, false), Regtest, true), None);
        // A wallet the Cube doesn't have has no Transfer.
        assert_eq!(from(Liquid, (false, true, true), Bitcoin, true), None);
    }

    #[test]
    fn only_a_loaded_non_zero_balance_holds_funds() {
        let mut home = panel();
        assert!(!home.holds_loaded_funds());

        // Still loading: an unconfirmed reading is not counted.
        home.spark_balance = Amount::from_sat(5_000);
        home.liquid_balance = Amount::from_sat(5_000);
        home.usdt_balance = 5_000;
        assert!(!home.holds_loaded_funds());

        // Loaded at zero.
        let mut home = panel();
        home.spark_balance_loaded = true;
        home.liquid_balance_loaded = true;
        home.usdt_balance_loaded = true;
        assert!(!home.holds_loaded_funds());

        let mut spark = panel();
        spark.set_loaded_spark_balance_for_test(1);
        assert!(spark.holds_loaded_funds());

        let mut usdb = panel();
        usdb.spark_balance_loaded = true;
        usdb.spark_usdb_balance = Some(coincube_spark_protocol::UsdbBalanceSnapshot {
            token_identifier: "btkn1test".into(),
            balance: 1,
            decimals: 6,
            ticker: "USDB".into(),
        });
        assert!(usdb.holds_loaded_funds());

        let mut liquid = panel();
        liquid.liquid_balance_loaded = true;
        liquid.liquid_balance = Amount::from_sat(1);
        assert!(liquid.holds_loaded_funds());

        let mut usdt = panel();
        usdt.usdt_balance_loaded = true;
        usdt.usdt_balance = 1;
        assert!(usdt.holds_loaded_funds());
    }

    /// Drive one home message through the real `State::update`.
    fn send(home: &mut GlobalHome, msg: HomeMessage) {
        let _ = home.update(
            None,
            &Cache::default(),
            Message::View(view::Message::Home(msg)),
        );
    }

    #[test]
    fn r06a_presync_spark_holding_stays_untrusted_until_postsync_response() {
        // This is only the inert state adapter: no wallet, Liquid client,
        // Spark bridge, seed, session, or configured data directory exists.
        // `send` drops returned Tasks without running them.
        let directory = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("r06a-synthetic-state-{}", std::process::id())),
        );
        let mut home = GlobalHome::new_without_wallet(
            None::<Arc<LiquidBackend>>,
            None,
            directory,
            coincube_core::miniscript::bitcoin::Network::Bitcoin,
            "synthetic-cube".into(),
            false,
        );
        let stale = coincube_spark_protocol::UsdbBalanceSnapshot {
            token_identifier: "btkn1synthetic".into(),
            balance: 2_500_000,
            decimals: 6,
            ticker: "USDB".into(),
        };
        send(
            &mut home,
            HomeMessage::SparkBalanceUpdated {
                btc: Amount::from_sat(10_000),
                usdb_balance: Some(stale.clone()),
                identity_pubkey: "spark-identity".into(),
            },
        );
        assert_eq!(
            home.spark_usdb_balance.as_ref().map(|s| s.balance),
            Some(2_500_000)
        );
        assert!(!home.spark_balance_loaded);
        assert!(home.spark_balance_received);

        send(&mut home, HomeMessage::SparkLoadFailed);
        assert!(home.spark_balance_error);
        assert!(!home.spark_balance_loaded);
        assert_eq!(
            home.spark_usdb_balance.as_ref().map(|s| s.balance),
            Some(stale.balance)
        );

        send(&mut home, HomeMessage::SparkSyncedObserved);
        assert!(home.spark_synced_seen);
        assert!(!home.spark_balance_loaded);
        let fresh = coincube_spark_protocol::UsdbBalanceSnapshot {
            token_identifier: "btkn1synthetic".into(),
            balance: 3_750_000,
            decimals: 6,
            ticker: "USDB".into(),
        };
        send(
            &mut home,
            HomeMessage::SparkBalanceUpdated {
                btc: Amount::from_sat(12_000),
                usdb_balance: Some(fresh.clone()),
                identity_pubkey: "spark-identity".into(),
            },
        );
        assert!(home.spark_balance_loaded);
        assert!(!home.spark_balance_error);
        assert_eq!(
            home.spark_usdb_balance.as_ref().map(|s| s.balance),
            Some(3_750_000)
        );

        send(&mut home, HomeMessage::SparkLoadFailed);
        assert!(home.spark_balance_loaded);
        assert!(home.spark_balance_error);
        assert_eq!(
            home.spark_usdb_balance.as_ref().map(|s| s.balance),
            Some(fresh.balance)
        );
    }

    #[test]
    fn spark_settles_on_a_synced_balance_not_a_failed_fetch() {
        let mut home = panel();
        assert!(
            home.spark_settled(),
            "a Cube with no Spark has nothing to wait for"
        );
        assert!(home.spark_load_poll().is_none());

        home.spark_backend = Some(Arc::new(SparkBackend::new(Arc::new(
            crate::app::breez_spark::SparkClient::disconnected_for_test(),
        ))));
        assert!(!home.spark_settled());
        assert!(
            home.spark_load_poll().is_some(),
            "the fallback poll runs while the opening waits on Spark"
        );
        send(&mut home, HomeMessage::SparkLoadFailed);
        assert!(!home.spark_settled(), "a failed fetch can precede the sync");
        send(&mut home, HomeMessage::SparkSyncedObserved);
        send(
            &mut home,
            HomeMessage::SparkBalanceUpdated {
                btc: Amount::from_sat(1_000),
                usdb_balance: None,
                identity_pubkey: "spark-identity".into(),
            },
        );
        assert!(home.spark_settled());
        assert!(home.spark_load_poll().is_none());
    }

    /// A refresh blip must not throw away a balance the user already has.
    ///
    /// The two cases pull in opposite directions and the flag has to tell them
    /// apart:
    ///
    /// - **First load fails.** Nothing sits behind it but the untouched
    ///   `Amount::ZERO`. Rendering that is not stale, it is wrong — so the row
    ///   says "Balance unavailable".
    /// - **A later refresh fails.** `liquid_balance` still holds the last good
    ///   value and the Total Balance above still counts it. Blanking the row
    ///   would lose information the user had *and* contradict the total sitting
    ///   on the same screen.
    #[test]
    fn a_refresh_failure_keeps_a_balance_we_already_have() {
        // First load fails: nothing behind it, so the row must not show a zero.
        let mut home = panel();
        send(
            &mut home,
            HomeMessage::LiquidBalanceFetchFailed("blip".into()),
        );
        assert!(
            home.liquid_balance_error,
            "a first-load failure has only the untouched zero behind it"
        );
        assert!(
            home.liquid_balance_loaded,
            "the aggregate must still be allowed to settle"
        );

        // A good value arrives.
        send(
            &mut home,
            HomeMessage::LiquidBalanceUpdated(Amount::from_sat(12_345)),
        );
        assert!(!home.liquid_balance_error);

        // A later refresh fails: keep the number we have.
        send(
            &mut home,
            HomeMessage::LiquidBalanceFetchFailed("blip".into()),
        );
        assert!(
            !home.liquid_balance_error,
            "a transient blip must not blank a balance the user already had"
        );
        assert_eq!(
            home.liquid_balance,
            Amount::from_sat(12_345),
            "the last good value must survive the failure"
        );
    }

    /// The USDt row follows the same rule — it is the twin this was mirrored
    /// from, and had the same defect.
    #[test]
    fn a_usdt_refresh_failure_keeps_a_balance_we_already_have() {
        let mut home = panel();
        send(&mut home, HomeMessage::UsdtBalanceFetchFailed);
        assert!(home.usdt_balance_error, "nothing behind a first failure");

        send(&mut home, HomeMessage::UsdtBalanceUpdated(42));
        assert!(!home.usdt_balance_error);

        send(&mut home, HomeMessage::UsdtBalanceFetchFailed);
        assert!(
            !home.usdt_balance_error,
            "a blip must not blank a USDt balance the user already had"
        );
        assert_eq!(home.usdt_balance, 42);
    }

    /// A wallet whose balance fetch *failed* must stop gating the aggregate.
    ///
    /// The failure path has to mark itself loaded — otherwise one wallet's
    /// network blip leaves the Total Balance spinning forever, taking down the
    /// Vault and Spark rows that answered perfectly well.
    #[test]
    fn a_failed_balance_fetch_does_not_hang_the_total() {
        let all_present = BalanceReadiness {
            has_spark: true,
            spark_loaded: true,
            has_liquid: true,
            liquid_loaded: true,
            usdt_loaded: true,
            vault_pending: false,
        };
        assert!(!total_balance_pending(all_present));

        // Still in flight: the spinner is correct here.
        assert!(total_balance_pending(BalanceReadiness {
            liquid_loaded: false,
            ..all_present
        }));

        // Failed, and marked loaded by the failure handler: the aggregate must
        // settle rather than punish the wallets that answered.
        assert!(!total_balance_pending(BalanceReadiness {
            liquid_loaded: true,
            ..all_present
        }));
        assert!(!total_balance_pending(BalanceReadiness {
            usdt_loaded: true,
            ..all_present
        }));

        // A gated-off Liquid wallet never loads and must not gate anything.
        assert!(!total_balance_pending(BalanceReadiness {
            has_liquid: false,
            liquid_loaded: false,
            usdt_loaded: false,
            ..all_present
        }));

        // A wallet that is present and genuinely outstanding still gates.
        assert!(total_balance_pending(BalanceReadiness {
            spark_loaded: false,
            ..all_present
        }));
        assert!(total_balance_pending(BalanceReadiness {
            vault_pending: true,
            ..all_present
        }));
    }

    /// Every message a task produces, in order.
    fn outputs(task: Task<Message>) -> Vec<Message> {
        use iced_runtime::futures::futures::StreamExt;
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            return Vec::new();
        };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(
                stream
                    .filter_map(|action| async move {
                        match action {
                            iced_runtime::Action::Output(message) => Some(message),
                            _ => None,
                        }
                    })
                    .collect(),
            )
    }

    /// #968 review finding 1: Tenshu v175 hides Liquid (master §3 decision
    /// 2), so `reload` takes the no-Liquid-backend path: it starts no Liquid
    /// fetch and settles both balances so nothing waits on them. The loaders'
    /// own `liquid_shown` checks (`RefreshLiquidBalance`,
    /// `LiquidPeginCompleted`) can't be driven here: the test client is
    /// disconnected, so it fetches nothing either way.
    #[test]
    fn reload_settles_hidden_liquid_without_fetching() {
        let mut hidden = panel();
        hidden.set_liquid_shown(false);
        let _ = hidden.reload(None, None);
        assert!(hidden.liquid_balance_loaded && hidden.usdt_balance_loaded);

        // Control: shown Liquid on the same network still waits for its
        // balances, so the settling above is the gate's doing.
        let mut shown = panel();
        shown.set_liquid_shown(true);
        let _ = shown.reload(None, None);
        assert!(!shown.liquid_balance_loaded && !shown.usdt_balance_loaded);
    }

    /// A fetch that started before Liquid was hidden can fail late. While
    /// hidden it raises no error, and still stops anything waiting on it.
    #[test]
    fn a_late_liquid_fetch_failure_shows_no_error_while_hidden() {
        let fail = |home: &mut GlobalHome, msg| {
            outputs(home.update(
                None,
                &Cache::default(),
                Message::View(view::Message::Home(msg)),
            ))
        };
        let mut hidden = panel();
        hidden.set_liquid_shown(false);
        let produced = fail(
            &mut hidden,
            HomeMessage::LiquidBalanceFetchFailed("blip".into()),
        );
        assert!(produced.is_empty());
        assert!(fail(&mut hidden, HomeMessage::UsdtBalanceFetchFailed).is_empty());
        assert!(hidden.liquid_balance_loaded && hidden.usdt_balance_loaded);

        // Control: the same failure with Liquid shown raises the toast.
        let mut shown = panel();
        shown.set_liquid_shown(true);
        let produced = fail(
            &mut shown,
            HomeMessage::LiquidBalanceFetchFailed("blip".into()),
        );
        assert!(matches!(
            &produced[..],
            [Message::View(view::Message::ShowError(_))]
        ));
    }

    /// The Total Balance doesn't wait on hidden Liquid, even for an account
    /// with the grant and a Liquid wallet on disk.
    #[test]
    fn the_total_does_not_wait_on_hidden_liquid() {
        let gate = crate::app::features::LiquidGate {
            server_enabled: true,
            local_state_exists: true,
        };
        let readiness = BalanceReadiness {
            has_spark: false,
            spark_loaded: false,
            has_liquid: crate::app::features::liquid_wallet_usable(
                crate::chain::ChainId::Bitcoin,
                gate,
            ),
            liquid_loaded: false,
            usdt_loaded: false,
            vault_pending: false,
        };
        assert!(!total_balance_pending(readiness));

        // Control: a shown Liquid wallet that hasn't loaded holds the Total.
        assert!(total_balance_pending(BalanceReadiness {
            has_liquid: true,
            ..readiness
        }));
    }

    /// A restored Vault mid-scan has a tip but no history yet: its empty balance is
    /// still loading, and must not enter the Total Balance as zero.
    #[test]
    fn an_empty_vault_mid_scan_is_still_loading() {
        use coincubed::commands::HistorySync;
        let scanning = HistorySync {
            full_scan_in_progress: true,
            ..Default::default()
        };
        assert!(vault_balance_pending(
            0,
            &HistorySync::default(),
            Amount::ZERO
        ));
        assert!(vault_balance_pending(970_247, &scanning, Amount::ZERO));
        // Coins already known stay on show while it rescans.
        assert!(!vault_balance_pending(
            970_247,
            &scanning,
            Amount::from_sat(1)
        ));
        assert!(!vault_balance_pending(
            970_247,
            &HistorySync::default(),
            Amount::ZERO
        ));
    }

    /// Between failed attempts there is no scan running, but the history still has
    /// not loaded: an empty Vault stays out of the Total Balance.
    #[test]
    fn an_empty_vault_that_keeps_failing_to_sync_is_still_loading() {
        use coincubed::commands::{HistorySync, PollFailure};
        let failing = |consecutive| HistorySync {
            last_failure: Some(PollFailure {
                message: "Esplora client error".into(),
                since: 1,
                last_at: 2,
                consecutive,
            }),
            ..Default::default()
        };
        assert!(vault_balance_pending(970_247, &failing(2), Amount::ZERO));
        // One failure is a hiccup, not a reason to hide the Vault.
        assert!(!vault_balance_pending(970_247, &failing(1), Amount::ZERO));
        assert!(!vault_balance_pending(
            970_247,
            &failing(5),
            Amount::from_sat(1)
        ));
    }

    #[test]
    fn transfer_defaults_to_an_available_wallet_or_another_cube() {
        use crate::chain::ChainId;
        use TransferDirection::{
            LiquidToSpark, LiquidToVault, SparkToOtherCube, VaultToOtherCube, VaultToSpark,
        };

        for (liquid, vault, spark, direction) in [
            (true, true, true, Some(LiquidToVault)),
            (true, false, true, Some(LiquidToSpark)),
            (true, true, false, Some(LiquidToVault)),
            (false, true, true, Some(VaultToSpark)),
            (false, true, false, Some(VaultToOtherCube)),
            (false, false, true, Some(SparkToOtherCube)),
            (true, false, false, None),
            (false, false, false, None),
        ] {
            assert_eq!(
                default_transfer_direction(liquid, vault, spark, ChainId::Bitcoin),
                direction
            );
        }
        assert_eq!(
            default_transfer_direction(false, false, true, ChainId::Regtest),
            None
        );
    }

    fn address() -> Address {
        Address::from_str(MAINNET_ADDR).unwrap().assume_checked()
    }

    fn outpoint(n: u32) -> OutPoint {
        OutPoint::new(Txid::from_str(&format!("{n:064x}")).unwrap(), n)
    }

    fn coin(
        n: u32,
        sats: u64,
        block_height: Option<i32>,
        is_from_self: bool,
        spent: bool,
    ) -> crate::daemon::model::Coin {
        crate::daemon::model::Coin {
            amount: Amount::from_sat(sats),
            outpoint: outpoint(n),
            address: address(),
            block_height,
            derivation_index: ChildNumber::from(n),
            spend_info: spent.then(|| coincubed::commands::LCSpendInfo {
                txid: Txid::from_str(&format!("{:064x}", n + 100)).unwrap(),
                height: None,
            }),
            is_immature: false,
            is_change: false,
            is_from_self,
        }
    }

    fn liquid_bitcoin_payment(payment_type: PaymentType) -> breez_sdk_liquid::prelude::Payment {
        breez_sdk_liquid::prelude::Payment {
            destination: Some("destination".to_string()),
            tx_id: Some("txid".to_string()),
            unblinding_data: Some("unblind".to_string()),
            timestamp: 1_722_000_000,
            amount_sat: 25_000,
            fees_sat: 125,
            swapper_fees_sat: Some(50),
            payment_type,
            status: breez_sdk_liquid::model::PaymentState::Pending,
            details: PaymentDetails::Bitcoin {
                swap_id: "swap-id".to_string(),
                bitcoin_address: MAINNET_ADDR.to_string(),
                description: "chain swap".to_string(),
                auto_accepted_fees: true,
                liquid_expiration_blockheight: 123,
                bitcoin_expiration_blockheight: 456,
                lockup_tx_id: Some("lockup".to_string()),
                claim_tx_id: Some("claim".to_string()),
                refund_tx_id: Some("refund".to_string()),
                refund_tx_amount_sat: Some(900),
            },
        }
    }

    #[test]
    fn default_transfer_feerate_is_valid_and_unwarned() {
        let feerate = default_transfer_feerate();

        assert_eq!(feerate.value, DEFAULT_TRANSFER_FEERATE_SATS_VB);
        assert!(feerate.valid);
        assert_eq!(feerate.warning, None);
    }

    #[test]
    fn cache_vault_pending_receive_counts_only_external_unconfirmed_unspent_coins() {
        let mut cache = Cache::default();
        cache.daemon_cache.coins = vec![
            coin(1, 5_000, None, false, false),
            coin(2, 7_000, None, false, false),
            coin(3, 11_000, Some(100), false, false),
            coin(4, 13_000, None, true, false),
            coin(5, 17_000, None, false, true),
        ];

        assert_eq!(cache_vault_pending_receive_sats(&cache), 12_000);
    }

    #[test]
    fn receive_address_info_exposes_address_label_target_and_mutable_labels() {
        let mut info = ReceiveAddressInfo {
            address: address(),
            index: ChildNumber::from(7),
            labels: HashMap::new(),
        };
        let address_key = info.address.to_string();

        assert_eq!(
            info.labelled(),
            vec![LabelItem::Address(info.address.clone())]
        );

        info.labels()
            .insert(address_key.clone(), "deposit address".to_string());

        assert_eq!(
            info.labels.get(&address_key).map(String::as_str),
            Some("deposit address")
        );
    }

    #[test]
    fn bitcoin_send_swap_id_only_returns_ids_for_sent_bitcoin_payments() {
        let send = liquid_bitcoin_payment(PaymentType::Send);
        let receive = liquid_bitcoin_payment(PaymentType::Receive);

        assert_eq!(bitcoin_send_swap_id(&send).as_deref(), Some("swap-id"));
        assert_eq!(bitcoin_send_swap_id(&receive), None);
    }

    #[test]
    fn modal_default_debug_is_stable() {
        assert_eq!(format!("{:?}", Modal::default()), "None");
    }

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
            TransferDirection::SparkToOtherCube,
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

    /// The Vault→Liquid/Spark transfer preview exists only to read `psbt.fee()`,
    /// and it re-runs on every valid keystroke of the feerate field with no
    /// debounce. `createspend` reserves a change index durably whenever it is
    /// asked for a fresh change address (`None`), so every one of those
    /// previews used to burn an index the wallet would never use. The preview
    /// must ask with the wallet's fixed sizing address instead, which reserves
    /// nothing and sizes the change output identically.
    mod transfer_preview_change_address {
        use super::*;
        use crate::app::view::global_home::TransferDirection;
        use crate::daemon::{Daemon, DaemonBackend, DaemonError};
        use crate::node::NodeType;
        use coincube_core::{
            descriptors::CoincubeDescriptor,
            miniscript::bitcoin::{address, bip32::ChildNumber, psbt::Psbt, Address, Network},
        };
        use coincubed::{
            bip329::Labels,
            commands::{CoinStatus, LabelItem, UpdateDerivIndexesResult},
        };
        use std::collections::HashSet;
        use std::sync::Mutex;

        const DESC: &str = "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr";

        /// Records what the preview asks the daemon for. Everything else is off
        /// limits: a preview that touched any other daemon call would be a
        /// different bug.
        #[derive(Debug, Default)]
        struct RecordingDaemon {
            calls: Mutex<Vec<(u64, Option<Address<address::NetworkUnchecked>>)>>,
        }

        #[async_trait::async_trait]
        impl Daemon for RecordingDaemon {
            fn backend(&self) -> DaemonBackend {
                DaemonBackend::EmbeddedCoincubed(Some(NodeType::Bitcoind))
            }

            fn config(&self) -> Option<&coincubed::config::Config> {
                None
            }

            async fn is_alive(
                &self,
                _datadir: &CoincubeDirectory,
                _network: Network,
            ) -> Result<(), DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn stop(&self) -> Result<(), DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn get_info(&self) -> Result<crate::daemon::model::GetInfoResult, DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn request_sync(&self) -> Result<(), DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn get_new_address(
                &self,
            ) -> Result<crate::daemon::model::GetAddressResult, DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn list_revealed_addresses(
                &self,
                _is_change: bool,
                _exclude_used: bool,
                _limit: usize,
                _start_index: Option<ChildNumber>,
            ) -> Result<crate::daemon::model::ListRevealedAddressesResult, DaemonError>
            {
                unreachable!("test daemon should not be queried")
            }

            async fn update_deriv_indexes(
                &self,
                _receive: Option<u32>,
                _change: Option<u32>,
            ) -> Result<UpdateDerivIndexesResult, DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn list_coins(
                &self,
                _statuses: &[CoinStatus],
                _outpoints: &[OutPoint],
            ) -> Result<crate::daemon::model::ListCoinsResult, DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn list_spend_txs(
                &self,
            ) -> Result<crate::daemon::model::ListSpendResult, DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn create_spend_tx(
                &self,
                _coins_outpoints: &[OutPoint],
                _destinations: &HashMap<Address<address::NetworkUnchecked>, u64>,
                feerate_vb: u64,
                change_address: Option<Address<address::NetworkUnchecked>>,
            ) -> Result<crate::daemon::model::CreateSpendResult, DaemonError> {
                self.calls
                    .lock()
                    .unwrap()
                    .push((feerate_vb, change_address));
                let tx = coincube_core::miniscript::bitcoin::Transaction {
                    version: coincube_core::miniscript::bitcoin::transaction::Version::TWO,
                    lock_time: coincube_core::miniscript::bitcoin::absolute::LockTime::ZERO,
                    input: vec![],
                    output: vec![],
                };
                Ok(crate::daemon::model::CreateSpendResult::Success {
                    psbt: Psbt::from_unsigned_tx(tx).expect("empty unsigned tx"),
                    warnings: vec![],
                })
            }

            async fn rbf_psbt(
                &self,
                _txid: &Txid,
                _is_cancel: bool,
                _feerate_vb: Option<u64>,
            ) -> Result<crate::daemon::model::CreateSpendResult, DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn update_spend_tx(&self, _psbt: &Psbt) -> Result<(), DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn delete_spend_tx(&self, _txid: &Txid) -> Result<(), DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn broadcast_spend_tx(&self, _txid: &Txid) -> Result<(), DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn start_rescan(&self, _t: u32) -> Result<(), DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn list_confirmed_txs(
                &self,
                _start: u32,
                _end: u32,
                _limit: u64,
            ) -> Result<crate::daemon::model::ListTransactionsResult, DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn create_recovery(
                &self,
                _address: Address<address::NetworkUnchecked>,
                _coins_outpoints: &[OutPoint],
                _feerate_vb: u64,
                _sequence: Option<u16>,
            ) -> Result<Psbt, DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn list_txs(
                &self,
                _txid: &[Txid],
            ) -> Result<crate::daemon::model::ListTransactionsResult, DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn get_labels(
                &self,
                _labels: &HashSet<LabelItem>,
            ) -> Result<HashMap<String, String>, DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn update_labels(
                &self,
                _labels: &HashMap<LabelItem, Option<String>>,
            ) -> Result<(), DaemonError> {
                unreachable!("test daemon should not be queried")
            }

            async fn get_labels_bip329(
                &self,
                _offset: u32,
                _limit: u32,
            ) -> Result<Labels, DaemonError> {
                unreachable!("test daemon should not be queried")
            }
        }

        /// Execute the async work a message handed back, exactly as the iced
        /// runtime would, so the daemon call actually happens.
        fn drain(task: Task<Message>) {
            use iced_runtime::futures::futures::StreamExt;
            let Some(stream) = iced_runtime::task::into_stream(task) else {
                return;
            };
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move { stream.for_each(|_| async {}).await });
        }

        /// A wallet-backed home sitting on the Vault→Liquid confirm screen with a
        /// valid amount and destination, the state in which the feerate field
        /// previews on every keystroke.
        fn home_on_transfer_screen() -> (GlobalHome, Arc<Wallet>) {
            let wallet = Arc::new(Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap()));
            let network = coincube_core::miniscript::bitcoin::Network::Bitcoin;
            let mut home = GlobalHome::new(
                wallet.clone(),
                Arc::new(LiquidBackend::new(Arc::new(
                    crate::app::breez_liquid::BreezClient::disconnected(network),
                ))),
                None,
                CoincubeDirectory::new(std::path::PathBuf::from("/nonexistent")),
                crate::chain::ChainId::Bitcoin,
                "cube".to_string(),
                false,
            );
            home.transfer_direction = Some(TransferDirection::VaultToLiquid);
            home.transfer_from = Some(WalletKind::Vault);
            home.transfer_to = Some(WalletKind::Liquid);
            home.current_view.step = 2;
            home.receive_address_info = Some(ReceiveAddressInfo {
                address: Address::from_str(MAINNET_ADDR).unwrap().assume_checked(),
                index: ChildNumber::from(0),
                labels: HashMap::new(),
            });
            home.destination_ready = Some((0, TransferDirection::VaultToLiquid));
            home.entered_amount = form::Value {
                value: "100000".to_string(),
                warning: None,
                valid: true,
            };
            (home, wallet)
        }

        #[test]
        fn feerate_edits_preview_with_the_fixed_sizing_address_never_a_fresh_one() {
            let (mut home, wallet) = home_on_transfer_screen();
            let daemon = Arc::new(RecordingDaemon::default());
            let as_daemon: Arc<dyn Daemon + Sync + Send> = daemon.clone();
            let cache = Cache {
                has_vault: true,
                ..Cache::default()
            };
            // Type a two-digit feerate one keystroke at a time, then correct it:
            // four valid intermediate values, four previews, four PSBTs thrown away.
            for keystroke in ["2", "25", "2", "20"] {
                drain(home.update(
                    Some(as_daemon.clone()),
                    &cache,
                    Message::View(view::Message::Home(HomeMessage::SetTransferFeerate(
                        keystroke.to_string(),
                    ))),
                ));
            }
            let calls = daemon.calls.lock().unwrap();
            assert_eq!(
                calls
                    .iter()
                    .map(|(feerate, _)| *feerate)
                    .collect::<Vec<_>>(),
                vec![2, 25, 2, 20],
                "every valid keystroke previews once"
            );
            let sizing = wallet.preview_change_address();
            for (feerate, change) in calls.iter() {
                assert_eq!(
                    change.as_ref(),
                    Some(&sizing),
                    "preview at {feerate} sat/vb asked the daemon for a fresh change address; \
                     that reserves a change index the wallet will never use"
                );
            }
        }

        #[test]
        fn hidden_vault_transfer_messages_cannot_construct_an_imported_spend() {
            let (mut home, _) = home_on_transfer_screen();
            let daemon = Arc::new(RecordingDaemon::default());
            let as_daemon: Arc<dyn Daemon + Sync + Send> = daemon.clone();
            // A common Bitcoin Wallet may be loaded without a Cube Vault.
            // Even forged/stale legacy transfer messages must stop before a
            // daemon create_spend_tx call or an in-memory picker mutation.
            // The shared fixture has a Vault source, and Cache::default()
            // reports a Vault. Clear both to model the absent-Vault premise.
            let cache = Cache {
                has_vault: false,
                ..Cache::default()
            };
            home.transfer_from = None;
            home.transfer_direction = None;
            home.wallet_picker = Some(PickerSide::From);
            drain(home.update(
                Some(as_daemon.clone()),
                &cache,
                Message::View(view::Message::Home(HomeMessage::SelectWalletInPicker(
                    WalletKind::Vault,
                ))),
            ));
            assert_eq!(home.wallet_picker, Some(PickerSide::From));
            assert_eq!(home.transfer_from, None);

            // Inline rows use the same guard as the old picker; a stale
            // direct selection cannot make an unavailable Vault the source.
            drain(home.update(
                Some(as_daemon.clone()),
                &cache,
                Message::View(view::Message::Home(HomeMessage::SelectTransferWallet(
                    PickerSide::From,
                    WalletKind::Vault,
                ))),
            ));
            assert_eq!(home.transfer_from, None);

            for message in [
                HomeMessage::SetTransferFeerate("25".to_string()),
                HomeMessage::SignVaultToLiquidTx,
                HomeMessage::ConfirmTransfer,
            ] {
                drain(home.update(
                    Some(as_daemon.clone()),
                    &cache,
                    Message::View(view::Message::Home(message)),
                ));
            }
            assert!(daemon.calls.lock().unwrap().is_empty());
            assert!(!home.is_sending);
        }

        /// `has_vault` is a cache flag. An imported wallet does not support
        /// inheritance, so a stale `true` must not let a Vault picker choice,
        /// a feerate preview, or a prepared Spark→Vault send through.
        #[test]
        fn a_stale_vault_flag_cannot_spend_an_imported_wallet() {
            use coincube_core::miniscript::bitcoin::{
                bip32::{DerivationPath, Xpriv, Xpub},
                secp256k1::Secp256k1,
            };
            use coincube_core::wallet_descriptor::ImportedWalletDescriptor;

            let secp = Secp256k1::new();
            let master = Xpriv::new_master(Network::Bitcoin, &[44; 32]).unwrap();
            let account = master
                .derive_priv(&secp, &DerivationPath::from_str("m/84h/0h/0h").unwrap())
                .unwrap();
            let key = format!(
                "[{}/84h/0h/0h]{}",
                master.fingerprint(&secp),
                Xpub::from_priv(&secp, &account)
            );
            let descriptor = ImportedWalletDescriptor::parse(
                &format!("wpkh({key}/0/*)"),
                Some(&format!("wpkh({key}/1/*)")),
                coincube_core::chain::ChainId::Bitcoin,
            )
            .unwrap();
            let (mut home, _) = home_on_transfer_screen();
            home.wallet = Some(Arc::new(Wallet::new(
                coincube_core::wallet_descriptor::WalletDescriptor::Imported(descriptor),
            )));
            assert!(!home.wallet.as_ref().unwrap().supports_inheritance());

            let daemon = Arc::new(RecordingDaemon::default());
            let as_daemon: Arc<dyn Daemon + Sync + Send> = daemon.clone();
            let cache = Cache {
                has_vault: true,
                ..Cache::default()
            };

            // The fixture already pairs Vault -> Liquid; the refused choice
            // must leave the picker open and the pair untouched.
            let pair_before = (home.transfer_from, home.transfer_to);
            home.wallet_picker = Some(PickerSide::From);
            drain(home.update(
                Some(as_daemon.clone()),
                &cache,
                Message::View(view::Message::Home(HomeMessage::SelectWalletInPicker(
                    WalletKind::Vault,
                ))),
            ));
            assert_eq!(home.wallet_picker, Some(PickerSide::From));
            assert_eq!((home.transfer_from, home.transfer_to), pair_before);

            drain(home.update(
                Some(as_daemon.clone()),
                &cache,
                Message::View(view::Message::Home(HomeMessage::SetTransferFeerate(
                    "25".to_string(),
                ))),
            ));
            drain(home.update(
                Some(as_daemon.clone()),
                &cache,
                Message::View(view::Message::Home(HomeMessage::SignVaultToLiquidTx)),
            ));
            assert!(daemon.calls.lock().unwrap().is_empty());
            assert!(!home.is_sending);

            home.transfer_direction = Some(TransferDirection::SparkToVault);
            home.spark_send_handle = Some("prepared-handle".to_string());
            drain(home.update(
                Some(as_daemon),
                &cache,
                Message::View(view::Message::Home(HomeMessage::ConfirmSparkSend)),
            ));
            assert_eq!(home.spark_send_handle.as_deref(), Some("prepared-handle"));
            assert!(daemon.calls.lock().unwrap().is_empty());
        }

        #[test]
        fn back_and_destination_switch_reject_old_address_until_new_request_completes() {
            let (mut home, wallet) = home_on_transfer_screen();
            let daemon = Arc::new(RecordingDaemon::default());
            let as_daemon: Arc<dyn Daemon + Sync + Send> = daemon.clone();
            let cache = Cache {
                has_vault: true,
                ..Cache::default()
            };
            let old_generation = home.transfer_request_generation;
            drain(home.update(
                Some(as_daemon.clone()),
                &cache,
                Message::View(view::Message::Home(HomeMessage::PreviousStep)),
            ));
            assert!(home.receive_address_info.is_none());
            assert!(!home.destination_is_ready());

            // This callback test has no Spark backend. Model the new pair and
            // pending request directly; the selection guard stays enforced.
            home.transfer_to = Some(WalletKind::Spark);
            home.transfer_direction = Some(TransferDirection::VaultToSpark);
            assert_eq!(
                home.transfer_direction,
                Some(TransferDirection::VaultToSpark)
            );
            assert!(home.receive_address_info.is_none());

            // A real Vault→Spark Next starts the asynchronous Spark address fetch.
            // Model that pending step without starting the Spark bridge in this test.
            let generation = home.invalidate_transfer_destination();
            home.current_view.step = 2;
            for (callback_generation, callback_direction) in [
                (old_generation, TransferDirection::VaultToLiquid),
                (generation, TransferDirection::VaultToLiquid),
            ] {
                drain(home.update(
                    Some(as_daemon.clone()),
                    &cache,
                    Message::View(view::Message::Home(HomeMessage::TransferDestinationReady {
                        generation: callback_generation,
                        direction: callback_direction,
                        result: Ok((MAINNET_ADDR.to_string(), ChildNumber::from(1))),
                    })),
                ));
                assert!(home.receive_address_info.is_none());
                assert!(!home.destination_is_ready());
            }
            drain(home.update(
                Some(as_daemon.clone()),
                &cache,
                Message::View(view::Message::Home(HomeMessage::TransferPsbtPreviewReady {
                    generation: old_generation,
                    direction: TransferDirection::VaultToLiquid,
                    feerate_vb: 2,
                    result: Ok((Amount::from_sat(500), Amount::from_sat(100_000))),
                })),
            ));
            assert!(home.spend_tx_fees.is_none());
            drain(home.update(
                Some(as_daemon.clone()),
                &cache,
                Message::View(view::Message::Home(HomeMessage::SignVaultToLiquidTx)),
            ));
            assert!(daemon.calls.lock().unwrap().is_empty());

            // A prior Vault→Liquid PSBT may finish after the new Vault→Spark
            // attempt has started. It must not clear B's pending state or
            // reopen a signer for A's old destination.
            let stale_psbt =
                Psbt::from_unsigned_tx(coincube_core::miniscript::bitcoin::Transaction {
                    version: coincube_core::miniscript::bitcoin::transaction::Version::TWO,
                    lock_time: coincube_core::miniscript::bitcoin::absolute::LockTime::ZERO,
                    input: vec![],
                    output: vec![],
                })
                .unwrap();
            home.is_sending = true;
            drain(home.update(
                Some(as_daemon.clone()),
                &cache,
                Message::View(view::Message::Home(HomeMessage::TransferPsbtReady {
                    request_id: 1,
                    generation: old_generation,
                    direction: TransferDirection::VaultToLiquid,
                    destination: MAINNET_ADDR.to_string(),
                    amount_sat: 100_000,
                    feerate_vb: 2,
                    result: Ok((
                        stale_psbt,
                        Some(wallet),
                        (cache.datadir_path.clone(), cache.network),
                    )),
                })),
            ));
            assert!(home.is_sending);
            assert!(home.transfer_spend_tx.is_none());
            assert!(matches!(home.modal, Modal::None));
            drain(home.update(
                Some(as_daemon.clone()),
                &cache,
                Message::View(view::Message::Home(HomeMessage::TransferSigningComplete {
                    request_id: 1,
                    generation: old_generation,
                    direction: TransferDirection::VaultToLiquid,
                })),
            ));
            assert!(!home.transfer_signed);
            assert!(home.is_sending);

            drain(home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::TransferDestinationReady {
                    generation,
                    direction: TransferDirection::VaultToSpark,
                    result: Ok((
                        "1BoatSLRHtKNngkdXEeobR76b53LETtpyT".to_string(),
                        ChildNumber::from(1),
                    )),
                })),
            ));
            assert!(home.destination_is_ready());
            assert_eq!(
                home.receive_address_info
                    .as_ref()
                    .unwrap()
                    .address
                    .to_string(),
                "1BoatSLRHtKNngkdXEeobR76b53LETtpyT"
            );
            drain(home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::TransferDestinationReady {
                    generation: old_generation,
                    direction: TransferDirection::VaultToLiquid,
                    result: Ok((MAINNET_ADDR.to_string(), ChildNumber::from(1))),
                })),
            ));
            assert_eq!(
                home.receive_address_info.unwrap().address.to_string(),
                "1BoatSLRHtKNngkdXEeobR76b53LETtpyT"
            );
        }

        #[test]
        fn current_psbt_result_releases_busy_state_after_fee_mismatch() {
            let (mut home, _) = home_on_transfer_screen();
            let cache = Cache {
                has_vault: true,
                ..Cache::default()
            };
            home.is_sending = true;
            home.active_psbt_request = Some(9);
            drain(home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::SetTransferFeerate(
                    "5".to_string(),
                ))),
            ));
            assert_eq!(home.transfer_feerate.value, "2");

            // A stale preset or external setting can still change a value
            // while work is in flight. Reject that PSBT, but release its own
            // busy state so the user can retry or go back.
            home.transfer_feerate.value = "5".to_string();
            let generation = home.transfer_request_generation;
            drain(home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::TransferPsbtReady {
                    request_id: 9,
                    generation,
                    direction: TransferDirection::VaultToLiquid,
                    destination: MAINNET_ADDR.to_string(),
                    amount_sat: 100_000,
                    feerate_vb: 2,
                    result: Err("late result".to_string()),
                })),
            ));
            assert!(!home.is_sending);
            assert!(home.active_psbt_request.is_none());
            assert!(home.transfer_spend_tx.is_none());

            home.active_psbt_request = Some(11);
            home.is_sending = true;
            drain(home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::TransferPsbtReady {
                    request_id: 10,
                    generation,
                    direction: TransferDirection::VaultToLiquid,
                    destination: MAINNET_ADDR.to_string(),
                    amount_sat: 100_000,
                    feerate_vb: 5,
                    result: Err("older result".to_string()),
                })),
            ));
            assert!(home.is_sending);
            assert_eq!(home.active_psbt_request, Some(11));
        }

        #[test]
        fn late_preparation_results_cannot_populate_a_new_transfer_attempt() {
            let (mut home, _) = home_on_transfer_screen();
            let cache = Cache::default();
            let old_generation = home.transfer_request_generation;
            home.transfer_direction = Some(TransferDirection::LiquidToSpark);
            let generation = home.invalidate_transfer_destination();
            let response = PreparePayOnchainResponse {
                receiver_amount_sat: 100_000,
                claim_fees_sat: 100,
                total_fees_sat: 1_000,
            };
            drain(home.update(
                None,
                &cache,
                Message::View(view::Message::Home(
                    HomeMessage::PrepareOnChainResponseReceived {
                        generation: old_generation,
                        direction: TransferDirection::LiquidToSpark,
                        response: response.clone(),
                    },
                )),
            ));
            assert!(home.prepare_onchain_send_response.is_none());
            drain(home.update(
                None,
                &cache,
                Message::View(view::Message::Home(
                    HomeMessage::PrepareOnChainResponseReceived {
                        generation,
                        direction: TransferDirection::LiquidToSpark,
                        response,
                    },
                )),
            ));
            assert!(home.prepare_onchain_send_response.is_some());

            home.transfer_direction = Some(TransferDirection::SparkToLiquid);
            let spark_generation = home.invalidate_transfer_destination();
            for callback_generation in [generation, old_generation] {
                drain(home.update(
                    None,
                    &cache,
                    Message::View(view::Message::Home(HomeMessage::SparkPrepareSendReady {
                        generation: callback_generation,
                        direction: TransferDirection::SparkToLiquid,
                        destination: MAINNET_ADDR.to_string(),
                        prepare_handle: "stale".to_string(),
                        fee_sat: 123,
                    })),
                ));
                assert!(home.spark_send_handle.is_none());
                assert!(home.receive_address_info.is_none());
            }
            drain(home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::SparkPrepareSendReady {
                    generation: spark_generation,
                    direction: TransferDirection::SparkToLiquid,
                    destination: MAINNET_ADDR.to_string(),
                    prepare_handle: "current".to_string(),
                    fee_sat: 123,
                })),
            ));
            assert_eq!(home.spark_send_handle.as_deref(), Some("current"));
            assert!(home.destination_is_ready());
        }

        /// Tenshu v175 hides Liquid, so neither transfer side accepts it, even
        /// with the grant and a Liquid wallet on disk: the pair, its destination
        /// and the request generation stay as they were. (The From choice that
        /// swaps an existing pair is now reachable only with Spark, whose
        /// bridge has no test double here; that code is unchanged.)
        #[test]
        fn inline_selection_refuses_hidden_liquid() {
            let (mut home, _) = home_on_transfer_screen();
            home.transfer_from = Some(WalletKind::Vault);
            home.transfer_to = Some(WalletKind::Liquid);
            let generation = home.transfer_request_generation;
            assert!(home.destination_is_ready());
            let cache = Cache {
                has_vault: true,
                liquid_gate: crate::app::features::LiquidGate {
                    server_enabled: true,
                    local_state_exists: true,
                },
                ..Cache::default()
            };
            for side in [PickerSide::From, PickerSide::To] {
                drain(home.update(
                    None,
                    &cache,
                    Message::View(view::Message::Home(HomeMessage::SelectTransferWallet(
                        side,
                        WalletKind::Liquid,
                    ))),
                ));
                assert_eq!(home.transfer_from, Some(WalletKind::Vault));
                assert_eq!(home.transfer_to, Some(WalletKind::Liquid));
                assert_eq!(
                    home.transfer_direction,
                    Some(TransferDirection::VaultToLiquid)
                );
                assert_eq!(home.transfer_request_generation, generation);
                assert!(home.destination_is_ready());
                assert_eq!(home.entered_amount.value, "100000");
                assert!(home.entered_amount.valid);
            }
        }

        #[test]
        fn the_sizing_address_is_the_wallet_own_change_index_zero() {
            let wallet = Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap());
            let secp =
                coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::verification_only();
            let expected = wallet
                .inheritance_descriptor()
                .unwrap()
                .change_descriptor()
                .derive(0.into(), &secp)
                .address(coincube_core::miniscript::bitcoin::Network::Bitcoin);
            assert_eq!(
                wallet.preview_change_address(),
                expected.as_unchecked().clone()
            );
        }

        fn transfer_cache() -> Cache {
            Cache {
                has_vault: true,
                // Never the real datadir: the Other Cube catalog reads it.
                datadir_path: CoincubeDirectory::new(
                    std::env::temp_dir().join("coincube-transfer-other-cube-none"),
                ),
                ..Cache::default()
            }
        }

        /// The messages a task produces, for feeding back into `update`.
        fn collect(task: Task<Message>) -> Vec<Message> {
            use iced_runtime::futures::futures::StreamExt;
            let Some(stream) = iced_runtime::task::into_stream(task) else {
                return Vec::new();
            };
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                stream
                    .filter_map(|action| async move {
                        match action {
                            iced_runtime::Action::Output(message) => Some(message),
                            _ => None,
                        }
                    })
                    .collect()
                    .await
            })
        }

        fn send(home: &mut GlobalHome, cache: &Cache, message: HomeMessage) {
            drain(home.update(None, cache, Message::View(view::Message::Home(message))));
        }

        fn checked_other_address() -> OtherReceiveAddress {
            OtherReceiveAddress::Bitcoin(
                crate::app::transfer_destination_loader::DestinationAddress {
                    address: Address::from_str(MAINNET_ADDR).unwrap().assume_checked(),
                    index: ChildNumber::from(3),
                },
            )
        }

        fn receiving_cube_catalog(with_vault: bool) -> (tempfile::TempDir, CoincubeDirectory) {
            use crate::app::settings::{
                CubeSettings, Settings, WalletSettings, SETTINGS_FILE_NAME,
            };
            use crate::chain::ChainId;

            let directory = tempfile::tempdir().unwrap();
            let root = CoincubeDirectory::new(directory.path().to_owned());
            let bitcoin = root.network_directory(ChainId::Bitcoin);
            std::fs::create_dir_all(bitcoin.path()).unwrap();
            let mut destination = CubeSettings::new_with_raw_id(
                "destination".into(),
                "Archive".into(),
                ChainId::Bitcoin,
            );
            let wallet = WalletSettings {
                name: "Archive Vault".into(),
                alias: None,
                descriptor_checksum: "destination".into(),
                pinned_at: Some(1),
                keys: vec![],
                hardware_wallets: vec![],
                remote_backend_auth: None,
                start_internal_bitcoind: None,
                pending_rescan: None,
                keychain_keys_recorded: false,
            };
            if with_vault {
                destination.vault_wallet_id = Some(wallet.wallet_id());
                destination.vault_fingerprint = Some("archive-vault".into());
            }
            destination.master_signer_fingerprint = Some("12345678".parse().unwrap());
            std::fs::write(
                bitcoin.path().join(SETTINGS_FILE_NAME),
                serde_json::to_vec(&Settings {
                    cubes: vec![destination],
                    wallets: with_vault.then_some(wallet).into_iter().collect(),
                    ..Settings::default()
                })
                .unwrap(),
            )
            .unwrap();

            (directory, root)
        }

        #[test]
        fn spark_only_cube_prepares_and_sends_to_another_cubes_vault() {
            spark_only_cube_sends_to_another_cube(WalletKind::Vault);
        }

        #[test]
        fn spark_only_cube_prepares_and_sends_to_another_cubes_spark() {
            spark_only_cube_sends_to_another_cube(WalletKind::Spark);
        }

        #[test]
        fn the_to_menu_lists_other_cubes_and_a_pick_starts_their_unlock() {
            use crate::app::breez_spark::SparkClient;
            use crate::chain::ChainId;

            let (_directory, root) = receiving_cube_catalog(true);
            let (client, _requests) = SparkClient::mock_transport();
            let cache = Cache {
                datadir_path: root.clone(),
                ..Cache::default()
            };
            let mut home = GlobalHome::new_without_wallet(
                None,
                Some(Arc::new(SparkBackend::new(Arc::new(client)))),
                root.clone(),
                ChainId::Bitcoin,
                "source".into(),
                false,
            );
            send(
                &mut home,
                &cache,
                HomeMessage::SparkBalanceUpdated {
                    btc: Amount::from_sat(200_000),
                    usdb_balance: None,
                    identity_pubkey: "source-spark".into(),
                },
            );
            send(&mut home, &cache, HomeMessage::NextStep);
            // Spark only: another Cube is the only route, picked from To.
            assert_eq!(home.transfer_page, TransferPage::Wallets);
            assert!(home.other_cubes.is_empty());

            send(
                &mut home,
                &cache,
                HomeMessage::OpenWalletPicker(PickerSide::To),
            );
            assert_eq!(home.wallet_picker, Some(PickerSide::To));
            assert!(home
                .other_cubes
                .iter()
                .any(|choice| choice.cube_id == "destination"));
            // A second press closes it.
            send(
                &mut home,
                &cache,
                HomeMessage::OpenWalletPicker(PickerSide::To),
            );
            assert_eq!(home.wallet_picker, None);

            send(
                &mut home,
                &cache,
                HomeMessage::PickOtherCubeWallet("destination".into(), WalletKind::Vault),
            );
            assert_eq!(
                home.transfer_direction,
                Some(TransferDirection::SparkToOtherCube)
            );
            assert_eq!(home.other_cube_selected.as_deref(), Some("destination"));
            assert_eq!(home.transfer_page, TransferPage::Unlock);
            assert!(home.other_scope.is_some());

            // The same pick again keeps the unlock in progress.
            home.other_phase = UnlockPhase::Checks(1);
            send(
                &mut home,
                &cache,
                HomeMessage::PickOtherCubeWallet("destination".into(), WalletKind::Vault),
            );
            assert_eq!(home.other_phase, UnlockPhase::Checks(1));
            assert!(home.other_scope.is_some());

            // An unknown Cube unlocks nothing.
            send(
                &mut home,
                &cache,
                HomeMessage::PickOtherCubeWallet("missing".into(), WalletKind::Vault),
            );
            assert_eq!(home.transfer_page, TransferPage::Wallets);
            assert!(home.other_scope.is_none());
        }

        /// A Vault → another Cube's Vault PSBT that finished after the page
        /// was left can't be signed: the Cube is locked, so signing starts
        /// its unlock over instead.
        #[test]
        fn a_vault_to_vault_psbt_finished_after_leaving_cannot_be_signed() {
            let (mut home, _wallet) = home_on_transfer_screen();
            let cache = transfer_cache();
            home.transfer_from = Some(WalletKind::Vault);
            home.transfer_to = None;
            home.transfer_direction = Some(TransferDirection::VaultToOtherCube);
            home.destination_ready = Some((
                home.transfer_request_generation,
                TransferDirection::VaultToOtherCube,
            ));
            home.other_cube_selected = Some("destination".into());
            home.other_phase = UnlockPhase::Checks(UNLOCK_CHECKS);
            home.other_address = Some(checked_other_address());
            home.is_sending = true;
            // Left mid-preparation: locked now, the preparation then lands.
            home.interrupt();
            home.is_sending = false;
            assert!(home.destination_is_ready(), "the address is still in place");
            let task = home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::SignVaultToLiquidTx)),
            );
            drain(task);
            assert_eq!(home.current_view.step, 1, "back to Details to unlock again");
            assert_eq!(home.transfer_page, TransferPage::Wallets);
            assert!(!home.transfer_signed);
        }

        /// A broadcast already in flight when the page is left still
        /// completes: Sent, and the Cube reported locked again.
        #[test]
        fn a_broadcast_in_flight_when_leaving_still_completes() {
            use crate::app::breez_spark::SparkClient;
            use crate::chain::ChainId;

            let (_directory, root) = receiving_cube_catalog(true);
            let (client, _requests) = SparkClient::mock_transport();
            let cache = Cache {
                datadir_path: root.clone(),
                ..Cache::default()
            };
            let mut home = GlobalHome::new_without_wallet(
                None,
                Some(Arc::new(SparkBackend::new(Arc::new(client)))),
                root.clone(),
                ChainId::Bitcoin,
                "source".into(),
                false,
            );
            send(
                &mut home,
                &cache,
                HomeMessage::SparkBalanceUpdated {
                    btc: Amount::from_sat(200_000),
                    usdb_balance: None,
                    identity_pubkey: "source-spark".into(),
                },
            );
            send(&mut home, &cache, HomeMessage::NextStep);
            send(
                &mut home,
                &cache,
                HomeMessage::PickOtherCubeWallet("destination".into(), WalletKind::Vault),
            );
            assert!(home.other_scope.is_some());
            home.current_view.step = 2;
            home.is_sending = true;
            home.interrupt();
            assert!(home.other_scope.is_none());
            send(
                &mut home,
                &cache,
                HomeMessage::TransferBroadcast {
                    amount_sat: 50_000,
                    destination_kind: WalletKind::Vault,
                    swap_id: None,
                },
            );
            assert!(!home.is_sending);
            assert_eq!(home.current_view.step, 3, "Sent");
            assert!(home.other_locked_again, "Sent says it's locked again");
        }

        /// Vault → another Cube (its Vault or its Spark), left mid-broadcast:
        /// a failed broadcast can't be confirmed again without a fresh unlock,
        /// and one already in flight still completes to Sent.
        #[test]
        fn a_vault_broadcast_after_leaving_fails_closed_or_completes() {
            for direction in [
                TransferDirection::VaultToOtherCube,
                TransferDirection::VaultToOtherSpark,
            ] {
                let setup = || {
                    let (mut home, _wallet) = home_on_transfer_screen();
                    home.transfer_from = Some(WalletKind::Vault);
                    home.transfer_to = None;
                    home.transfer_direction = Some(direction);
                    home.destination_ready = Some((home.transfer_request_generation, direction));
                    home.transfer_signed = true;
                    home.other_cube_selected = Some("destination".into());
                    home.other_phase = UnlockPhase::Checks(UNLOCK_CHECKS);
                    home.other_address = Some(checked_other_address());
                    home.is_sending = true;
                    home.interrupt();
                    home
                };
                let cache = transfer_cache();

                // The broadcast fails: Confirm restarts the unlock, sends nothing.
                let mut failed = setup();
                send(
                    &mut failed,
                    &cache,
                    HomeMessage::Error(TransferError::PreparationExpired),
                );
                assert!(!failed.is_sending);
                send(&mut failed, &cache, HomeMessage::ConfirmTransfer);
                assert_eq!(failed.current_view.step, 1, "{:?}", direction);
                assert_eq!(failed.transfer_page, TransferPage::Wallets);
                assert!(!failed.is_sending, "{:?}: nothing sent again", direction);

                // The broadcast was already out: it completes to Sent, through
                // the callback each route really uses.
                let mut done = setup();
                send(&mut done, &cache, success_callback(direction));
                assert!(!done.is_sending);
                assert_eq!(done.current_view.step, 3, "{:?}", direction);
                assert!(done.other_scope.is_none() && done.other_address.is_none());
            }
        }

        /// The message a successful Vault → another Cube broadcast delivers:
        /// to its Spark, the deposit is reported like a Spark-bound transfer.
        fn success_callback(direction: TransferDirection) -> HomeMessage {
            match direction {
                TransferDirection::VaultToOtherSpark => HomeMessage::TransferBroadcast {
                    amount_sat: 100_000,
                    destination_kind: WalletKind::Spark,
                    swap_id: None,
                },
                _ => HomeMessage::TransferSuccessful,
            }
        }

        /// From a real held Vault-source unlock, for another Cube's Vault and
        /// its Spark: a valid unlock passes the review check; leaving
        /// mid-broadcast revokes it, so a failed broadcast restarts the
        /// unlock, while one already out completes to Sent.
        #[test]
        fn a_held_vault_source_unlock_through_leaving_and_broadcast() {
            for (with_vault, direction) in [
                (true, TransferDirection::VaultToOtherCube),
                (false, TransferDirection::VaultToOtherSpark),
            ] {
                let (_directory, root) = receiving_cube_catalog(with_vault);
                let cache = Cache {
                    datadir_path: root,
                    has_vault: true,
                    ..Cache::default()
                };
                let daemon = Arc::new(RecordingDaemon::default());
                let as_daemon: Arc<dyn Daemon + Sync + Send> = daemon.clone();
                // The Sign handler with a daemon. Its task isn't run: the
                // PSBT request it dispatches stays in flight.
                let sign = |home: &mut GlobalHome| {
                    let _in_flight = home.update(
                        Some(as_daemon.clone()),
                        &cache,
                        Message::View(view::Message::Home(HomeMessage::SignVaultToLiquidTx)),
                    );
                };
                // Review with a PSBT being built under the held unlock.
                let building = || {
                    let (mut home, wallet) = home_on_transfer_screen();
                    send(&mut home, &cache, HomeMessage::BackToHome);
                    send(&mut home, &cache, HomeMessage::NextStep);
                    send(&mut home, &cache, HomeMessage::ContinueFromWallets);
                    assert_eq!(home.transfer_direction, Some(direction));
                    send(&mut home, &cache, HomeMessage::ContinueWithCube);
                    assert!(home.other_scope.is_some(), "{:?}: a held unlock", direction);
                    home.other_address = Some(checked_other_address());
                    home.other_phase = UnlockPhase::Checks(UNLOCK_CHECKS);
                    send(&mut home, &cache, HomeMessage::ContinueFromUnlock);
                    let task = home.update(
                        None,
                        &cache,
                        Message::View(view::Message::Home(HomeMessage::NextStep)),
                    );
                    for message in collect(task) {
                        drain(home.update(None, &cache, message));
                    }
                    assert!(home.destination_is_ready(), "{:?}", direction);
                    // Control: the valid unlock passes the review check.
                    assert!(
                        home.check_other_review_address(&cache).is_ok(),
                        "{:?}: a valid unlock signs and broadcasts",
                        direction
                    );
                    // Control through the handler: with the valid unlock,
                    // signing dispatches the PSBT request to the daemon.
                    home.transfer_feerate = form::Value {
                        value: "2".into(),
                        warning: None,
                        valid: true,
                    };
                    home.transfer_unit = BitcoinDisplayUnit::Sats;
                    home.entered_amount = form::Value {
                        value: "100000".into(),
                        warning: None,
                        valid: true,
                    };
                    sign(&mut home);
                    assert!(
                        home.active_psbt_request.is_some() && home.is_sending,
                        "{:?}: a valid unlock reaches the daemon",
                        direction
                    );
                    assert_eq!(home.current_view.step, 2);
                    (home, wallet)
                };
                let at_review = || {
                    let (mut home, _) = building();
                    // The user leaves while it builds.
                    home.interrupt();
                    assert!(home.other_scope.is_none(), "{:?}", direction);
                    assert!(home.check_other_review_address(&cache).is_err());
                    home
                };

                // Control, staying on the page: with the valid unlock, a
                // landing PSBT and Confirm both get past the unlock check
                // (the transfer stays on Review; nothing restarts). The test
                // PSBT is empty, so analysis then stops it with an error
                // instead of opening the signer; Confirm has no signed
                // transaction to send. Locked, the same handlers restart the
                // unlock (below).
                let (mut valid, wallet) = building();
                let request_id = valid.active_psbt_request.expect("a PSBT request");
                let generation = valid.transfer_request_generation;
                let psbt =
                    Psbt::from_unsigned_tx(coincube_core::miniscript::bitcoin::Transaction {
                        version: coincube_core::miniscript::bitcoin::transaction::Version::TWO,
                        lock_time: coincube_core::miniscript::bitcoin::absolute::LockTime::ZERO,
                        input: vec![],
                        output: vec![],
                    })
                    .unwrap();
                let messages = collect(valid.update(
                    Some(as_daemon.clone()),
                    &cache,
                    Message::View(view::Message::Home(HomeMessage::TransferPsbtReady {
                        request_id,
                        generation,
                        direction,
                        destination: MAINNET_ADDR.to_string(),
                        amount_sat: 100_000,
                        feerate_vb: 2,
                        result: Ok((
                            psbt,
                            Some(wallet),
                            (cache.datadir_path.clone(), cache.network),
                        )),
                    })),
                ));
                assert_eq!(valid.current_view.step, 2, "{:?}: no restart", direction);
                assert!(
                    valid.other_scope.is_some(),
                    "{:?}: still unlocked",
                    direction
                );
                assert!(
                    messages.iter().any(|message| matches!(
                        message,
                        Message::View(view::Message::ShowError(error))
                            if error.contains("signature info")
                    )),
                    "{:?}: past the unlock check, into PSBT analysis",
                    direction
                );
                valid.transfer_psbt_ready_for =
                    Some((valid.next_psbt_request_id, generation, direction));
                valid.transfer_signed = true;
                send(&mut valid, &cache, HomeMessage::ConfirmTransfer);
                assert_eq!(
                    valid.current_view.step, 2,
                    "{:?}: Confirm keeps Review",
                    direction
                );
                assert!(valid.other_scope.is_some(), "{:?}", direction);

                let mut failed = at_review();
                send(
                    &mut failed,
                    &cache,
                    HomeMessage::Error(TransferError::PreparationExpired),
                );
                // Signing again can't reach the daemon: the unlock restarts.
                failed.active_psbt_request = None;
                sign(&mut failed);
                assert!(failed.active_psbt_request.is_none(), "{:?}", direction);
                assert_eq!(failed.current_view.step, 1, "{:?}", direction);
                assert!(!failed.is_sending, "{:?}: nothing sent again", direction);

                // Nor can a broadcast of an earlier signed transaction.
                let mut signed = at_review();
                send(
                    &mut signed,
                    &cache,
                    HomeMessage::Error(TransferError::PreparationExpired),
                );
                signed.transfer_signed = true;
                send(&mut signed, &cache, HomeMessage::ConfirmTransfer);
                assert_eq!(signed.current_view.step, 1, "{:?}", direction);
                assert!(!signed.is_sending, "{:?}: nothing broadcast", direction);

                let mut done = at_review();
                send(&mut done, &cache, success_callback(direction));
                assert_eq!(done.current_view.step, 3, "{:?}: Sent", direction);
                assert!(done.other_locked_again, "{:?}", direction);
            }
        }

        /// A PSBT for another Cube's Vault or Spark that lands after the page
        /// was left opens no signer: the Cube is locked, so its unlock starts
        /// over on Details.
        #[test]
        fn a_late_other_cube_psbt_opens_no_signer() {
            for direction in [
                TransferDirection::VaultToOtherCube,
                TransferDirection::VaultToOtherSpark,
            ] {
                let (mut home, wallet) = home_on_transfer_screen();
                let cache = transfer_cache();
                home.transfer_from = Some(WalletKind::Vault);
                home.transfer_to = None;
                home.transfer_direction = Some(direction);
                let generation = home.transfer_request_generation;
                home.destination_ready = Some((generation, direction));
                home.transfer_feerate.value = "2".into();
                home.other_cube_selected = Some("destination".into());
                home.other_phase = UnlockPhase::Checks(UNLOCK_CHECKS);
                home.other_address = Some(checked_other_address());
                // Preparing the PSBT, the user leaves by the side menu.
                home.is_sending = true;
                home.active_psbt_request = Some(7);
                home.interrupt();
                let psbt =
                    Psbt::from_unsigned_tx(coincube_core::miniscript::bitcoin::Transaction {
                        version: coincube_core::miniscript::bitcoin::transaction::Version::TWO,
                        lock_time: coincube_core::miniscript::bitcoin::absolute::LockTime::ZERO,
                        input: vec![],
                        output: vec![],
                    })
                    .unwrap();
                drain(home.update(
                    None,
                    &cache,
                    Message::View(view::Message::Home(HomeMessage::TransferPsbtReady {
                        request_id: 7,
                        generation,
                        direction,
                        destination: MAINNET_ADDR.to_string(),
                        amount_sat: 100_000,
                        feerate_vb: 2,
                        result: Ok((
                            psbt,
                            Some(wallet),
                            (cache.datadir_path.clone(), cache.network),
                        )),
                    })),
                ));
                assert!(!home.is_sending, "{:?}", direction);
                assert!(
                    matches!(home.modal, Modal::None),
                    "{:?}: no signer",
                    direction
                );
                assert!(home.transfer_spend_tx.is_none());
                assert_eq!(home.current_view.step, 1, "{:?}", direction);
                assert_eq!(home.transfer_page, TransferPage::Wallets);
            }
        }

        /// The side menu leaving Transfer revokes a real scoped unlock, with
        /// or without a send or PSBT preparation in flight.
        #[test]
        fn leaving_transfer_revokes_a_held_unlock() {
            use crate::app::breez_spark::SparkClient;
            use crate::chain::ChainId;

            let (_directory, root) = receiving_cube_catalog(true);
            let (client, _requests) = SparkClient::mock_transport();
            let cache = Cache {
                datadir_path: root.clone(),
                ..Cache::default()
            };
            for sending in [false, true] {
                let mut home = GlobalHome::new_without_wallet(
                    None,
                    Some(Arc::new(SparkBackend::new(Arc::new(client.clone())))),
                    root.clone(),
                    ChainId::Bitcoin,
                    "source".into(),
                    false,
                );
                send(
                    &mut home,
                    &cache,
                    HomeMessage::SparkBalanceUpdated {
                        btc: Amount::from_sat(200_000),
                        usdb_balance: None,
                        identity_pubkey: "source-spark".into(),
                    },
                );
                send(&mut home, &cache, HomeMessage::NextStep);
                send(
                    &mut home,
                    &cache,
                    HomeMessage::PickOtherCubeWallet("destination".into(), WalletKind::Vault),
                );
                assert!(home.other_scope.is_some(), "the pick holds a scoped unlock");
                home.is_sending = sending;
                home.interrupt();
                assert!(home.other_scope.is_none(), "sending: {}", sending);
                assert!(home.other_address.is_none());
                if sending {
                    assert!(home.other_locked_again, "Done says it locked again");
                    // The in-flight work fails: still locked, and Confirm
                    // can't send again without a fresh unlock.
                    send(
                        &mut home,
                        &cache,
                        HomeMessage::Error(TransferError::PreparationExpired),
                    );
                    assert!(home.other_scope.is_none());
                    home.current_view.step = 2;
                    home.receive_address_info = Some(ReceiveAddressInfo {
                        address: Address::from_str(MAINNET_ADDR).unwrap().assume_checked(),
                        index: ChildNumber::from(3),
                        labels: HashMap::new(),
                    });
                    home.destination_ready = Some((
                        home.transfer_request_generation,
                        TransferDirection::SparkToOtherCube,
                    ));
                    send(&mut home, &cache, HomeMessage::ConfirmSparkSend);
                    assert_eq!(home.current_view.step, 1, "the unlock starts over");
                    assert_eq!(home.transfer_page, TransferPage::Wallets);
                    assert!(!home.is_sending, "nothing was sent again");
                } else {
                    assert_eq!(home.transfer_page, TransferPage::Wallets);
                    assert!(home.other_cube_selected.is_none());
                }
            }
        }

        fn spark_only_cube_sends_to_another_cube(wallet: WalletKind) {
            use crate::app::breez_spark::SparkClient;
            use crate::chain::ChainId;
            use coincube_spark_protocol::{Method, OkPayload, PrepareSendOk, SendPaymentOk};
            use iced_runtime::futures::futures::StreamExt;

            let (_directory, root) = receiving_cube_catalog(true);
            let (client, mut requests) = SparkClient::mock_transport();
            let client = Arc::new(client);
            let cache = Cache {
                datadir_path: root.clone(),
                ..Cache::default()
            };
            let mut home = GlobalHome::new_without_wallet(
                None,
                Some(Arc::new(SparkBackend::new(client.clone()))),
                root.clone(),
                ChainId::Bitcoin,
                "source".into(),
                false,
            );
            send(
                &mut home,
                &cache,
                HomeMessage::SparkBalanceUpdated {
                    btc: Amount::from_sat(200_000),
                    usdb_balance: None,
                    identity_pubkey: "source-spark".into(),
                },
            );
            send(&mut home, &cache, HomeMessage::NextStep);
            assert_eq!(
                home.transfer_direction,
                Some(TransferDirection::SparkToOtherCube)
            );
            assert_eq!(home.transfer_to, None);
            send(&mut home, &cache, HomeMessage::ContinueFromWallets);
            assert_eq!(home.other_cube_selected.as_deref(), Some("destination"));
            send(
                &mut home,
                &cache,
                HomeMessage::SelectOtherCubeWallet(wallet),
            );
            assert_eq!(
                home.transfer_direction,
                TransferDirection::to_other_wallet(WalletKind::Spark, wallet, ChainId::Bitcoin)
            );
            send(&mut home, &cache, HomeMessage::ContinueWithCube);
            assert_eq!(home.transfer_page, TransferPage::Unlock);
            assert!(home.other_scope.is_some());

            // The restricted loader's PIN, daemon, and descriptor checks are
            // tested separately. Start here with its verified address result.
            let (address, method, artifact) = if wallet == WalletKind::Spark {
                (
                    "sp1test-address",
                    "SparkAddress",
                    OtherReceiveAddress::Spark("sp1test-address".into()),
                )
            } else {
                (MAINNET_ADDR, "BitcoinAddress", checked_other_address())
            };
            home.other_address = Some(artifact);
            home.other_phase = UnlockPhase::Checks(UNLOCK_CHECKS);
            send(&mut home, &cache, HomeMessage::ContinueFromUnlock);
            home.transfer_unit = BitcoinDisplayUnit::Sats;
            send(
                &mut home,
                &cache,
                HomeMessage::AmountEdited("100000".into()),
            );

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let mut run_with_reply = |task, reply| {
                let mut stream =
                    iced_runtime::task::into_stream(task).expect("a Spark request task");
                rt.block_on(async {
                    tokio::time::timeout(std::time::Duration::from_secs(5), async {
                        let (action, request) = tokio::join!(stream.next(), async {
                            let request = requests.recv().await.unwrap();
                            client.mock_reply(request.id, Some(reply)).await;
                            request
                        });
                        let Some(iced_runtime::Action::Output(message)) = action else {
                            panic!("missing Spark transfer result");
                        };
                        (message, request.method)
                    })
                    .await
                    .expect("the task should call the Spark bridge")
                })
            };
            let prepare = home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::NextStep)),
            );
            let (message, request) = run_with_reply(
                prepare,
                OkPayload::PrepareSend(PrepareSendOk {
                    handle: "prepared-for-archive".into(),
                    amount_sat: 100_000,
                    fee_sat: 800,
                    method: method.into(),
                    cross_chain: None,
                    has_token_leg: Some(false),
                }),
            );
            assert!(
                matches!(request, Method::PrepareSend(params) if params.input == address && params.amount_sat == Some(100_000))
            );
            drain(home.update(None, &cache, message));
            assert!(home.destination_is_ready());
            assert_eq!(home.spark_send_fee_sat, Some(800));
            if wallet == WalletKind::Spark {
                assert!(home.receive_address_info.is_none());
            } else {
                assert_eq!(
                    home.receive_address_info.as_ref().unwrap().index,
                    ChildNumber::from(3)
                );
            }
            send(&mut home, &cache, HomeMessage::NextStep);
            send(
                &mut home,
                &cache,
                HomeMessage::AmountEdited("125000".into()),
            );
            assert_eq!(home.current_view.step, 2, "only a send advances Review");
            assert_eq!(home.entered_amount.value, "100000");

            let confirm = home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::ConfirmSparkSend)),
            );
            let (message, method) = run_with_reply(
                confirm,
                OkPayload::SendPayment(SendPaymentOk {
                    payment_id: "archive-payment".into(),
                    amount_sat: 100_000,
                    fee_sat: 800,
                }),
            );
            assert!(
                matches!(method, Method::SendPayment(params) if params.prepare_handle == "prepared-for-archive")
            );
            drain(home.update(None, &cache, message));
            assert_eq!(home.current_view.step, 3);
            assert!(home.other_scope.is_none() && home.other_address.is_none());
            assert!(home.other_locked_again);
            assert_eq!(home.pending_vault_receive_sats, 0);
            assert!(home.pending_spark_incoming.is_none());
        }

        #[test]
        fn other_cube_pin_takes_four_digits_and_clears_on_leaving_unlock() {
            use crate::pin_input::Message as Pin;
            let (mut home, _wallet) = home_on_transfer_screen();
            let cache = transfer_cache();
            home.current_view.step = 1;
            home.transfer_direction = Some(TransferDirection::VaultToOtherCube);
            home.transfer_page = TransferPage::Unlock;

            for (i, digit) in ["1", "2", "3"].iter().enumerate() {
                send(
                    &mut home,
                    &cache,
                    HomeMessage::OtherCubePin(Pin::DigitChanged(i, digit.to_string())),
                );
            }
            assert!(!home.other_pin.is_complete());
            // Enter on three digits checks nothing.
            send(&mut home, &cache, HomeMessage::OtherCubePin(Pin::Submit));
            assert_eq!(home.other_phase, UnlockPhase::Credential { busy: false });
            send(
                &mut home,
                &cache,
                HomeMessage::OtherCubePin(Pin::DigitChanged(3, "4".into())),
            );
            assert_eq!(home.other_pin.value().as_str(), "1234");

            send(&mut home, &cache, HomeMessage::PreviousStep);
            assert_eq!(home.transfer_page, TransferPage::OtherCube);
            assert!(home.other_pin.value().is_empty());
        }

        #[test]
        fn vault_only_transfer_uses_the_other_cubes_spark_deposit_address() {
            let (_directory, root) = receiving_cube_catalog(false);
            let (mut home, _) = home_on_transfer_screen();
            let cache = Cache {
                datadir_path: root,
                has_vault: true,
                ..Cache::default()
            };
            send(&mut home, &cache, HomeMessage::BackToHome);
            send(&mut home, &cache, HomeMessage::NextStep);
            assert_eq!(
                home.transfer_direction,
                Some(TransferDirection::VaultToOtherCube)
            );
            send(&mut home, &cache, HomeMessage::ContinueFromWallets);
            assert_eq!(
                home.transfer_direction,
                Some(TransferDirection::VaultToOtherSpark)
            );
            send(&mut home, &cache, HomeMessage::ContinueWithCube);
            assert!(home.other_scope.is_some());

            home.other_address = Some(checked_other_address());
            home.other_phase = UnlockPhase::Checks(UNLOCK_CHECKS);
            send(&mut home, &cache, HomeMessage::ContinueFromUnlock);
            let task = home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::NextStep)),
            );
            for message in collect(task) {
                drain(home.update(None, &cache, message));
            }
            assert!(home.destination_is_ready());
            assert_eq!(
                home.receive_address_info
                    .as_ref()
                    .unwrap()
                    .address
                    .to_string(),
                MAINNET_ADDR
            );
            send(
                &mut home,
                &cache,
                HomeMessage::TransferBroadcast {
                    amount_sat: 100_000,
                    destination_kind: WalletKind::Spark,
                    swap_id: None,
                },
            );
            assert_eq!(home.current_view.step, 3);
            assert!(home.other_locked_again);
            assert!(home.pending_spark_incoming.is_none());
        }

        #[test]
        fn max_drains_spark_and_shows_what_arrives() {
            let (mut home, _wallet) = home_on_transfer_screen();
            let cache = transfer_cache();
            home.current_view.step = 1;
            home.transfer_page = TransferPage::Amount;
            home.transfer_direction = Some(TransferDirection::SparkToLiquid);
            home.transfer_unit = BitcoinDisplayUnit::Sats;
            home.spark_balance = Amount::from_sat(50_000);

            send(&mut home, &cache, HomeMessage::TransferMax);
            assert!(home.sends_max());
            assert_eq!(home.entered_amount.value, "50000");

            // Spark's fee is inside the amount; it comes off once.
            let generation = home.invalidate_transfer_destination();
            home.current_view.step = 2;
            for _ in 0..2 {
                send(
                    &mut home,
                    &cache,
                    HomeMessage::SparkPrepareSendReady {
                        generation,
                        direction: TransferDirection::SparkToLiquid,
                        destination: MAINNET_ADDR.to_string(),
                        prepare_handle: "max".to_string(),
                        fee_sat: 1_200,
                    },
                );
            }
            assert_eq!(home.entered_amount.value, "48800");

            // Back on Amount the whole balance shows again; typing ends Max.
            send(&mut home, &cache, HomeMessage::PreviousStep);
            assert_eq!(home.current_view.step, 1);
            assert_eq!(home.entered_amount.value, "50000");
            send(&mut home, &cache, HomeMessage::AmountEdited("1000".into()));
            assert!(!home.sends_max());
        }

        #[test]
        fn max_drains_liquid_to_the_quoted_receiver_amount() {
            let (mut home, _wallet) = home_on_transfer_screen();
            let cache = transfer_cache();
            home.current_view.step = 1;
            home.transfer_page = TransferPage::Amount;
            home.transfer_direction = Some(TransferDirection::LiquidToSpark);
            home.transfer_unit = BitcoinDisplayUnit::Sats;
            home.liquid_balance = Amount::from_sat(60_000);

            send(&mut home, &cache, HomeMessage::TransferMax);
            assert_eq!(home.entered_amount.value, "60000");
            let generation = home.invalidate_transfer_destination();
            home.current_view.step = 2;
            send(
                &mut home,
                &cache,
                HomeMessage::PrepareOnChainResponseReceived {
                    generation,
                    direction: TransferDirection::LiquidToSpark,
                    response: PreparePayOnchainResponse {
                        receiver_amount_sat: 58_500,
                        claim_fees_sat: 100,
                        total_fees_sat: 1_500,
                    },
                },
            );
            assert_eq!(home.entered_amount.value, "58500");
        }

        #[test]
        fn max_vault_sweep_waits_for_its_price_and_shows_what_arrives() {
            let (mut home, _wallet) = home_on_transfer_screen();
            let cache = transfer_cache();
            home.transfer_unit = BitcoinDisplayUnit::Sats;
            home.transfer_max = Some(WalletKind::Vault);
            assert!(!home.transfer_action_ready(), "no price, no confirm");

            let feerate_vb = home.transfer_feerate.value.trim().parse().unwrap();
            let generation = home.transfer_request_generation;
            send(
                &mut home,
                &cache,
                HomeMessage::TransferPsbtPreviewReady {
                    generation,
                    direction: TransferDirection::VaultToLiquid,
                    feerate_vb,
                    result: Ok((Amount::from_sat(500), Amount::from_sat(99_500))),
                },
            );
            assert_eq!(home.spend_tx_fees, Some(Amount::from_sat(500)));
            assert_eq!(home.entered_amount.value, "99500");
            assert!(home.transfer_action_ready());

            // Max taken for another source does nothing here.
            home.transfer_max = Some(WalletKind::Spark);
            assert!(!home.sends_max());
        }

        #[test]
        fn another_cube_route_steps_back_and_relocks_on_every_exit() {
            let (mut home, _wallet) = home_on_transfer_screen();
            let cache = transfer_cache();
            home.current_view.step = 1;
            home.transfer_page = TransferPage::Wallets;

            send(&mut home, &cache, HomeMessage::SelectAnotherCube);
            assert_eq!(
                home.transfer_direction,
                Some(TransferDirection::VaultToOtherCube)
            );
            assert_eq!(home.transfer_to, None);

            send(&mut home, &cache, HomeMessage::ContinueFromWallets);
            assert_eq!(home.transfer_page, TransferPage::OtherCube);
            assert!(home.other_cubes.is_empty());
            // Nothing listed, nothing to unlock.
            send(&mut home, &cache, HomeMessage::ContinueWithCube);
            assert_eq!(home.transfer_page, TransferPage::OtherCube);
            assert!(home.other_scope.is_none());

            // A finished unlock, then Back from Amount keeps it…
            home.transfer_page = TransferPage::Amount;
            home.other_phase = UnlockPhase::Checks(UNLOCK_CHECKS);
            home.other_address = Some(checked_other_address());
            send(&mut home, &cache, HomeMessage::PreviousStep);
            assert_eq!(home.transfer_page, TransferPage::Unlock);
            assert!(home.other_address.is_some());
            // …and leaving Unlock locks the other Cube again.
            send(&mut home, &cache, HomeMessage::PreviousStep);
            assert_eq!(home.transfer_page, TransferPage::OtherCube);
            assert!(home.other_address.is_none());
            assert_eq!(home.other_phase, UnlockPhase::default());
            send(&mut home, &cache, HomeMessage::PreviousStep);
            assert_eq!(home.transfer_page, TransferPage::Wallets);

            // Closing from any page resets the route.
            home.transfer_page = TransferPage::Amount;
            home.other_address = Some(checked_other_address());
            send(&mut home, &cache, HomeMessage::BackToHome);
            assert_eq!(home.current_view.step, 0);
            assert_eq!(home.transfer_page, TransferPage::Wallets);
            assert!(home.other_address.is_none() && home.other_scope.is_none());
        }

        /// `home_on_transfer_screen` with a Spark wallet beside the Vault.
        fn home_with_spark_and_vault() -> GlobalHome {
            let (mut home, _wallet) = home_on_transfer_screen();
            let (client, _requests) = crate::app::breez_spark::SparkClient::mock_transport();
            home.spark_backend = Some(Arc::new(SparkBackend::new(Arc::new(client))));
            home
        }

        /// `transfer_cache` once every balance the Transfer form caps
        /// against has loaded: the wallet pages' Transfer is ready.
        fn loaded(home: &mut GlobalHome) -> Cache {
            home.spark_balance_loaded = true;
            let mut cache = transfer_cache();
            cache.daemon_cache.blockheight = 1;
            cache
        }

        /// The view messages a task emits.
        fn emitted(task: Task<Message>) -> Vec<view::Message> {
            collect(task)
                .into_iter()
                .filter_map(|message| match message {
                    Message::View(message) => Some(message),
                    _ => None,
                })
                .collect()
        }

        #[test]
        fn a_wallet_page_transfer_starts_from_that_wallet() {
            for (from, direction) in [
                (WalletKind::Spark, TransferDirection::SparkToVault),
                (WalletKind::Vault, TransferDirection::VaultToSpark),
            ] {
                let mut home = home_with_spark_and_vault();
                let cache = loaded(&mut home);
                assert_eq!(
                    home.wallet_transfer_entry(&cache, from),
                    WalletTransferEntry::Ready
                );
                // A transfer left open on the Overview starts over.
                assert_eq!(home.current_view.step, 2);
                drain(home.start_transfer_from(None, &cache, from));
                assert_eq!(home.current_view.step, 1);
                assert_eq!(home.transfer_direction, Some(direction));
                assert_eq!(home.transfer_page, TransferPage::Amount);
                assert_eq!(home.transfer_origin, Some(from));
            }
        }

        #[test]
        fn leaving_a_wallet_page_transfer_returns_to_that_page() {
            use crate::app::menu::{SparkSubMenu, VaultSubMenu};
            for (from, page) in [
                (WalletKind::Spark, Menu::Spark(SparkSubMenu::Overview)),
                (WalletKind::Vault, Menu::Vault(VaultSubMenu::Overview)),
            ] {
                let mut home = home_with_spark_and_vault();
                let cache = loaded(&mut home);
                drain(home.start_transfer_from(None, &cache, from));
                let out = emitted(home.update(
                    None,
                    &cache,
                    Message::View(view::Message::Home(HomeMessage::LeaveTransfer)),
                ));
                assert!(
                    out.iter()
                        .any(|m| matches!(m, view::Message::Menu(menu) if *menu == page)),
                    "{:?}: {:?}",
                    from,
                    out
                );
                assert_eq!(home.current_view.step, 0);
                assert_eq!(home.transfer_origin, None);
                assert_eq!(transfer_origin_menu(from), &page);
            }

            // Opened from the Overview, leaving stays there.
            let mut home = home_with_spark_and_vault();
            let cache = loaded(&mut home);
            let out = emitted(home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::LeaveTransfer)),
            ));
            assert!(
                !out.iter().any(|m| matches!(m, view::Message::Menu(_))),
                "{:?}",
                out
            );
            assert_eq!(home.current_view.step, 0);
        }

        #[test]
        fn a_wallet_page_transfer_keeps_its_page_until_the_side_menu_leaves() {
            let mut home = home_with_spark_and_vault();
            let cache = loaded(&mut home);
            drain(home.start_transfer_from(None, &cache, WalletKind::Vault));

            // New transfer, from Done: the same source, still returning there.
            home.current_view.step = 3;
            send(&mut home, &cache, HomeMessage::RestartTransfer);
            assert_eq!(home.current_view.step, 1);
            assert_eq!(home.transfer_from, Some(WalletKind::Vault));
            assert_eq!(home.transfer_origin, Some(WalletKind::Vault));

            // Leaving through the side menu hands the open flow to the
            // Overview: its Back no longer returns to the Vault page.
            home.interrupt();
            assert_eq!(home.transfer_origin, None);
            assert_eq!(home.current_view.step, 1);

            // Activity, from Done, doesn't return to the page either.
            drain(home.start_transfer_from(None, &cache, WalletKind::Spark));
            let out = emitted(home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::OpenTransferActivity)),
            ));
            let menus: Vec<&Menu> = out
                .iter()
                .filter_map(|m| match m {
                    view::Message::Menu(menu) => Some(menu),
                    _ => None,
                })
                .collect();
            assert_eq!(
                menus,
                [&Menu::Cube(crate::app::menu::CubeSubMenu::Activity)],
                "{:?}",
                out
            );
            assert_eq!(home.transfer_origin, None);
        }

        #[test]
        fn a_wallet_page_transfer_waits_for_balances_and_in_flight_sends() {
            let mut home = home_with_spark_and_vault();
            // The Vault hasn't polled and Spark hasn't reported.
            let cache = transfer_cache();
            assert_eq!(
                home.wallet_transfer_entry(&cache, WalletKind::Spark),
                WalletTransferEntry::Loading
            );
            drain(home.start_transfer_from(None, &cache, WalletKind::Spark));
            assert_eq!(home.transfer_origin, None, "a disabled press opens nothing");

            let cache = loaded(&mut home);
            home.is_sending = true;
            assert_eq!(
                home.wallet_transfer_entry(&cache, WalletKind::Vault),
                WalletTransferEntry::Loading
            );
            let step = home.current_view.step;
            drain(home.start_transfer_from(None, &cache, WalletKind::Vault));
            assert_eq!(home.current_view.step, step, "the send in flight is kept");
        }

        #[test]
        fn a_lone_wallet_transfers_only_when_another_cube_can_receive() {
            // A Vault-only Cube: nowhere to send until another Cube exists.
            let (mut home, _wallet) = home_on_transfer_screen();
            let mut cache = loaded(&mut home);
            home.refresh_other_cube_receivers(&cache);
            assert_eq!(
                home.wallet_transfer_entry(&cache, WalletKind::Vault),
                WalletTransferEntry::NoDestination
            );

            let (_directory, root) = receiving_cube_catalog(true);
            cache.datadir_path = root;
            home.refresh_other_cube_receivers(&cache);
            assert_eq!(
                home.wallet_transfer_entry(&cache, WalletKind::Vault),
                WalletTransferEntry::Ready
            );
            drain(home.start_transfer_from(None, &cache, WalletKind::Vault));
            assert_eq!(
                home.transfer_direction,
                Some(TransferDirection::VaultToOtherCube)
            );
            assert_eq!(home.transfer_page, TransferPage::Wallets);
        }

        #[test]
        fn a_same_cube_pair_opens_on_its_amount() {
            let mut home = home_with_spark_and_vault();
            let cache = transfer_cache();
            home.current_view.step = 0;
            send(&mut home, &cache, HomeMessage::NextStep);
            assert_eq!(home.current_view.step, 1);
            assert!(home
                .transfer_direction
                .is_some_and(|direction| !direction.is_other_cube()));
            assert_eq!(home.transfer_page, TransferPage::Amount);
        }

        #[test]
        fn leaving_another_cube_from_either_menu_locks_it_and_keeps_a_pair() {
            let mut home = home_with_spark_and_vault();
            let cache = transfer_cache();
            let unlocked = |home: &mut GlobalHome| {
                home.current_view.step = 1;
                home.transfer_from = Some(WalletKind::Vault);
                home.transfer_to = None;
                home.transfer_direction = Some(TransferDirection::VaultToOtherCube);
                home.transfer_page = TransferPage::Amount;
                home.other_cube_selected = Some("destination".into());
                home.other_phase = UnlockPhase::Checks(UNLOCK_CHECKS);
                home.other_address = Some(checked_other_address());
                home.wallet_picker = Some(PickerSide::To);
            };

            // To: one of this Cube's wallets.
            unlocked(&mut home);
            send(
                &mut home,
                &cache,
                HomeMessage::SelectTransferWallet(PickerSide::To, WalletKind::Spark),
            );
            assert_eq!(
                home.transfer_direction,
                Some(TransferDirection::VaultToSpark)
            );
            assert_eq!(home.transfer_page, TransferPage::Amount);
            assert!(home.other_address.is_none() && home.other_cube_selected.is_none());
            assert_eq!(home.other_phase, UnlockPhase::default());
            assert_eq!(home.wallet_picker, None);

            // From: a new source; the old one becomes the destination.
            unlocked(&mut home);
            send(
                &mut home,
                &cache,
                HomeMessage::SelectTransferWallet(PickerSide::From, WalletKind::Spark),
            );
            assert_eq!(
                home.transfer_direction,
                Some(TransferDirection::SparkToVault)
            );
            assert_eq!(home.transfer_page, TransferPage::Amount);
            assert!(home.other_address.is_none() && home.other_cube_selected.is_none());

            // The current source again changes nothing.
            unlocked(&mut home);
            send(
                &mut home,
                &cache,
                HomeMessage::SelectTransferWallet(PickerSide::From, WalletKind::Vault),
            );
            assert_eq!(
                home.transfer_direction,
                Some(TransferDirection::VaultToOtherCube)
            );
            assert!(home.other_address.is_some());
            assert_eq!(home.wallet_picker, None);
        }

        #[test]
        fn leaving_transfer_by_the_menu_locks_the_other_cube() {
            let (mut home, _wallet) = home_on_transfer_screen();
            let unlocked = |home: &mut GlobalHome, step: usize| {
                home.current_view.step = step;
                home.transfer_from = Some(WalletKind::Vault);
                home.transfer_to = None;
                home.transfer_direction = Some(TransferDirection::VaultToOtherCube);
                home.transfer_page = TransferPage::Amount;
                home.other_cube_selected = Some("destination".into());
                home.other_phase = UnlockPhase::Checks(UNLOCK_CHECKS);
                home.other_address = Some(checked_other_address());
            };
            for step in [1, 2] {
                unlocked(&mut home, step);
                home.interrupt();
                assert!(home.other_address.is_none() && home.other_cube_selected.is_none());
                assert_eq!(home.other_phase, UnlockPhase::default());
                assert_eq!(home.current_view.step, 1, "from step {}", step);
                assert_eq!(home.transfer_page, TransferPage::Wallets);
            }
            // A send or PSBT preparation in flight keeps its step, but the
            // Cube locks now: whether it then fails or succeeds, nothing
            // reuses the unlock.
            unlocked(&mut home, 2);
            home.is_sending = true;
            home.interrupt();
            assert!(home.other_address.is_none() && home.other_scope.is_none());
            assert_eq!(home.other_phase, UnlockPhase::default());
            assert_eq!(home.current_view.step, 2);
            assert!(
                home.check_other_review_address(&transfer_cache()).is_err(),
                "a failed send can't confirm against the locked Cube"
            );
            // The send fails: the busy flag clears, the Cube stays locked.
            send(
                &mut home,
                &transfer_cache(),
                HomeMessage::Error(TransferError::PreparationExpired),
            );
            assert!(!home.is_sending);
            assert!(home.other_scope.is_none() && home.other_address.is_none());
        }

        #[test]
        fn prepare_and_another_cube_belong_to_their_pages() {
            let (mut home, _wallet) = home_on_transfer_screen();
            let cache = transfer_cache();
            home.current_view.step = 1;
            home.transfer_page = TransferPage::Wallets;
            let generation = home.transfer_request_generation;
            // Prepare is the Amount page's action only.
            send(&mut home, &cache, HomeMessage::NextStep);
            assert_eq!(home.current_view.step, 1);
            assert_eq!(home.transfer_request_generation, generation);

            // Liquid cannot send to another Cube from this dialog.
            home.transfer_from = Some(WalletKind::Liquid);
            home.transfer_to = Some(WalletKind::Vault);
            home.transfer_direction = Some(TransferDirection::LiquidToVault);
            send(&mut home, &cache, HomeMessage::SelectAnotherCube);
            assert_eq!(
                home.transfer_direction,
                Some(TransferDirection::LiquidToVault)
            );

            // The other Vault's checked address becomes the destination at
            // Prepare, under the request generation it started.
            home.transfer_from = Some(WalletKind::Vault);
            home.transfer_to = None;
            home.transfer_direction = Some(TransferDirection::VaultToOtherCube);
            home.transfer_page = TransferPage::Amount;
            home.other_address = Some(checked_other_address());
            let task = home.update(
                None,
                &cache,
                Message::View(view::Message::Home(HomeMessage::NextStep)),
            );
            assert_eq!(home.current_view.step, 2);
            for message in collect(task) {
                drain(home.update(None, &cache, message));
            }
            assert_eq!(
                home.receive_address_info
                    .as_ref()
                    .map(|info| info.address.to_string()),
                Some(MAINNET_ADDR.to_string())
            );
        }

        #[test]
        fn another_cube_broadcast_is_not_this_vaults_pending_deposit() {
            let (mut home, _wallet) = home_on_transfer_screen();
            let cache = transfer_cache();
            home.transfer_direction = Some(TransferDirection::VaultToOtherCube);
            home.transfer_to = None;
            home.other_address = Some(checked_other_address());
            // The Vault broadcast reports a Vault destination as success.
            send(&mut home, &cache, HomeMessage::TransferSuccessful);
            assert_eq!(home.current_view.step, 3);
            assert_eq!(home.pending_vault_receive_sats, 0);
            assert!(home.other_address.is_none(), "the other Cube locked again");
        }

        #[test]
        fn closing_the_review_qr_keeps_a_signed_transfer_signed() {
            let (mut home, _wallet) = home_on_transfer_screen();
            let cache = transfer_cache();
            home.transfer_signed = true;
            home.transfer_psbt_ready_for = Some((1, 0, TransferDirection::VaultToLiquid));
            home.modal = Modal::ShowQrCode(
                ShowQrCodeModal::new(
                    &Address::from_str(MAINNET_ADDR).unwrap().assume_checked(),
                    ChildNumber::from(0),
                )
                .unwrap(),
            );
            drain(home.update(None, &cache, Message::View(view::Message::Close)));
            assert!(matches!(home.modal, Modal::None));
            assert!(home.transfer_signed, "Confirm & Broadcast stays available");
            assert!(home.transfer_psbt_ready_for.is_some());
        }

        /// Tenshu v175 hides Liquid, so a Liquid → Vault transfer persisted by
        /// an earlier session shows no "Pending deposit" on Overview, even for
        /// an account with the grant and a Liquid wallet on disk.
        #[test]
        fn a_restored_liquid_transfer_stays_off_overview_while_liquid_is_hidden() {
            use crate::app::view::shared::landing::capture;
            let wallet = Arc::new(Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap()));
            let network = coincube_core::miniscript::bitcoin::Network::Bitcoin;
            let home = || {
                GlobalHome::new(
                    wallet.clone(),
                    Arc::new(LiquidBackend::new(Arc::new(
                        crate::app::breez_liquid::BreezClient::disconnected(network),
                    ))),
                    None,
                    CoincubeDirectory::new(std::path::PathBuf::from("/nonexistent")),
                    crate::chain::ChainId::Bitcoin,
                    "cube".to_string(),
                    false,
                )
            };
            let gate = crate::app::features::LiquidGate {
                server_enabled: true,
                local_state_exists: true,
            };
            let cache = Cache {
                has_vault: true,
                liquid_gate: gate,
                daemon_cache: crate::app::cache::DaemonCache {
                    blockheight: 900_000,
                    ..Default::default()
                },
                ..Cache::default()
            };
            let menu = Menu::Cube(crate::app::menu::CubeSubMenu::Overview);
            let restored = || {
                Message::View(view::Message::Home(HomeMessage::PendingTransferRestored {
                    amount_sat: 250_000,
                    stage: TransferStage::SwappingLbtcToBtc,
                    swap_id: "synthetic-swap".to_string(),
                }))
            };
            let overview = |home: &GlobalHome| {
                capture::inspect(
                    "overview-restored-liquid-transfer",
                    home.view(&menu, &cache),
                    (1440, 900),
                    "dark",
                    coincube_ui::theme::Theme::dark(),
                )
            };

            let mut hidden = home();
            hidden.set_liquid_shown(gate.show());
            let _ = hidden.update(None, &cache, restored());
            assert!(hidden.pending_vault_incoming.is_none());
            assert!(!overview(&hidden).contains("Pending deposit"));

            // The same restore with Liquid shown puts the card on the Vault,
            // so its absence above is the gate's doing.
            let mut shown = home();
            shown.set_liquid_shown(true);
            let _ = shown.update(None, &cache, restored());
            assert!(overview(&shown).contains("Pending deposit"));
        }
    }
}

#[cfg(test)]
mod pending_spark_shift_tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::Network;

    fn file(ids: &[&str]) -> settings::Settings {
        settings::Settings {
            cubes: ids
                .iter()
                .map(|id| {
                    settings::CubeSettings::new_with_raw_id(
                        id.to_string(),
                        id.to_string(),
                        Network::Bitcoin,
                    )
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn storing_a_conversion_touches_only_its_own_cube() {
        let record = settings::PendingSparkShift::sample();
        let stored = with_pending_spark_shift(file(&["a", "b"]), "b", Some(record.clone()));
        assert_eq!(stored.cubes[0].pending_spark_shift, None);
        assert_eq!(stored.cubes[1].pending_spark_shift, Some(record));

        let cleared = with_pending_spark_shift(stored, "b", None);
        assert!(cleared
            .cubes
            .iter()
            .all(|c| c.pending_spark_shift.is_none()));
    }

    /// A Cube missing from the file (removed since) must leave the file as it
    /// was: an updater that gave up would delete every Cube on the network.
    #[test]
    fn storing_for_a_missing_cube_keeps_the_file() {
        let stored = with_pending_spark_shift(
            file(&["a"]),
            "gone",
            Some(settings::PendingSparkShift::sample()),
        );
        assert_eq!(stored.cubes.len(), 1);
        assert_eq!(stored.cubes[0].pending_spark_shift, None);
    }
}
