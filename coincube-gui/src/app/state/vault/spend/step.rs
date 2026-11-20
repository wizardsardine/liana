use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    convert::TryInto,
    iter::FromIterator,
    str::FromStr,
    sync::{
        atomic::{AtomicU64, Ordering as AtomicOrdering},
        Arc,
    },
};

use crate::services::branta::{self, LookupRequest, LookupResult, LookupTicket};
use coincube_core::wallet_descriptor::SpendAnalysis;
use coincube_core::{
    miniscript::bitcoin::{
        address,
        bip32::{DerivationPath, Fingerprint},
        psbt::Psbt,
        secp256k1, Address, Amount, Denomination, Network, OutPoint, ScriptBuf,
    },
    spend::{SpendCreationError, DUST_OUTPUT_SATS, MAX_FEERATE},
};
use coincubed::commands::ListCoinsEntry;
use iced::{Subscription, Task};

static PREPARATION_ID: AtomicU64 = AtomicU64::new(1);
static FEE_ESTIMATE_ID: AtomicU64 = AtomicU64::new(1);
/// Redraft generations are drawn from one process-wide counter, not one per
/// `DefineSpend`: task results go to whichever panel is on screen, so a draft
/// started in one spend form (Send, Recovery, or a Send panel since replaced)
/// must never carry a generation another form could accept.
static REDRAFT_SEQ: AtomicU64 = AtomicU64::new(1);

fn next_redraft_seq() -> u64 {
    REDRAFT_SEQ.fetch_add(1, AtomicOrdering::Relaxed)
}
const TAMPER_WARNING: &str = "This payment request’s address does not match its verification data. Do not send. Return and replace the payment request.";
const AMOUNT_MISMATCH_WARNING: &str = "The prepared amount does not match the Bitcoin payment request. Return and resolve the amount before sending.";

use coincube_ui::{
    component::{
        amount::{format_btc_string, BitcoinDisplayUnit},
        form,
    },
    widget::Element,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        message::Message,
        state::vault::psbt,
        view::{self, vault::fiat::FiatAmount, CreateSpendMessage},
        wallet::{SyncStatus, Wallet},
    },
    daemon::{
        model::{
            coin_is_owned, next_recovery_sequence, remaining_sequence, Coin, CreateSpendResult,
            SpendTx,
        },
        Daemon,
    },
    services::feeestimation::fee_estimation::FeeEstimator,
};

/// How long to wait after the last form edit before running a redraft
/// (coin selection / `create_spend_tx`). Coalesces a burst of keystrokes
/// into a single backend call instead of one per character.
const REDRAFT_DEBOUNCE_MS: u64 = 350;
/// Block target of the Normal (~1h) fee preset, the default for a new or
/// cleared form.
pub(crate) const NORMAL_FEE_TARGET: usize = 6;

#[derive(Clone)]
pub struct TransactionDraft {
    network: Network,
    inputs: Vec<Coin>,
    recipients: Vec<Recipient>,
    generated: Option<(Psbt, Vec<String>)>,
    recipient_identities: Option<psbt::RecipientIdentities>,
    batch_label: Option<String>,
    labels: HashMap<String, String>,
    /// The timelock of the recovery path to use for spending.
    ///
    /// If the primary path will be used, this value will remain as `None`.
    /// Otherwise, its value should always be set to a recovery path,
    /// which may change from one to another.
    recovery_timelock: Option<u16>,
}

impl TransactionDraft {
    pub fn new(network: Network, recovery_timelock: Option<u16>) -> Self {
        Self {
            network,
            inputs: Vec::new(),
            recipients: Vec::new(),
            generated: None,
            recipient_identities: None,
            batch_label: None,
            labels: HashMap::new(),
            recovery_timelock,
        }
    }

    #[cfg(feature = "mcp-transactions")]
    pub(super) fn from_mcp(native: crate::app::local_mcp::transactions::NativeDraft) -> Self {
        let mut draft = Self::new(native.chain.bitcoin_network(), None);
        draft.inputs = native.coins;
        draft.generated = Some((native.psbt, native.warnings));
        draft
    }

    pub fn is_recovery(&self) -> bool {
        self.recovery_timelock.is_some()
    }
}

pub trait Step {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message>;
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message>;
    fn apply(&self, _draft: &mut TransactionDraft) {}
    fn can_advance(&self) -> bool {
        true
    }
    fn interrupt(&mut self) {}
    /// The step is on screen again after `interrupt`: request whatever was
    /// in flight when it was left. Called only while the step is shown, as
    /// tasks' results go to whatever is on screen when they land.
    fn resume(&mut self) -> Task<Message> {
        Task::none()
    }
    /// Lets panel-level tests read the fee state of a panel's DefineSpend.
    #[cfg(test)]
    fn as_define_spend(&self) -> Option<&DefineSpend> {
        None
    }
    fn load(&mut self, _coins: &[Coin], _tip_height: i32, _draft: &TransactionDraft) {}
    fn reload_wallet(&mut self, _wallet: Arc<Wallet>) {}
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
}

/// Filter the coins that should be available for selection.
///
/// `selected` will be `None` if the coins are being filtered for the first time.
/// In that case, all suitable coins will be selected for a recovery spend,
/// while for a primary path spend, no coins will be selected.
fn filter_coins(
    coins: &[Coin],
    recovery_timelock: Option<u16>,
    tip_height: i32,
    selected: Option<HashSet<OutPoint>>,
) -> Vec<(Coin, bool)> {
    coins
        .iter()
        .filter_map(|c| {
            if c.spend_info.is_none() && !c.is_immature {
                if let Some(recovery_timelock) = recovery_timelock {
                    c.block_height
                        .filter(|bh| {
                            tip_height + 1 >= bh + <u16 as Into<i32>>::into(recovery_timelock)
                        })
                        .map(|_| {
                            (
                                c.clone(),
                                selected
                                    .as_ref()
                                    .map(|sel| sel.contains(&c.outpoint))
                                    .unwrap_or(true),
                            )
                        })
                } else {
                    Some((
                        c.clone(),
                        selected
                            .as_ref()
                            .map(|sel| sel.contains(&c.outpoint))
                            .unwrap_or(false),
                    ))
                }
            } else {
                None
            }
        })
        .collect()
}

pub struct DefineSpend {
    recipients: Vec<Recipient>,
    /// If set, this is the index of a recipient that should
    /// receive the max amount.
    send_max_to_recipient: Option<usize>,
    /// Will be `true` if coins for spend were manually selected by user.
    /// Otherwise, will be `false` (including for self-send & recovery).
    is_user_coin_selection: bool,
    /// Choose coins was pressed: the coin list is open even though the
    /// draft still picks the coins.
    coins_shown: bool,
    is_valid: bool,
    is_duplicate: bool,

    network: Network,
    wallet: Arc<Wallet>,
    /// Leave as `None` for a primary path spend. Otherwise, this is the timelock
    /// corresponding to the recovery path to use for the spend.
    ///
    /// For a recovery path spend, this value can change from one timelock to another, but
    /// it must never be set to `None`.
    recovery_timelock: Option<u16>,
    tip_height: u32,
    coins: Vec<(Coin, bool)>,
    coins_labels: HashMap<String, String>,
    batch_label: form::Value<String>,
    amount_left_to_select: Option<Amount>,
    feerate: form::Value<String>,
    loading_fee_estimate: Option<usize>,
    /// Identity of the preset fetch `loading_fee_estimate` waits for. An
    /// estimate carrying any other identity, or arriving once nothing is
    /// loading, was replaced by Custom, a typed rate or another preset.
    fee_estimate_request: u64,
    /// The preset whose estimate was on its way when the step was left
    /// (another menu entry, or Back): fetched again by `resume` once the
    /// step is shown again (the panel's `reload`, or Next/Previous).
    resume_fee_estimate: Option<usize>,
    /// A draft was pending when the step was left: drafted again by
    /// `resume` once the step is shown again.
    resume_redraft: bool,
    /// Block target (1/6/24) of the preset whose estimate currently fills the
    /// feerate field, so the matching Fast/Normal/Slow button can render as
    /// selected. Cleared when the user types a feerate manually.
    selected_feerate_target: Option<usize>,
    /// The Custom fee segment is chosen (W4): the field is shown, and the
    /// presets are not selected.
    custom_feerate: bool,
    /// The last rate typed into the field, restored when Custom is chosen
    /// again after a preset replaced it.
    custom_feerate_value: Option<String>,
    fee_amount: Option<Amount>,
    generated: Option<(Psbt, Vec<String>)>,
    generating: bool,
    warning: Option<Error>,
    /// Whether this is the first step of the spend creation.
    /// Required in order to know whether the user can navigate to a previous step.
    is_first_step: bool,
    balance: Amount,
    unconfirmed_balance: Amount,
    sync_status: SyncStatus,
    bitcoin_unit: BitcoinDisplayUnit,
    /// Generation token for debounced/async redrafts, drawn from the
    /// process-wide `REDRAFT_SEQ`. Replaced every time a redraft is scheduled
    /// so that stale debounce ticks and in-flight coin-selection results —
    /// this form's or another spend form's — are discarded.
    redraft_seq: u64,
    /// A redraft is scheduled or running and its result has not landed yet:
    /// the footer says it is calculating instead of asking for fields the
    /// user has already filled in.
    draft_pending: bool,
    preparation_id: u64,
    lookup_ticket: Option<LookupTicket>,
    recipient_identities: Option<psbt::RecipientIdentities>,
    tampered: bool,
    tampered_destinations: HashSet<ScriptBuf>,
}

#[allow(clippy::too_many_arguments)]
impl DefineSpend {
    pub fn new(
        network: Network,
        wallet: Arc<Wallet>,
        coins: &[Coin],
        tip_height: u32,
        recovery_timelock: Option<u16>,
        is_first_step: bool,
        balance: Amount,
        unconfirmed_balance: Amount,
        sync_status: SyncStatus,
        bitcoin_unit: BitcoinDisplayUnit,
    ) -> Self {
        let coins = filter_coins(
            coins,
            recovery_timelock,
            tip_height.try_into().expect("i32 by consensus"),
            None,
        );

        Self {
            network,
            wallet,
            recovery_timelock,
            tip_height,
            generated: None,
            generating: false,
            coins,
            coins_labels: HashMap::new(),
            batch_label: form::Value::default(),
            recipients: vec![Recipient::new(recovery_timelock.is_some())],
            // For recovery, send max to the (single) recipient.
            send_max_to_recipient: recovery_timelock.map(|_| 0),
            is_user_coin_selection: false,
            coins_shown: false,
            is_valid: false,
            is_duplicate: false,
            feerate: form::Value::default(),
            fee_amount: None,
            selected_feerate_target: None,
            custom_feerate: false,
            custom_feerate_value: None,
            amount_left_to_select: None,
            warning: None,
            is_first_step,
            loading_fee_estimate: None,
            fee_estimate_request: 0,
            resume_fee_estimate: None,
            resume_redraft: false,
            balance,
            unconfirmed_balance,
            sync_status,
            bitcoin_unit,
            redraft_seq: next_redraft_seq(),
            draft_pending: false,
            preparation_id: PREPARATION_ID.fetch_add(1, AtomicOrdering::Relaxed),
            lookup_ticket: None,
            recipient_identities: None,
            tampered: false,
            tampered_destinations: HashSet::new(),
        }
    }

    fn update_spend_form(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        msg: view::CreateSpendMessage,
    ) -> Task<Message> {
        // Opening the coin list changes nothing the draft depends on.
        if let view::CreateSpendMessage::ChooseCoins = msg {
            self.coins_shown = true;
            return Task::none();
        }
        // Automatic while already automatic: nothing to redraft.
        if matches!(msg, view::CreateSpendMessage::AutomaticCoins) && !self.coin_list_shown() {
            return Task::none();
        }
        if self.is_stale_fee_estimate(&msg) {
            return Task::none();
        }
        if !matches!(msg, view::CreateSpendMessage::Generate) {
            self.invalidate_preparation();
        }
        match msg {
            view::CreateSpendMessage::BatchLabelEdited(label) => {
                self.batch_label.valid = label.len() <= 100;
                self.batch_label.value = label;
            }
            view::CreateSpendMessage::Clear => {
                // `Self::new` draws a fresh redraft generation, so a draft or
                // debounce tick still in flight from before Clear can never
                // match a generation of the cleared form.
                *self = Self::new(
                    self.network,
                    self.wallet.clone(),
                    self.coins
                        .iter()
                        .map(|(c, _)| c.clone())
                        .collect::<Vec<ListCoinsEntry>>()
                        .as_slice(),
                    self.tip_height,
                    self.recovery_timelock,
                    self.is_first_step,
                    self.balance,
                    self.unconfirmed_balance,
                    self.sync_status.clone(),
                    self.bitcoin_unit,
                );
                // A cleared form starts on Normal, as a new one does.
                return Task::done(Message::View(view::Message::CreateSpend(
                    CreateSpendMessage::FetchFeeEstimate(NORMAL_FEE_TARGET),
                )));
            }
            view::CreateSpendMessage::AutomaticCoins => {
                // Hand the choice back to automatic selection: the redraft
                // below picks the coins again.
                self.coins_shown = false;
                self.is_user_coin_selection = false;
                // The ticked coins were the user's: until the automatic draft
                // lands, Prepare must not build from them.
                self.amount_left_to_select = None;
            }
            view::CreateSpendMessage::AddRecipient => {
                self.recipients.push(Recipient::default());
            }
            view::CreateSpendMessage::DeleteRecipient(i) => self.delete_recipient(i),
            view::CreateSpendMessage::RecipientEdited(i, _, _)
            | view::CreateSpendMessage::RecipientFiatAmountEdited(i, _, _) => {
                self.recipients
                    .get_mut(i)
                    .unwrap()
                    .update(cache.network, self.bitcoin_unit, msg);
            }
            view::CreateSpendMessage::FeerateEdited(s) => {
                // A typed rate: no preset is selected, and a preset
                // fetch still in flight is dropped when it lands.
                self.set_feerate(s);
                self.custom_feerate_value = Some(self.feerate.value.clone());
                self.selected_feerate_target = None;
                self.loading_fee_estimate = None;
                // A feerate change must re-run coin selection: it
                // changes the fee and therefore `amount_left_to_select`
                // (which gates "Next"). Falling through to the
                // debounced redraft recomputes `fee_amount` too, so we
                // no longer clear it here or skip the redraft — doing
                // so left the form stuck on "Enter recipient address
                // and amount" whenever the feerate was edited last.
            }
            view::CreateSpendMessage::SessionError(error) => {
                let err: Error = error.into();
                let err_msg = crate::user_error::report(&err);
                self.warning = Some(err);
                return Task::done(Message::View(view::Message::ShowError(err_msg)));
            }
            view::CreateSpendMessage::CustomFeerate => {
                self.custom_feerate = true;
                self.selected_feerate_target = None;
                // A preset fetch still in flight would replace the
                // custom rate when it lands: drop it.
                let dropped_fetch = self.loading_fee_estimate.take().is_some();
                if let Some(value) = self
                    .custom_feerate_value
                    .clone()
                    .filter(|value| *value != self.feerate.value)
                {
                    // Applied here rather than sent as a message: a message
                    // lands on whatever is on screen, and would be lost if
                    // the user left Send before it was handled. It drafts
                    // the restored rate as a typed one does.
                    return self.update_spend_form(
                        daemon,
                        cache,
                        CreateSpendMessage::FeerateEdited(value),
                    );
                }
                // The dropped fetch had set the draft aside: the rate in the
                // field stays, so draft it again.
                if dropped_fetch {
                    return self.schedule_redraft();
                }
                return Task::none();
            }
            view::CreateSpendMessage::FeeEstimated { feerate, .. } => {
                // The pending preset's estimate (stale ones returned
                // above): it fills the field and stays selected.
                self.selected_feerate_target = self.loading_fee_estimate.take();
                // A draft that landed while the estimate was on its way was
                // built at the previous rate: Prepare waits for the redraft.
                self.amount_left_to_select = None;
                match feerate {
                    Some(fee) => {
                        self.set_feerate(fee.to_string());
                        // Falls through to the redraft, as a typed
                        // rate does.
                    }
                    None => {
                        // Fall back to the rate already in the field (now
                        // shown, as no preset fills it) and draft it again,
                        // so the form is not left waiting on a draft that
                        // never comes. The failure is reported by the error
                        // alone: a `warning` would end `drafting()` and let
                        // Prepare through before that draft lands (and the
                        // draft clears it anyway).
                        self.selected_feerate_target = None;
                        // As a landed estimate does (`set_feerate`), drop an
                        // earlier warning too, so `drafting()` holds Prepare.
                        self.warning = None;
                        let err: Error = SpendCreationError::FeeEstimationFailed.into();
                        let err_msg = crate::user_error::report(&err);
                        let redraft = self.schedule_redraft();
                        return Task::batch([
                            Task::done(Message::View(view::Message::ShowError(err_msg))),
                            redraft,
                        ]);
                    }
                }
            }
            view::CreateSpendMessage::FetchFeeEstimate(block_target) => {
                return self.fetch_fee_estimate(block_target);
            }
            view::CreateSpendMessage::Generate => return self.prepare_spend(daemon),
            view::CreateSpendMessage::SelectCoin(i) => {
                if let Some(coin) = self.coins.get_mut(i) {
                    coin.1 = !coin.1;
                    // Once user edits selection, auto-selection can no longer be used.
                    self.is_user_coin_selection = true;
                }
            }
            view::CreateSpendMessage::SendMaxToRecipient(i) => self.toggle_send_max(i),
            _ => {}
        }

        // Re-run coin selection after every form edit (as the
        // pre-refactor code did unconditionally). Coin selection /
        // `create_spend_tx` is async and potentially a network
        // round-trip, so we schedule it debounced and off the UI
        // thread rather than blocking on every keystroke.
        self.tampered = self
            .recipients
            .iter()
            .filter_map(|recipient| destination_binding(&recipient.address.value))
            .any(|destination| self.tampered_destinations.contains(&destination));
        if !self.tampered
            && self
                .warning
                .as_ref()
                .is_some_and(|warning| warning.to_string().contains(TAMPER_WARNING))
        {
            self.warning = None;
        }
        let redraft_task = self.schedule_redraft();
        self.check_valid();
        redraft_task
    }

    fn prepare_spend(&mut self, daemon: Arc<dyn Daemon + Sync + Send>) -> Task<Message> {
        // A preset's estimate is on its way (the rate in the field is about
        // to be replaced), or the draft at the current rate has not landed:
        // the same holds as the view's Prepare.
        if self.generating
            || self.loading_fee_estimate.is_some()
            || self.drafting()
            || self.tampered
            || !self.form_values_are_valid(false)
            || self.exists_duplicate()
        {
            return Task::none();
        }
        let inputs: Vec<OutPoint> = self
            .coins
            .iter()
            .filter_map(|(coin, selected)| selected.then_some(coin.outpoint))
            .collect();
        let outputs: HashMap<_, _> = self
            .recipients
            .iter()
            .map(|recipient| {
                (
                    recipient.destination().expect("validated destination"),
                    recipient.amount().expect("validated amount"),
                )
            })
            .collect();
        let feerate_vb = self.feerate.value.parse::<u64>().unwrap_or(0);
        self.warning = None;
        self.generating = true;
        self.preparation_id = PREPARATION_ID.fetch_add(1, AtomicOrdering::Relaxed);
        let generation = self.preparation_id;
        self.lookup_ticket = if self.recipients.is_empty() {
            None
        } else {
            branta::begin(self.network)
        };
        let ticket = self.lookup_ticket.clone();
        let recipients = self.recipients.clone();
        // A Vault or an imported wallet: both say which outputs are their own.
        let descriptor = self.wallet.main_descriptor.clone();
        let requests: Vec<_> = self
            .recipients
            .iter()
            .enumerate()
            .filter_map(|(row, recipient)| {
                let address = recipient
                    .destination()
                    .ok()?
                    .require_network(self.network)
                    .ok()?;
                LookupRequest::bitcoin(&recipient.address.value, &address)
                    .map(|request| (row, request))
            })
            .collect();
        let recovery = self.recovery_timelock.map(|timelock| {
            (
                timelock,
                self.recipients[0]
                    .destination()
                    .expect("validated recovery address"),
            )
        });
        Task::perform(
            async move {
                let prepare = async {
                    if let Some((timelock, address)) = recovery {
                        daemon
                            .create_recovery(address, &inputs, feerate_vb, Some(timelock))
                            .await
                            .map(|psbt| (psbt, vec![]))
                            .map_err(Error::from)
                    } else {
                        daemon
                            .create_spend_tx(&inputs, &outputs, feerate_vb, None)
                            .await
                            .map_err(Error::from)
                            .and_then(|result| match result {
                                CreateSpendResult::Success { psbt, warnings } => {
                                    Ok((psbt, warnings))
                                }
                                CreateSpendResult::InsufficientFunds { missing } => {
                                    Err(SpendCreationError::CoinSelection(
                                        coincube_core::spend::InsufficientFunds { missing },
                                    )
                                    .into())
                                }
                            })
                    }
                };
                let result = prepare.await.and_then(|(psbt, warnings)| {
                    validate_requested_amounts(&recipients, &psbt)?;
                    Ok((psbt, warnings))
                });
                let identities = if let (Ok((psbt, _)), Some(ticket)) = (&result, ticket) {
                    let Some(association) = associate_recipients(&recipients, psbt) else {
                        // Identity metadata is optional. A recovery/send-max output
                        // can legitimately differ from the earlier redraft snapshot.
                        return (result, Vec::new());
                    };
                    let owned: HashSet<_> = descriptor
                        .spend_change_indexes(psbt, &secp256k1::Secp256k1::verification_only())
                        .into_iter()
                        .collect();
                    let requests = requests
                        .into_iter()
                        .filter(|(row, _)| {
                            association
                                .get(*row)
                                .is_some_and(|output| !owned.contains(output))
                        })
                        .collect();
                    ticket.lookup_many(requests).await
                } else {
                    Vec::new()
                };
                (result, identities)
            },
            move |(result, identities)| Message::VaultSpendPrepared {
                generation,
                result,
                identities,
            },
        )
    }

    fn apply_prepared_spend(
        &mut self,
        generation: u64,
        result: Result<(Psbt, Vec<String>), Error>,
        identities: Vec<(usize, LookupResult)>,
    ) -> Task<Message> {
        if !self.generating || generation != self.preparation_id {
            return Task::none();
        }
        self.generating = false;
        let identities = if self
            .lookup_ticket
            .as_ref()
            .is_some_and(LookupTicket::is_current)
        {
            identities
        } else {
            Vec::new()
        };
        if identities
            .iter()
            .any(|(_, result)| matches!(result, LookupResult::Tampered))
        {
            self.record_tampered_destinations(&identities);
            return Task::none();
        }
        match result {
            Ok((psbt, warnings)) => {
                // Explicit BIP21 amounts are payment instructions, independent of
                // optional identity association and the privacy preference.
                if let Err(error) = validate_requested_amounts(&self.recipients, &psbt) {
                    self.warning = Some(error);
                    return Task::none();
                }
                let association = associate_recipients(&self.recipients, &psbt).unwrap_or_default();
                let bound = identities
                    .into_iter()
                    .filter_map(|(row, result)| association.get(row).map(|vout| (*vout, result)))
                    .collect();
                self.recipient_identities =
                    self.lookup_ticket
                        .clone()
                        .map(|ticket| psbt::RecipientIdentities {
                            ticket,
                            transaction_id: psbt.unsigned_tx.compute_txid(),
                            results: bound,
                        });
                self.generated = Some((psbt, warnings));
                Task::done(Message::View(view::Message::Next))
            }
            // Shown as the compose hint; the generic error toast would
            // hide why, and preparing again cannot change the outcome.
            Err(e) if is_amount_mismatch(&e) => {
                self.warning = Some(e);
                Task::none()
            }
            Err(e) => {
                let err_msg = crate::user_error::report(&e);
                self.warning = Some(e);
                Task::done(Message::View(view::Message::ShowError(err_msg)))
            }
        }
    }

    fn delete_recipient(&mut self, i: usize) {
        self.recipients.remove(i);
        if self.recipients.len() < 2 {
            self.batch_label.valid = true;
            self.batch_label.value = "".to_string();
        }
        if let Some(j) = self.send_max_to_recipient {
            match j.cmp(&i) {
                Ordering::Equal => {
                    self.send_max_to_recipient = None;
                }
                Ordering::Greater => {
                    self.send_max_to_recipient = Some(
                        j.checked_sub(1)
                            .expect("j must be greater than 0 in this case"),
                    );
                }
                _ => {}
            }
        }
    }

    fn toggle_send_max(&mut self, i: usize) {
        if let Some(recipient) = self.recipients.get_mut(i) {
            if self.send_max_to_recipient == Some(i) {
                // If already set to this recipient, then unset it.
                self.send_max_to_recipient = None;
                recipient.dust_warning = None;
                recipient.estimated_max = None;
            } else {
                // Either it's set to some other recipient or not at all.
                self.send_max_to_recipient = Some(i);
            }
        }
    }

    fn is_stale_fee_estimate(&self, message: &view::CreateSpendMessage) -> bool {
        match message {
            view::CreateSpendMessage::FeeEstimated { request, .. } => {
                self.loading_fee_estimate.is_none() || *request != self.fee_estimate_request
            }
            _ => false,
        }
    }

    fn record_tampered_destinations(&mut self, identities: &[(usize, LookupResult)]) {
        self.tampered = true;
        for (row, result) in identities {
            if matches!(result, LookupResult::Tampered) {
                if let Some(recipient) = self.recipients.get(*row) {
                    if let Some(destination) = destination_binding(&recipient.address.value) {
                        self.tampered_destinations.insert(destination);
                    }
                }
            }
        }
        self.generated = None;
        self.warning = Some(Error::Unexpected(TAMPER_WARNING.to_string()));
        self.check_valid();
    }

    /// Fetch the estimate for a preset's block target (1/6/24). The current
    /// draft was built at the rate this preset replaces: Prepare must not
    /// build from it. A draft still in flight is dropped too; `FeeEstimated`
    /// drafts again.
    fn fetch_fee_estimate(&mut self, block_target: usize) -> Task<Message> {
        self.custom_feerate = false;
        self.loading_fee_estimate = Some(block_target);
        self.fee_estimate_request = FEE_ESTIMATE_ID.fetch_add(1, AtomicOrdering::Relaxed);
        self.amount_left_to_select = None;
        self.redraft_seq = next_redraft_seq();
        self.draft_pending = false;
        let request = self.fee_estimate_request;
        Task::perform(
            async move {
                let fee_estimator = FeeEstimator::new();
                let feerate = match block_target {
                    1 => fee_estimator.get_high_priority_rate().await,
                    6 => fee_estimator.get_mid_priority_rate().await,
                    _ => fee_estimator.get_low_priority_rate().await,
                };

                feerate
            },
            move |feerate| {
                Message::View(view::Message::CreateSpend(
                    CreateSpendMessage::FeeEstimated {
                        request,
                        feerate: feerate.ok(),
                    },
                ))
            },
        )
    }

    fn invalidate_preparation(&mut self) {
        self.preparation_id = PREPARATION_ID.fetch_add(1, AtomicOrdering::Relaxed);
        self.generating = false;
        self.generated = None;
        self.lookup_ticket = None;
        self.recipient_identities = None;
    }

    pub fn with_preselected_coins(mut self, preselected_coins: &[OutPoint]) -> Self {
        for (coin, selected) in &mut self.coins {
            *selected = preselected_coins.contains(&coin.outpoint);
        }
        self
    }

    /// Treat the current selection as the user's own, so drafting spends
    /// exactly these coins instead of handing the choice to automatic coin
    /// selection. Coin control's Send selected uses this.
    pub fn with_user_coin_selection(mut self) -> Self {
        self.is_user_coin_selection = true;
        self
    }

    pub fn with_coins_sorted(mut self, blockheight: u32) -> Self {
        self.sort_coins(blockheight);
        self
    }

    /// Start the form on Normal (~1h, 6-block target): its estimate is
    /// fetched by `resume` once the step is on screen (the panel's Next or
    /// `reload`). A recovery spend uses this because it opens on the
    /// path-selection step, where the panel's `reload` default would be lost.
    pub fn starting_on_normal_feerate(mut self) -> Self {
        self.resume_fee_estimate = Some(NORMAL_FEE_TARGET);
        self
    }

    fn sort_coins(&mut self, blockheight: u32) {
        let timelock = self.timelock();
        self.coins.sort_by(|(a, a_selected), (b, b_selected)| {
            if *a_selected && !b_selected || !a_selected && *b_selected {
                b_selected.cmp(a_selected)
            } else if timelock.is_none_or(|timelock| {
                remaining_sequence(a, blockheight, timelock)
                    == remaining_sequence(b, blockheight, timelock)
            }) {
                // bigger amount first
                b.amount.cmp(&a.amount)
            } else {
                // smallest blockheight (remaining_sequence) first
                a.block_height.cmp(&b.block_height)
            }
        });
    }

    pub fn self_send(mut self) -> Self {
        self.recipients = Vec::new();
        self.is_user_coin_selection = true;
        self
    }

    /// This is used for calculating a coin's remaining sequence.
    ///
    /// Use the first timelock if this is a primary path spend and otherwise the same
    /// timelock as used for the recovery. `None` for an imported wallet, which
    /// has no recovery clock.
    pub fn timelock(&self) -> Option<u16> {
        self.recovery_timelock
            .or_else(|| self.wallet.recovery_timelock())
    }

    // If `is_redraft`, the validation of recipients will take into account
    // whether any should receive the max amount. Otherwise, all recipients
    // will be fully validated.
    fn form_values_are_valid(&self, is_redraft: bool) -> bool {
        self.feerate.valid
            && !self.feerate.value.is_empty()
            && (self.batch_label.valid || self.recipients.len() < 2)
            // Recipients will be empty for self-send.
            && self.recipients.iter().enumerate().all(|(i, r)|
            r.valid() || (is_redraft && self.send_max_to_recipient == Some(i) && r.address_valid()))
    }

    fn exists_duplicate(&self) -> bool {
        let mut destinations = HashSet::new();
        self.recipients
            .iter()
            .filter_map(|recipient| recipient.destination().ok())
            .any(|destination| !destinations.insert(destination))
    }

    fn check_valid(&mut self) {
        self.is_valid = !self.tampered
            && self.form_values_are_valid(false)
            && self.coins.iter().any(|(_, selected)| *selected);
        self.is_duplicate = self.exists_duplicate();
    }

    /// A warning that blocks Prepare whatever the draft's coin selection says:
    /// an authenticated Branta tamper result, or a prepared output that does
    /// not honour the BIP21 amount. A later successful redraft clears the
    /// latter with `warning`.
    fn blocking_warning(&self) -> Option<String> {
        if self.tampered {
            Some(TAMPER_WARNING.to_string())
        } else {
            self.warning
                .as_ref()
                .filter(|warning| is_amount_mismatch(warning))
                .map(|_| AMOUNT_MISMATCH_WARNING.to_string())
        }
    }
    /// Schedule a debounced redraft. Bumps the generation token and returns a
    /// task that, after a short delay, fires `Message::RedraftDebounce` with
    /// that token. If the user keeps typing, each keystroke bumps the token so
    /// only the final, settled edit actually runs coin selection.
    fn schedule_redraft(&mut self) -> Task<Message> {
        self.redraft_seq = next_redraft_seq();
        self.draft_pending = true;
        let seq = self.redraft_seq;
        Task::perform(
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(REDRAFT_DEBOUNCE_MS)).await;
                seq
            },
            Message::RedraftDebounce,
        )
    }

    /// run_redraft calculates the amount left to select and auto selects coins
    /// if the user did not select a coin manually.
    ///
    /// The actual coin selection / `create_spend_tx` is `async` and must NOT
    /// block the UI thread, so the heavy work is dispatched as a `Task` whose
    /// result is delivered via `Message::RedraftResult` (stamped with `seq`)
    /// and applied by `apply_redraft_result`. The cheap, synchronous form
    /// validation and "nothing to draft" early-returns are handled inline.
    fn run_redraft(&mut self, daemon: Arc<dyn Daemon + Sync + Send>, seq: u64) -> Task<Message> {
        if !self.form_values_are_valid(true) || self.exists_duplicate() {
            self.draft_pending = false;
            self.clear_invalid_redraft();
            return Task::none();
        }
        let is_self_transfer = self.recipients.is_empty();
        // Define the destinations for a primary path spend from all non-max recipients.
        // TODO: Set this variable only in the non-recovery case. For now, we use it later below
        // for setting `amount_left_to_select`, which is only required in the non-recovery case.
        let destinations: HashMap<Address<address::NetworkUnchecked>, u64> = self
            .recipients
            .iter()
            .enumerate()
            .filter_map(|(i, recipient)| {
                // A recipient that receives the max should be treated as change for coin selection.
                // Note that we only give a change output if its value is above the dust
                // threshold, but a user can only send payments above the same dust threshold,
                // so using change output to determine the max amount for a recipient will
                // not prevent a value that could otherwise be entered manually by the user.
                if self.send_max_to_recipient == Some(i) {
                    None
                } else {
                    Some((
                        recipient.destination().expect("Checked before"),
                        recipient.amount().expect("Checked before"),
                    ))
                }
            })
            .collect();

        // Index of the recipient (if any) that should receive the max. We carry
        // only the index through the async boundary and re-borrow on apply.
        let recipient_with_max = self.send_max_to_recipient;
        let Some(outpoints) = self.redraft_outpoints(is_self_transfer, recipient_with_max, || {
            destinations.values().sum()
        }) else {
            self.draft_pending = false;
            return Task::none();
        };
        self.draft_pending = true;

        // If sending the max to a recipient, use that recipient's address as the
        // change/recovery address.
        // Otherwise, for a primary path spend, use a fixed change address from the user's
        // own wallet so that we don't increment the change index.
        let max_address = if let Some(i) = recipient_with_max {
            self.recipients[i]
                .destination()
                .expect("Checked before")
                .as_unchecked()
                .clone()
        } else {
            self.wallet.preview_change_address()
        };

        let feerate_vb = self.feerate.value.parse::<u64>().expect("Checked before");
        let recovery_timelock = self.recovery_timelock;
        let destinations_is_empty = destinations.is_empty();
        // Clone for the result message; the original is moved into the future.
        let max_address_for_msg = max_address.clone();
        // Mixed send-max (a remainder recipient plus other payments) must not
        // put the remainder in the change slot: the daemon only accepts an
        // external change address when the destination list is empty (a sweep).
        let wallet_change = self.wallet.preview_change_address();
        let fold_remainder = recipient_with_max.is_some() && !destinations.is_empty();

        // Run coin selection / PSBT construction off the UI thread. The result
        // is delivered as `Message::RedraftResult` and applied (if still
        // current) by `apply_redraft_result` — never blocking on the UI thread.
        Task::perform(
            async move {
                // If recovery timelock is set, create a recovery transaction. Otherwise, a regular spend.
                if let Some(reco_tl) = recovery_timelock {
                    daemon
                        .create_recovery(max_address.clone(), &outpoints, feerate_vb, Some(reco_tl))
                        .await
                        // Map the PSBT to `CreateSpendResult`. We only need the PSBT below.
                        .map(|psbt| CreateSpendResult::Success {
                            psbt,
                            warnings: vec![],
                        })
                        .map_err(|e| e.into())
                } else if fold_remainder {
                    Self::fold_mixed_send_max(
                        daemon,
                        &outpoints,
                        destinations,
                        feerate_vb,
                        max_address,
                        wallet_change,
                    )
                    .await
                } else {
                    daemon
                        .create_spend_tx(
                            &outpoints,
                            &destinations,
                            feerate_vb,
                            Some(max_address.clone()),
                        )
                        .await
                        .map_err(|e| e.into())
                }
            },
            move |result| Message::RedraftResult {
                seq,
                max_address: max_address_for_msg.clone(),
                recipient_with_max,
                destinations_is_empty,
                result,
            },
        )
    }

    fn clear_invalid_redraft(&mut self) {
        // The current form details are not valid to draft a spend, so remove any previously
        // calculated amount as it will no longer be valid and could be misleading, e.g. if
        // the user removes the amount from one of the recipients.
        // We can leave any coins selected as they will either be automatically updated
        // as soon as the form is valid or the user has selected these specific coins and
        // so we should not touch them.
        self.amount_left_to_select = None;
        // Clear any error from a previous draft: the form is no longer in a
        // state we'd draft from, so a stale error must not be shown as the
        // disabled hint in place of "Enter recipient address and amount.".
        self.warning = None;
        // Remove any max amount from a recipient as it could be misleading.
        if let Some(i) = self.send_max_to_recipient {
            self.recipients
                .get_mut(i)
                .expect("max has been requested for this recipient so it must exist")
                .update(
                    self.network,
                    self.bitcoin_unit,
                    view::CreateSpendMessage::RecipientEdited(i, "amount", "".to_string()),
                );
        }
        self.fee_amount = None;
    }

    fn redraft_outpoints(
        &mut self,
        is_self_transfer: bool,
        recipient_with_max: Option<usize>,
        destination_total: impl FnOnce() -> u64,
    ) -> Option<Vec<OutPoint>> {
        let outpoints = if self.is_user_coin_selection
            || is_self_transfer
            || self.recovery_timelock.is_some()
        {
            // If user has edited selection, or otherwise for self-transfers and recovery spends, we pass the outpoints list.
            // (We could also in principle pass an empty outpoints list for a recovery spend if all should
            // be selected, i.e. not user edited, but this way we don't need to worry about checking selected
            // coins match those used in the recovery).
            let outpoints: Vec<_> = self
                .coins
                .iter()
                .filter_map(|(coin, selected)| selected.then_some(coin.outpoint))
                .collect();
            if outpoints.is_empty() {
                // If the user has deselected all coins, set any recipient's max amount to 0.
                if let Some(i) = recipient_with_max {
                    if let Some(recipient) = self.recipients.get_mut(i) {
                        recipient.update(
                            self.network,
                            self.bitcoin_unit,
                            view::CreateSpendMessage::RecipientEdited(i, "amount", "0".to_string()),
                        );
                    }
                }
                // Simply set the amount left to select as the total destination value. Note this
                // doesn't take account of the fee, but passing an empty list to `create_spend_tx`
                // would use auto-selection and so we settle for this approximation.
                // Note that for a recovery, the amount left to select is ignored by the view.
                self.amount_left_to_select = Some(Amount::from_sat(destination_total()));
                self.fee_amount = None;
                return None;
            }
            outpoints
        } else if self.send_max_to_recipient.is_some() {
            // If user has not selected coins, send the max available from all owned coins.
            self.coins
                .iter()
                .filter_map(|(c, _)| coin_is_owned(c).then_some(c.outpoint))
                .collect()
        } else {
            Vec::new() // pass empty list for auto-selection
        };
        Some(outpoints)
    }

    /// Turns a mixed send-max into explicit destination amounts.
    ///
    /// The first build uses the wallet's preview change address and reads the
    /// leftover. That leftover is then a normal payment to the max recipient, and
    /// the build is repeated so any fee change is added to the same recipient or
    /// stepped down by `feerate * 64` when the extra output no longer fits.
    async fn fold_mixed_send_max(
        daemon: Arc<dyn Daemon + Sync + Send>,
        outpoints: &[OutPoint],
        mut destinations: HashMap<Address<address::NetworkUnchecked>, u64>,
        feerate_vb: u64,
        recipient: Address<address::NetworkUnchecked>,
        wallet_change: Address<address::NetworkUnchecked>,
    ) -> Result<CreateSpendResult, Error> {
        let change_script = wallet_change.clone().assume_checked().script_pubkey();
        let mut drafted: Option<CreateSpendResult> = None;
        for _ in 0..4 {
            let result = daemon
                .create_spend_tx(
                    outpoints,
                    &destinations,
                    feerate_vb,
                    Some(wallet_change.clone()),
                )
                .await
                .map_err(crate::app::error::Error::from)?;
            match result {
                CreateSpendResult::InsufficientFunds { missing } => {
                    let current = destinations.get(&recipient).copied().unwrap_or(0);
                    let reduced = current.saturating_sub(feerate_vb.saturating_mul(64));
                    if current < DUST_OUTPUT_SATS
                        || reduced < DUST_OUTPUT_SATS
                        || reduced == current
                    {
                        if let Some(previous) = drafted {
                            return Ok(previous);
                        }
                        return Ok(CreateSpendResult::InsufficientFunds { missing });
                    }
                    destinations.insert(recipient.clone(), reduced);
                }
                CreateSpendResult::Success { psbt, warnings } => {
                    let change_sats = psbt
                        .unsigned_tx
                        .output
                        .iter()
                        .find(|output| output.script_pubkey == change_script)
                        .map(|output| output.value.to_sat())
                        .unwrap_or(0);
                    if change_sats == 0 {
                        return Ok(CreateSpendResult::Success { psbt, warnings });
                    }
                    let current = destinations.get(&recipient).copied().unwrap_or(0);
                    destinations.insert(recipient.clone(), current.saturating_add(change_sats));
                    drafted = Some(CreateSpendResult::Success { psbt, warnings });
                }
            }
        }
        daemon
            .create_spend_tx(outpoints, &destinations, feerate_vb, Some(wallet_change))
            .await
            .map_err(crate::app::error::Error::from)
    }

    /// Apply the result of an async redraft to the form state. Mirrors the
    /// logic that previously ran inline after the (blocking) `create_spend_tx`
    /// call. `recipient_with_max`, `max_address` and `destinations_is_empty`
    /// are snapshots from when the redraft was dispatched.
    fn apply_redraft_result(
        &mut self,
        max_address: Address<address::NetworkUnchecked>,
        recipient_with_max: Option<usize>,
        // No longer consulted: under-dust handling now derives everything from
        // the recipient count and coin-selection result, matching upstream #1928.
        _destinations_is_empty: bool,
        result: Result<CreateSpendResult, Error>,
    ) {
        // Drop any stale under-dust state before processing the fresh result.
        for recipient in &mut self.recipients {
            recipient.dust_warning = None;
            recipient.estimated_max = None;
        }
        match result {
            Ok(CreateSpendResult::Success { psbt, .. }) => {
                self.apply_successful_redraft(psbt, max_address, recipient_with_max)
            }
            Ok(CreateSpendResult::InsufficientFunds { missing }) => {
                self.apply_insufficient_redraft(missing, recipient_with_max)
            }
            Err(e) => {
                self.warning = Some(e);
                self.fee_amount = None;
            }
        }
    }

    fn apply_successful_redraft(
        &mut self,
        psbt: Psbt,
        max_address: Address<address::NetworkUnchecked>,
        recipient_with_max: Option<usize>,
    ) {
        self.warning = None;
        self.fee_amount = Some(psbt.fee().expect("Valid fees"));
        // Update selected coins for auto-selection (non-recovery case).
        if !self.is_user_coin_selection && self.recovery_timelock.is_none() {
            let selected_coins: Vec<OutPoint> = psbt
                .unsigned_tx
                .input
                .iter()
                .map(|c| c.previous_output)
                .collect();
            // Mark coins as selected.
            for (coin, selected) in &mut self.coins {
                *selected = selected_coins.contains(&coin.outpoint);
            }
        }
        // As coin selection was successful, we can assume there is nothing left to select.
        self.amount_left_to_select = Some(Amount::from_sat(0));
        if let Some(i) = recipient_with_max {
            // If there's no change output, any excess must be below the dust threshold
            // and so the max available for this recipient is 0.
            let amount_sat = psbt
                .unsigned_tx
                .output
                .iter()
                .find(|o| o.script_pubkey == max_address.clone().assume_checked().script_pubkey())
                .map(|change_output| change_output.value.to_sat())
                .unwrap_or(0);
            let amount = match self.bitcoin_unit {
                BitcoinDisplayUnit::BTC => Amount::from_sat(amount_sat).to_btc().to_string(),
                BitcoinDisplayUnit::Sats => amount_sat.to_string(),
            };
            if let Some(recipient) = self.recipients.get_mut(i) {
                recipient.update(
                    self.network,
                    self.bitcoin_unit,
                    view::CreateSpendMessage::RecipientEdited(i, "amount", amount),
                );
            }
        }
    }

    fn apply_insufficient_redraft(&mut self, missing: u64, recipient_with_max: Option<usize>) {
        self.fee_amount = None;
        // To be sure, we exclude recovery transactions here, although they
        // can't currently reach this part of code.
        if !self.is_user_coin_selection && self.recovery_timelock.is_none() {
            // The missing amount is based on all candidates for coin selection
            // being used, which are all owned coins.
            for (coin, selected) in &mut self.coins {
                *selected = coin_is_owned(coin);
            }
        }
        // Whether every available coin is already selected. Read the actual
        // selection state so this is correct for manual and recovery spends,
        // where the auto-selection branch above is skipped.
        let all_selected = self.coins.iter().all(|(_, selected)| *selected);
        if let Some(i) = recipient_with_max {
            // An output has MAX selected, but the available amount is lower than
            // the dust limit. Rather than showing a confusing near-zero amount,
            // surface an explicit under-dust message and clear the amount field.
            self.amount_left_to_select = None;
            let total_recipients = self.recipients.len();
            // Derive the threshold string from DUST_OUTPUT_SATS so the message
            // can never drift out of sync with the constant.
            let min_amount = format_btc_string(Amount::from_sat(DUST_OUTPUT_SATS).to_btc());
            self.apply_under_dust_warning(i, total_recipients, min_amount, all_selected, missing);
        } else {
            self.warning = None;
            self.amount_left_to_select = Some(Amount::from_sat(missing));
        }
    }

    fn apply_under_dust_warning(
        &mut self,
        i: usize,
        total_recipients: usize,
        min_amount: String,
        all_selected: bool,
        missing: u64,
    ) {
        if let Some(recipient) = self.recipients.get_mut(i) {
            recipient.dust_warning = Some(if all_selected {
                coincube_ui::localized_format!(
                    "Minimum amount is {min_amount} BTC. Add funds to your wallet to spend the coin(s).",
                    "El importe mínimo es {min_amount} BTC. Agrega fondos a tu cartera para gastar las monedas."
                )
            } else {
                coincube_ui::localized_format!(
                    "Minimum amount is {min_amount} BTC. Select more coins to continue.",
                    "El importe mínimo es {min_amount} BTC. Selecciona más monedas para continuar."
                )
            });
            recipient.update(
                self.network,
                self.bitcoin_unit,
                view::CreateSpendMessage::RecipientEdited(i, "amount", String::new()),
            );
            if total_recipients > 1 {
                recipient.estimated_max = None;
            } else if missing > DUST_OUTPUT_SATS {
                // NOTE: Not reachable in theory.
            } else {
                recipient.estimated_max = Some(Amount::from_sat(DUST_OUTPUT_SATS - missing));
            }
        }
    }
}

impl Step for DefineSpend {
    fn load(&mut self, coins: &[Coin], tip_height: i32, draft: &TransactionDraft) {
        self.tip_height = tip_height as u32;
        match (self.recovery_timelock, draft.recovery_timelock) {
            (Some(old_tl), Some(new_tl)) => {
                if old_tl != new_tl {
                    // If the timelock has changed, reinitialise this step.
                    // The fresh form starts on Normal, as a new recovery
                    // does: the step is about to be shown, so the estimate
                    // goes out with the message that brought it on screen.
                    let new = Self::new(
                        self.network,
                        self.wallet.clone(),
                        coins,
                        tip_height as u32,
                        Some(new_tl),
                        self.is_first_step,
                        self.balance,
                        self.unconfirmed_balance,
                        self.sync_status.clone(),
                        self.bitcoin_unit,
                    )
                    .starting_on_normal_feerate();
                    *self = new;
                    return;
                } else {
                    // If the timelock has not changed, we keep the existing coins selection if it has already been edited
                    // by the user or otherwise if the form values are valid. The form values being valid means a redraft has
                    // been performed and so we keep the currently selected coins in order that the recipient amount displayed
                    // matches the selection.
                    let selected = (self.is_user_coin_selection
                        || self.form_values_are_valid(true))
                    .then_some(
                        self.coins
                            .iter()
                            .filter_map(|(coin, sel)| sel.then_some(coin.outpoint))
                            .collect(),
                    );
                    self.coins = filter_coins(coins, self.recovery_timelock, tip_height, selected);
                }
            }
            _ => {
                // We don't handle the case of a recovery timelock being added or removed:
                // A spend is either primary or recovery and cannot change from one to the other.
            }
        }
        self.check_valid();
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::UpdatePanelCache(_) => {
                // Update sync_status from cache, same as VaultOverview does
                self.sync_status = crate::app::wallet::sync_status(
                    daemon.backend(),
                    cache.blockheight(),
                    cache.sync_progress(),
                    cache.last_poll_timestamp(),
                    cache.last_poll_at_startup,
                    cache.history_sync(),
                );

                // Recalculate balance from currency cache
                let (balance, unconfirmed_balance, _, _) = crate::app::state::coins_summary(
                    cache.coins(),
                    cache.blockheight().max(0) as u32,
                    self.wallet.recovery_timelock(),
                );
                self.balance = balance;
                self.unconfirmed_balance = unconfirmed_balance;
                self.bitcoin_unit = cache.bitcoin_unit;
            }
            Message::View(view::Message::Previous) => {
                self.invalidate_preparation();
                // Back from the review step: a draft that started while
                // preparing delivered its result to that step, where it was
                // dropped. Draft again rather than wait on it.
                if self.draft_pending {
                    return self.schedule_redraft();
                }
            }
            Message::View(view::Message::CreateSpend(msg)) => {
                return self.update_spend_form(daemon, cache, msg);
            }
            Message::VaultSpendPrepared { generation, result, identities } => {
                return self.apply_prepared_spend(generation, result, identities);
            }
            Message::Labels(res) => match res {
                Ok(labels) => {
                    self.coins_labels = labels;
                }
                Err(e) => {
                    let err_msg = crate::user_error::report(&e);
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },
            Message::CoinsTipHeight(res_coins, res_tip) => match (res_coins, res_tip) {
                (Ok(coins), Ok(tip)) => {
                    self.tip_height = tip as u32;
                    // If it's a recovery spend and user has never edited the selection, then we won't pass
                    // any selection and will let `filter_coins` select all by default.
                    let selected = (self.recovery_timelock.is_none()
                        || self.is_user_coin_selection)
                        .then_some(HashSet::from_iter(self.coins.iter().filter_map(
                            |(c, selected)| {
                                if *selected {
                                    Some(c.outpoint)
                                } else {
                                    None
                                }
                            },
                        )));
                    self.coins = filter_coins(&coins, self.recovery_timelock, tip, selected);
                    self.sort_coins(tip as u32);
                    // In case some selected coins are not spendable anymore and
                    // new coins make more sense to be selected. A redraft is triggered
                    // if all forms are valid (checked in the redraft method). This
                    // is not a keystroke path, so run it immediately (still off the
                    // UI thread) rather than debouncing.
                    self.redraft_seq = next_redraft_seq();
                    let seq = self.redraft_seq;
                    let task = self.run_redraft(daemon, seq);
                    self.check_valid();
                    return task;
                }
                (Err(e), _) | (Ok(_), Err(e)) => {
                    let err_msg = crate::user_error::report(&e);
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },
            Message::RedraftDebounce(seq) => {
                // Only the latest scheduled redraft survives; older ticks were
                // superseded by subsequent edits that bumped `redraft_seq`.
                if seq == self.redraft_seq {
                    return self.run_redraft(daemon, seq);
                }
            }
            Message::RedraftResult {
                seq,
                max_address,
                recipient_with_max,
                destinations_is_empty,
                result,
            }
                // Ignore a stale result the user has already typed past.
                if seq == self.redraft_seq && !self.generating => {
                    self.draft_pending = false;
                    self.apply_redraft_result(
                        max_address,
                        recipient_with_max,
                        destinations_is_empty,
                        result,
                    );
                    self.check_valid();
                }
            // Preparation took over while this draft ran: it is dropped, so
            // it is no longer pending.
            Message::RedraftResult { seq, .. } if seq == self.redraft_seq => {
                self.draft_pending = false;
            }
            _ => {}
        }
        Task::none()
    }

    fn can_advance(&self) -> bool {
        self.generated.is_some() && !self.tampered
    }

    fn interrupt(&mut self) {
        self.invalidate_preparation();
        // An estimate, debounce tick or draft in flight is delivered to
        // whatever is on screen when it lands, not to this step: `resume`
        // requests it again once the step is back. Until then the loading
        // estimate and pending draft stay set, so Prepare stays held.
        self.resume_fee_estimate = self.loading_fee_estimate;
        self.resume_redraft = self.draft_pending;
    }

    fn resume(&mut self) -> Task<Message> {
        let redraft = std::mem::take(&mut self.resume_redraft);
        match self.resume_fee_estimate.take() {
            // Resume an interrupted estimate or start the recovery default
            // now that the form is visible. Earlier replies are stale.
            Some(block_target) => self.fetch_fee_estimate(block_target),
            _ if redraft && self.draft_pending => self.schedule_redraft(),
            _ => Task::none(),
        }
    }

    #[cfg(test)]
    fn as_define_spend(&self) -> Option<&DefineSpend> {
        Some(self)
    }

    fn apply(&self, draft: &mut TransactionDraft) {
        draft.recipient_identities = self.recipient_identities.clone();
        draft.inputs = self
            .coins
            .iter()
            .filter_map(|(coin, selected)| if *selected { Some(coin) } else { None })
            .cloned()
            .collect();
        if let Some((psbt, _)) = &self.generated {
            draft.labels.clone_from(&self.coins_labels);
            for (i, output) in psbt.unsigned_tx.output.iter().enumerate() {
                if let Some(label) = self
                    .recipients
                    .iter()
                    .find(|recipient| {
                        !recipient.label.value.is_empty()
                            && recipient
                                .destination()
                                .unwrap()
                                .assume_checked()
                                .matches_script_pubkey(&output.script_pubkey)
                            && output.value.to_sat() == recipient.amount().unwrap()
                    })
                    .map(|recipient| recipient.label.value.to_string())
                {
                    draft.labels.insert(
                        OutPoint {
                            txid: psbt.unsigned_tx.compute_txid(),
                            vout: i as u32,
                        }
                        .to_string(),
                        label,
                    );
                }
            }
        }
        draft.recipients.clone_from(&self.recipients);
        if self.recipients.len() > 1 {
            draft.batch_label = Some(self.batch_label.value.clone());
        }
        draft.generated.clone_from(&self.generated);
    }

    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let converter: Option<view::FiatAmountConverter> =
            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
        let max_under_dust = self
            .recipients
            .iter()
            .any(|r| r.estimated_max.is_some() || r.dust_warning.is_some());
        // Mirror the view's "Insufficient funds." disabled-action hint: a
        // non-recovery draft with ≥1 coin selected that still has a non-zero
        // amount left to select can't be funded. (An unset/invalid feerate or
        // duplicate recipient forces `amount_left_to_select` to `None`, so
        // those states don't trip this.)
        let insufficient = self.recovery_timelock.is_none()
            && self.coins.iter().any(|(_, selected)| *selected)
            && self.amount_left_to_select.is_some_and(|a| a.to_sat() != 0);
        let blocking_warning = self.blocking_warning();
        view::vault::spend::create_spend_tx(
            &self.balance,
            &self.unconfirmed_balance,
            menu,
            cache,
            converter.as_ref(),
            self.recipients
                .iter()
                .enumerate()
                .map(|(i, recipient)| {
                    recipient
                        .view(
                            i,
                            self.send_max_to_recipient == Some(i),
                            converter.as_ref(),
                            self.bitcoin_unit,
                            insufficient,
                        )
                        .map(view::Message::CreateSpend)
                })
                .collect(),
            self.is_valid,
            self.is_duplicate,
            self.timelock(),
            self.recovery_timelock,
            &self.coins,
            &self.coins_labels,
            &self.batch_label,
            self.amount_left_to_select.as_ref(),
            if blocking_warning.is_some() {
                None
            } else {
                self.warning.as_ref().map(|e| e.to_string())
            },
            self.drafting(),
            blocking_warning,
            &self.feerate,
            self.fee_amount.as_ref(),
            &self.sync_status,
            self.is_first_step,
            self.loading_fee_estimate,
            self.selected_feerate_target,
            self.custom_feerate_shown(),
            self.threshold(),
            self.coin_list_shown(),
            self.generating,
            self.bitcoin_unit,
            max_under_dust,
        )
    }

    fn reload_wallet(&mut self, wallet: Arc<Wallet>) {
        self.wallet = wallet;
    }
}

impl DefineSpend {
    /// Whether the fee field shows: Custom chosen, or no preset filling it
    /// (a rate typed before any preset).
    fn set_feerate(&mut self, s: String) {
        if let Ok(value) = s.parse::<u64>() {
            self.feerate.value = s;
            self.feerate.valid = value != 0 && value <= MAX_FEERATE;
        } else if s.is_empty() {
            self.feerate.value = "".to_string();
            self.feerate.valid = true;
        } else {
            self.feerate.valid = false;
        }
        self.warning = None;
    }

    /// The form is complete and its draft has not landed yet.
    fn drafting(&self) -> bool {
        self.draft_pending
            && self.warning.is_none()
            && self.form_values_are_valid(true)
            && !self.exists_duplicate()
    }

    /// The coin list is open: Choose coins was pressed, the user already
    /// picked coins, or the spend has no automatic selection (a self-transfer
    /// or a recovery spends the coins chosen for it).
    pub(crate) fn coin_list_shown(&self) -> bool {
        self.coins_shown
            || self.is_user_coin_selection
            || self.recipients.is_empty()
            || self.recovery_timelock.is_some()
    }

    pub(crate) fn custom_feerate_shown(&self) -> bool {
        self.custom_feerate
            || (self.selected_feerate_target.is_none() && self.loading_fee_estimate.is_none())
    }

    /// The primary path's threshold and key count, for the Send footer.
    fn threshold(&self) -> Option<(usize, usize)> {
        let policy = self.wallet.policy()?;
        Some(match policy.primary_path() {
            coincube_core::descriptors::PathInfo::Single(_) => (1, 1),
            coincube_core::descriptors::PathInfo::Multi(threshold, keys) => {
                (*threshold, keys.len())
            }
        })
    }
}

/// A known mismatch remains blocked for this destination for the lifetime of
/// the form. Changing/removing verification fields or disabling checks must not
/// become a bypass. Bind only the Bitcoin script, so query decoding, duplicate
/// keys, address casing and URI presentation cannot change the latch. No raw
/// request or verification secrets are retained here.
fn destination_binding(raw: &str) -> Option<ScriptBuf> {
    parse_payment_request(raw)
        .ok()
        .map(|(address, _)| address.assume_checked().script_pubkey())
}

/// Preserve row identity even when equal addresses/amounts appear more than once.
/// Bind to the transaction actually shown and signed, independent of response order.
/// A missing association suppresses identity metadata; it never rejects a spend.
fn associate_recipients(recipients: &[Recipient], psbt: &Psbt) -> Option<Vec<usize>> {
    let mut used = HashSet::new();
    recipients
        .iter()
        .map(|recipient| {
            let address = recipient.destination().ok()?.assume_checked();
            let amount = recipient.amount().ok()?;
            let output = psbt
                .unsigned_tx
                .output
                .iter()
                .enumerate()
                .find(|(index, output)| {
                    !used.contains(index)
                        && output.value.to_sat() == amount
                        && address.matches_script_pubkey(&output.script_pubkey)
                })
                .map(|(index, _)| index)?;
            used.insert(output);
            Some(output)
        })
        .collect()
}

/// Unlike optional identity metadata, an explicit BIP21 amount constrains the
/// payment itself. Check the actual prepared outputs even when Branta is disabled
/// or a recovery/send-max amount changed after the form's earlier redraft.
fn is_amount_mismatch(error: &Error) -> bool {
    matches!(error, Error::Unexpected(message) if message == AMOUNT_MISMATCH_WARNING)
}

fn validate_requested_amounts(recipients: &[Recipient], psbt: &Psbt) -> Result<(), Error> {
    let mut used = HashSet::new();
    for recipient in recipients {
        let Ok((address, Some(amount))) = parse_payment_request(&recipient.address.value) else {
            continue;
        };
        let address = address.assume_checked();
        let output = psbt
            .unsigned_tx
            .output
            .iter()
            .enumerate()
            .find(|(index, output)| {
                !used.contains(index)
                    && output.value == amount
                    && address.matches_script_pubkey(&output.script_pubkey)
            })
            .map(|(index, _)| index)
            .ok_or_else(|| Error::Unexpected(AMOUNT_MISMATCH_WARNING.to_string()))?;
        used.insert(output);
    }
    Ok(())
}

/// Parse locally, retaining the original URI in the form for the SDK. Unknown
/// required BIP21 fields and ambiguous duplicate amounts fail closed.
fn parse_payment_request(
    raw: &str,
) -> Result<(Address<address::NetworkUnchecked>, Option<Amount>), Error> {
    let raw = raw.trim();
    if raw
        .get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("bitcoin:"))
    {
        let uri = reqwest::Url::parse(raw).map_err(|_| invalid_payment_request())?;
        if uri.host_str().is_some() || uri.fragment().is_some() {
            return Err(invalid_payment_request());
        }
        let address = Address::from_str(uri.path()).map_err(|_| invalid_payment_request())?;
        Ok((address, payment_request_amount(&uri)?))
    } else {
        Ok((
            Address::from_str(raw).map_err(|_| invalid_payment_request())?,
            None,
        ))
    }
}

fn invalid_payment_request() -> Error {
    Error::Unexpected("Invalid Bitcoin payment request".to_string())
}

fn payment_request_amount(uri: &reqwest::Url) -> Result<Option<Amount>, Error> {
    let mut amount = None;
    for (key, value) in uri.query_pairs() {
        if key.starts_with("req-") {
            return Err(invalid_payment_request());
        }
        if key == "amount" {
            if amount.is_some() {
                return Err(invalid_payment_request());
            }
            let parsed = Amount::from_str_in(&value, Denomination::Bitcoin)
                .map_err(|_| invalid_payment_request())?;
            if parsed == Amount::ZERO {
                return Err(invalid_payment_request());
            }
            amount = Some(parsed);
        }
    }
    Ok(amount)
}

#[derive(Default, Clone)]
struct Recipient {
    label: form::Value<String>,
    address: form::Value<String>,
    amount: form::Value<String>,
    bitcoin_unit: BitcoinDisplayUnit,
    // Estimated max spendable for this recipient when a send-max lands under the
    // dust limit; used to show a fiat estimate alongside the under-dust message.
    estimated_max: Option<Amount>,
    // This is only `Some` if the user has entered a fiat amount directly.
    fiat_amount: Option<form::Value<String>>,
    fiat_converter: Option<view::FiatAmountConverter>, // the converter at the time of entering the fiat amount
    is_recovery: bool,
    // Set when a send-max output would be below the dust limit.
    dust_warning: Option<String>,
}

impl Recipient {
    fn new(is_recovery: bool) -> Self {
        Self {
            is_recovery,
            ..Default::default()
        }
    }

    fn destination(&self) -> Result<Address<address::NetworkUnchecked>, Error> {
        parse_payment_request(&self.address.value).map(|(address, _)| address)
    }

    fn amount(&self) -> Result<u64, Error> {
        if self.amount.value.is_empty() {
            return Err(Error::Unexpected("Amount should be non-zero".to_string()));
        }

        let amount = match self.bitcoin_unit {
            BitcoinDisplayUnit::BTC => {
                Amount::from_str_in(&self.amount.value, Denomination::Bitcoin)
                    .map_err(|_| Error::Unexpected("cannot parse output amount".to_string()))?
            }
            BitcoinDisplayUnit::Sats => {
                let sats = self
                    .amount
                    .value
                    .replace(' ', "")
                    .parse::<u64>()
                    .map_err(|_| Error::Unexpected("cannot parse output amount".to_string()))?;
                Amount::from_sat(sats)
            }
        };

        if amount.to_sat() == 0 {
            return Err(Error::Unexpected("Amount should be non-zero".to_string()));
        }

        if amount.to_sat() < DUST_OUTPUT_SATS {
            return Err(Error::Unexpected("Amount should be non-zero".to_string()));
        }

        if let Ok(address) = self.destination() {
            if amount <= address.assume_checked().script_pubkey().minimal_non_dust() {
                return Err(Error::Unexpected(
                    "Amount must be superior to script dust value".to_string(),
                ));
            }
        }

        if let Ok((_, Some(requested))) = parse_payment_request(&self.address.value) {
            if requested != amount {
                return Err(Error::Unexpected(
                    "Amount must match the Bitcoin payment request".to_string(),
                ));
            }
        }
        Ok(amount.to_sat())
    }

    fn address_valid(&self) -> bool {
        !self.address.value.is_empty() && self.address.valid
    }

    fn valid(&self) -> bool {
        self.address_valid()
            && !self.amount.value.is_empty()
            && self.amount.valid
            && self.label.valid
    }

    fn update(
        &mut self,
        network: Network,
        bitcoin_unit: BitcoinDisplayUnit,
        message: view::CreateSpendMessage,
    ) {
        match message {
            view::CreateSpendMessage::RecipientEdited(_, "address", address) => {
                self.edit_address(network, bitcoin_unit, address)
            }
            view::CreateSpendMessage::RecipientFiatAmountEdited(_, fiat_amt_str, converter) => {
                return self.edit_fiat_amount(fiat_amt_str, converter)
            }
            view::CreateSpendMessage::RecipientEdited(_, "amount", amount) => {
                self.edit_bitcoin_amount(bitcoin_unit, amount)
            }
            view::CreateSpendMessage::RecipientEdited(_, "label", label) => {
                self.label.valid = label.len() <= 100;
                self.label.value = label;
            }
            _ => {}
        }
        self.enforce_payment_request_amount();
    }

    fn edit_address(
        &mut self,
        network: Network,
        bitcoin_unit: BitcoinDisplayUnit,
        address: String,
    ) {
        self.address.value = address;
        if self.amount.value.is_empty() {
            if let Ok((_, Some(amount))) = parse_payment_request(&self.address.value) {
                self.bitcoin_unit = bitcoin_unit;
                self.amount.value = match bitcoin_unit {
                    BitcoinDisplayUnit::BTC => amount.to_btc().to_string(),
                    BitcoinDisplayUnit::Sats => amount.to_sat().to_string(),
                };
            }
        }
        if let Ok(address) = self.destination() {
            self.address.valid = address.is_valid_for_network(network);
            if !self.amount.value.is_empty() {
                self.amount.valid = self.amount().is_ok();
            }
        } else if self.address.value.is_empty() {
            // Make the error disappear if we deleted the invalid address
            self.address.valid = true;
        } else {
            self.address.valid = false;
        }
    }

    fn edit_fiat_amount(&mut self, fiat_amt_str: String, converter: view::FiatAmountConverter) {
        // Clear any warning on the BTC amount as it is no longer the last edited field.
        self.amount.warning = None;
        self.fiat_converter = Some(converter);
        if fiat_amt_str.is_empty() {
            self.fiat_amount = Some(form::Value::default());
            self.amount = form::Value::default(); // reset the BTC amount to be consistent
            return;
        }
        // Don't allow more than currency's decimal places for fiat amounts.
        if fiat_amt_str
            .split_once(".")
            .filter(|(_, decimals)| {
                converter.currency().decimals() == 0
                    || decimals.len() > converter.currency().decimals()
            })
            .is_some()
        {
            return;
        }
        let Ok(fiat_amount) = FiatAmount::from_str_in(&fiat_amt_str, converter.currency()) else {
            // We don't save the invalid fiat amount.
            return;
        };
        match converter.convert_to_btc(&fiat_amount) {
            Ok(btc_amount) => {
                // Update both BTC and fiat amounts.
                self.amount.value = btc_amount.to_btc().to_string();
                self.bitcoin_unit = BitcoinDisplayUnit::BTC;
                self.amount.valid = self.amount().is_ok();
                self.fiat_amount = Some(form::Value {
                    value: fiat_amt_str,
                    valid: true,
                    warning: None,
                });
            }
            Err(e) => {
                // Probably the BTC amount is too large.
                tracing::debug!("Could not convert fiat to BTC: {e}");
                self.fiat_amount = Some(form::Value {
                    value: fiat_amt_str,
                    valid: false,
                    warning: Some(coincube_ui::locale::copy("Could not convert to BTC")),
                });
            }
        }
        self.enforce_payment_request_amount();
    }

    fn edit_bitcoin_amount(&mut self, bitcoin_unit: BitcoinDisplayUnit, amount: String) {
        self.bitcoin_unit = bitcoin_unit;
        self.fiat_amount = None; // Clear any fiat amount to indicate BTC amount is now primary.
        self.fiat_converter = None;
        self.amount.warning = None;

        // If a float has been pasted with more than 8 decimal places, and truncating
        // it to 8 places yields a valid BTC amount, truncate it and show a warning.
        // This can only happen on paste, as the input otherwise allows up to 8 decimal
        // places. Sats entry has no fractional part, so it is kept as-is.
        self.amount.value = match bitcoin_unit {
            BitcoinDisplayUnit::BTC => f64::from_str(&amount)
                .ok()
                .and_then(|_| amount.split_once('.'))
                .filter(|(_, fraction)| fraction.len() > 8)
                .map(|(integer, fraction)| format!("{}.{}", integer, &fraction[..8]))
                .filter(|truncated| Amount::from_str_in(truncated, Denomination::Bitcoin).is_ok())
                .inspect(|_| {
                    self.amount.warning = Some(coincube_ui::locale::copy(
                        "Amount has been truncated to 8 decimal places",
                    ));
                })
                .unwrap_or(amount),
            BitcoinDisplayUnit::Sats => amount,
        };

        if !self.amount.value.is_empty() {
            self.amount.valid = self.amount().is_ok();
        } else {
            // Make the error disappear if we deleted the invalid amount
            self.amount.valid = true;
        }
    }

    fn enforce_payment_request_amount(&mut self) {
        if let Ok((_, Some(_))) = parse_payment_request(&self.address.value) {
            if !self.amount.value.is_empty() && self.amount().is_err() {
                self.amount.valid = false;
                self.amount.warning = Some(coincube_ui::locale::copy(
                    "Amount must match the Bitcoin payment request",
                ));
            }
        }
    }

    fn view(
        &self,
        i: usize,
        is_max_selected: bool,
        fiat_converter: Option<&view::FiatAmountConverter>,
        bitcoin_unit: BitcoinDisplayUnit,
        insufficient: bool,
    ) -> Element<view::CreateSpendMessage> {
        let mut fiat_form_value = self.fiat_amount.as_ref();

        // If we have a fiat converter, check if it has changed since the last time we set
        // the fiat amount. If it has changed, we may need to update the fiat amount
        // depending on whether the currency or price has changed.
        if let (Some(current_conv), Some(prev_conv)) =
            (fiat_converter, self.fiat_converter.as_ref())
        {
            if current_conv.currency() != prev_conv.currency() {
                fiat_form_value = None; // convert any BTC amount to fiat in the new currency
            } else if current_conv.price_per_btc() != prev_conv.price_per_btc() {
                // If the fiat amount is not valid, keep it as is so the user can correct it.
                // Else, convert any BTC amount to fiat at the new price.
                if self.fiat_amount.as_ref().is_some_and(|fa| fa.valid) {
                    fiat_form_value = None;
                }
            }
        }
        view::vault::spend::recipient_view(
            i,
            &self.address,
            &self.amount,
            fiat_form_value,
            fiat_converter,
            &self.label,
            is_max_selected,
            self.is_recovery,
            &self.dust_warning,
            self.estimated_max,
            bitcoin_unit,
            insufficient,
        )
    }
}

pub struct SaveSpend {
    wallet: Arc<Wallet>,
    spend: Option<(psbt::PsbtState, Vec<String>)>,
    curve: secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    mcp_review: bool,
}

impl SaveSpend {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self {
            wallet,
            spend: None,
            curve: secp256k1::Secp256k1::verification_only(),
            mcp_review: false,
        }
    }
    #[cfg(feature = "mcp-transactions")]
    pub(crate) fn with_mcp_review(mut self) -> Self {
        self.mcp_review = true;
        self
    }
}

impl Step for SaveSpend {
    fn load(&mut self, _coins: &[Coin], _tip_height: i32, draft: &TransactionDraft) {
        let (psbt, warnings) = draft.generated.clone().unwrap();
        let mut tx = SpendTx::new(
            None,
            psbt,
            draft.inputs.clone(),
            &self.wallet.main_descriptor,
            &self.curve,
            draft.network,
        );
        tx.labels.clone_from(&draft.labels);

        if tx.is_batch() {
            if let Some(label) = &draft.batch_label {
                tx.labels.insert(
                    tx.psbt.unsigned_tx.compute_txid().to_string(),
                    label.clone(),
                );
            }
        } else if let Some(recipient) = draft.recipients.first() {
            if !recipient.label.value.is_empty() {
                let label = recipient.label.value.clone();
                tx.labels
                    .insert(tx.psbt.unsigned_tx.compute_txid().to_string(), label);
            }
        }

        self.spend = Some((
            psbt::PsbtState::new(self.wallet.clone(), tx, false)
                .with_recipient_identities(draft.recipient_identities.clone()),
            warnings,
        ));
    }

    fn reload_wallet(&mut self, wallet: Arc<Wallet>) {
        self.wallet = wallet;
    }

    fn interrupt(&mut self) {
        if let Some((psbt_state, _)) = &mut self.spend {
            psbt_state.interrupt()
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        if let Some((psbt_state, _)) = &self.spend {
            psbt_state.subscription()
        } else {
            Subscription::none()
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if self.mcp_review {
            match &message {
                #[cfg(feature = "mcp-transactions")]
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::ContinueNativeSigning,
                )) => {
                    self.mcp_review = false;
                    return Task::none();
                }
                Message::View(view::Message::Spend(_)) => return Task::none(),
                _ => {}
            }
        }
        if let Some((psbt_state, _)) = &mut self.spend {
            psbt_state.update(daemon, cache, message)
        } else {
            Task::none()
        }
    }

    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let (psbt_state, warnings) = self.spend.as_ref().unwrap();
        let Some(policy) = psbt_state.desc_policy.as_ref() else {
            return iced::widget::text(coincube_ui::locale::copy(
                "This wallet's spending policy can't be shown.",
            ))
            .into();
        };
        let content = view::vault::spend::spend_view(
            psbt_state.action_scope(),
            menu,
            cache,
            &psbt_state.tx,
            warnings,
            psbt_state.saved,
            policy,
            &psbt_state.wallet.keys_aliases,
            psbt_state.labels_edited.cache(),
            cache.network,
            if let Some(psbt::PsbtModal::Sign(m)) = &psbt_state.modal {
                m.is_signing()
            } else {
                false
            },
            cache.bitcoin_unit,
            psbt_state.recipient_identities(),
            psbt_state.replay_presentation(cache),
            psbt_state.broadcast_ready(cache),
            self.mcp_review,
        );
        if let Some(modal) = &psbt_state.modal {
            modal.as_ref().view(content)
        } else {
            content
        }
    }
}

pub struct SelectRecoveryPath {
    wallet: Arc<Wallet>,
    recovery_paths: Vec<RecoveryPath>,
    selected_path: Option<usize>,
    warning: Option<Error>,
}

impl SelectRecoveryPath {
    pub fn new(wallet: Arc<Wallet>, coins: &[Coin], tip_height: i32) -> Self {
        Self {
            recovery_paths: recovery_paths(&wallet, coins, tip_height),
            wallet,
            selected_path: None,
            warning: None,
        }
    }

    pub fn load_from_coins_and_tip_height(&mut self, coins: &[Coin], tip_height: i32) {
        self.warning = None;
        // Update the available recovery paths, maintaining any selected path.
        let selected_seq = self.selected_path.and_then(|selected| {
            self.recovery_paths
                .get(selected)
                .map(|reco_path| reco_path.sequence)
        });
        self.recovery_paths = recovery_paths(&self.wallet, coins, tip_height);
        self.selected_path = selected_seq.and_then(|seq| {
            self.recovery_paths
                .iter()
                .enumerate()
                .find_map(|(i, path)| (path.sequence == seq).then_some(i))
        });
    }
}

impl Step for SelectRecoveryPath {
    fn load(&mut self, coins: &[Coin], tip_height: i32, _draft: &TransactionDraft) {
        self.load_from_coins_and_tip_height(coins, tip_height);
    }

    fn reload_wallet(&mut self, wallet: Arc<Wallet>) {
        self.wallet = wallet;
    }

    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        view::vault::recovery::recovery(
            menu,
            cache,
            self.recovery_paths
                .iter()
                .enumerate()
                .filter_map(|(i, path)| {
                    if path.number_of_coins > 0 {
                        Some(view::vault::recovery::recovery_path_view(
                            i,
                            path.threshold,
                            &path.origins,
                            path.total_amount,
                            path.number_of_coins,
                            &self.wallet.keys_aliases,
                            self.selected_path == Some(i),
                        ))
                    } else {
                        None
                    }
                })
                .collect(),
            self.selected_path,
            self.recovery_paths
                .iter()
                .filter_map(|path| path.remaining_sequence)
                .filter(|sequence| *sequence > 1)
                .min(),
        )
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::CoinsTipHeight(res_coins, res_tip) => match (res_coins, res_tip) {
                (Ok(coins), Ok(tip)) => {
                    self.load_from_coins_and_tip_height(&coins, tip);
                }
                (Err(e), _) | (Ok(_), Err(e)) => {
                    let err_msg = crate::user_error::report(&e);
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },
            Message::View(view::Message::CreateSpend(view::CreateSpendMessage::SelectPath(
                index,
            ))) => {
                if Some(index) == self.selected_path {
                    self.selected_path = None;
                } else {
                    self.selected_path = Some(index);
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn apply(&self, draft: &mut TransactionDraft) {
        if let Some(selected_path) = self.selected_path {
            if let Some(path) = self.recovery_paths.get(selected_path) {
                draft.recovery_timelock = Some(path.sequence);
            }
        }
    }
}

pub struct RecoveryPath {
    threshold: usize,
    sequence: u16,
    origins: Vec<(Fingerprint, HashSet<DerivationPath>)>,
    total_amount: Amount,
    number_of_coins: usize,
    remaining_sequence: Option<u32>,
}

fn recovery_paths(wallet: &Wallet, coins: &[Coin], tip_height: i32) -> Vec<RecoveryPath> {
    let Some(descriptor) = wallet.inheritance_descriptor() else {
        return Vec::new();
    };
    descriptor
        .policy()
        .recovery_paths()
        .iter()
        .map(|(&sequence, path)| {
            let (number_of_coins, total_amount) = coins
                .iter()
                .filter(|coin| {
                    coin.block_height.is_some() // only confirmed coins are included in a recovery transaction
                        && coin.spend_info.is_none()
                        && remaining_sequence(coin, tip_height as u32, sequence) <= 1
                })
                .fold(
                    (0, Amount::from_sat(0)),
                    |(number_of_coins, total_amount), coin| {
                        (number_of_coins + 1, total_amount + coin.amount)
                    },
                );

            let (threshold, origins) = path.thresh_origins();
            RecoveryPath {
                remaining_sequence: next_recovery_sequence(
                    coins,
                    tip_height.max(0) as u32,
                    sequence,
                ),
                total_amount,
                number_of_coins,
                sequence,
                threshold,
                origins: origins.into_iter().collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::descriptors::CoincubeDescriptor;
    use coincube_core::miniscript::bitcoin::{bip32::ChildNumber, Txid};
    use std::str::FromStr;

    const DESC: &str = "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr";
    const MAINNET_ADDR: &str = "bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg";

    fn wallet() -> Arc<Wallet> {
        Arc::new(Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap()))
    }

    fn outpoint(n: u32) -> OutPoint {
        OutPoint::new(Txid::from_str(&format!("{n:064x}")).unwrap(), n)
    }

    fn address() -> Address {
        Address::from_str(MAINNET_ADDR).unwrap().assume_checked()
    }

    fn coin(n: u32, sats: u64, block_height: Option<i32>) -> Coin {
        Coin {
            amount: Amount::from_sat(sats),
            outpoint: outpoint(n),
            address: address(),
            block_height,
            derivation_index: ChildNumber::from(n),
            spend_info: None,
            is_immature: false,
            is_change: false,
            is_from_self: false,
        }
    }

    #[test]
    fn filter_coins_excludes_unspendable_and_sets_default_selection_by_path() {
        let mut spent = coin(2, 20_000, Some(90));
        spent.spend_info = Some(coincubed::commands::LCSpendInfo {
            txid: Txid::from_str(&format!("{:064x}", 99)).unwrap(),
            height: None,
        });
        let mut immature = coin(3, 30_000, Some(90));
        immature.is_immature = true;
        let unconfirmed = coin(4, 40_000, None);
        let mature = coin(1, 10_000, Some(90));
        let too_young = coin(5, 50_000, Some(96));
        let coins = vec![
            mature.clone(),
            spent,
            immature,
            unconfirmed.clone(),
            too_young.clone(),
        ];

        let primary = filter_coins(&coins, None, 100, None);
        assert_eq!(primary.len(), 3);
        assert!(primary.iter().all(|(_, selected)| !*selected));
        assert!(primary
            .iter()
            .any(|(coin, _)| coin.outpoint == unconfirmed.outpoint));

        let recovery = filter_coins(&coins, Some(10), 100, None);
        assert_eq!(recovery.len(), 1);
        assert_eq!(recovery[0].0.outpoint, mature.outpoint);
        assert!(recovery[0].1);

        let selected = HashSet::from_iter([too_young.outpoint]);
        let later_recovery = filter_coins(&coins, Some(10), 105, Some(selected));
        assert_eq!(later_recovery.len(), 2);
        assert_eq!(
            later_recovery
                .iter()
                .find(|(coin, _)| coin.outpoint == mature.outpoint)
                .map(|(_, selected)| *selected),
            Some(false)
        );
        assert_eq!(
            later_recovery
                .iter()
                .find(|(coin, _)| coin.outpoint == too_young.outpoint)
                .map(|(_, selected)| *selected),
            Some(true)
        );
    }

    #[test]
    fn define_spend_initializes_primary_recovery_and_self_send_modes() {
        let wallet = wallet();
        let coins = vec![coin(1, 10_000, Some(90)), coin(2, 20_000, Some(91))];

        let primary = DefineSpend::new(
            Network::Bitcoin,
            wallet.clone(),
            &coins,
            100,
            None,
            true,
            Amount::from_sat(30_000),
            Amount::ZERO,
            SyncStatus::Synced,
            BitcoinDisplayUnit::Sats,
        )
        .with_preselected_coins(&[coins[1].outpoint])
        .with_coins_sorted(100);
        assert_eq!(primary.recipients.len(), 1);
        assert_eq!(primary.send_max_to_recipient, None);
        assert!(primary.coins[0].1);
        assert_eq!(primary.coins[0].0.outpoint, coins[1].outpoint);
        assert_eq!(primary.timelock(), wallet.recovery_timelock());

        let recovery = DefineSpend::new(
            Network::Bitcoin,
            wallet.clone(),
            &coins,
            100,
            Some(10),
            false,
            Amount::from_sat(30_000),
            Amount::ZERO,
            SyncStatus::Synced,
            BitcoinDisplayUnit::Sats,
        );
        assert_eq!(recovery.send_max_to_recipient, Some(0));
        assert!(recovery.recipients[0].is_recovery);
        assert!(recovery.coins.iter().all(|(_, selected)| *selected));
        assert_eq!(recovery.timelock(), Some(10));

        let self_send = recovery.self_send();
        assert!(self_send.recipients.is_empty());
        assert!(self_send.is_user_coin_selection);
    }

    #[test]
    fn coins_sent_from_coin_control_are_the_users_own_selection() {
        let wallet = wallet();
        let coins = vec![coin(1, 10_000, Some(90)), coin(2, 20_000, Some(91))];
        let preselected = DefineSpend::new(
            Network::Bitcoin,
            wallet.clone(),
            &coins,
            100,
            None,
            true,
            Amount::from_sat(30_000),
            Amount::ZERO,
            SyncStatus::Synced,
            BitcoinDisplayUnit::Sats,
        )
        .with_preselected_coins(&[coins[0].outpoint]);
        assert!(
            !preselected.is_user_coin_selection,
            "a preselection alone still lets automatic selection choose"
        );
        let chosen = preselected.with_user_coin_selection();
        assert!(
            chosen.is_user_coin_selection,
            "drafting passes exactly the chosen outpoints"
        );
        assert_eq!(
            chosen
                .coins
                .iter()
                .filter(|(_, selected)| *selected)
                .map(|(coin, _)| coin.outpoint)
                .collect::<Vec<_>>(),
            vec![coins[0].outpoint]
        );
    }

    #[test]
    fn recipient_validates_address_amount_label_and_truncates_btc_precision() {
        let mut recipient = Recipient::default();
        recipient.update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "address", "not an address".to_string()),
        );
        assert!(!recipient.address.valid);

        recipient.update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "address", MAINNET_ADDR.to_string()),
        );
        assert!(recipient.address_valid());

        recipient.update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "amount", "1 234".to_string()),
        );
        assert_eq!(recipient.amount().unwrap(), 1_234);
        assert!(recipient.valid());

        recipient.update(
            Network::Bitcoin,
            BitcoinDisplayUnit::BTC,
            view::CreateSpendMessage::RecipientEdited(0, "amount", "0.123456789".to_string()),
        );
        assert_eq!(recipient.amount.value, "0.12345678");
        assert_eq!(
            recipient.amount.warning,
            Some("Amount has been truncated to 8 decimal places")
        );

        recipient.update(
            Network::Bitcoin,
            BitcoinDisplayUnit::BTC,
            view::CreateSpendMessage::RecipientEdited(0, "label", "x".repeat(101)),
        );
        assert!(!recipient.label.valid);
        assert!(!recipient.valid());
    }

    #[test]
    fn recipient_rejects_empty_zero_dust_and_wrong_network_amounts() {
        let mut recipient = Recipient::default();
        assert!(recipient.amount().is_err());

        recipient.update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "address", MAINNET_ADDR.to_string()),
        );
        recipient.update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "amount", "0".to_string()),
        );
        assert!(recipient.amount().is_err());

        recipient.update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "amount", "1".to_string()),
        );
        assert!(recipient.amount().is_err());

        recipient.update(
            Network::Signet,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "address", MAINNET_ADDR.to_string()),
        );
        assert!(!recipient.address.valid);
    }

    #[test]
    fn duplicate_detection_and_check_valid_track_form_and_coin_selection() {
        let wallet = wallet();
        let coins = vec![coin(1, 10_000, Some(90))];
        let mut state = DefineSpend::new(
            Network::Bitcoin,
            wallet,
            &coins,
            100,
            None,
            true,
            Amount::from_sat(10_000),
            Amount::ZERO,
            SyncStatus::Synced,
            BitcoinDisplayUnit::Sats,
        )
        .with_preselected_coins(&[coins[0].outpoint]);

        state.feerate = form::Value {
            value: "2".to_string(),
            valid: true,
            warning: None,
        };
        state.recipients[0].update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "address", MAINNET_ADDR.to_string()),
        );
        state.recipients[0].update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "amount", "1000".to_string()),
        );
        state.check_valid();
        assert!(state.is_valid);
        assert!(!state.is_duplicate);

        state.recipients.push(state.recipients[0].clone());
        state.batch_label.valid = true;
        state.check_valid();
        assert!(state.exists_duplicate());
        assert!(state.is_duplicate);

        state.batch_label.valid = false;
        state.check_valid();
        assert!(!state.is_valid);
    }

    #[test]
    fn redraft_result_insufficient_funds_sets_missing_or_under_dust_state() {
        let wallet = wallet();
        let coins = vec![coin(1, 10_000, Some(90))];
        let mut state = DefineSpend::new(
            Network::Bitcoin,
            wallet,
            &coins,
            100,
            None,
            true,
            Amount::from_sat(10_000),
            Amount::ZERO,
            SyncStatus::Synced,
            BitcoinDisplayUnit::Sats,
        );
        state.coins[0].1 = true;
        state.apply_redraft_result(
            Address::from_str(MAINNET_ADDR).unwrap(),
            None,
            false,
            Ok(CreateSpendResult::InsufficientFunds { missing: 2_500 }),
        );
        assert_eq!(state.amount_left_to_select, Some(Amount::from_sat(2_500)));
        assert!(state.coins[0].1);
        assert!(state.fee_amount.is_none());

        state.send_max_to_recipient = Some(0);
        state.recipients[0].update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "address", MAINNET_ADDR.to_string()),
        );
        state.recipients[0].update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "amount", "1000".to_string()),
        );
        state.apply_redraft_result(
            Address::from_str(MAINNET_ADDR).unwrap(),
            Some(0),
            false,
            Ok(CreateSpendResult::InsufficientFunds { missing: 100 }),
        );
        assert!(state.amount_left_to_select.is_none());
        assert!(state.recipients[0]
            .dust_warning
            .as_deref()
            .is_some_and(|warning| warning.contains("Minimum amount")));
        assert!(state.recipients[0].amount.value.is_empty());
    }

    #[test]
    fn recovery_paths_count_only_confirmed_mature_unspent_coins() {
        let wallet = wallet();
        let mut spent = coin(2, 20_000, Some(80));
        spent.spend_info = Some(coincubed::commands::LCSpendInfo {
            txid: Txid::from_str(&format!("{:064x}", 88)).unwrap(),
            height: Some(100),
        });
        let coins = vec![
            coin(1, 10_000, Some(90)),
            spent,
            coin(3, 30_000, None),
            coin(4, 40_000, Some(95)),
        ];

        let paths = recovery_paths(&wallet, &coins, 100);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].sequence, 10);
        assert_eq!(paths[0].number_of_coins, 1);
        assert_eq!(paths[0].total_amount, Amount::from_sat(10_000));
        assert_eq!(paths[0].threshold, 1);
        assert_eq!(paths[0].remaining_sequence, Some(0));
        assert_eq!(
            paths[0].origins[0].0,
            Fingerprint::from_str("8a64f2a9").unwrap()
        );
    }

    #[test]
    fn recovery_availability_and_countdown_follow_confirmations_and_spends() {
        let wallet = wallet();
        let mut spent = coin(1, 10_000, Some(80));
        spent.spend_info = Some(coincubed::commands::LCSpendInfo {
            txid: outpoint(99).txid,
            height: None,
        });
        let mut coins = vec![spent, coin(2, 20_000, None), coin(3, 30_000, Some(95))];
        let paths = recovery_paths(&wallet, &coins, 100);
        assert_eq!(paths[0].number_of_coins, 0);
        assert_eq!(paths[0].remaining_sequence, Some(5));
        // The same timing used by Overview agrees with the spend selector:
        // a transaction can enter the next block when one block remains.
        let paths = recovery_paths(&wallet, &coins, 104);
        assert_eq!(paths[0].number_of_coins, 1);
        assert_eq!(paths[0].remaining_sequence, Some(1));
        coins.pop();
        assert_eq!(next_recovery_sequence(&coins, 104, 10), None);
        assert_eq!(next_recovery_sequence(&[], 104, 10), None);
    }

    #[test]
    fn recovery_countdown_is_visible_and_updates_until_path_opens() {
        use crate::app::view::shared::landing::capture;
        use coincube_ui::theme;

        let descriptor = DESC
            .split('#')
            .next()
            .unwrap()
            .replace("older(10)", "older(52560)");
        let wallet = Arc::new(Wallet::new(
            CoincubeDescriptor::from_str(&descriptor).unwrap(),
        ));
        let coins = [coin(1, 10_000, Some(83_906))];
        let mut selector = SelectRecoveryPath::new(wallet, &coins, 100_000);
        let menu = Menu::Vault(crate::app::menu::VaultSubMenu::Recovery);
        let cache = Cache::default();
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                let mounted = capture::inspect_filled(
                    "recovery-locked-countdown",
                    selector.view(&menu, &cache),
                    outer,
                    appearance,
                    theme,
                    outer,
                );
                assert!(mounted.contains("No recovery path is currently available."));
                assert!(mounted.contains("Recovery path available in ≈ 254 days."));
                assert!(!mounted.contains("can recover"));
                assert!(!mounted.contains("Next"));
            }
        }
        selector.load_from_coins_and_tip_height(&coins, 100_144);
        assert_eq!(selector.recovery_paths[0].remaining_sequence, Some(36_322));
        selector.load_from_coins_and_tip_height(&coins, 136_465);
        assert_eq!(selector.recovery_paths[0].number_of_coins, 1);
        let mounted = capture::inspect_filled(
            "recovery-open",
            selector.view(&menu, &cache),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
            (1024, 768),
        );
        assert!(mounted.contains("1 recovery path is available:"));
        assert!(mounted.contains("can recover"));
        assert!(!mounted.contains("available in ≈"));
        assert!(mounted.contains("Next"));
    }

    #[test]
    fn select_recovery_path_preserves_selection_across_reload_when_sequence_remains() {
        let wallet = wallet();
        let coins = vec![coin(1, 10_000, Some(90))];
        let mut selector = SelectRecoveryPath::new(wallet, &coins, 100);
        selector.selected_path = Some(0);

        selector.load_from_coins_and_tip_height(&[coin(2, 20_000, Some(80))], 100);
        assert_eq!(selector.selected_path, Some(0));
        assert_eq!(
            selector.recovery_paths[0].total_amount,
            Amount::from_sat(20_000)
        );

        let mut draft = TransactionDraft::new(Network::Bitcoin, None);
        selector.apply(&mut draft);
        assert_eq!(draft.recovery_timelock, Some(10));

        let daemon = crate::utils::mock::Daemon::new(vec![]);
        let _ = selector.update(
            Arc::new(crate::daemon::client::Coincubed::new(daemon.run())),
            &Cache::default(),
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::SelectPath(0),
            )),
        );
        assert_eq!(selector.selected_path, None);
    }
    fn branta_form() -> DefineSpend {
        let mut state = DefineSpend::new(
            Network::Bitcoin,
            wallet(),
            &[coin(1, 50_000, Some(90))],
            100,
            None,
            true,
            Amount::from_sat(50_000),
            Amount::ZERO,
            SyncStatus::Synced,
            BitcoinDisplayUnit::Sats,
        );
        state.recipients[0] = bip21_recipient(1_000);
        state.feerate.value = "2".into();
        state.feerate.valid = true;
        state.coins[0].1 = true;
        state.check_valid();
        assert!(state.form_values_are_valid(false));
        assert!(state.is_valid);
        assert!(!state.is_duplicate);
        state
    }

    fn bip21_recipient(sats: u64) -> Recipient {
        let mut recipient = Recipient::default();
        recipient.update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(
                0,
                "address",
                format!(
                    "bitcoin:{MAINNET_ADDR}?amount={}&branta_id=example&branta_secret=secret",
                    Amount::from_sat(sats).to_btc()
                ),
            ),
        );
        recipient
    }

    fn prepared_outputs(amounts: &[u64]) -> Psbt {
        use coincube_core::miniscript::bitcoin::{absolute, transaction, Transaction, TxOut};
        Psbt::from_unsigned_tx(Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![],
            output: amounts
                .iter()
                .map(|sats| TxOut {
                    value: Amount::from_sat(*sats),
                    script_pubkey: address().script_pubkey(),
                })
                .collect(),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn imported_single_recipient_max_drafts_a_sweep_to_the_recipient() {
        use crate::services::split_test_wallets::{imported_vault, Shape};
        use iced::futures::StreamExt;
        use serde_json::json;

        let mut state = branta_form();
        state.wallet = Arc::new(imported_vault(Shape::Wpkh));
        state.recipients[0] = Recipient::default();
        state.recipients[0].update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "address", MAINNET_ADDR.into()),
        );
        state.send_max_to_recipient = Some(0);
        let recipient = state.recipients[0].destination().unwrap();
        let outpoint = state.coins[0].0.outpoint;
        let mut psbt = prepared_outputs(&[49_000]);
        psbt.unsigned_tx.output[0].script_pubkey =
            recipient.clone().assume_checked().script_pubkey();
        let response = serde_json::to_value(CreateSpendResult::Success {
            psbt,
            warnings: vec![],
        })
        .unwrap();
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![(
                Some(json!({"method": "createspend", "params": [{}, [outpoint], 2, &recipient]})),
                Ok(response),
            )])
            .run(),
        ));

        let mut stream = iced_runtime::task::into_stream(state.run_redraft(daemon, 7)).unwrap();
        let Some(iced_runtime::Action::Output(Message::RedraftResult {
            result: Ok(CreateSpendResult::Success { psbt, .. }),
            ..
        })) = stream.next().await
        else {
            panic!("expected a successful MAX draft");
        };
        let output = &psbt.unsigned_tx.output[0];
        assert_eq!(
            output.script_pubkey,
            recipient.assume_checked().script_pubkey()
        );
        assert_eq!(output.value.to_sat(), 49_000);
    }

    #[test]
    fn bip21_preserves_original_populates_amount_and_checks_network() {
        let mut recipient = bip21_recipient(1_234);
        assert!(recipient.address.value.contains("branta_secret=secret"));
        assert_eq!(
            recipient.destination().unwrap(),
            Address::from_str(MAINNET_ADDR).unwrap()
        );
        assert_eq!(recipient.amount().unwrap(), 1_234);
        assert!(recipient.valid());
        recipient.update(
            Network::Testnet,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(
                0,
                "address",
                recipient.address.value.clone(),
            ),
        );
        assert!(!recipient.address_valid());
    }

    #[test]
    fn bip21_amount_mismatch_cannot_be_ignored_or_overwritten_by_max() {
        let mut recipient = bip21_recipient(1_000);
        assert!(recipient.valid());
        recipient.update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(0, "amount", "2000".into()),
        );
        assert!(!recipient.valid());
        assert!(recipient.amount().is_err());
        assert_eq!(
            recipient.amount.warning,
            Some("Amount must match the Bitcoin payment request")
        );
        let mut state = branta_form();
        state.recipients[0] = recipient;
        state.send_max_to_recipient = Some(0);
        assert!(!state.form_values_are_valid(false));
    }

    #[test]
    fn bip21_rejects_ambiguous_amounts_required_fields_and_foreign_schemes() {
        for suffix in [
            "amount=0.001&amount=0.002",
            "amount=-1",
            "amount=0",
            "req-unknown=x",
            "amount=0.000000001",
            "amount=oops",
        ] {
            assert!(parse_payment_request(&format!("bitcoin:{MAINNET_ADDR}?{suffix}")).is_err());
        }
        assert!(parse_payment_request(&format!("liquidnetwork:{MAINNET_ADDR}")).is_err());
        assert!(parse_payment_request(&format!("bitcoin://{MAINNET_ADDR}")).is_err());
    }

    #[test]
    fn duplicate_address_policy_applies_to_bip21_and_plain_address() {
        let mut state = branta_form();
        let mut plain = bip21_recipient(2_000);
        plain.update(
            Network::Bitcoin,
            BitcoinDisplayUnit::Sats,
            view::CreateSpendMessage::RecipientEdited(1, "address", MAINNET_ADDR.into()),
        );
        state.recipients.push(plain);
        assert!(state.exists_duplicate());
    }

    #[test]
    fn recipient_output_association_preserves_rows_and_duplicate_destinations() {
        let recipients = vec![
            bip21_recipient(1_000),
            bip21_recipient(2_000),
            bip21_recipient(1_000),
        ];
        let psbt = prepared_outputs(&[2_000, 1_000, 1_000]);
        assert_eq!(
            associate_recipients(&recipients, &psbt).unwrap(),
            vec![1, 0, 2]
        );
        assert!(associate_recipients(&recipients, &prepared_outputs(&[1_000, 2_000])).is_none());
        assert!(associate_recipients(&[bip21_recipient(1_001)], &psbt).is_none());
        assert!(associate_recipients(&[], &psbt).unwrap().is_empty());
    }

    /// Choose coins opens the list without touching the draft; picking a
    /// coin keeps it open, and Clear goes back to the automatic selection.
    #[tokio::test]
    async fn choose_coins_opens_the_list_and_clear_closes_it() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        let send = |state: &mut DefineSpend, message: view::CreateSpendMessage| {
            state.update(
                daemon.clone(),
                &Cache::default(),
                Message::View(view::Message::CreateSpend(message)),
            )
        };
        assert!(!state.coin_list_shown());
        let preparation = state.preparation_id;
        let task = send(&mut state, view::CreateSpendMessage::ChooseCoins);
        assert_eq!(task.units(), 0, "nothing to redraft");
        assert!(state.coin_list_shown());
        assert_eq!(state.preparation_id, preparation, "the draft is untouched");

        drop(send(&mut state, view::CreateSpendMessage::SelectCoin(0)));
        assert!(state.is_user_coin_selection);
        drop(send(&mut state, view::CreateSpendMessage::AutomaticCoins));
        assert!(!state.coin_list_shown(), "Automatic closes the list");
        assert!(
            !state.is_user_coin_selection,
            "the draft picks the coins again"
        );

        drop(send(&mut state, view::CreateSpendMessage::ChooseCoins));
        drop(send(&mut state, view::CreateSpendMessage::Clear));
        assert!(!state.coin_list_shown());
        drop(send(&mut state, view::CreateSpendMessage::SelectCoin(0)));
        assert!(state.coin_list_shown(), "a chosen coin keeps the list open");
    }

    /// Clear starts the fee on Normal again, as a new form does, never on an
    /// empty Custom field; and it keeps the redraft generation counting up.
    #[tokio::test]
    async fn clear_resets_the_fee_to_normal() {
        use iced_runtime::futures::futures::StreamExt;
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        drop(state.update(
            daemon.clone(),
            &Cache::default(),
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::CustomFeerate,
            )),
        ));
        let seq = state.redraft_seq;
        let task = state.update(
            daemon,
            &Cache::default(),
            Message::View(view::Message::CreateSpend(view::CreateSpendMessage::Clear)),
        );
        assert!(state.redraft_seq > seq);
        let messages: Vec<Message> = iced_runtime::task::into_stream(task)
            .expect("a fee estimate request")
            .filter_map(|action| async move {
                match action {
                    iced_runtime::Action::Output(message) => Some(message),
                    _ => None,
                }
            })
            .collect()
            .await;
        assert!(
            matches!(
                messages.as_slice(),
                [Message::View(view::Message::CreateSpend(
                    view::CreateSpendMessage::FetchFeeEstimate(NORMAL_FEE_TARGET)
                ))]
            ),
            "{:?}",
            messages
        );
    }

    /// While a complete form's draft is pending the footer says it is
    /// calculating; the draft landing (or the form going incomplete) ends it.
    #[tokio::test]
    async fn a_pending_draft_reads_as_drafting_only_while_the_form_is_complete() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        assert!(state.form_values_are_valid(true), "fixture is complete");
        drop(state.schedule_redraft());
        assert!(state.drafting());
        state.feerate.value.clear();
        assert!(!state.drafting(), "an incomplete form is not drafting");
        // The debounce firing on the incomplete form ends the pending draft.
        let seq = state.redraft_seq;
        drop(state.run_redraft(daemon, seq));
        assert!(!state.draft_pending);
    }

    /// Drive one debounced redraft through `update` the way the runtime does:
    /// fire the debounce tick, run the dispatched draft against `daemon`, and
    /// return its `RedraftResult` for the caller to feed back.
    async fn land_redraft(
        state: &mut DefineSpend,
        daemon: Arc<dyn Daemon + Sync + Send>,
    ) -> Message {
        use iced_runtime::futures::futures::StreamExt;
        let seq = state.redraft_seq;
        let task = state.update(
            daemon.clone(),
            &Cache::default(),
            Message::RedraftDebounce(seq),
        );
        let mut results: Vec<Message> = iced_runtime::task::into_stream(task)
            .expect("a dispatched draft")
            .filter_map(|action| async move {
                match action {
                    iced_runtime::Action::Output(message) => Some(message),
                    _ => None,
                }
            })
            .collect()
            .await;
        assert_eq!(results.len(), 1, "one draft result");
        results.pop().unwrap()
    }

    fn redraft_message_clone(message: &Message) -> Message {
        match message {
            Message::RedraftResult {
                seq,
                max_address,
                recipient_with_max,
                destinations_is_empty,
                result: Ok(result),
            } => Message::RedraftResult {
                seq: *seq,
                max_address: max_address.clone(),
                recipient_with_max: *recipient_with_max,
                destinations_is_empty: *destinations_is_empty,
                result: Ok(result.clone()),
            },
            other => panic!("expected a successful draft, got {:?}", other),
        }
    }

    /// A draft spending exactly `inputs` (outpoint, value) with one 1,000 sat
    /// payment.
    fn draft_spending(inputs: &[(OutPoint, u64)]) -> serde_json::Value {
        use coincube_core::miniscript::bitcoin::{psbt::Input, TxIn, TxOut};
        let mut psbt = prepared_outputs(&[1_000]);
        psbt.unsigned_tx.input = inputs
            .iter()
            .map(|(outpoint, _)| TxIn {
                previous_output: *outpoint,
                ..Default::default()
            })
            .collect();
        psbt.inputs = inputs
            .iter()
            .map(|(_, sats)| Input {
                witness_utxo: Some(TxOut {
                    value: Amount::from_sat(*sats),
                    script_pubkey: address().script_pubkey(),
                }),
                ..Default::default()
            })
            .collect();
        serde_json::to_value(CreateSpendResult::Success {
            psbt,
            warnings: vec![],
        })
        .unwrap()
    }

    /// Automatic, end to end: after the user picked coins, choosing Automatic
    /// redrafts with an empty outpoint list (the daemon's automatic coin
    /// selection), and the coins that draft spends replace the user's ticks.
    #[tokio::test]
    async fn automatic_redrafts_with_automatic_coin_selection() {
        use serde_json::json;
        let mut state = branta_form();
        state.coins.push((coin(2, 80_000, Some(90)), false));
        let destinations = json!({ MAINNET_ADDR: 1_000 });
        let change = state.wallet.preview_change_address();
        // The user's pick: both coins, by hand.
        let idle: Arc<dyn Daemon + Sync + Send> = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        drop(state.update(
            idle.clone(),
            &Cache::default(),
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::SelectCoin(1),
            )),
        ));
        assert!(state.is_user_coin_selection);
        assert!(state.coins.iter().all(|(_, selected)| *selected));

        drop(state.update(
            idle,
            &Cache::default(),
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::AutomaticCoins,
            )),
        ));
        assert!(state.drafting());
        // Automatic selection: no outpoints passed, the daemon chooses. It
        // picks only the second coin.
        let daemon: Arc<dyn Daemon + Sync + Send> =
            Arc::new(crate::daemon::client::Coincubed::new(
                crate::utils::mock::Daemon::new(vec![(
                    Some(json!({"method": "createspend", "params": [destinations, [], 2, change]})),
                    Ok(draft_spending(&[(outpoint(2), 80_000)])),
                )])
                .run(),
            ));
        let result = land_redraft(&mut state, daemon.clone()).await;
        drop(state.update(daemon, &Cache::default(), result));

        assert!(!state.drafting());
        assert!(!state.coin_list_shown());
        assert_eq!(
            state
                .coins
                .iter()
                .map(|(coin, selected)| (coin.outpoint, *selected))
                .collect::<Vec<_>>(),
            vec![(outpoint(1), false), (outpoint(2), true)],
            "the automatic draft's coins replace the user's"
        );
        assert_eq!(state.amount_left_to_select, Some(Amount::ZERO));
        assert!(state.is_valid);
    }

    /// A draft that was running when the user pressed Clear must not land on
    /// the cleared form, even once the cleared form has drafted again.
    #[tokio::test]
    async fn a_draft_from_before_clear_is_ignored() {
        use serde_json::json;
        let mut state = branta_form();
        state.coins.push((coin(2, 80_000, Some(90)), false));
        let destinations = json!({ MAINNET_ADDR: 1_000 });
        let change = state.wallet.preview_change_address();
        let daemon: Arc<dyn Daemon + Sync + Send> =
            Arc::new(crate::daemon::client::Coincubed::new(
                crate::utils::mock::Daemon::new(vec![(
                    Some(json!({"method": "createspend", "params": [destinations, [], 2, change]})),
                    Ok(draft_spending(&[(outpoint(2), 80_000)])),
                )])
                .run(),
            ));
        drop(state.schedule_redraft());
        let stale = land_redraft(&mut state, daemon.clone()).await;

        drop(state.update(
            daemon.clone(),
            &Cache::default(),
            Message::View(view::Message::CreateSpend(view::CreateSpendMessage::Clear)),
        ));
        let selection = |state: &DefineSpend| -> Vec<bool> {
            state.coins.iter().map(|(_, selected)| *selected).collect()
        };
        let cleared_coins = selection(&state);
        // The cleared form schedules a draft of its own. Before Clear kept the
        // generation counting, this first post-Clear draft reused the old
        // draft's seq, so the old result landed on the cleared form.
        drop(state.schedule_redraft());
        drop(state.update(
            daemon.clone(),
            &Cache::default(),
            redraft_message_clone(&stale),
        ));
        assert_eq!(state.fee_amount, None, "no fee from the old draft");
        assert_eq!(state.amount_left_to_select, None);
        assert_eq!(
            selection(&state),
            cleared_coins,
            "the old draft's coins are not applied"
        );
        // Nor does its debounce tick redraft the cleared form.
        let Message::RedraftResult { seq, .. } = &stale else {
            unreachable!()
        };
        let task = state.update(daemon, &Cache::default(), Message::RedraftDebounce(*seq));
        assert_eq!(task.units(), 0, "a pre-Clear debounce tick does nothing");
    }

    /// Two spend forms (Send and Recovery, or a Send panel and the one that
    /// replaced it) never share a redraft generation: results are delivered
    /// to whichever panel is on screen, so a per-form counter let one form's
    /// draft be accepted by another.
    #[test]
    fn spend_forms_never_share_a_redraft_generation() {
        let mut send = branta_form();
        let mut recovery = branta_form();
        let mut seen = std::collections::HashSet::new();
        assert!(seen.insert(send.redraft_seq));
        assert!(seen.insert(recovery.redraft_seq));
        for _ in 0..8 {
            drop(send.schedule_redraft());
            assert!(seen.insert(send.redraft_seq), "send reused a generation");
            drop(recovery.schedule_redraft());
            assert!(
                seen.insert(recovery.redraft_seq),
                "recovery reused a generation"
            );
        }
    }

    /// A draft started in one spend form must not land on another form that
    /// is on screen when it completes, even on that form's first draft.
    #[tokio::test]
    async fn a_draft_from_another_spend_form_is_ignored() {
        use serde_json::json;
        let mut old_send = branta_form();
        old_send.coins.push((coin(2, 80_000, Some(90)), false));
        let destinations = json!({ MAINNET_ADDR: 1_000 });
        let change = old_send.wallet.preview_change_address();
        let daemon: Arc<dyn Daemon + Sync + Send> =
            Arc::new(crate::daemon::client::Coincubed::new(
                crate::utils::mock::Daemon::new(vec![(
                    Some(json!({"method": "createspend", "params": [destinations, [], 2, change]})),
                    Ok(draft_spending(&[(outpoint(2), 80_000)])),
                )])
                .run(),
            ));
        drop(old_send.schedule_redraft());
        let stale = land_redraft(&mut old_send, daemon.clone()).await;

        // A fresh form (the panel that replaced Send, or Recovery) schedules
        // its own first draft. With per-form counters both drafts were seq 1.
        let mut other = branta_form();
        other.coins.push((coin(2, 80_000, Some(90)), false));
        let selection = |state: &DefineSpend| -> Vec<bool> {
            state.coins.iter().map(|(_, selected)| *selected).collect()
        };
        let coins_before = selection(&other);
        drop(other.schedule_redraft());
        drop(other.update(
            daemon.clone(),
            &Cache::default(),
            redraft_message_clone(&stale),
        ));
        assert_eq!(other.fee_amount, None, "no fee from the other form's draft");
        assert_eq!(other.amount_left_to_select, None);
        assert!(other.draft_pending, "its own draft is still pending");
        assert_eq!(
            selection(&other),
            coins_before,
            "the other form's coins are not applied"
        );
        // Nor does the other form's debounce tick redraft this one.
        let Message::RedraftResult { seq, .. } = &stale else {
            unreachable!()
        };
        let task = other.update(daemon, &Cache::default(), Message::RedraftDebounce(*seq));
        assert_eq!(task.units(), 0, "another form's debounce tick does nothing");
    }

    /// Automatic hands the choice back: Prepare waits for the automatic
    /// draft instead of building from the coins the user had ticked.
    #[tokio::test]
    async fn automatic_waits_for_the_automatic_draft() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        let send = |state: &mut DefineSpend, message: view::CreateSpendMessage| {
            state.update(
                daemon.clone(),
                &Cache::default(),
                Message::View(view::Message::CreateSpend(message)),
            )
        };
        let seq = state.redraft_seq;
        assert_eq!(
            send(&mut state, view::CreateSpendMessage::AutomaticCoins).units(),
            0,
            "already automatic: nothing to redraft"
        );
        assert_eq!(state.redraft_seq, seq);

        drop(send(&mut state, view::CreateSpendMessage::SelectCoin(0)));
        state.amount_left_to_select = Some(Amount::ZERO);
        drop(send(&mut state, view::CreateSpendMessage::AutomaticCoins));
        assert_eq!(state.amount_left_to_select, None);
        assert!(state.drafting());
    }

    /// Switching presets on a complete form: Prepare does not build at the
    /// old rate while the new estimate is on its way, waits for the redraft
    /// once it lands, and a failed estimate falls back to the rate already in
    /// the field instead of leaving the form waiting.
    #[tokio::test]
    async fn a_preset_switch_holds_prepare_until_its_estimate_is_drafted() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        let send = |state: &mut DefineSpend, message: view::CreateSpendMessage| {
            state.update(
                daemon.clone(),
                &Cache::default(),
                Message::View(view::Message::CreateSpend(message)),
            )
        };
        // Normal's draft has landed: the form is ready to prepare.
        state.selected_feerate_target = Some(NORMAL_FEE_TARGET);
        state.amount_left_to_select = Some(Amount::ZERO);

        // Fast: the Normal draft is set aside, and one still in flight is
        // dropped (its generation no longer matches).
        let seq = state.redraft_seq;
        drop(send(
            &mut state,
            view::CreateSpendMessage::FetchFeeEstimate(1),
        ));
        assert_eq!(state.loading_fee_estimate, Some(1));
        assert_eq!(state.amount_left_to_select, None);
        assert_ne!(state.redraft_seq, seq);
        assert!(!state.draft_pending);
        assert_eq!(
            state.feerate.value, "2",
            "the old rate stays until replaced"
        );

        // A draft at the old rate landing meanwhile is ignored.
        drop(state.update(
            daemon.clone(),
            &Cache::default(),
            Message::RedraftResult {
                seq,
                max_address:
                    Address::from_str("bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg").unwrap(),
                recipient_with_max: None,
                destinations_is_empty: false,
                result: Err(SpendCreationError::FeeEstimationFailed.into()),
            },
        ));
        assert_eq!(state.amount_left_to_select, None);
        assert!(state.warning.is_none());

        // Prepare is a no-op while the estimate is on its way.
        assert_eq!(
            send(&mut state, view::CreateSpendMessage::Generate).units(),
            0
        );
        assert!(!state.generating);

        // Fast's estimate lands: it fills the field and the redraft at the
        // new rate is pending, so the footer reads "Calculating…".
        let fast = state.fee_estimate_request;
        drop(send(
            &mut state,
            view::CreateSpendMessage::FeeEstimated {
                request: fast,
                feerate: Some(40),
            },
        ));
        assert_eq!(state.loading_fee_estimate, None);
        assert_eq!(state.selected_feerate_target, Some(1));
        assert_eq!(state.feerate.value, "40");
        assert_eq!(state.amount_left_to_select, None);
        assert!(state.drafting());

        // Slow fails: the field keeps Fast's 40, now shown as no preset fills
        // it, and the form drafts it again rather than waiting forever.
        state.amount_left_to_select = Some(Amount::ZERO);
        drop(send(
            &mut state,
            view::CreateSpendMessage::FetchFeeEstimate(24),
        ));
        let slow = state.fee_estimate_request;
        // A warning from elsewhere (e.g. a failed label load) must not let
        // Prepare through before the fallback draft lands.
        state.warning = Some(Error::Unexpected("synthetic labels failure".into()));
        let failed = send(
            &mut state,
            view::CreateSpendMessage::FeeEstimated {
                request: slow,
                feerate: None,
            },
        );
        assert_eq!(failed.units(), 2, "the error and the redraft");
        assert_eq!(state.loading_fee_estimate, None);
        assert_eq!(state.selected_feerate_target, None);
        assert_eq!(state.feerate.value, "40");
        assert!(state.custom_feerate_shown());
        // Reported by the error alone, so the draft still holds Prepare
        // (a recovery or self-transfer never waits on `amount_left`).
        assert!(state.warning.is_none());
        assert!(state.drafting());
        assert_eq!(
            send(&mut state, view::CreateSpendMessage::Generate).units(),
            0
        );
        assert!(!state.generating);

        // Custom while a preset is fetching, with the field's rate already
        // the custom one: nothing to restore, but the draft set aside by the
        // fetch is drafted again.
        state.custom_feerate_value = Some("40".into());
        state.warning = None;
        state.draft_pending = false;
        drop(send(
            &mut state,
            view::CreateSpendMessage::FetchFeeEstimate(6),
        ));
        assert!(!state.draft_pending);
        drop(send(&mut state, view::CreateSpendMessage::CustomFeerate));
        assert_eq!(state.loading_fee_estimate, None);
        assert!(state.draft_pending);
        assert!(state.drafting());

        // Custom restoring a different typed rate mid-fetch: applied at
        // once (a returned message could land on another page) and drafted.
        state.custom_feerate_value = Some("12".into());
        state.draft_pending = false;
        drop(send(
            &mut state,
            view::CreateSpendMessage::FetchFeeEstimate(6),
        ));
        drop(send(&mut state, view::CreateSpendMessage::CustomFeerate));
        assert_eq!(state.loading_fee_estimate, None);
        assert_eq!(state.feerate.value, "12");
        assert!(state.drafting());
    }

    /// Leaving the step (another menu entry, or Back) while a preset's
    /// estimate or a draft is on its way: those land on whatever is on
    /// screen, so the step requests them again once it is shown (`resume`,
    /// from the panel's `reload` or a step change), never while hidden, and
    /// holds Prepare until then.
    #[tokio::test]
    async fn leaving_mid_fetch_or_mid_draft_resumes_once_shown_again() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        let send = |state: &mut DefineSpend, message: Message| {
            state.update(daemon.clone(), &Cache::default(), message)
        };
        let generate = || {
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::Generate,
            ))
        };
        state.amount_left_to_select = Some(Amount::ZERO);

        // Fast is fetching when the step is left.
        drop(send(
            &mut state,
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::FetchFeeEstimate(1),
            )),
        ));
        let fast = state.fee_estimate_request;
        state.interrupt();
        assert_eq!(state.resume_fee_estimate, Some(1));
        assert_eq!(state.loading_fee_estimate, Some(1), "Prepare stays held");

        // While hidden, the cache refresh still reaches the panel: nothing
        // is requested, as its result would land on another page.
        drop(send(&mut state, Message::UpdatePanelCache(false)));
        assert_eq!(state.resume_fee_estimate, Some(1));
        assert_eq!(state.fee_estimate_request, fast);
        // Prepare is refused before the step is back.
        assert_eq!(send(&mut state, generate()).units(), 0);
        assert!(!state.generating);

        // Shown again: Fast is fetched anew.
        assert!(state.resume().units() > 0);
        assert_eq!(state.resume_fee_estimate, None);
        assert_eq!(state.loading_fee_estimate, Some(1));
        assert_ne!(state.fee_estimate_request, fast);
        assert_eq!(state.resume().units(), 0, "nothing left to resume");
        // The first fetch's estimate, if it does reach the step, is dropped.
        drop(send(
            &mut state,
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::FeeEstimated {
                    request: fast,
                    feerate: Some(40),
                },
            )),
        ));
        assert_eq!(state.feerate.value, "2");
        assert_eq!(state.loading_fee_estimate, Some(1));

        // A draft pending when the step is left: Prepare stays held (the
        // previous draft's selection is stale), and the draft is requested
        // again once shown.
        state.loading_fee_estimate = None;
        state.amount_left_to_select = Some(Amount::ZERO);
        drop(state.schedule_redraft());
        state.interrupt();
        assert!(state.resume_redraft);
        assert!(state.drafting());
        drop(send(&mut state, Message::UpdatePanelCache(false)));
        assert!(state.resume_redraft, "not while hidden");
        assert_eq!(send(&mut state, generate()).units(), 0);
        assert!(!state.generating);
        let seq = state.redraft_seq;
        assert!(state.resume().units() > 0);
        assert!(!state.resume_redraft);
        assert!(state.draft_pending);
        assert_ne!(state.redraft_seq, seq);
    }

    /// A recovery's DefineSpend starts on Normal once shown, keeps the rate
    /// the user set when Back/Next returns to the same path, and starts on
    /// Normal again when another path (timelock) re-creates it.
    #[tokio::test]
    async fn recovery_form_starts_on_normal_again_after_a_path_change() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let coins = [coin(1, 50_000, Some(50))];
        let mut state = DefineSpend::new(
            Network::Bitcoin,
            wallet(),
            &coins,
            100,
            Some(10),
            false,
            Amount::from_sat(50_000),
            Amount::ZERO,
            SyncStatus::Synced,
            BitcoinDisplayUnit::Sats,
        )
        .starting_on_normal_feerate();
        // The panel's Next: load the step, resume it, then hand it the
        // message.
        let next = |state: &mut DefineSpend, timelock: u16| {
            state.load(
                &coins,
                100,
                &TransactionDraft::new(Network::Bitcoin, Some(timelock)),
            );
            let resumed = state.resume();
            Task::batch([
                resumed,
                state.update(
                    daemon.clone(),
                    &Cache::default(),
                    Message::View(view::Message::Next),
                ),
            ])
        };

        // First shown: Normal is fetched.
        drop(next(&mut state, 10));
        assert_eq!(state.loading_fee_estimate, Some(NORMAL_FEE_TARGET));

        // The user types their own rate, goes Back and Next on the same path:
        // the rate is kept, nothing is fetched.
        drop(state.update(
            daemon.clone(),
            &Cache::default(),
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::FeerateEdited("12".into()),
            )),
        ));
        assert_eq!(state.loading_fee_estimate, None);
        state.interrupt();
        drop(next(&mut state, 10));
        assert_eq!(state.feerate.value, "12");
        assert_eq!(state.loading_fee_estimate, None);
        assert_eq!(state.resume_fee_estimate, None);

        // Back, another path, Next: the re-created form starts on Normal.
        state.interrupt();
        drop(next(&mut state, 20));
        assert_eq!(state.timelock(), Some(20));
        assert_eq!(state.loading_fee_estimate, Some(NORMAL_FEE_TARGET));
        assert_eq!(state.resume_fee_estimate, None);
    }

    /// A recovery through the real panel: its DefineSpend fetches Normal once
    /// Next first shows it (not while the path step is on screen), Back saves
    /// a fetch still in flight for the next Next (its estimate lands on the
    /// path step and is lost), and a rate the user typed survives Back and
    /// Next on the same path.
    #[tokio::test]
    async fn the_recovery_panel_starts_on_normal_once_and_keeps_a_typed_rate() {
        use super::super::CreateSpendPanel;
        use crate::app::state::State;
        let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
            crate::daemon::client::Coincubed::new(crate::utils::mock::Daemon::new(vec![]).run()),
        );
        let cache = Cache::default();
        let mut panel = CreateSpendPanel::new_recovery(
            wallet(),
            &[coin(1, 50_000, Some(50))],
            100,
            Network::Bitcoin,
            Amount::from_sat(50_000),
            Amount::ZERO,
            SyncStatus::Synced,
            BitcoinDisplayUnit::Sats,
        );
        let form = |panel: &CreateSpendPanel| {
            let form = panel.steps[1]
                .as_define_spend()
                .expect("the recovery's DefineSpend");
            (
                form.loading_fee_estimate,
                form.resume_fee_estimate,
                form.fee_estimate_request,
                form.feerate.value.clone(),
            )
        };
        let send = |panel: &mut CreateSpendPanel, message: view::Message| {
            panel.update(Some(daemon.clone()), &cache, Message::View(message))
        };

        // On the path step, the panel's reload requests nothing of the form.
        drop(panel.reload(Some(daemon.clone()), Some(wallet())));
        assert_eq!(panel.current, 0);
        let (loading, resume, _, _) = form(&panel);
        assert_eq!(loading, None);
        assert_eq!(resume, Some(NORMAL_FEE_TARGET));

        // Next shows it: Normal is fetched, once.
        drop(send(&mut panel, view::Message::Next));
        assert_eq!(panel.current, 1);
        let (loading, resume, first, _) = form(&panel);
        assert_eq!(loading, Some(NORMAL_FEE_TARGET));
        assert_eq!(resume, None);

        // Back while it is fetching saves the fetch; Next on the same path
        // requests it anew.
        drop(send(&mut panel, view::Message::Previous));
        assert_eq!(panel.current, 0);
        assert_eq!(form(&panel).1, Some(NORMAL_FEE_TARGET));
        drop(send(&mut panel, view::Message::Next));
        let (loading, resume, again, _) = form(&panel);
        assert_eq!(loading, Some(NORMAL_FEE_TARGET));
        assert_eq!(resume, None);
        assert_ne!(again, first);

        // A typed rate survives Back and Next on the same path: nothing is
        // fetched over it.
        drop(send(
            &mut panel,
            view::Message::CreateSpend(view::CreateSpendMessage::FeerateEdited("12".into())),
        ));
        drop(send(&mut panel, view::Message::Previous));
        drop(send(&mut panel, view::Message::Next));
        assert_eq!(panel.current, 1);
        let (loading, resume, request, rate) = form(&panel);
        assert_eq!(rate, "12");
        assert_eq!(loading, None);
        assert_eq!(resume, None);
        assert_eq!(request, again);
    }

    /// Fee segments map to rates: a preset fetches its block target's
    /// estimate and hides the field; Custom shows the field and keeps the rate
    /// typed into it, even after a preset replaced it.
    #[tokio::test]
    async fn fee_segments_map_to_rates_and_custom_keeps_the_typed_value() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        let send = |state: &mut DefineSpend, message: view::CreateSpendMessage| {
            state.update(
                daemon.clone(),
                &Cache::default(),
                Message::View(view::Message::CreateSpend(message)),
            )
        };
        // Typed before any preset: the field shows.
        drop(send(
            &mut state,
            view::CreateSpendMessage::FeerateEdited("12".into()),
        ));
        assert!(state.custom_feerate_shown());

        // Normal: its block target is fetched and its estimate fills the
        // field, which hides.
        drop(send(
            &mut state,
            view::CreateSpendMessage::FetchFeeEstimate(6),
        ));
        assert_eq!(state.loading_fee_estimate, Some(6));
        let normal = state.fee_estimate_request;
        drop(send(
            &mut state,
            view::CreateSpendMessage::FeeEstimated {
                request: normal,
                feerate: Some(4),
            },
        ));
        assert_eq!(state.selected_feerate_target, Some(6));
        assert_eq!(state.feerate.value, "4");
        assert!(!state.custom_feerate_shown());

        // Custom: the field shows again with the typed 12, not Normal's 4.
        drop(send(&mut state, view::CreateSpendMessage::CustomFeerate));
        assert!(state.custom_feerate_shown());
        assert_eq!(state.selected_feerate_target, None);
        assert_eq!(state.feerate.value, "12");
        assert!(state.custom_feerate_shown(), "still Custom after restoring");

        // A preset fetch in flight when Custom is chosen doesn't land on it:
        // its estimate, delivered late, leaves the rate, the stored custom
        // rate and the selection alone, and a late failure raises nothing.
        drop(send(
            &mut state,
            view::CreateSpendMessage::FetchFeeEstimate(1),
        ));
        let fast = state.fee_estimate_request;
        drop(send(&mut state, view::CreateSpendMessage::CustomFeerate));
        assert_eq!(state.loading_fee_estimate, None);
        assert!(
            send(
                &mut state,
                view::CreateSpendMessage::FeeEstimated {
                    request: fast,
                    feerate: Some(30),
                },
            )
            .units()
                == 0
        );
        assert_eq!(state.feerate.value, "12");
        assert_eq!(state.custom_feerate_value.as_deref(), Some("12"));
        assert_eq!(state.selected_feerate_target, None);
        assert!(state.custom_feerate_shown());
        drop(send(
            &mut state,
            view::CreateSpendMessage::FeeEstimated {
                request: fast,
                feerate: None,
            },
        ));
        assert!(state.warning.is_none());

        // Normal, then Fast before Normal's estimate returns: Normal's late
        // estimate is dropped, and Fast's fills the field as Fast.
        drop(send(
            &mut state,
            view::CreateSpendMessage::FetchFeeEstimate(6),
        ));
        let normal = state.fee_estimate_request;
        drop(send(
            &mut state,
            view::CreateSpendMessage::FetchFeeEstimate(1),
        ));
        let fast = state.fee_estimate_request;
        assert_ne!(normal, fast);
        drop(send(
            &mut state,
            view::CreateSpendMessage::FeeEstimated {
                request: normal,
                feerate: Some(5),
            },
        ));
        assert_eq!(state.loading_fee_estimate, Some(1));
        assert_eq!(state.feerate.value, "12");
        drop(send(
            &mut state,
            view::CreateSpendMessage::FeeEstimated {
                request: fast,
                feerate: Some(40),
            },
        ));
        assert_eq!(state.selected_feerate_target, Some(1));
        assert_eq!(state.feerate.value, "40");
        assert_eq!(state.custom_feerate_value.as_deref(), Some("12"));

        // A typed rate also drops a fetch still in flight.
        drop(send(
            &mut state,
            view::CreateSpendMessage::FetchFeeEstimate(24),
        ));
        let slow = state.fee_estimate_request;
        drop(send(&mut state, view::CreateSpendMessage::CustomFeerate));
        drop(send(
            &mut state,
            view::CreateSpendMessage::FeerateEdited("9".into()),
        ));
        drop(send(
            &mut state,
            view::CreateSpendMessage::FeeEstimated {
                request: slow,
                feerate: Some(2),
            },
        ));
        assert_eq!(state.feerate.value, "9");
        assert_eq!(state.selected_feerate_target, None);
    }

    #[tokio::test]
    async fn authenticated_mismatch_blocks_transition_until_request_replacement() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        let ticket = branta::test_ticket(true);
        state.lookup_ticket = Some(ticket.clone());
        state.generating = true;
        let _ = state.update(
            daemon.clone(),
            &Cache::default(),
            Message::VaultSpendPrepared {
                generation: state.preparation_id,
                result: Ok((prepared_outputs(&[1_000]), vec![])),
                identities: vec![(0, LookupResult::Tampered)],
            },
        );
        assert!(state.tampered);
        assert!(!state.can_advance());
        assert!(state.generated.is_none());
        ticket.set_test_enabled(false);
        state.recipients.push(bip21_recipient(2_000));
        let _ = state.update(
            daemon.clone(),
            &Cache::default(),
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::RecipientEdited(
                    1,
                    "address",
                    "1BoatSLRHtKNngkdXEeobR76b53LETtpyT".into(),
                ),
            )),
        );
        assert!(
            state.tampered,
            "editing a different row must not bypass a mismatch"
        );
        let original = state.recipients[0].address.value.clone();
        let _ = state.update(
            daemon.clone(),
            &Cache::default(),
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::RecipientEdited(0, "address", original),
            )),
        );
        assert!(
            state.tampered,
            "re-pasting the same request must remain blocked"
        );
        let _ = state.update(
            daemon.clone(),
            &Cache::default(),
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::FeerateEdited("3".into()),
            )),
        );
        assert!(state.tampered);
        assert!(!state.can_advance());
        let _ = state.update(
            daemon,
            &Cache::default(),
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::RecipientEdited(
                    0,
                    "address",
                    "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".into(),
                ),
            )),
        );
        assert!(!state.tampered);
        assert!(!state.can_advance());
    }

    #[tokio::test]
    async fn stale_preparation_cannot_replace_newer_review_or_restore_edited_identity() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        let old = state.preparation_id;
        state.invalidate_preparation();
        state.generating = true;
        let current = state.preparation_id;
        let _ = state.update(
            daemon.clone(),
            &Cache::default(),
            Message::VaultSpendPrepared {
                generation: old,
                result: Ok((prepared_outputs(&[1_000]), vec![])),
                identities: vec![(0, LookupResult::Tampered)],
            },
        );
        assert!(state.generating);
        assert!(state.generated.is_none());
        assert!(!state.tampered);
        let _ = state.update(
            daemon.clone(),
            &Cache::default(),
            Message::VaultSpendPrepared {
                generation: current,
                result: Ok((prepared_outputs(&[1_000]), vec![])),
                identities: vec![],
            },
        );
        assert!(state.can_advance());
        let _ = state.update(
            daemon.clone(),
            &Cache::default(),
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::RecipientEdited(0, "address", MAINNET_ADDR.into()),
            )),
        );
        assert!(!state.can_advance());
        let _ = state.update(
            daemon,
            &Cache::default(),
            Message::VaultSpendPrepared {
                generation: current,
                result: Ok((prepared_outputs(&[1_000]), vec![])),
                identities: vec![],
            },
        );
        assert!(state.generated.is_none());
    }

    #[tokio::test]
    async fn disabled_inflight_result_is_silent_and_cannot_reappear_after_reenable() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        let ticket = branta::test_ticket(true);
        let identity = branta::test_identity(&ticket, "Example merchant");
        state.lookup_ticket = Some(ticket.clone());
        state.generating = true;
        ticket.set_test_enabled(false);
        ticket.set_test_enabled(true);
        let _ = state.update(
            daemon,
            &Cache::default(),
            Message::VaultSpendPrepared {
                generation: state.preparation_id,
                result: Ok((prepared_outputs(&[1_000]), vec![])),
                identities: vec![(0, LookupResult::Identified(vec![identity]))],
            },
        );
        assert!(state.can_advance());
        assert!(state
            .recipient_identities
            .as_ref()
            .unwrap()
            .results
            .is_empty());
    }

    #[tokio::test]
    async fn out_of_order_results_are_attached_to_exact_prepared_outputs() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        state.recipients.push(bip21_recipient(2_000));
        let ticket = branta::test_ticket(true);
        let identity = branta::test_identity(&ticket, "Second recipient");
        state.lookup_ticket = Some(ticket);
        state.generating = true;
        let psbt = prepared_outputs(&[2_000, 1_000]);
        let txid = psbt.unsigned_tx.compute_txid();
        let _ = state.update(
            daemon,
            &Cache::default(),
            Message::VaultSpendPrepared {
                generation: state.preparation_id,
                result: Ok((psbt, vec![])),
                identities: vec![
                    (1, LookupResult::Identified(vec![identity])),
                    (0, LookupResult::Silent),
                ],
            },
        );
        let review = state.recipient_identities.as_ref().unwrap();
        assert_eq!(review.transaction_id, txid);
        assert_eq!(review.results[0].0, 0);
        assert_eq!(review.results[1].0, 1);
        assert!(matches!(review.results[0].1, LookupResult::Identified(_)));
    }
    #[test]
    fn mismatch_binding_survives_query_order_amount_label_and_address_case_edits() {
        let original = format!("bitcoin:{MAINNET_ADDR}?branta_id=x&branta_secret=y&amount=0.001");
        let reordered = format!(
            "bitcoin:{}?label=shop&amount=0.002&branta_secret=y&branta_id=x",
            MAINNET_ADDR.to_ascii_uppercase()
        );
        assert_eq!(
            destination_binding(&original),
            destination_binding(&reordered)
        );
        assert_eq!(
            destination_binding(&original),
            destination_binding(MAINNET_ADDR)
        );
    }
    #[tokio::test]
    async fn disabling_checks_and_rewriting_verification_fields_cannot_bypass_known_mismatch() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        let original = format!("bitcoin:{MAINNET_ADDR}?branta_id=abc+def=&branta_secret=1234");
        state.recipients[0].address.value = original.clone();
        let ticket = branta::test_ticket(true);
        state.lookup_ticket = Some(ticket.clone());
        state.generating = true;
        let _ = state.update(
            daemon.clone(),
            &Cache::default(),
            Message::VaultSpendPrepared {
                generation: state.preparation_id,
                result: Ok((prepared_outputs(&[1_000]), vec![])),
                identities: vec![(0, LookupResult::Tampered)],
            },
        );
        ticket.set_test_enabled(false);
        for request in [
            format!("bitcoin:{MAINNET_ADDR}?branta_id=abc%2Bdef%3D&branta_secret=1234"),
            format!("bitcoin:{MAINNET_ADDR}?branta_id=ignored&%62RANTA_ID=abc%2Bdef%3D&branta_secret=1234"),
            format!("{original}&branta_id=abc+def="),
            format!("{original}&branta_extra=unrelated"),
            format!("bitcoin:{MAINNET_ADDR}"),
            MAINNET_ADDR.to_string(),
        ] {
            let _ = state.update(daemon.clone(), &Cache::default(), Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::RecipientEdited(0, "address", request),
            )));
            assert!(state.form_values_are_valid(false));
            assert!(!state.exists_duplicate());
            assert!(state.tampered);
            assert!(!state.can_advance());
            let _ = state.update(daemon.clone(), &Cache::default(), Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::Generate,
            )));
            assert!(!state.generating);
            assert!(state.generated.is_none());
        }
        let _ = state.update(
            daemon.clone(),
            &Cache::default(),
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::RecipientEdited(
                    0,
                    "address",
                    "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".into(),
                ),
            )),
        );
        assert!(!state.tampered);
        let _ = state.update(
            daemon,
            &Cache::default(),
            Message::View(view::Message::CreateSpend(
                view::CreateSpendMessage::RecipientEdited(0, "address", original),
            )),
        );
        assert!(
            state.tampered,
            "reintroducing a blocked destination must restore the block"
        );
    }
    #[test]
    fn invalid_destinations_are_not_treated_as_duplicates() {
        let mut state = branta_form();
        state.recipients[0].address.value = "invalid first address".into();
        let mut second = bip21_recipient(2_000);
        second.address.value = "invalid second address".into();
        state.recipients.push(second);
        assert!(!state.exists_duplicate());
        state.recipients[1].address.value = "invalid first address".into();
        assert!(!state.exists_duplicate());
        state.recipients[0].address.value = MAINNET_ADDR.into();
        assert!(!state.exists_duplicate());
        state.recipients[1].address.value = format!("bitcoin:{MAINNET_ADDR}");
        assert!(state.exists_duplicate());
    }

    #[tokio::test]
    async fn recovery_and_send_max_accept_changed_snapshot_without_identity_metadata() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        for recovery in [false, true] {
            for enabled in [false, true] {
                let mut state = branta_form();
                state.send_max_to_recipient = Some(0);
                state.recovery_timelock = recovery.then_some(10);
                // This request has no explicit amount. The form's 1,000-sat
                // redraft snapshot must not constrain the daemon's final maximum.
                state.recipients[0].address.value =
                    format!("bitcoin:{MAINNET_ADDR}?branta_id=example&branta_secret=secret");
                let ticket = branta::test_ticket(enabled);
                let identity = branta::test_identity(&ticket, "Example recipient");
                state.lookup_ticket = enabled.then_some(ticket);
                state.generating = true;
                let psbt = prepared_outputs(&[2_000]);
                assert!(associate_recipients(&state.recipients, &psbt).is_none());
                assert!(validate_requested_amounts(&state.recipients, &psbt).is_ok());
                let _ = state.update(
                    daemon.clone(),
                    &Cache::default(),
                    Message::VaultSpendPrepared {
                        generation: state.preparation_id,
                        result: Ok((psbt, vec![])),
                        identities: vec![(0, LookupResult::Identified(vec![identity]))],
                    },
                );
                assert!(state.can_advance());
                assert!(state.warning.is_none());
                assert_eq!(
                    state.generated.as_ref().unwrap().0.unsigned_tx.output[0]
                        .value
                        .to_sat(),
                    2_000
                );
                assert!(state
                    .recipient_identities
                    .as_ref()
                    .is_none_or(|review| review.results.is_empty()));
            }
        }
    }

    #[tokio::test]
    async fn explicit_bip21_amount_constrains_actual_recovery_and_max_outputs_even_when_disabled() {
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        for recovery in [false, true] {
            for enabled in [false, true] {
                let mut state = branta_form();
                state.send_max_to_recipient = Some(0);
                state.recovery_timelock = recovery.then_some(10);
                state.lookup_ticket = enabled.then(|| branta::test_ticket(true));
                state.generating = true;
                let psbt = prepared_outputs(&[2_000]);
                assert!(validate_requested_amounts(&state.recipients, &psbt).is_err());
                let _ = state.update(
                    daemon.clone(),
                    &Cache::default(),
                    Message::VaultSpendPrepared {
                        generation: state.preparation_id,
                        result: Ok((psbt, vec![])),
                        identities: vec![],
                    },
                );
                assert!(!state.can_advance());
                assert!(state.generated.is_none());
                assert_eq!(
                    state.blocking_warning().as_deref(),
                    Some(AMOUNT_MISMATCH_WARNING)
                );
                assert!(
                    !state.tampered,
                    "amount disagreement is distinct from authenticated tampering"
                );
            }
        }
    }

    /// Preparation itself rejects a BIP21 amount mismatch. Its reason must
    /// become the compose hint, not the generic "Something went wrong" toast.
    #[tokio::test]
    async fn a_prepared_bip21_amount_mismatch_is_the_blocking_hint_not_a_generic_toast() {
        use iced::futures::StreamExt;
        let daemon = Arc::new(crate::daemon::client::Coincubed::new(
            crate::utils::mock::Daemon::new(vec![]).run(),
        ));
        let mut state = branta_form();
        state.generating = true;
        let task = state.update(
            daemon,
            &Cache::default(),
            Message::VaultSpendPrepared {
                generation: state.preparation_id,
                result: Err(Error::Unexpected(AMOUNT_MISMATCH_WARNING.to_string())),
                identities: vec![],
            },
        );
        if let Some(mut stream) = iced_runtime::task::into_stream(task) {
            while let Some(action) = stream.next().await {
                assert!(
                    !matches!(
                        action,
                        iced_runtime::Action::Output(Message::View(view::Message::ShowError(_)))
                    ),
                    "the mismatch must not be reported as a generic error"
                );
            }
        }
        assert!(!state.can_advance());
        assert_eq!(
            state.blocking_warning().as_deref(),
            Some(AMOUNT_MISMATCH_WARNING)
        );
        assert!(!state.tampered);
    }
}
