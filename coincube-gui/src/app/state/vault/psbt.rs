use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use coincube_ui::locale::copy as localized_copy;
use iced::Subscription;

use coincube_core::{
    border_wallet::{
        build_mnemonic, sign_psbt_with_border_wallet, sign_psbt_with_border_wallet_unified,
        CellRef, GridRecoveryPhrase, OrderedPattern, WordGrid, PATTERN_LENGTH,
    },
    descriptors::CoincubePolicy,
    miniscript::bitcoin::{bip32::Fingerprint, psbt::Psbt, secp256k1, Network, OutPoint, Txid},
    psbt_unified::UnifiedPsbt,
    unified_finalize::UnifiedFinalizeError,
};
use coincubed::commands::CoinStatus;
use iced::Task;
use zeroize::{Zeroize, Zeroizing};

use coincube_ui::component::form;
use coincube_ui::{widget::modal, widget::Element};

use crate::daemon::model::LabelsLoader;

static NEXT_PSBT_ACTION_SCOPE: AtomicU64 = AtomicU64::new(1);

/// Restore only a mounted PSBT control when no other tab or sheet owns focus.
fn restore_psbt_focus(target: iced::advanced::widget::Id) -> Task<Message> {
    use iced::advanced::widget::{
        operation::{focusable, Outcome},
        Id, Operation,
    };
    struct Restore {
        target: Id,
        mounted: bool,
        focused: bool,
    }
    impl Operation<()> for Restore {
        fn focusable(
            &mut self,
            id: Option<&Id>,
            _bounds: iced::Rectangle,
            state: &mut dyn focusable::Focusable,
        ) {
            self.mounted |= id == Some(&self.target);
            self.focused |= state.is_focused();
        }
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<()>)) {
            operate(self);
        }
        fn finish(&self) -> Outcome<()> {
            if self.mounted && !self.focused {
                Outcome::Chain(Box::new(focusable::focus(self.target.clone())))
            } else {
                Outcome::None
            }
        }
    }
    iced_runtime::task::effect(iced_runtime::Action::widget(Restore {
        target,
        mounted: false,
        focused: false,
    }))
}
use crate::export::{ImportExportMessage, ImportExportType, Progress};
use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::{Menu, VaultSubMenu},
        message::Message,
        state::vault::{
            label::{label_item_from_str, LabelsEdited},
            replay::{self, ReplayReview},
        },
        view,
        view::BorderWalletReconMessage,
        wallet::{Wallet, WalletError},
    },
    chain::ChainId,
    daemon::{
        model::{LabelItem, Labelled, SpendStatus, SpendTx},
        Daemon,
    },
    dir::CoincubeDirectory,
    hw::{HardwareWallet, HardwareWallets},
};

use super::export::VaultExportModal;

pub trait Modal {
    fn load(&self, _daemon: Arc<dyn Daemon + Sync + Send>) -> Task<Message> {
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _message: Message,
        _tx: &mut SpendTx,
    ) -> Task<Message> {
        Task::none()
    }

    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message>;
}

pub enum PsbtModal {
    Save(SaveModal),
    /// Unified signing picker. Owns local signers (HW, master, border) and
    /// — nested inside `SignModal` — the multi-signer Keychain flow. Keychain
    /// sessions run per-signer on demand; signatures returned by Keychain
    /// signers are merged into the same SpendTx via the daemon's
    /// `update_spend_tx`, so the picker can broadcast once every path
    /// threshold is met regardless of which signer types contributed.
    Sign(SignModal),
    /// Shown when a Keychain signer is selected but Connect isn't ready.
    /// Offers Connect sign-in or local Wi-Fi pairing as ways forward,
    /// instead of dead-ending on a technical "missing field" toast.
    KeychainUnavailable(KeychainUnavailableModal),
    Broadcast(BroadcastModal),
    Delete(DeleteModal),
    Export(VaultExportModal),
}

impl<'a> AsRef<dyn Modal + 'a> for PsbtModal {
    fn as_ref(&self) -> &(dyn Modal + 'a) {
        match &self {
            Self::Save(a) => a,
            Self::Sign(a) => a,
            Self::KeychainUnavailable(a) => a,
            Self::Broadcast(a) => a,
            Self::Delete(a) => a,
            Self::Export(a) => a,
        }
    }
}

impl<'a> AsMut<dyn Modal + 'a> for PsbtModal {
    fn as_mut(&mut self) -> &mut (dyn Modal + 'a) {
        match self {
            Self::Save(a) => a,
            Self::Sign(a) => a,
            Self::KeychainUnavailable(a) => a,
            Self::Broadcast(a) => a,
            Self::Delete(a) => a,
            Self::Export(a) => a,
        }
    }
}

/// Ephemeral identity metadata; never serialized with a PSBT or synchronized.
#[derive(Clone)]
pub struct RecipientIdentities {
    pub ticket: crate::services::branta::LookupTicket,
    pub transaction_id: Txid,
    pub results: Vec<(usize, crate::services::branta::LookupResult)>,
}

pub struct PsbtState {
    action_scope: u64,
    /// A direct signer choice may remove the focused row when its result lands.
    signer_focus_return: bool,
    pub wallet: Arc<Wallet>,
    pub desc_policy: Option<CoincubePolicy>,
    pub tx: SpendTx,
    pub saved: bool,
    pub warning: Option<Error>,
    pub labels_edited: LabelsEdited,
    recipient_identities: Option<RecipientIdentities>,
    pub modal: Option<PsbtModal>,
    /// The replay-protection review of this spend. `Some` only on a Bitcoin
    /// Blake2b Cube; Bitcoin-family readiness uses its ordinary finalizer.
    /// Recomputed from the verified witness after every
    /// signature merge ([`Self::refresh_replay`]).
    pub replay: Option<ReplayReview>,
    /// The spend screen's own re-check of a replayable spend's inputs
    /// against the twin chain (`#276` I13, cache lifecycle): a sync-time
    /// *not entangled* has a shelf life, so it is asked again at the moment
    /// it matters. See [`Self::revalidate_entanglement`].
    pub entangled_check: EntangledCheck,
    /// The PSBT digest the last re-check was started for, so each new set of
    /// signatures gets exactly one re-check.
    revalidated_for: Option<[u8; 32]>,
    /// Finalization is cryptographic work: reuse it only for the identical PSBT,
    /// including signatures, prevouts, scripts and transaction bytes.
    bitcoin_finalization: std::sync::Mutex<Option<([u8; 32], bool)>>,
    /// This screen's last re-qualification of the spend's split key. The key
    /// counts toward *Split*, and a recorded split may be signed or
    /// broadcast, only while it holds a fresh positive answer
    /// ([`SPLIT_KEY_CHECK_LIFETIME`]); a proof in the cache is never enough
    /// by itself, because nothing re-checks it after a reorg.
    split_key_check: SplitKeyCheck,
}

/// How long a positive split-key re-qualification admits signing and
/// broadcast. Past it the next message on the screen asks again.
pub const SPLIT_KEY_CHECK_LIFETIME: std::time::Duration = std::time::Duration::from_secs(120);
/// A failed re-qualification is asked again after this long, or at once when
/// the user presses Sign or Broadcast.
const SPLIT_KEY_RETRY: std::time::Duration = std::time::Duration::from_secs(30);

/// Shown while the split key is being re-qualified.
pub const SPLIT_KEY_CHECKING_COPY: &str =
    "Checking the split key on both chains before this split can be signed or sent…";
/// Shown when the last positive re-qualification has lapsed.
pub const SPLIT_KEY_STALE_COPY: &str =
    "The split key needs checking again before this split can be signed or sent.";

/// State of [`PsbtState::split_key_check`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum SplitKeyCheck {
    Idle,
    /// A re-qualification of `key` is running; only a reply for `generation`
    /// is accepted.
    InFlight {
        generation: u64,
        key: OutPoint,
    },
    /// The last re-qualification of `key` finished at `at`.
    Done {
        key: OutPoint,
        at: std::time::Instant,
        result: Result<(), String>,
    },
}

/// State of the spend screen's entanglement re-check ([`PsbtState::entangled_check`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntangledCheck {
    /// Not applicable (not replayable, or a Bitcoin-family Cube) or not yet
    /// started for the current signatures.
    Idle,
    /// These deposits are being asked about right now; Broadcast is disabled
    /// and says so until the answer lands. `generation` is the process-wide
    /// token the reply must carry to be accepted: a reply for an older
    /// generation — from an earlier instance of this screen, or from before
    /// a signature was added — never clears the current claim.
    InFlight { generation: u64, txids: Vec<Txid> },
    /// The re-check finished. `unresolved` are the deposits it could not get
    /// an answer for (Connect unreachable, no session, …): the acknowledgement
    /// path stays open and the copy says the check could not complete —
    /// blocking on *Unknown* would make a legacy-only spend impossible
    /// whenever Connect is down, which the brief rules out.
    Done { unresolved: Vec<Txid> },
}

impl EntangledCheck {
    pub fn in_flight(&self) -> bool {
        matches!(self, Self::InFlight { .. })
    }
}

/// Process-wide generation counter for [`EntangledCheck::InFlight`]. Global
/// rather than per screen so two instances of the same spend screen (close
/// and reopen) never hand out the same token.
static NEXT_CHECK_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_check_generation() -> u64 {
    NEXT_CHECK_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl PsbtState {
    pub fn new(wallet: Arc<Wallet>, mut tx: SpendTx, saved: bool) -> Self {
        let replay = wallet.chain.is_blake2b().then(|| {
            // `SpendTx::new` counted `partial_sigs` only; on BTCB2 the unified
            // records count too (see `replay::spend_info_for_chain`).
            if let Some(descriptor) = wallet.inheritance_descriptor() {
                if let Ok(sigs) = replay::spend_info_for_chain(wallet.chain, descriptor, &tx.psbt) {
                    tx.sigs = sigs;
                }
            }
            ReplayReview::new(&tx.psbt, &secp256k1::Secp256k1::verification_only())
        });
        Self {
            action_scope: NEXT_PSBT_ACTION_SCOPE.fetch_add(1, Ordering::Relaxed),
            signer_focus_return: false,
            desc_policy: wallet
                .inheritance_descriptor()
                .map(|descriptor| descriptor.policy()),
            wallet,
            labels_edited: LabelsEdited::default(),
            recipient_identities: None,
            warning: None,
            modal: None,
            tx,
            saved,
            replay,
            entangled_check: EntangledCheck::Idle,
            revalidated_for: None,
            bitcoin_finalization: std::sync::Mutex::new(None),
            split_key_check: SplitKeyCheck::Idle,
        }
    }

    /// The input that may spend a split key according to `cache` now: a coin
    /// with a proof to re-qualify ([`Cache::has_split_key_path`]), not seen
    /// on the twin chain; the recorded key of a journaled split first. A
    /// candidate only; see [`Self::split_key_input`].
    fn split_key_candidate(&self, cache: &Cache) -> Option<(usize, OutPoint)> {
        let spend = self.tx.psbt.unsigned_tx.compute_txid();
        let recorded = cache.recorded_split_key(spend);
        let eligible = |outpoint: &OutPoint| {
            cache.entanglement_of(&outpoint.txid)
                != crate::services::entangled::Entanglement::Entangled
                && cache.has_split_key_path(spend, *outpoint)
        };
        let inputs = || {
            self.tx
                .psbt
                .unsigned_tx
                .input
                .iter()
                .map(|input| input.previous_output)
                .enumerate()
        };
        inputs()
            .find(|(_, outpoint)| Some(*outpoint) == recorded && eligible(outpoint))
            .or_else(|| inputs().find(|(_, outpoint)| eligible(outpoint)))
    }

    /// The input that spends a split key now: a candidate this screen has
    /// re-qualified against both chains within [`SPLIT_KEY_CHECK_LIFETIME`].
    fn split_key_input(&self, cache: &Cache) -> Option<(usize, OutPoint)> {
        self.split_key_candidate(cache)
            .filter(|(_, key)| self.split_key_qualified(*key))
    }

    fn split_key_qualified(&self, key: OutPoint) -> bool {
        matches!(
            &self.split_key_check,
            SplitKeyCheck::Done { key: checked, at, result: Ok(()) }
                if *checked == key && at.elapsed() < SPLIT_KEY_CHECK_LIFETIME
        )
    }

    /// Why this spend may not be signed or broadcast yet because of its
    /// split key: a split this Cube recorded as using one is admitted only
    /// while the key is freshly re-qualified, on either chain.
    fn split_key_refusal(&self, cache: &Cache) -> Option<String> {
        let key = cache.recorded_split_key(self.tx.psbt.unsigned_tx.compute_txid())?;
        if self
            .split_key_input(cache)
            .is_some_and(|(_, input)| input == key)
        {
            return None;
        }
        Some(match &self.split_key_check {
            SplitKeyCheck::InFlight { key: checking, .. } if *checking == key => {
                SPLIT_KEY_CHECKING_COPY.to_string()
            }
            SplitKeyCheck::Done {
                key: checked,
                result: Err(reason),
                ..
            } if *checked == key => reason.clone(),
            _ if self.split_key_candidate(cache).is_none() => {
                "The split key is no longer a split key. Build the split again.".to_string()
            }
            _ => SPLIT_KEY_STALE_COPY.to_string(),
        })
    }

    /// Ask the app to re-qualify the split key when there is one and this
    /// screen has no fresh answer for it: on entering the screen, once a
    /// positive answer lapses, after [`SPLIT_KEY_RETRY`] for a failed one, or
    /// at once for a failed one when `retry` (Sign or Broadcast pressed).
    fn requalify_split_key(&mut self, cache: &Cache, retry: bool) -> Task<Message> {
        let Some((_, key)) = self.split_key_candidate(cache) else {
            return Task::none();
        };
        match &self.split_key_check {
            SplitKeyCheck::InFlight { key: checking, .. } if *checking == key => {
                return Task::none();
            }
            SplitKeyCheck::Done {
                key: checked,
                at,
                result,
            } if *checked == key => {
                let fresh = match result {
                    Ok(()) => at.elapsed() < SPLIT_KEY_CHECK_LIFETIME,
                    Err(_) => !retry && at.elapsed() < SPLIT_KEY_RETRY,
                };
                if fresh {
                    return Task::none();
                }
            }
            _ => {}
        }
        let generation = next_check_generation();
        self.split_key_check = SplitKeyCheck::InFlight { generation, key };
        Task::done(Message::SplitKeyCheck {
            spend: self.tx.psbt.unsigned_tx.compute_txid(),
            key,
            generation,
        })
    }

    /// Refuse a signer or broadcast action on a recorded split whose key is
    /// not freshly re-qualified: ask again, and say so. `None` admits it.
    fn refuse_for_split_key(&mut self, cache: &Cache) -> Option<Task<Message>> {
        let reason = self.split_key_refusal(cache)?;
        let requalify = self.requalify_split_key(cache, true);
        // A new check is now running; its answer says why if it fails.
        let reason = if matches!(self.split_key_check, SplitKeyCheck::InFlight { .. }) {
            SPLIT_KEY_CHECKING_COPY.to_string()
        } else {
            reason
        };
        Some(Task::batch([
            requalify,
            Task::done(Message::View(view::Message::ShowError(reason))),
        ]))
    }

    /// The signer picker is open on a recorded split whose key failed its
    /// re-qualification (or is no longer a split key): take it down, as the
    /// Broadcast dialog is, unless a local signer is mid-operation. Keychain
    /// sessions are cancelled once, including those of a picker hidden while
    /// a device signed; the dismissed picker may stay mounted to drain them.
    fn close_picker_if_split_key_refused(&mut self, cache: &Cache) -> Task<Message> {
        if matches!(self.split_key_check, SplitKeyCheck::InFlight { .. })
            || self.split_key_refusal(cache).is_none()
        {
            return Task::none();
        }
        let Some(PsbtModal::Sign(sign)) = self.modal.as_mut() else {
            return Task::none();
        };
        // A picker already dismissed is only draining its cancelled
        // Keychain sessions; dismissing it again would re-send every pending
        // cancel on each message. One merely hidden while a local signer
        // worked still has live sessions, so it is dismissed now.
        let dismissed =
            !sign.display_modal && sign.keychain.as_ref().is_some_and(|k| k.is_dismissed());
        if sign.is_signing() || dismissed {
            return Task::none();
        }
        let (cancel, keep) = sign.begin_dismiss();
        if !keep {
            self.modal = None;
        }
        cancel
    }

    /// Apply a re-qualification reply for the generation in flight; anything
    /// else is stale and ignored. A failure on a recorded split says why.
    fn apply_split_key_reply(
        &mut self,
        cache: &Cache,
        spend: Txid,
        key: OutPoint,
        generation: u64,
        result: Result<(), String>,
    ) -> Task<Message> {
        let current = matches!(
            &self.split_key_check,
            SplitKeyCheck::InFlight { generation: g, key: k } if *g == generation && *k == key
        );
        if !current || spend != self.tx.psbt.unsigned_tx.compute_txid() {
            return Task::none();
        }
        let failed = result.clone().err();
        self.split_key_check = SplitKeyCheck::Done {
            key,
            at: std::time::Instant::now(),
            result,
        };
        match failed {
            Some(reason) if cache.recorded_split_key(spend) == Some(key) => {
                Task::done(Message::View(view::Message::ShowError(reason)))
            }
            _ => Task::none(),
        }
    }

    /// The replay review as of `cache` now. A split key counts only while
    /// the cache holds it as one **and** this screen re-qualified it within
    /// [`SPLIT_KEY_CHECK_LIFETIME`]: a reorg, a twin sighting, a failed or
    /// lapsed check or a reload that drops its proof takes the *Split* status
    /// away, so the entangled-coin block applies again.
    pub fn current_review(&self, cache: &Cache) -> Option<std::borrow::Cow<'_, ReplayReview>> {
        let review = self.replay.as_ref()?;
        let key = self.split_key_input(cache);
        Some(if review.split_key() == key {
            std::borrow::Cow::Borrowed(review)
        } else {
            std::borrow::Cow::Owned(review.clone().with_split_key(key))
        })
    }

    pub(crate) fn action_scope(&self) -> u64 {
        self.action_scope
    }

    fn action_id(&self, name: &str) -> iced::advanced::widget::Id {
        view::vault::psbt::action_id(self.action_scope, name)
    }

    fn overview_focus_id(&self, cache: &Cache) -> iced::advanced::widget::Id {
        if self.tx.status != SpendStatus::Pending {
            return self.action_id("overview.copy-txid");
        }
        let sign = match &self.replay {
            None => !self.broadcast_ready(cache),
            Some(review) => !review.signatures_complete(&self.entangled_inputs(cache)),
        };
        self.action_id(if sign {
            "overview.sign"
        } else if self.broadcast_ready(cache) {
            "overview.broadcast"
        } else {
            // Replay acknowledgement, a recheck, or a Claim can leave
            // Broadcast visible but disabled. Copy Tx ID remains enabled.
            "overview.copy-txid"
        })
    }

    /// Recompute the replay review from the current (merged) PSBT. A no-op on
    /// a Bitcoin-family Cube. The acknowledgement survives only if the PSBT is
    /// byte-identical to the one it was given for
    /// ([`ReplayReview::refreshed`]).
    fn refresh_replay(&mut self) {
        if let Some(review) = &self.replay {
            self.replay =
                Some(review.refreshed(&self.tx.psbt, &secp256k1::Secp256k1::verification_only()));
        }
    }

    /// Re-check, at the moment it matters, the twin-chain entanglement of the
    /// inputs of a **replayable** spend that the cache does not hold as
    /// *Entangled* — a sync-time *not entangled* can go stale, and a never
    /// looked-up input deserves an answer before the user acknowledges
    /// anything. Runs once per set of signatures (keyed on the PSBT digest);
    /// on entering the screen it is kicked by the first message, and again
    /// whenever the status becomes replayable. The gate stays synchronous:
    /// while the check is in flight Broadcast is disabled and says it is
    /// checking; the answer arrives as [`Message::EntangledRevalidated`],
    /// which the app caches and routes back here. A *Protected* spend checks
    /// nothing — no answer could change it.
    fn revalidate_entanglement(&mut self, cache: &Cache) -> Task<Message> {
        let Some(review) = self.current_review(cache).map(std::borrow::Cow::into_owned) else {
            return Task::none();
        };
        if !review.status().needs_acknowledgement() {
            self.entangled_check = EntangledCheck::Idle;
            return Task::none();
        }
        let digest = review.psbt_digest();
        if self.revalidated_for == Some(digest) {
            return Task::none();
        }
        let mut txids: Vec<Txid> = self
            .tx
            .psbt
            .unsigned_tx
            .input
            .iter()
            .map(|txin| txin.previous_output.txid)
            .filter(|txid| {
                !matches!(
                    cache.entanglement_of(txid),
                    crate::services::entangled::Entanglement::Entangled
                )
            })
            .collect();
        txids.sort();
        txids.dedup();
        if txids.is_empty() {
            self.revalidated_for = Some(digest);
            self.entangled_check = EntangledCheck::Done {
                unresolved: Vec::new(),
            };
            return Task::none();
        }
        let Some(tokens) = cache.connect_tokens.clone() else {
            // No Connect session: the check cannot run. Say so; the
            // acknowledgement path stays open. `revalidated_for` is left
            // unset, so the check runs once a session arrives on this same
            // screen instead of staying "could not check" for good.
            self.entangled_check = EntangledCheck::Done { unresolved: txids };
            return Task::none();
        };
        // Recorded only now that the check can run.
        self.revalidated_for = Some(digest);
        let generation = next_check_generation();
        self.entangled_check = EntangledCheck::InFlight {
            generation,
            txids: txids.clone(),
        };
        let chain = self.wallet.chain;
        let origin = crate::app::cache::LookupOrigin {
            app: cache.app_generation,
            chain,
        };
        let spend = self.tx.psbt.unsigned_tx.compute_txid();
        Task::perform(
            async move {
                let mut client = crate::services::coincube::CoincubeClient::new();
                let access_token = tokens.read().await.access_token.clone();
                client.set_token(&access_token);
                crate::services::entangled::lookup_all(client, chain, txids).await
            },
            move |answers| Message::EntangledRevalidated {
                origin,
                spend,
                generation,
                answers,
            },
        )
    }

    /// Apply a re-check reply: only if it answers the generation currently in
    /// flight for this spend. Anything else is stale — an older screen
    /// instance, or an earlier signature set — and is ignored here. The
    /// cache is the app's concern, settled before the reply was routed: every
    /// resolved answer is recorded there under its own observation instant,
    /// so this screen has nothing to hand back.
    fn apply_entangled_reply(
        &mut self,
        spend: Txid,
        generation: u64,
        answers: &[crate::services::entangled::LookupAnswer],
    ) {
        let current = match &self.entangled_check {
            EntangledCheck::InFlight { generation, .. } => *generation,
            _ => return,
        };
        if generation != current || spend != self.tx.psbt.unsigned_tx.compute_txid() {
            return;
        }
        let unresolved = answers
            .iter()
            .filter(|reply| !reply.answer.is_resolved())
            .map(|reply| reply.txid)
            .collect();
        self.entangled_check = EntangledCheck::Done { unresolved };
    }

    /// The Broadcast dialog is open on a spend that is no longer ready — a
    /// lookup landed, a re-check started, a signature changed since the
    /// gated click that opened it. Close it back to the spend screen, where
    /// the pill names the reason, and say why. No path dispatches
    /// `broadcast_spend_tx` ungated: [`Self::update_inner`] intercepts Confirm
    /// with the same check, and the dialog is never opened unready.
    fn close_broadcast_dialog_if_not_ready(&mut self, cache: &Cache) -> Task<Message> {
        let awaiting = matches!(
            &self.modal,
            Some(PsbtModal::Broadcast(dialog)) if dialog.awaiting_confirmation()
        );
        if !awaiting || self.broadcast_ready(cache) {
            return Task::none();
        }
        self.modal = None;
        match self.not_ready_reason(cache) {
            Some(reason) => Task::done(Message::View(view::Message::ShowError(reason))),
            None => Task::none(),
        }
    }

    /// Why this spend is not ready for a broadcast attempt.
    fn not_ready_reason(&self, cache: &Cache) -> Option<String> {
        if let Some(reason) = self.split_key_refusal(cache) {
            return Some(reason);
        }
        let Some(review) = self.current_review(cache) else {
            return (!self.bitcoin_signatures_complete()).then(||
                localized_copy("This transaction needs valid signatures for every input before it can be broadcast.").to_string()
            );
        };
        Some(replay::not_ready_reason(
            &review,
            &self.entangled_inputs(cache),
            self.entangled_check.in_flight(),
        ))
    }

    /// Inputs (by index) whose re-check could not get an answer, for the
    /// "could not complete" copy.
    fn unresolved_inputs(&self) -> Vec<usize> {
        let EntangledCheck::Done { unresolved } = &self.entangled_check else {
            return Vec::new();
        };
        self.tx
            .psbt
            .unsigned_tx
            .input
            .iter()
            .enumerate()
            .filter(|(_, txin)| unresolved.contains(&txin.previous_output.txid))
            .map(|(index, _)| index)
            .collect()
    }

    /// Which inputs spend an entangled (or unchecked) deposit, resolved from
    /// the cache **now** — never frozen into the review at signature time, so
    /// an I13 lookup that lands after the last signature still tightens the
    /// gate. Empty on a Bitcoin-family Cube.
    fn entangled_inputs(
        &self,
        cache: &Cache,
    ) -> Vec<(usize, crate::services::entangled::Entanglement)> {
        if self.replay.is_none() {
            return Vec::new();
        }
        replay::entangled_inputs(&self.tx.psbt, |txid| cache.entanglement_of(txid))
    }

    /// Presence-based signature counts are progress hints, not authorization.
    /// Verify the Bitcoin witness using the same finalizer as the daemon and
    /// the interpreter-backed extractor, without mutating the stored PSBT.
    fn bitcoin_signatures_complete(&self) -> bool {
        use coincube_core::miniscript::psbt::PsbtExt;
        if self.wallet.chain.is_blake2b() || self.tx.path_ready().is_none() {
            return false;
        }
        let digest = replay::psbt_digest(&self.tx.psbt);
        let mut cached = self
            .bitcoin_finalization
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some((previous, ready)) = *cached {
            if previous == digest {
                return ready;
            }
        }
        let secp = secp256k1::Secp256k1::verification_only();
        let mut candidate = self.tx.psbt.clone();
        let ready = candidate.finalize_mut(&secp).is_ok() && candidate.extract(&secp).is_ok();
        *cached = Some((digest, ready));
        ready
    }

    /// Whether this spend may be broadcast now — the one definition the
    /// Broadcast handler and the view share ([`replay::broadcast_ready`]):
    /// verified finalization on a Bitcoin-family Cube; on Bitcoin Blake2b the
    /// unified finalizer's verdict, the I13 requirement on known-entangled inputs,
    /// and the acknowledgement. On either chain a recorded split-key split
    /// also needs a fresh re-qualification of its key.
    pub fn broadcast_ready(&self, cache: &Cache) -> bool {
        replay::broadcast_ready(
            self.bitcoin_signatures_complete(),
            self.current_review(cache).as_deref(),
            &self.entangled_inputs(cache),
        ) && !self.entangled_check.in_flight()
            && self.split_key_refusal(cache).is_none()
    }

    pub fn with_recipient_identities(mut self, identities: Option<RecipientIdentities>) -> Self {
        self.recipient_identities = identities;
        self
    }

    pub fn recipient_identities(&self) -> &[(usize, crate::services::branta::LookupResult)] {
        self.recipient_identities
            .as_ref()
            .filter(|review| {
                review.ticket.is_current()
                    && review.transaction_id == self.tx.psbt.unsigned_tx.compute_txid()
            })
            .map(|review| review.results.as_slice())
            .unwrap_or(&[])
    }

    pub fn interrupt(&mut self) {
        self.modal = None;
        self.signer_focus_return = false;
    }

    /// Single authority for reflecting collected signatures. Recomputes
    /// `tx.sigs` from the authoritative merged `tx.psbt`, refreshes the Sign
    /// picker's per-key "Signed" set from the *counted* signers (so a row can
    /// never show Signed without being counted), and closes the picker once a
    /// spending path is satisfied (or it was dismissed and its Keychain
    /// sessions drained). Idempotent — safe to call after every signature
    /// merge, decoupled from the persist round-trip.
    fn reconcile_and_maybe_close(&mut self, cache: &Cache) -> Task<Message> {
        if let Some(descriptor) = self.wallet.inheritance_descriptor() {
            if let Ok(sigs) =
                replay::spend_info_for_chain(self.wallet.chain, descriptor, &self.tx.psbt)
            {
                self.tx.sigs = sigs;
            }
        }
        self.refresh_replay();
        let recheck = self.revalidate_entanglement(cache);
        // Derive the picker's "Signed" indicator from the counted signers so
        // the per-key rows and the "X of N collected" badge can't diverge.
        let counted = self.tx.signers();
        let entangled = self.entangled_inputs(cache);
        let bitcoin_complete = self.bitcoin_signatures_complete();
        let close = match self.modal.as_mut() {
            Some(PsbtModal::Sign(sign)) => {
                sign.set_counted_signers(counted);
                // Threshold close waits for any Keychain signature that is
                // merged in memory but not yet durably persisted — otherwise a
                // slow or failing persist would tear the picker down before the
                // daemon holds the signature it must broadcast (and before the
                // failing row could be marked Failed). Dismissal-driven close
                // has its own drain gate and is unaffected.
                // On Bitcoin Blake2b "path satisfied" is the finaliser's
                // verdict over verified signatures, not the partial-sig count,
                // and a known-entangled input left replayable keeps the picker
                // open so its required replay-capable signature can be added
                // (`ReplayReview::signatures_complete`); the acknowledgement is
                // asked for at Broadcast, not here.
                let path_satisfied = match &self.replay {
                    None => bitcoin_complete,
                    Some(review) => review.signatures_complete(&entangled),
                };
                (path_satisfied && !sign.keychain_persistence_pending())
                    || sign.should_close_after_dismiss()
            }
            _ => false,
        };
        if close {
            // Best-effort: cancel any Keychain sessions still in flight so they
            // don't outlive the closed picker.
            let extra = if let Some(PsbtModal::Sign(sign)) = self.modal.as_mut() {
                sign.cancel_keychain_if_active()
            } else {
                Task::none()
            };
            self.modal = None;
            return Task::batch([extra, recheck]);
        }
        recheck
    }

    pub fn subscription(&self) -> Subscription<Message> {
        if let Some(modal) = &self.modal {
            modal.as_ref().subscription()
        } else {
            Subscription::none()
        }
    }

    pub fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Task<Message> {
        if let Some(modal) = &self.modal {
            modal.as_ref().load(daemon)
        } else {
            Task::none()
        }
    }

    pub fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let was_picker = matches!(&self.modal, Some(PsbtModal::Sign(sign)) if sign.display_modal);
        let was_unavailable = matches!(self.modal, Some(PsbtModal::KeychainUnavailable(_)));
        let opened_picker = matches!(
            &message,
            Message::View(view::Message::Spend(view::SpendTxMessage::Sign))
        );
        let dismissed = matches!(
            &message,
            Message::View(view::Message::Spend(view::SpendTxMessage::Cancel))
        );
        let direct_signer = matches!(
            &message,
            Message::View(view::Message::SelectPsbtHardwareWallet(_))
                | Message::View(view::Message::Spend(
                    view::SpendTxMessage::SelectMasterSigner
                        | view::SpendTxMessage::SelectBorderWallet(_)
                        | view::SpendTxMessage::SelectKeychainSigner(_)
                        | view::SpendTxMessage::RequestFromEveryone
                        | view::SpendTxMessage::RetryKeychainSigner(_)
                ))
        );
        if direct_signer && was_picker {
            self.signer_focus_return = true;
        }
        if dismissed {
            self.signer_focus_return = false;
        }
        // A recorded split is signed and broadcast only with its key freshly
        // re-qualified: opening or re-showing the picker, every signer pick,
        // the Border Wallet wizard's Next (its last step signs; editing the
        // phrase or pattern is not gated) and Broadcast. Confirm is gated
        // through `broadcast_ready`.
        let admission = direct_signer
            || matches!(
                &message,
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::Sign | view::SpendTxMessage::Broadcast
                ))
            )
            || matches!(
                &message,
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::BorderWalletRecon(BorderWalletReconMessage::Next)
                ))
            );
        if admission {
            if let Some(refused) = self.refuse_for_split_key(cache) {
                return refused;
            }
        }
        // Order matters: the message is applied **before** any new re-check
        // is kicked, so a pass can never start a check and then resolve it
        // with an older answer. A reply is accepted only for the generation
        // currently in flight ([`Self::apply_entangled_reply`]).
        let task = match message {
            Message::EntangledRevalidated {
                spend,
                generation,
                ref answers,
                ..
            } => {
                self.apply_entangled_reply(spend, generation, answers);
                Task::none()
            }
            Message::SplitKeyChecked {
                spend,
                key,
                generation,
                result,
            } => self.apply_split_key_reply(cache, spend, key, generation, result),
            message => self.update_inner(daemon, cache, message),
        };
        // Likewise the split key: re-qualified on entering the screen and
        // when its answer lapses; a picker it no longer admits comes down.
        let requalify = self.requalify_split_key(cache, false);
        let picker = self.close_picker_if_split_key_refused(cache);
        // Entering the screen: the first message through here starts the
        // entanglement re-check of a replayable spend (once per signature
        // set; a no-op otherwise).
        let recheck = self.revalidate_entanglement(cache);
        // And whatever just happened, a Broadcast dialog open on a spend that
        // is no longer ready comes down with the reason.
        let dialog = self.close_broadcast_dialog_if_not_ready(cache);
        let focus = if opened_picker {
            match &self.modal {
                Some(PsbtModal::Sign(sign)) if sign.display_modal => restore_psbt_focus(
                    view::vault::psbt::action_id(sign.action_scope, "picker.close"),
                ),
                _ => Task::none(),
            }
        } else if direct_signer {
            match &self.modal {
                Some(PsbtModal::KeychainUnavailable(_)) => {
                    restore_psbt_focus(self.action_id("keychain-unavailable.connect"))
                }
                Some(PsbtModal::Sign(sign))
                    if sign.display_modal && sign.border_wallet_recon.is_none() =>
                {
                    restore_psbt_focus(view::vault::psbt::action_id(
                        sign.action_scope,
                        "picker.close",
                    ))
                }
                _ => Task::none(),
            }
        } else if (dismissed && (was_picker || was_unavailable))
            || (was_picker && self.modal.is_none() && std::mem::take(&mut self.signer_focus_return))
        {
            restore_psbt_focus(self.overview_focus_id(cache))
        } else {
            Task::none()
        };
        Task::batch([task, requalify, picker, recheck, dialog, focus])
    }

    fn update_inner(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if self
            .recipient_identities
            .as_ref()
            .is_some_and(|review| !review.ticket.is_current())
        {
            self.recipient_identities = None;
        }
        if let Message::View(view::Message::OpenVaultRecipientIdentity(output, identity_index)) =
            &message
        {
            if let Some((_, crate::services::branta::LookupResult::Identified(identities))) = self
                .recipient_identities()
                .iter()
                .find(|(vout, _)| vout == output)
            {
                if let Some(identity) = identities.get(*identity_index) {
                    identity.open();
                }
            }
            return Task::none();
        }
        match message {
            Message::View(view::Message::ExportPsbt) => {
                if self.modal.is_none() {
                    if let Err(reason) =
                        enforce_legacy_signature_policy(self.wallet.chain, &self.tx.psbt)
                    {
                        let message = replay::unsafe_legacy_policy_copy(&reason)
                            .map(str::to_owned)
                            .unwrap_or_else(|| {
                                crate::user_error::report(&Error::Unexpected(reason.to_string()))
                            });
                        let error = Error::Unexpected(reason.to_string());
                        self.warning = Some(error);
                        return Task::done(Message::View(view::Message::ShowError(message)));
                    }
                    let psbt_str = self.tx.psbt.to_string();
                    let modal = VaultExportModal::new(None, ImportExportType::ExportPsbt(psbt_str));
                    let launch = modal.launch(true);
                    self.modal = Some(PsbtModal::Export(modal));
                    return launch;
                }
            }
            Message::View(view::Message::ImportPsbt) => {
                if self.modal.is_none() {
                    let modal = VaultExportModal::new(
                        Some(daemon.clone()),
                        ImportExportType::ImportPsbt(Some(self.tx.psbt.unsigned_tx.compute_txid())),
                    );
                    let launch = modal.launch(false);
                    self.modal = Some(PsbtModal::Export(modal));
                    return launch;
                }
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Close)) => {
                if matches!(self.modal, Some(PsbtModal::Export(_))) {
                    self.modal = None;
                }
            }
            Message::View(view::Message::ImportExport(m)) => {
                if let Some(PsbtModal::Export(modal)) = self.modal.as_mut() {
                    return modal.update(m);
                }
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Cancel)) => {
                // Dismissing the unified signing picker. A local
                // hardware/master/border op in flight just hides the picker
                // (keeping the device-confirmation state). Otherwise cancel
                // any in-flight Keychain sessions server-side — otherwise
                // signers keep seeing the request until its 24h TTL elapses.
                // `cancel_all()` cancels sessions that already have an id;
                // entries whose `CreateSigningSession` RPC is still in flight
                // are cancelled later once their `SessionCreated` lands, which
                // needs the picker to stay mounted. So when sessions are still
                // undrained we keep the picker mounted-but-hidden — it
                // self-closes via `Message::Updated(Ok)` once they all reach a
                // terminal state (see `should_close_after_dismiss`).
                if let Some(PsbtModal::Sign(sign)) = &mut self.modal {
                    if sign.is_signing() {
                        // A local device op is awaiting confirmation — just
                        // hide the picker, keeping it mounted so the op's
                        // result still lands. Any keychain sessions keep
                        // running and are drained on the next close.
                        sign.display_modal = false;
                        return Task::none();
                    }
                    let (cancel, keep) = sign.begin_dismiss();
                    if !keep {
                        self.modal = None;
                    }
                    return cancel;
                }

                // Any other modal (save / broadcast / export / keychain
                // unavailable): dismissing just closes it.
                self.modal = None;
                // After a successful broadcast we keep the user on the
                // PSBT detail page rather than reloading back to the
                // list: the daemon derives `SpendStatus::Broadcast` from
                // `coin.spend_info` (filled in by its mempool poller),
                // so an immediate reload would overwrite our optimistic
                // `Broadcast` status with stale `Pending` and re-expose
                // the Broadcast button. The list view stays consistent
                // because `BroadcastModal` calls `Wallet::record_broadcast`
                // on RPC success, and `PsbtsPanel`'s `SpendTxs` handler
                // runs `Wallet::apply_spend_tx_overrides` on every
                // refresh to promote matching Pending entries to
                // Broadcast.
                return Task::none();
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::KeychainConnectSignIn)) => {
                // From the "Keychain needs Connect" modal: close it and
                // hand off to the Connect sign-in flow (handled at the
                // tab level, which jumps to the Home tab when needed).
                self.modal = None;
                return Task::done(Message::View(view::Message::OpenConnectSignIn));
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::KeychainEnsureConnect)) => {
                // From the modal when already signed in: close it and run
                // the on-demand Connect bootstrap so the signing stream
                // comes up. `EnsureConnectReady` is the single owner of
                // missing-cube registration, avoiding concurrent registration
                // RPCs from this modal and the app bootstrap.
                self.modal = None;
                return Task::done(Message::EnsureConnectReady);
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::KeychainPairPhone)) => {
                // From the "Keychain needs Connect" modal: close it and
                // navigate to Vault → Settings → Pair so the user can
                // pair a phone over Wi-Fi and sign locally.
                self.modal = None;
                return Task::done(Message::View(view::Message::Menu(Menu::Vault(
                    VaultSubMenu::Settings(Some(crate::app::menu::SettingsOption::LocalSigning)),
                ))));
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Delete)) => {
                self.modal = Some(PsbtModal::Delete(DeleteModal::default()));
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Sign)) => {
                if let Some(PsbtModal::Sign(SignModal { display_modal, .. })) = &mut self.modal {
                    *display_modal = true;
                    return Task::none();
                }
                self.signer_focus_return = false;

                // Build (and launch) the nested Keychain flow up front when
                // Connect is ready, so keychain rows populate with friendly
                // labels by the time the user reads the picker. When Connect
                // isn't ready this is `None` and the picker shows keychain
                // rows derived from the descriptor, disabled with a hint.
                let (keychain, kc_launch) =
                    build_keychain_if_ready(cache, &self.wallet, &self.tx.psbt);
                let modal = SignModal::new(
                    self.tx.signers(),
                    self.wallet.clone(),
                    cache.datadir_path.clone(),
                    cache.network,
                    self.saved,
                    self.tx.recovery_timelock(),
                    keychain,
                    true,
                )
                .with_connect_account(ConnectAccountStatus::from_cache(cache));
                let cmd = modal.load(daemon);
                self.modal = Some(PsbtModal::Sign(modal));
                return Task::batch([cmd, kc_launch]);
            }
            Message::View(view::Message::Spend(
                view::SpendTxMessage::SelectKeychainSigner(_)
                | view::SpendTxMessage::RequestFromEveryone,
            )) => {
                // Connect-readiness check, moved here from the old
                // button-press: only when the user actually selects a
                // Keychain signer do we require Connect. If it isn't ready,
                // surface the sign-in / pair-a-phone modal (replacing the
                // picker) instead of dead-ending on a technical error.
                let missing = keychain_connect_missing(cache);
                if !missing.is_empty() {
                    tracing::info!(
                        "Keychain signer selected but Connect not ready (missing {})",
                        missing.join(", ")
                    );
                    self.modal = Some(PsbtModal::KeychainUnavailable(KeychainUnavailableModal {
                        signed_in: connect_session_available(cache),
                        action_scope: self.action_scope,
                    }));
                    self.signer_focus_return = false;
                    return Task::none();
                }
                // Connect is ready. If the picker was opened before Connect
                // came up, its nested Keychain flow is still `None` — build and
                // launch it now (checked without holding a mutable borrow so
                // `build_keychain_if_ready` can read `self.wallet`/`self.tx`).
                // This click just kicks off the resolve; the keychain rows
                // become actionable once it returns.
                let needs_init = matches!(
                    &self.modal,
                    Some(PsbtModal::Sign(sign)) if sign.keychain_needs_init()
                );
                if needs_init {
                    let (keychain, launch) =
                        build_keychain_if_ready(cache, &self.wallet, &self.tx.psbt);
                    if let Some(PsbtModal::Sign(sign)) = self.modal.as_mut() {
                        sign.set_keychain(keychain);
                    }
                    return launch;
                }
                // Ready and initialized: forward to the picker's nested flow.
                if let Some(modal) = self.modal.as_mut() {
                    return modal.as_mut().update(daemon.clone(), message, &mut self.tx);
                }
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::AcknowledgeReplay(
                acknowledged,
            ))) => {
                if let Some(review) = self.replay.as_mut() {
                    review.set_acknowledged(acknowledged);
                }
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Broadcast)) => {
                // The button is disabled until `broadcast_ready`; this is the
                // same gate for anything that reaches the handler another way.
                if !self.broadcast_ready(cache) {
                    return Task::none();
                }
                let outpoints: Vec<_> = self.tx.coins.keys().cloned().collect();
                return Task::perform(
                    async move {
                        daemon
                            .list_coins(&[CoinStatus::Spending], &outpoints)
                            .await
                            .map(|res| {
                                res.coins
                                    .iter()
                                    .filter_map(|c| c.spend_info.map(|info| info.txid))
                                    .collect()
                            })
                            .map_err(|e| e.into())
                    },
                    Message::BroadcastModal,
                );
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::Save)) => {
                self.modal = Some(PsbtModal::Save(SaveModal::default()));
            }
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    std::iter::once(&mut self.tx).map(|tx| tx as &mut dyn LabelsLoader),
                ) {
                    Ok(cmd) => {
                        return cmd;
                    }
                    Err(e) => {
                        let err_msg = crate::user_error::report(&e);
                        self.warning = Some(e);
                        return Task::done(Message::View(view::Message::ShowError(err_msg)));
                    }
                };
            }
            Message::Reconcile => {
                // UI-only: recompute collected signatures and maybe close the
                // picker after an in-memory merge. Unlike `Updated(Ok)` this
                // does NOT set `saved` — the persist may still be in flight or
                // may fail, and marking the tx saved here would wrongly enable
                // Export/Delete on a never-persisted spend.
                return self.reconcile_and_maybe_close(cache);
            }
            Message::Updated(Ok(_)) => {
                self.saved = true;
                if let Some(modal) = self.modal.as_mut() {
                    // Let the modal run its own side-effects (e.g. Keychain
                    // bookkeeping), then reconcile the collected signatures and
                    // decide whether to close. The recompute/close/close-after-
                    // dismiss logic lives in `reconcile_and_maybe_close` so
                    // every merge site (local sign, Keychain merge, dismissal
                    // drain) drives the same idempotent path regardless of
                    // persist ordering.
                    let cmd = modal.as_mut().update(daemon.clone(), message, &mut self.tx);
                    return Task::batch([cmd, self.reconcile_and_maybe_close(cache)]);
                }
            }
            Message::BroadcastModal(res) => match res {
                Ok(conflicting_txids) => {
                    // The click that asked for this dialog was gated, but the
                    // `list_coins` round trip is a window: a lookup landing in
                    // it must not open a dialog whose Confirm the gate would
                    // then refuse. Re-check now, and say why if it moved.
                    if !self.broadcast_ready(cache) {
                        return match self.not_ready_reason(cache) {
                            Some(reason) => {
                                Task::done(Message::View(view::Message::ShowError(reason)))
                            }
                            None => Task::none(),
                        };
                    }
                    use coincube_ui::component::amount::DisplayAmount;
                    let is_self_transfer = self.tx.is_send_to_self();
                    // For a self-transfer every output is change, so `spend_amount`
                    // is 0 by construction. Showing "0 has been sent" is nonsense;
                    // surface the total moved (sum of change outputs) instead.
                    let display_amount = if is_self_transfer {
                        self.tx
                            .psbt
                            .unsigned_tx
                            .output
                            .iter()
                            .enumerate()
                            .filter_map(|(i, o)| {
                                if self.tx.change_indexes.contains(&i) {
                                    Some(o.value)
                                } else {
                                    None
                                }
                            })
                            .sum()
                    } else {
                        self.tx.spend_amount
                    };
                    // Include the unit label ("SATS" / "BTC") so the success
                    // screen reads e.g. "149,794 SATS has been sent successfully"
                    // rather than a bare number.
                    let amount_display = format!(
                        "{} {}",
                        display_amount.to_formatted_string_with_unit(cache.bitcoin_unit),
                        cache.bitcoin_unit.label(),
                    );
                    self.modal = Some(PsbtModal::Broadcast(BroadcastModal {
                        conflicting_txids,
                        broadcast: false,
                        broadcasting: false,
                        error: None,
                        sent_quote: coincube_ui::component::quote_display::random_quote(
                            "bitcoin-send",
                        ),
                        sent_image_handle:
                            coincube_ui::component::quote_display::image_handle_for_context(
                                "bitcoin-send",
                            ),
                        spend_amount_display: amount_display,
                        fee_display: self.tx.fee_amount.map(|fee| {
                            format!(
                                "{} {}",
                                fee.to_formatted_string_with_unit(cache.bitcoin_unit),
                                cache.bitcoin_unit.label(),
                            )
                        }),
                        txid: self.tx.psbt.unsigned_tx.compute_txid(),
                        is_self_transfer,
                        wallet: self.wallet.clone(),
                        recipient_identities: self.recipient_identities.clone().filter(|review| {
                            review.ticket.is_current()
                                && review.transaction_id == self.tx.psbt.unsigned_tx.compute_txid()
                        }),
                        recipient_outputs: self.tx.psbt.unsigned_tx.output.clone(),
                        network: cache.network,
                        bitcoin_unit: cache.bitcoin_unit,
                        theme_mode: cache.theme_mode,
                    }));
                }
                Err(e) => {
                    let err_msg = crate::user_error::report(&e);
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },
            Message::Export(ImportExportMessage::Progress(Progress::Psbt(psbt))) => {
                if let Err(e) =
                    merge_signatures_for_chain(self.wallet.chain, &mut self.tx.psbt, &psbt)
                {
                    let err_msg = replay::unsafe_legacy_policy_copy(&e)
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            crate::user_error::report(&Error::Unexpected(format!(
                                "Couldn't merge the imported PSBT: {e}"
                            )))
                        });
                    let e = Error::Unexpected(format!("Couldn't merge the imported PSBT: {e}"));
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
                let Some(descriptor) = self.wallet.inheritance_descriptor() else {
                    return Task::done(Message::View(view::Message::ShowError(
                        "Use the imported wallet signer for this PSBT.".to_string(),
                    )));
                };
                self.tx.sigs =
                    replay::spend_info_for_chain(self.wallet.chain, descriptor, &self.tx.psbt)
                        .expect("already checked in PSBT import logic");
                self.refresh_replay();
            }
            // Final dispatch is gated on the **current** cache, not on the
            // click that opened the dialog: a lookup that lands, a re-check
            // that starts or a signature that changes in between must not be
            // able to reach `broadcast_spend_tx`. Same gate as the Broadcast
            // arm above; on refusal the dialog closes and the reason is shown.
            Message::View(view::Message::Spend(view::SpendTxMessage::Confirm))
                if matches!(self.modal, Some(PsbtModal::Broadcast(_))) =>
            {
                if !self.broadcast_ready(cache) {
                    return self.close_broadcast_dialog_if_not_ready(cache);
                }
                if let Some(modal) = self.modal.as_mut() {
                    return modal.as_mut().update(daemon.clone(), message, &mut self.tx);
                }
            }
            _ => {
                if let Some(modal) = self.modal.as_mut() {
                    return modal.as_mut().update(daemon.clone(), message, &mut self.tx);
                }
            }
        }
        Task::none()
    }

    /// The replay pill's inputs for the view: the review plus which inputs
    /// spend an entangled (or unchecked) deposit, resolved from the cache at
    /// render time. `None` on a Bitcoin-family Cube.
    pub fn replay_presentation(&self, cache: &Cache) -> Option<view::vault::psbt::ReplayPill<'_>> {
        self.current_review(cache)
            .map(|review| view::vault::psbt::ReplayPill {
                review,
                entangled: self.entangled_inputs(cache),
                broadcast_ready: self.broadcast_ready(cache),
                checking: self.entangled_check.in_flight(),
                unresolved: self.unresolved_inputs(),
            })
    }

    pub fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let Some(policy) = self.desc_policy.as_ref() else {
            return iced::widget::text(localized_copy(
                "Use the imported wallet signing screen for this PSBT.",
            ))
            .into();
        };
        let content = view::vault::psbt::psbt_view(
            self.action_scope,
            cache,
            &self.tx,
            self.saved,
            policy,
            &self.wallet.keys_aliases,
            self.labels_edited.cache(),
            cache.network,
            if let Some(PsbtModal::Sign(m)) = &self.modal {
                m.is_signing()
            } else {
                false
            },
            cache.bitcoin_unit,
            self.replay_presentation(cache),
            self.broadcast_ready(cache),
            self.recipient_identities(),
        );
        if let Some(modal) = &self.modal {
            modal.as_ref().view(content)
        } else {
            content
        }
    }
}

#[derive(Default)]
pub struct SaveModal {
    saved: bool,
    error: Option<Error>,
}

impl Modal for SaveModal {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::Confirm)) => {
                let daemon = daemon.clone();
                let psbt = tx.psbt.clone();
                let mut labels = HashMap::<LabelItem, Option<String>>::new();
                for (item, label) in tx.labels() {
                    if !label.is_empty() {
                        labels.insert(label_item_from_str(item), Some(label.clone()));
                    }
                }
                return Task::perform(
                    async move {
                        daemon.update_spend_tx(&psbt).await?;
                        daemon.update_labels(&labels).await.map_err(|e| e.into())
                    },
                    Message::Updated,
                );
            }
            Message::Updated(res) => match res {
                Ok(()) => self.saved = true,
                Err(e) => {
                    let err_msg = crate::user_error::report(&e);
                    self.error = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },
            _ => {}
        }
        Task::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(content, view::vault::psbt::save_action(self.saved))
            .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
            .into()
    }
}

pub struct BroadcastModal {
    /// Set once `broadcast_spend_tx` has returned successfully —
    /// drives the celebration screen.
    broadcast: bool,
    /// True while the daemon's `broadcast_spend_tx` RPC is in flight.
    /// Used to give the user clear "Broadcasting…" feedback and to
    /// suppress accidental duplicate clicks / blur-cancel during the
    /// window between pressing Broadcast and the daemon's reply.
    broadcasting: bool,
    error: Option<Error>,
    /// IDs of any directly conflicting transactions.
    conflicting_txids: HashSet<Txid>,
    /// Quote and image handle for the celebration screen.
    sent_quote: coincube_ui::component::quote_display::Quote,
    sent_image_handle: iced::widget::image::Handle,
    /// Formatted spend amount for the celebration display.
    spend_amount_display: String,
    /// Network fee with its unit, for the receipt.
    fee_display: Option<String>,
    /// The broadcast transaction, for the receipt's Copy.
    txid: Txid,
    /// True when every output of the tx returns to the wallet's own change
    /// addresses. Used by the celebration view to render the right phrasing.
    is_self_transfer: bool,
    /// Wallet handle used to register a successful broadcast in
    /// `Wallet::recently_broadcast` so the other panels (Transactions,
    /// Overview balance, Send) can optimistically reflect the spend
    /// before the daemon's mempool poller catches up.
    wallet: Arc<Wallet>,
    recipient_identities: Option<RecipientIdentities>,
    recipient_outputs: Vec<coincube_core::miniscript::bitcoin::TxOut>,
    network: Network,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    theme_mode: coincube_ui::theme::palette::ThemeMode,
}

impl BroadcastModal {
    /// The dialog is waiting for the user's Confirm: nothing dispatched yet,
    /// nothing succeeded. Only then does the readiness gate apply to it — a
    /// broadcast in flight or done is past the point the gate protects.
    pub fn awaiting_confirmation(&self) -> bool {
        !self.broadcasting && !self.broadcast
    }
}

impl Modal for BroadcastModal {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::Confirm)) => {
                // Ignore re-clicks while a broadcast is already in
                // flight or has succeeded — without this guard, the
                // "Broadcast" button could fire `broadcast_spend_tx`
                // twice on rapid double-taps, and the second call
                // would error out with "unknown spend" after the first
                // one drained the PSBT from the daemon's DB.
                if self.broadcasting || self.broadcast {
                    return Task::none();
                }
                self.broadcasting = true;
                let daemon = daemon.clone();
                let psbt = tx.psbt.clone();
                self.error = None;
                self.broadcasting = true;
                let txid = psbt.unsigned_tx.compute_txid();
                tracing::info!(
                    target: "coincube_gui::broadcast",
                    %txid,
                    "Broadcast requested"
                );
                return Task::perform(
                    async move { daemon.broadcast_spend_tx(&txid).await.map_err(|e| e.into()) },
                    Message::Updated,
                );
            }
            Message::Updated(res) => {
                // Either outcome ends the in-flight state.
                self.broadcasting = false;
                match res {
                    Ok(()) => {
                        tx.status = SpendStatus::Broadcast;
                        self.broadcast = true;
                        // Record on the wallet so the rest of the GUI
                        // (Transactions list, Overview balance, Send
                        // coin filter) can apply the same optimistic
                        // override until the daemon's poller observes
                        // the spend. Only runs on RPC success — a
                        // failed broadcast never enters the override
                        // set.
                        self.wallet.record_broadcast(
                            tx.psbt.unsigned_tx.clone(),
                            tx.coins.values().cloned().collect(),
                            tx.change_indexes.clone(),
                            tx.network,
                        );
                        tracing::info!(
                            target: "coincube_gui::broadcast",
                            txid = %tx.psbt.unsigned_tx.compute_txid(),
                            "Broadcast completed"
                        );
                    }
                    Err(e) => {
                        let err_msg = crate::user_error::report(&e);
                        self.error = Some(e);
                        return Task::done(Message::View(view::Message::ShowError(err_msg)));
                    }
                }
            }
            _ => {}
        }
        Task::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        // While the broadcast RPC is in flight, suppress blur-to-cancel.
        // An accidental tap outside the modal at that moment would
        // dismiss the user's only visual confirmation that something
        // is happening and look like an aborted broadcast — even
        // though the daemon call itself is unaffected by closing the
        // modal.
        let on_blur = if self.broadcasting {
            None
        } else {
            Some(view::Message::Spend(view::SpendTxMessage::Cancel))
        };
        let identity_review = self
            .recipient_identities
            .as_ref()
            .filter(|review| review.ticket.is_current() && review.results.iter().any(|(_, result)|
                matches!(result, crate::services::branta::LookupResult::Identified(identities) if !identities.is_empty())))
            .map(|review| {
                view::vault::psbt::broadcast_recipient_identities(
                    &review.results,
                    &self.recipient_outputs,
                    self.network,
                    self.bitcoin_unit,
                    self.theme_mode,
                )
            });
        modal::Modal::new(
            content,
            view::vault::psbt::broadcast_action_with_identity_review(
                self.wallet.chain,
                &self.conflicting_txids,
                self.broadcast,
                self.broadcasting,
                // The same copy as the toast, never the daemon's own message.
                self.error
                    .as_ref()
                    .map(|e| crate::user_error::UserError::from(e).toast()),
                &self.spend_amount_display,
                &self.sent_quote,
                &self.sent_image_handle,
                self.is_self_transfer,
                identity_review,
                view::vault::psbt::BroadcastReceipt {
                    fee: self.fee_display.as_deref(),
                    txid: Some(self.txid),
                },
            ),
        )
        .on_blur(on_blur)
        .into()
    }
}

/// "Keychain signing needs Connect" dialog. Pure UI — its two actions
/// (sign in / pair a phone) are handled in `PsbtState::update`, which
/// clears this modal and emits the matching navigation message.
/// The signed-in Connect account relative to the Cube's owner
/// ([`crate::app::settings::CubeSettings::connect_owner`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectAccountStatus {
    /// No Connect session.
    SignedOut,
    /// Signed in as the account that owns this Cube.
    Owner,
    /// Signed in as a different account than the owner. Deliberately carries
    /// no owner identity: the picker must not reveal whose Cube this is.
    OtherAccount,
    /// Signed in, but the Cube's owner was never recorded.
    SignedInUnknownOwner,
}

impl ConnectAccountStatus {
    fn from_cache(cache: &Cache) -> Self {
        // No account email means no usable session, whatever the other
        // session flags say — they can outlive a sign-out. Treat it as signed
        // out so the row offers "Sign in to Connect".
        if !connect_session_available(cache) || cache.connect_email.is_none() {
            return Self::SignedOut;
        }
        // The server refused to register this Cube for the signed-in account:
        // authoritative, and the only signal for Cubes with no recorded owner.
        if cache.cube_owned_by_other_account {
            return Self::OtherAccount;
        }
        match (&cache.cube_connect_owner, cache.connect_email.as_deref()) {
            (Some(owner), Some(email)) if owner.is_email(email) => Self::Owner,
            (Some(_), _) => Self::OtherAccount,
            (None, _) => Self::SignedInUnknownOwner,
        }
    }
}

pub struct KeychainUnavailableModal {
    /// Whether the user already has Connect tokens. When true the blocker
    /// is device/stream readiness rather than a missing sign-in, which the
    /// view uses to adjust its wording.
    pub signed_in: bool,
    action_scope: u64,
}

fn connect_session_available(cache: &Cache) -> bool {
    cache.connect_authenticated
        || cache.has_connect_session
        || (cache.connect_tokens.is_some() && cache.connect_email.is_some())
}

/// Connect-readiness fields required to open a Keychain signing session.
/// Returns the names of any that are missing — empty means ready.
fn keychain_connect_missing(cache: &Cache) -> Vec<&'static str> {
    [
        ("connect_grpc_url", cache.connect_grpc_url.is_none()),
        ("connect_tokens", cache.connect_tokens.is_none()),
        ("connect_device_id", cache.connect_device_id.is_none()),
        (
            "current_cube_server_id",
            cache.current_cube_server_id.is_none(),
        ),
    ]
    .iter()
    .filter_map(|(name, is_missing)| is_missing.then_some(*name))
    .collect()
}

/// Build and launch a nested `KeychainSignModal` when Connect is ready.
/// Returns `(None, Task::none())` when Connect isn't ready — the unified
/// picker still shows keychain rows (disabled, derived from the descriptor)
/// so the user can be prompted to sign in when they click one.
fn build_keychain_if_ready(
    cache: &Cache,
    wallet: &Arc<Wallet>,
    psbt: &Psbt,
) -> (
    Option<super::keychain_sign::KeychainSignModal>,
    Task<Message>,
) {
    if !keychain_connect_missing(cache).is_empty() {
        return (None, Task::none());
    }
    let grpc_url = cache.connect_grpc_url.clone().expect("checked above");
    let tokens = cache.connect_tokens.clone().expect("checked above");
    let device_id = cache.connect_device_id.clone().expect("checked above");
    let cube_server_id = cache.current_cube_server_id.expect("checked above");
    // The REST client's bearer is set asynchronously inside
    // `KeychainSignModal::launch()` (an async context) rather than here on
    // the synchronous `update` path, which can't `blocking_read` the token.
    let coincube_client = crate::services::coincube::CoincubeClient::new();
    let modal = super::keychain_sign::KeychainSignModal::new(
        wallet.clone(),
        coincube_client,
        tokens,
        grpc_url,
        device_id,
        cube_server_id,
        cache.cube_id.clone(),
        psbt.clone(),
        cache.connect_transport_key.clone(),
    )
    .with_stream_health(cache.connect_stream_status);
    let launch = modal.launch();
    (Some(modal), launch)
}

impl Modal for KeychainUnavailableModal {
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(
            content,
            view::vault::psbt::keychain_unavailable_action(self.action_scope, self.signed_in),
        )
        .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
        .into()
    }
}

#[derive(Default)]
pub struct DeleteModal {
    deleted: bool,
    error: Option<Error>,
}

impl Modal for DeleteModal {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::Confirm)) => {
                let daemon = daemon.clone();
                let psbt = tx.psbt.clone();
                self.error = None;
                return Task::perform(
                    async move {
                        daemon
                            .delete_spend_tx(&psbt.unsigned_tx.compute_txid())
                            .await
                            .map_err(|e| e.into())
                    },
                    Message::Updated,
                );
            }
            Message::Updated(res) => match res {
                Ok(()) => self.deleted = true,
                Err(e) => {
                    let err_msg = crate::user_error::report(&e);
                    self.error = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },
            _ => {}
        }
        Task::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(content, view::vault::psbt::delete_action(self.deleted))
            .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
            .into()
    }
}

/// Reconstruction step within the border wallet signing flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconStep {
    RecoveryPhrase,
    Grid,
}

/// State for reconstructing a Border Wallet key to sign a PSBT.
///
/// This is embedded within `SignModal` and represents a multi-step wizard
/// where the user re-enters their recovery phrase and pattern to transiently
/// reconstruct the private key for signing.
pub struct BorderWalletReconstructionState {
    pub target_fingerprint: Fingerprint,
    pub network: Network,
    pub step: ReconStep,

    // Recovery phrase (12 words) — zeroized on drop.
    pub phrase_words: Vec<form::Value<String>>,
    pub phrase_valid: bool,

    // Grid + pattern
    pub grid: Option<WordGrid>,
    pub pattern: OrderedPattern,

    // Derived checksum word — displayed for visual confirmation once pattern is complete.
    pub checksum_word: Option<String>,

    /// Whether [`Self::phrase_words`] currently holds a phrase this Cube
    /// re-derived, untouched since.
    ///
    /// Cleared by any edit, so the step never claims a derivation the words no
    /// longer match. Only drives what the phrase step says — the words are
    /// ordinary editable fields either way.
    pub phrase_prefilled: bool,

    pub error: Option<String>,
}

impl BorderWalletReconstructionState {
    fn new(target_fingerprint: Fingerprint, network: Network) -> Self {
        Self {
            target_fingerprint,
            network,
            step: ReconStep::RecoveryPhrase,
            phrase_words: vec![form::Value::default(); 12],
            phrase_valid: false,
            grid: None,
            pattern: OrderedPattern::new(),
            checksum_word: None,
            phrase_prefilled: false,
            error: None,
        }
    }

    /// Fill the twelve fields with an Entropy Grid phrase re-derived for this
    /// key — BIP-85 at `m/83696968'/39'/0'/12'/0'`, exactly what the
    /// installer's Border Wallet wizard put there on "Generate"
    /// (`GridRecoveryPhrase::from_master_signer`).
    ///
    /// A Border Wallet key enrolled from a seed this machine still holds is
    /// therefore reconstructible without the user retyping twelve words they
    /// never chose; on a passkey Cube that seed comes back from the WebAuthn
    /// assertion at unlock, so there is nothing to type.
    ///
    /// Choosing *which* seed is the caller's job
    /// ([`SignModal::border_wallet_grid_phrase`]) and is where the correctness
    /// lives — this function only writes what it is given.
    ///
    /// This prefill signs nothing on its own: the pattern is the secret half of
    /// a Border Wallet key and still has to be supplied on the next step. The
    /// words stay editable.
    fn prefill_phrase(&mut self, phrase: &GridRecoveryPhrase) {
        for (field, word) in self
            .phrase_words
            .iter_mut()
            .zip(phrase.as_str().split_whitespace())
        {
            field.value.zeroize();
            field.value = word.to_string();
            field.valid = true;
            field.warning = None;
        }
        self.phrase_valid = self.phrase_words.iter().all(|w| !w.value.trim().is_empty());
        self.phrase_prefilled = self.phrase_valid;
    }

    /// Recompute the checksum word if the pattern is complete, otherwise clear it.
    fn refresh_checksum(&mut self) {
        if self.pattern.is_complete() {
            if let Some(grid) = &self.grid {
                if let Ok((_mnemonic, checksum)) = build_mnemonic(grid, &self.pattern) {
                    self.checksum_word = Some(checksum.to_string());
                    return;
                }
            }
        }
        self.checksum_word = None;
    }

    /// Handle a reconstruction message. Returns `Some((fingerprint, mnemonic))`
    /// when reconstruction is complete and ready to sign.
    fn update(
        &mut self,
        msg: BorderWalletReconMessage,
    ) -> Option<(Fingerprint, coincube_core::bip39::Mnemonic)> {
        match msg {
            BorderWalletReconMessage::PhraseWordEdited(index, word) => {
                if index < 12 {
                    self.phrase_words[index].value = word;
                    self.phrase_words[index].valid = true;
                    self.phrase_words[index].warning = None;
                }
                self.phrase_valid = self.phrase_words.iter().all(|w| !w.value.trim().is_empty());
                // One keystroke and the fields are no longer the phrase we
                // derived, so the step must stop saying they are. Same rule the
                // installer's wizard applies to its own provenance — claiming a
                // derivation the words no longer match is worse than claiming
                // nothing, because the user reads it as confirmation.
                self.phrase_prefilled = false;
            }
            BorderWalletReconMessage::Next => {
                self.error = None;
                match self.step {
                    ReconStep::RecoveryPhrase => {
                        let phrase_str = Zeroizing::new(
                            self.phrase_words
                                .iter()
                                .map(|w| w.value.trim().to_lowercase())
                                .collect::<Vec<_>>()
                                .join(" "),
                        );
                        match GridRecoveryPhrase::from_phrase(&phrase_str) {
                            Ok(rp) => {
                                self.grid = Some(rp.generate_grid());
                                self.pattern = OrderedPattern::new();
                                self.step = ReconStep::Grid;
                            }
                            Err(_) => {
                                self.error = Some(
                                    "Invalid recovery phrase. Please enter a valid 12-word BIP39 mnemonic."
                                        .to_string(),
                                );
                            }
                        }
                    }
                    ReconStep::Grid => {
                        if !self.pattern.is_complete() {
                            self.error = Some(coincube_ui::localized_format!(
                                "Please select exactly {} cells. Currently selected: {}",
                                "Selecciona exactamente {} celdas. Seleccionadas: {}",
                                PATTERN_LENGTH,
                                self.pattern.len()
                            ));
                            return None;
                        }
                        if let Some(grid) = &self.grid {
                            match build_mnemonic(grid, &self.pattern) {
                                Ok((mnemonic, _checksum)) => {
                                    return Some((self.target_fingerprint, mnemonic));
                                }
                                Err(e) => {
                                    // The raw error names BIP39 internals the
                                    // user can do nothing with; what matters is
                                    // that the pattern doesn't reconstruct.
                                    log::error!(
                                        "[{}] border wallet mnemonic: {}",
                                        crate::user_error::CC_WALLET,
                                        e
                                    );
                                    self.error = Some(
                                        "That pattern doesn't rebuild this wallet's recovery phrase. \
                                         Check the grid and your cell order, then try again."
                                            .to_string(),
                                    );
                                }
                            }
                        }
                    }
                }
            }
            BorderWalletReconMessage::Previous => {
                self.error = None;
                match self.step {
                    ReconStep::RecoveryPhrase => {
                        // Will be handled as cancel by the caller
                    }
                    ReconStep::Grid => {
                        self.step = ReconStep::RecoveryPhrase;
                    }
                }
            }
            BorderWalletReconMessage::ToggleCell(row, col) => {
                let cell = CellRef::new(row, col);
                if let Some(pos) = self.pattern.cells().iter().position(|c| c == &cell) {
                    self.pattern.remove_at(pos);
                    self.error = None;
                } else {
                    match self.pattern.add(cell) {
                        Ok(()) => self.error = None,
                        Err(e) => {
                            self.error = Some(crate::user_error::border_wallet_cell_message(&e))
                        }
                    }
                }
                self.refresh_checksum();
            }
            BorderWalletReconMessage::UndoLastCell => {
                self.pattern.undo_last();
                self.error = None;
                self.refresh_checksum();
            }
            BorderWalletReconMessage::ClearPattern => {
                self.pattern.clear();
                self.error = None;
                self.refresh_checksum();
            }
            BorderWalletReconMessage::Cancel => {
                // Handled by the caller (SignModal) to clear the reconstruction state
            }
        }
        None
    }
}

/// Zeroize all secret-bearing buffers when the reconstruction state is dropped.
///
/// This covers the recovery phrase words, the checksum word, the grid
/// (a deterministic permutation of BIP39 words derived from the phrase),
/// and the pattern (cell selections that reconstruct the mnemonic).
impl Drop for BorderWalletReconstructionState {
    fn drop(&mut self) {
        for word in &mut self.phrase_words {
            word.value.zeroize();
        }
        if let Some(ref mut cw) = self.checksum_word {
            cw.zeroize();
        }
        self.checksum_word = None;
        self.grid = None;
        self.pattern.clear();
        self.phrase_prefilled = false;
    }
}

pub struct SignModal {
    action_scope: u64,
    wallet: Arc<Wallet>,
    hws: HardwareWallets,
    network: Network,
    error: Option<Error>,
    /// A signer-specific limitation that should be visible inside the picker.
    signer_notice: Option<String>,
    signing: HashSet<Fingerprint>,
    signed: HashSet<Fingerprint>,
    is_saved: bool,
    display_modal: bool,
    recovery_timelock: Option<u16>,
    border_wallet_recon: Option<BorderWalletReconstructionState>,
    /// Nested multi-signer Keychain flow. `Some` when Connect was ready at
    /// picker-open time (built + launched then). `None` when Connect isn't
    /// ready — keychain rows still render (disabled, derived from the
    /// descriptor) so the user can be prompted to sign in on click.
    keychain: Option<super::keychain_sign::KeychainSignModal>,
    /// Whether this picker offers Keychain signing at all. `true` for the
    /// vault PSBT flow; `false` for contexts that sign locally only (e.g. the
    /// Home send-to-self transfer), which suppresses keychain rows entirely
    /// regardless of Connect state.
    keychain_enabled: bool,
    /// Which Connect account (if any) is signed in relative to this Cube's
    /// owner, at picker-open time. Decides what unresolved Keychain rows ask
    /// for: a sign-in, or a switch to the owning account.
    connect_account: ConnectAccountStatus,
    /// Spending-path identities the user has expanded, keyed the same way as
    /// `ToggleSpendPath`: `None` = primary, `Some(seq)` = a recovery path.
    /// Inactive cards default to collapsed.
    expanded_paths: HashSet<Option<u16>>,
    /// Keys whose last signing attempt to a Keychain phone over the local
    /// network failed, mapped to whether the phone asked to be paired again.
    /// Their rows offer Connect instead; cleared by the next local attempt.
    lan_failures: HashMap<Fingerprint, bool>,
    /// Keys with a signing request out to a Keychain phone over the local
    /// network, so its result is handled as a Wi-Fi one.
    lan_signing: HashSet<Fingerprint>,
}

impl SignModal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        signed: HashSet<Fingerprint>,
        wallet: Arc<Wallet>,
        datadir_path: CoincubeDirectory,
        network: Network,
        is_saved: bool,
        recovery_timelock: Option<u16>,
        keychain: Option<super::keychain_sign::KeychainSignModal>,
        keychain_enabled: bool,
    ) -> Self {
        Self {
            action_scope: NEXT_PSBT_ACTION_SCOPE.fetch_add(1, Ordering::Relaxed),
            signing: HashSet::new(),
            hws: HardwareWallets::new(datadir_path, network).with_wallet(wallet.clone()),
            wallet,
            network,
            error: None,
            signer_notice: None,
            signed,
            is_saved,
            display_modal: true,
            recovery_timelock,
            border_wallet_recon: None,
            keychain,
            keychain_enabled,
            connect_account: ConnectAccountStatus::SignedOut,
            expanded_paths: HashSet::new(),
            lan_failures: HashMap::new(),
            lan_signing: HashSet::new(),
        }
    }

    /// Record the Connect account state, so unresolved Keychain rows ask
    /// the user to switch accounts rather than to sign in when they're
    /// already signed in as someone else.
    pub fn with_connect_account(mut self, status: ConnectAccountStatus) -> Self {
        self.connect_account = status;
        self
    }

    /// The Entropy Grid phrase for `fingerprint`, if this machine can still
    /// re-derive it.
    ///
    /// Dispatches on the provenance recorded at enrolment
    /// ([`crate::app::settings::GridSeedSource`]) rather than on whichever hot
    /// signer the Vault loaded, because the two are not the same thing and
    /// guessing has two distinct failure modes:
    ///
    /// - **Wrong words.** `Wallet::signer` is "some descriptor key whose seed is
    ///   on this machine". A Vault that reuses an earlier Vault's key can load a
    ///   seed that is *not* the one the wizard derived from, and BIP-85 off it
    ///   yields twelve confident, wrong words.
    /// - **No words.** The wizard derives from the Cube's master seed,
    ///   which need not be a descriptor key. `Wallet::signer` can be `None`
    ///   while the exact source remains unlocked in the Cube session.
    ///
    /// Naming the seed at enrolment settles both: derive from *that*
    /// fingerprint, looking first at the loaded hot signer and then at the
    /// session, and decline when neither is it.
    fn border_wallet_grid_phrase(&self, fingerprint: &Fingerprint) -> Option<GridRecoveryPhrase> {
        use crate::app::settings::GridSeedSource;
        match self.wallet.border_wallet_grid_seed.get(fingerprint) {
            // Random or hand-typed: the user holds the only copy.
            Some(GridSeedSource::Independent) => None,
            Some(GridSeedSource::MasterDerived {
                fingerprint: seed, ..
            }) => self.grid_phrase_from_seed(*seed),
            // Enrolled before provenance was tracked. No seed is named, so the
            // loaded hot signer is the only candidate — today's behaviour, kept
            // so Vaults built before this existed still offer their phrase. The
            // words are editable and labelled as derived, which is what makes a
            // wrong guess here recoverable rather than misleading.
            None => self
                .wallet
                .signer
                .as_ref()
                .and_then(|s| s.derive_grid_recovery_phrase().ok()),
        }
    }

    /// Re-derive a grid phrase from one specific seed, `None` unless this
    /// machine actually holds it.
    ///
    /// Two places it can be: loaded as the Vault's hot signer, or — for a
    /// master seed that is not a descriptor key, which is the passkey and
    /// Cube-master shape — unlocked in the session. The session lookup is
    /// fingerprint-checked by [`crate::app::session::unlocked_signer`] and
    /// scoped to the open Cube, so it cannot answer with another seed.
    fn grid_phrase_from_seed(&self, seed: Fingerprint) -> Option<GridRecoveryPhrase> {
        if let Some(signer) = self
            .wallet
            .signer
            .as_ref()
            .filter(|s| s.fingerprint() == seed)
        {
            return signer.derive_grid_recovery_phrase().ok();
        }
        let cube_id = crate::app::session::current_cube_id()?;
        let master = crate::app::session::unlocked_signer(&cube_id, seed)?;
        GridRecoveryPhrase::from_master_signer(
            &master,
            &coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only(),
        )
        .ok()
    }

    pub fn is_signing(&self) -> bool {
        !self.signing.is_empty()
    }

    /// On a Bitcoin Blake2b Cube, refuse to dispatch *any* signer — local,
    /// device or Keychain — for a PSBT that asks for or carries an
    /// `ANYONECANPAY` sighash, or a reserved unified record the adapter
    /// rejects ([`replay::refuse_before_dispatch`]). Returns the error task to
    /// run instead. A no-op on every other chain.
    fn refuse_before_dispatch(&mut self, psbt: &Psbt) -> Option<Task<Message>> {
        if !self.wallet.chain.is_blake2b() {
            return None;
        }
        match replay::refuse_before_dispatch(psbt) {
            Ok(()) => None,
            Err(refused) => {
                let e = Error::Unexpected(refused.to_string());
                let err_msg = crate::user_error::report(&e);
                self.error = Some(e);
                Some(Task::done(Message::View(view::Message::ShowError(err_msg))))
            }
        }
    }

    /// Begin dismissing the picker. Cancels any in-flight keychain sessions
    /// server-side and reports whether the modal must stay mounted (hidden)
    /// to drain deferred cancels — mirroring the old standalone keychain
    /// modal's dismissal contract. When kept, the picker self-closes via
    /// `Message::Updated(Ok)` once every session reaches a terminal state.
    pub fn begin_dismiss(&mut self) -> (Task<Message>, bool) {
        let Some(k) = self.keychain.as_mut() else {
            return (Task::none(), false);
        };
        let cancel = k.cancel_all();
        // Keep the hidden modal mounted while sessions still need draining OR a
        // signed-PSBT capture (fetch or persist) is in flight — so a `Completed`
        // session's fetch can still merge and a `Persisted(Err)` can still mark
        // its row Failed instead of landing on a dropped modal.
        let keep = k.has_undrained_sessions() || k.has_capture_in_flight();
        if keep {
            k.mark_dismissed();
            self.display_modal = false;
        }
        (cancel, keep)
    }

    /// True once the picker was dismissed (hidden) and its keychain sessions
    /// have all drained *and* no signed-PSBT capture (fetch or persist) is still
    /// in flight — the signal the panel uses to finally drop it.
    pub fn should_close_after_dismiss(&self) -> bool {
        !self.display_modal
            && self
                .keychain
                .as_ref()
                .is_none_or(|k| !k.has_undrained_sessions() && !k.has_capture_in_flight())
    }

    /// True when this picker offers Keychain signing but the nested flow
    /// hasn't been built yet — the picker was opened before Connect was ready.
    /// Once Connect comes up, the panel builds + launches it on demand.
    pub fn keychain_needs_init(&self) -> bool {
        self.keychain_enabled && self.keychain.is_none()
    }

    /// Attach a freshly-built (and launched) nested Keychain flow. Called once
    /// Connect becomes ready for a picker that opened without it.
    pub fn set_keychain(&mut self, keychain: Option<super::keychain_sign::KeychainSignModal>) {
        self.keychain = keychain;
    }

    /// Replace the per-key "Signed" set with the authoritative signers counted
    /// from the merged PSBT (`SpendTx::signers()`). Called on every reconcile so
    /// a row can only read Signed once its signature is actually counted — the
    /// optimistic `signed.insert` on local-sign return is corrected here.
    pub fn set_counted_signers(&mut self, counted: HashSet<Fingerprint>) {
        self.signed = counted;
    }

    /// True while the nested Keychain flow has a signature merged into the
    /// PSBT but not yet durably persisted. The picker must not close on
    /// threshold until this resolves, so a persist failure can mark the row
    /// Failed instead of tearing the modal down on an unsaved signature.
    pub fn keychain_persistence_pending(&self) -> bool {
        self.keychain
            .as_ref()
            .is_some_and(|k| k.has_persistence_pending())
    }

    /// Best-effort cancel of any keychain sessions still in flight, used when
    /// the picker is about to close because the threshold was met so those
    /// sessions don't outlive it server-side.
    pub fn cancel_keychain_if_active(&mut self) -> Task<Message> {
        match self.keychain.as_mut() {
            Some(k) if k.has_undrained_sessions() => k.cancel_all(),
            _ => Task::none(),
        }
    }

    /// Build the spending-path cards for the unified picker, mirroring the
    /// vault-creation "Set keys" layout: one card per descriptor path (primary
    /// + each recovery), each listing its keys with per-key signability.
    fn signing_paths(&self) -> Vec<view::vault::psbt::SigningPath> {
        let Some(descriptor) = self.wallet.inheritance_descriptor() else {
            return Vec::new();
        };
        let policy = descriptor.policy();
        let mut paths = Vec::new();
        // The transaction spends through exactly one path: the primary when it
        // has no recovery timelock, else the recovery path matching that
        // timelock. Keys in the other paths render disabled.
        let primary_active = self.recovery_timelock.is_none();
        paths.push(self.build_signing_path(
            "Primary spending option:".to_string(),
            true,
            None,
            primary_active,
            policy.primary_path(),
        ));
        for (seq, info) in policy.recovery_paths() {
            let active = self.recovery_timelock == Some(*seq);
            paths.push(self.build_signing_path(
                "Recovery spending option:".to_string(),
                false,
                Some(*seq),
                active,
                info,
            ));
        }
        paths
    }

    fn build_signing_path(
        &self,
        title: String,
        is_primary: bool,
        sequence: Option<u16>,
        active: bool,
        path_info: &coincube_core::descriptors::PathInfo,
    ) -> view::vault::psbt::SigningPath {
        use view::vault::psbt::{SigningKeyAction, SigningKeyRow, SigningKeyState, SigningPath};
        let (threshold, origins) = path_info.thresh_origins();
        let mut fps: Vec<Fingerprint> = origins.into_keys().collect();
        fps.sort();
        let total = fps.len();
        let mut collected = 0;
        let mut idle_keychain = 0;
        let mut keys = Vec::new();
        for fp in fps {
            let (kind, state) = self.classify_signing_key(fp, active);
            if matches!(state, SigningKeyState::Signed) {
                collected += 1;
            }
            if matches!(
                state,
                SigningKeyState::Available(SigningKeyAction::Keychain)
            ) {
                idle_keychain += 1;
            }
            keys.push(SigningKeyRow {
                fingerprint: fp,
                label: self.signing_key_label(fp),
                kind,
                state,
            });
        }
        SigningPath {
            title,
            is_primary,
            sequence,
            active,
            threshold,
            total,
            collected,
            keys,
            // Only worth offering the batch affordance when it saves clicks —
            // i.e. more than one idle Keychain signer to request at once.
            can_request_all: active && self.keychain.is_some() && idle_keychain > 1,
            // Active path is always expanded; inactive paths start collapsed
            // and expand only when the user toggles them (keyed by `sequence`:
            // None = primary, Some(seq) = recovery).
            expanded: active || self.expanded_paths.contains(&sequence),
        }
    }

    /// Determine a descriptor key's display kind (for its icon) and signing
    /// state (signed / in-progress / available / retry / disabled). Signability
    /// is derived dynamically — a connected hardware wallet, the master signer,
    /// a border wallet, or a Connect-resolved Keychain signer — so an
    /// unidentified key is shown disabled rather than guessed as Keychain.
    fn classify_signing_key(
        &self,
        fp: Fingerprint,
        active: bool,
    ) -> (
        view::vault::psbt::SigningKeyKind,
        view::vault::psbt::SigningKeyState,
    ) {
        use super::keychain_sign::PendingSessionStatus;
        use view::vault::psbt::{
            SigningKeyAction as Act, SigningKeyKind as Kind, SigningKeyState as St,
        };

        let master_fp = self.wallet.signer.as_ref().map(|s| s.fingerprint());
        let kc = self.keychain_session(fp);
        let kind = self.signing_key_kind(fp, master_fp, kc.map(|(_, status)| status));

        // A resolved Keychain session in a give-up state (rejected / expired /
        // failed — including a *persist* failure) must fall through to the Retry
        // branch below, even if its signature is transiently counted in
        // `self.signed`. A failed `update_spend_tx` leaves the signature
        // merged-but-unsaved, so rendering it Signed would hide the Retry
        // affordance and overstate the durable "X of N collected" count.
        let kc_needs_retry = matches!(kc, Some((_, status)) if status.is_give_up());
        // Signature already collected. `self.signed` is the single source of
        // truth — it mirrors the signers actually counted in the merged PSBT
        // (refreshed by `set_counted_signers` on every reconcile), so the row
        // and the "X of N collected" badge can't disagree. No persistence-based
        // shortcut: a Keychain response only counts once its signature is merged
        // and counted, which is exactly what puts the key into `self.signed`.
        if self.signed.contains(&fp) && !kc_needs_retry {
            return (kind, St::Signed);
        }
        // Inactive spending path — nothing here can sign this transaction.
        if !active {
            return (
                kind,
                St::Disabled(
                    "This spending path isn't available for this transaction.".to_string(),
                ),
            );
        }
        // Local device operation in flight.
        if self.signing.contains(&fp) {
            return (kind, St::InProgress("Signing…".to_string()));
        }
        // Master signer on this computer.
        if master_fp == Some(fp) {
            return (kind, St::Available(Act::Master));
        }
        // Border wallet key.
        if self.wallet.border_wallet_fingerprints.contains(&fp) {
            return (kind, St::Available(Act::BorderWallet));
        }
        // A currently-connected hardware wallet matching this key.
        if let Some(i) = self
            .hws
            .list
            .iter()
            .position(|hw| hw.fingerprint() == Some(fp) && !hw.is_lan_phone())
        {
            match &self.hws.list[i] {
                HardwareWallet::Supported {
                    registered: Some(false),
                    ..
                } => {
                    return (
                        Kind::Hardware,
                        St::Disabled("Register the wallet on this device to sign.".to_string()),
                    );
                }
                HardwareWallet::Supported { .. } => {
                    return (Kind::Hardware, St::Available(Act::Hardware(i)));
                }
                HardwareWallet::Locked { .. } => {
                    return (
                        Kind::Hardware,
                        St::Disabled("Unlock this device to sign.".to_string()),
                    );
                }
                _ => {}
            }
        }
        // A Keychain phone reachable over the local network. That road is the
        // default, unless a Connect session for this key is already under way
        // or finished — then the Connect branch below owns the row, so the
        // user sees that session's progress and no second request goes out
        // over Wi-Fi.
        let connect_idle = kc.is_some_and(|(_, status)| status.is_idle());
        if kc.is_none() || connect_idle {
            if let Some(i) = self.lan_phone_index(fp) {
                let state = match self.lan_failures.get(&fp) {
                    None => St::Available(Act::Hardware(i)),
                    Some(true) => St::LanFailed {
                        reason: "This Keychain needs to be paired again.".to_string(),
                        retry: None,
                        connect: connect_idle,
                    },
                    Some(false) => St::LanFailed {
                        reason: "Couldn't sign over Wi-Fi.".to_string(),
                        retry: Some(i),
                        connect: connect_idle,
                    },
                };
                return (Kind::LanKeychain, state);
            }
        }
        // A Connect-resolved Keychain signer.
        if let Some((idx, status)) = kc {
            return match status {
                PendingSessionStatus::Idle => (Kind::Keychain, St::Available(Act::Keychain)),
                PendingSessionStatus::Rejected
                | PendingSessionStatus::Expired
                | PendingSessionStatus::Failed => (Kind::Keychain, St::Retry(idx)),
                other => (Kind::Keychain, St::InProgress(other.label().to_string())),
            };
        }
        // A hot key whose seed IS on this machine but which this Cube's
        // credential would not open — the restore wrote it under a credential
        // that no longer opens it, or the Cube's PIN changed underneath it.
        // Checked after every path that can actually sign (a key may also be
        // reachable via hardware or Keychain, and those still win), but before
        // the unidentified fallbacks: without this the row reads "connect this
        // signing device", which sends the user looking for a device that was
        // never involved — the key is right here, just unreachable.
        if self.wallet.unopenable_seed_keys.contains(&fp) {
            return (
                Kind::Unknown,
                St::Disabled(
                    "This key's seed is on this computer, but this Cube's credential didn't \
                     unlock it."
                        .to_string(),
                ),
            );
        }
        // The same seed, but nothing was tried against it — this load had no
        // credential at all (no session PIN for this Cube). Saying the PIN
        // failed here would be false, and would send the user to re-check a
        // credential that was never tested instead of to the one thing that
        // fixes it.
        if self.wallet.locked_seed_keys.contains(&fp) {
            return (
                Kind::Unknown,
                St::Disabled(
                    "This key's seed is on this computer. Reopen this Cube with its PIN to sign \
                     with it."
                        .to_string(),
                ),
            );
        }
        // Keychain flow launched but still resolving — we don't yet know which
        // unidentified keys are Keychain signers, so show a neutral hint rather
        // than a misleading "connect a device" message.
        if self.keychain.as_ref().is_some_and(|k| k.is_loading()) {
            return (
                Kind::Unknown,
                St::Disabled("Looking up Keychain signers…".to_string()),
            );
        }
        // Unidentified: an external key whose device isn't connected and which
        // Connect hasn't resolved. Shown disabled — never guessed as Keychain.
        // When Keychain is possible here but Connect is signed out, offer a
        // clickable "Sign in to Connect" so any Keychain signer among these
        // rows can be resolved.
        if self.keychain_enabled && self.keychain.is_none() {
            match &self.connect_account {
                // Signed out, or signed in as the owner while Keychain is
                // still coming up: "Sign in to Connect" either routes to Home
                // to sign in or refreshes the session in place (the tab-level
                // `OpenConnectSignIn` handler decides which).
                ConnectAccountStatus::SignedOut | ConnectAccountStatus::Owner => {
                    return (
                        Kind::Unknown,
                        St::NeedsSignIn("Connect a device to sign with this key.".to_string()),
                    );
                }
                ConnectAccountStatus::OtherAccount => {
                    return (
                        Kind::Unknown,
                        St::NeedsAccountSwitch(
                            "This Cube belongs to a different Connect account. Switch \
                             accounts to sign with this key."
                                .to_string(),
                        ),
                    );
                }
                // Owner unrecorded (a Cube registered before owners were
                // tracked): signed in, yet Keychain never came up — most
                // likely the wrong account, but we can't name the right one.
                ConnectAccountStatus::SignedInUnknownOwner => {
                    return (
                        Kind::Unknown,
                        St::NeedsAccountSwitch(
                            "You may be signed in under a different account than the one that \
                             created this Cube. Switch accounts to sign with this key."
                                .to_string(),
                        ),
                    );
                }
            }
        }
        (
            Kind::Unknown,
            St::Disabled("Connect this signing device to sign.".to_string()),
        )
    }

    /// Icon kind for a descriptor key: master / border / connected-hardware /
    /// Keychain over the local network / resolved-keychain, else unknown.
    ///
    /// `keychain` is the key's Connect session status, once resolved. A phone
    /// on the local network reads as [`Kind::LanKeychain`] only while that is
    /// the road in use — i.e. no Connect session has left `Idle` — matching
    /// [`Self::classify_signing_key`].
    fn signing_key_kind(
        &self,
        fp: Fingerprint,
        master_fp: Option<Fingerprint>,
        keychain: Option<super::keychain_sign::PendingSessionStatus>,
    ) -> view::vault::psbt::SigningKeyKind {
        use view::vault::psbt::SigningKeyKind as Kind;
        if master_fp == Some(fp) {
            Kind::Master
        } else if self.wallet.border_wallet_fingerprints.contains(&fp) {
            Kind::BorderWallet
        } else if self
            .hws
            .list
            .iter()
            .any(|hw| hw.fingerprint() == Some(fp) && !hw.is_lan_phone())
        {
            Kind::Hardware
        } else if keychain.is_none_or(|status| status.is_idle())
            && self.lan_phone_index(fp).is_some()
        {
            Kind::LanKeychain
        } else if keychain.is_some() {
            Kind::Keychain
        } else {
            Kind::Unknown
        }
    }

    /// The Connect Keychain session for `fp` (its `pending` index and
    /// status), only once the nested flow has resolved.
    fn keychain_session(
        &self,
        fp: Fingerprint,
    ) -> Option<(usize, super::keychain_sign::PendingSessionStatus)> {
        self.keychain
            .as_ref()
            .filter(|k| k.is_resolved())
            .and_then(|k| {
                k.pending()
                    .iter()
                    .enumerate()
                    .find(|(_, p)| p.fingerprint == fp)
                    .map(|(i, p)| (i, p.status))
            })
    }

    /// Device-list index of a Keychain phone holding `fp` that is reachable
    /// over the local network right now.
    fn lan_phone_index(&self, fp: Fingerprint) -> Option<usize> {
        self.hws
            .list
            .iter()
            .position(|hw| hw.is_lan_phone() && hw.fingerprint() == Some(fp))
    }

    /// Display label for a descriptor key: user alias, else the resolved
    /// Keychain signer's name, else the short fingerprint.
    fn signing_key_label(&self, fp: Fingerprint) -> String {
        if let Some(alias) = self.wallet.keys_aliases.get(&fp) {
            return alias.clone();
        }
        if let Some(k) = self.keychain.as_ref().filter(|k| k.is_resolved()) {
            if let Some(p) = k.pending().iter().find(|p| p.fingerprint == fp) {
                return p.label.clone();
            }
        }
        format!("#{}", fp)
    }

    /// Notices shown above the signing paths. On a Bitcoin Blake2b Vault
    /// whose picker lists a connected hardware device for one of its keys,
    /// the device copy from the Split flow comes first: the device calls the
    /// spend a Bitcoin transaction, so the user checks the amounts and the
    /// address before approving. Bitcoin-family Vaults are unchanged.
    fn sign_notices(&self, paths: &[view::vault::psbt::SigningPath]) -> Vec<String> {
        let device_listed = paths.iter().any(|path| {
            path.keys
                .iter()
                .any(|row| matches!(row.kind, view::vault::psbt::SigningKeyKind::Hardware))
        });
        let mut notices = Vec::new();
        if self.wallet.chain.is_blake2b() && device_listed {
            notices.push(crate::split_hardware::sign::DEVICE_SHOWS_BITCOIN_WARNING.to_string());
        }
        notices.extend(self.keychain_notices());
        notices
    }

    /// Keychain-flow banners for the unified picker (errors, degraded stream,
    /// unaddressable signers) — surfaced above the signer list. Empty when no
    /// keychain flow is active or everything is healthy.
    ///
    /// `pub(super)` so `keychain_sign`'s tests can pin this surface to the
    /// same copy as the modal's own view.
    pub(super) fn keychain_notices(&self) -> Vec<String> {
        let Some(k) = self.keychain.as_ref() else {
            return Vec::new();
        };
        let mut notices = Vec::new();
        if let Some(err) = k.error() {
            notices.push(coincube_ui::localized_format!(
                "Couldn't start Keychain signing: {}",
                "No se pudo iniciar la firma con Keychain: {}",
                coincube_ui::locale::notice(err.to_string())
            ));
        }
        if let Some(banner) = k.stream_health_banner() {
            notices.push(banner);
        }
        // Same entries, same copy as the Keychain modal's own view — never a
        // second hand-written diagnosis here (this site used to say "this
        // signer has no registered device" for every reason).
        for u in k.unresolved() {
            notices.push(super::keychain_sign::friendly_unresolved_signer(u));
        }
        notices
    }
}

/// Ensure any in-progress Border Wallet reconstruction state is dropped
/// (triggering its own `Drop` zeroization) when the sign modal goes away.
impl Drop for SignModal {
    fn drop(&mut self) {
        self.border_wallet_recon = None;
    }
}

impl Modal for SignModal {
    fn subscription(&self) -> Subscription<Message> {
        // Local device refresh plus, when a Keychain flow is active, its
        // poll-fallback subscription so missed realtime `SessionEvent`s still
        // get picked up while the user can also sign locally.
        let hws = self.hws.refresh().map(Message::HardwareWallets);
        match self.keychain.as_ref() {
            Some(k) => Subscription::batch([hws, k.subscription()]),
            None => hws,
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::SelectHardwareWallet(i)) => {
                if let Some(refused) = self.refuse_before_dispatch(&tx.psbt) {
                    return refused;
                }
                if let Some(fingerprint) =
                    self.hws.list.get(i).and_then(HardwareWallet::fingerprint)
                {
                    return self.update(
                        daemon,
                        Message::View(view::Message::SelectPsbtHardwareWallet(fingerprint)),
                        tx,
                    );
                }
                let reason = "This signing device is no longer ready. Reconnect or unlock it, then try again.";
                self.signer_notice = Some(reason.to_string());
                return Task::done(Message::View(view::Message::ShowError(reason.to_string())));
            }
            Message::View(view::Message::SelectPsbtHardwareWallet(requested_fp)) => {
                if let Some(refused) = self.refuse_before_dispatch(&tx.psbt) {
                    return refused;
                }
                self.error = None;
                self.signer_notice = None;
                let selected = self
                    .hws
                    .list
                    .iter()
                    .find(|hw| hw.fingerprint() == Some(requested_fp) && !hw.is_lan_phone())
                    .or_else(|| {
                        self.hws
                            .list
                            .iter()
                            .find(|hw| hw.fingerprint() == Some(requested_fp))
                    })
                    .and_then(|hw| match hw {
                        HardwareWallet::Supported {
                            fingerprint,
                            device,
                            ..
                        } => Some((*fingerprint, device.clone(), hw.is_lan_phone())),
                        _ => None,
                    });
                if let Some((fingerprint, device, over_lan)) = selected {
                    // A fresh attempt over the local network supersedes the
                    // last failure; the row goes back to "Signing…". Recorded
                    // here rather than looked up when the result lands: a
                    // phone whose connection died has usually left the device
                    // list by then, and its failure is still a Wi-Fi one.
                    if over_lan {
                        self.lan_failures.remove(&fingerprint);
                        self.lan_signing.insert(fingerprint);
                    }
                    // Keep the modal open (as the master-signer path below
                    // does) so the selected device shows its "Processing… /
                    // Please check your device" state while we wait for the
                    // signature, rather than the modal vanishing with no
                    // indication that the device is awaiting confirmation.
                    self.signing.insert(fingerprint);
                    let psbt = tx.psbt.clone();
                    return Task::perform(
                        sign_psbt(self.wallet.clone(), device, psbt),
                        move |res| Message::Signed(fingerprint, res),
                    );
                }
                let reason = "This signing device is no longer ready. Reconnect or unlock it, then try again.";
                self.signer_notice = Some(reason.to_string());
                return Task::done(Message::View(view::Message::ShowError(reason.to_string())));
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::SelectMasterSigner)) => {
                if let Some(refused) = self.refuse_before_dispatch(&tx.psbt) {
                    return refused;
                }
                if let Some(fingerprint) = self.wallet.signer.as_ref().map(|s| s.fingerprint()) {
                    self.signing.insert(fingerprint);
                }
                return Task::perform(
                    sign_psbt_with_master_signer(self.wallet.clone(), tx.psbt.clone()),
                    |(fg, res)| Message::Signed(fg, res),
                );
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::SelectBorderWallet(fg))) => {
                if let Some(refused) = self.refuse_before_dispatch(&tx.psbt) {
                    return refused;
                }
                let network = self.network;
                let mut recon = BorderWalletReconstructionState::new(fg, network);
                // Offered only when this machine still holds the seed this key
                // was actually enrolled from — never from whichever hot signer
                // happens to be loaded, which can be a different seed entirely.
                if let Some(phrase) = self.border_wallet_grid_phrase(&fg) {
                    recon.prefill_phrase(&phrase);
                }
                self.border_wallet_recon = Some(recon);
                // Keep modal displayed but now showing the reconstruction wizard
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::BorderWalletRecon(msg))) => {
                let is_cancel = matches!(msg, BorderWalletReconMessage::Cancel);
                let is_previous_on_phrase = matches!(msg, BorderWalletReconMessage::Previous)
                    && self
                        .border_wallet_recon
                        .as_ref()
                        .is_some_and(|r| r.step == ReconStep::RecoveryPhrase);

                if is_cancel || is_previous_on_phrase {
                    self.border_wallet_recon = None;
                    return Task::none();
                }

                if let Some(recon) = &mut self.border_wallet_recon {
                    if let Some((fingerprint, mnemonic)) = recon.update(msg) {
                        let network = recon.network;
                        let chain = self.wallet.chain;
                        let psbt = tx.psbt.clone();
                        // Clear reconstruction state (zeroizes phrase words via Drop).
                        self.border_wallet_recon = None;
                        self.display_modal = false;
                        self.signing.insert(fingerprint);
                        return Task::perform(
                            async move {
                                let result = sign_border_wallet_for_chain(
                                    chain,
                                    mnemonic,
                                    fingerprint,
                                    network,
                                    psbt,
                                );
                                match result {
                                    Ok((fg, signed_psbt)) => (fg, Ok(signed_psbt)),
                                    Err(e) => (
                                        fingerprint,
                                        Err(Error::Wallet(WalletError::BorderWallet(
                                            e.to_string(),
                                        ))),
                                    ),
                                }
                            },
                            |(fg, res)| Message::Signed(fg, res),
                        );
                    }
                }
            }
            Message::Signed(fingerprint, res) => {
                self.signing.remove(&fingerprint);
                let over_lan = self.lan_signing.remove(&fingerprint);
                match res {
                    Err(e) => {
                        self.display_modal = true;
                        if !matches!(e, Error::HardwareWallet(async_hwi::Error::UserRefused)) {
                            // A Keychain phone over the local network: remember
                            // the failure so its row offers Connect instead, and
                            // word the toast for a phone, not a USB device.
                            if over_lan {
                                let needs_repair = matches!(
                                    &e,
                                    Error::HardwareWallet(hw) if crate::phone_signer::needs_repair(hw)
                                );
                                self.lan_failures.insert(fingerprint, needs_repair);
                                let connect_available = self
                                    .keychain_session(fingerprint)
                                    .is_some_and(|(_, status)| status.is_idle());
                                let err_msg =
                                    crate::user_error::report_lan_keychain(&e, connect_available);
                                self.error = Some(e);
                                return Task::done(Message::View(view::Message::ShowError(
                                    err_msg,
                                )));
                            }
                            let err_msg = crate::user_error::report(&e);
                            self.error = Some(e);
                            return Task::done(Message::View(view::Message::ShowError(err_msg)));
                        }
                    }
                    Ok(psbt) => {
                        self.error = None;
                        self.signer_notice = None;
                        // Bring the picker back into view after a local sign
                        // (the border-wallet path hides it during the async
                        // reconstruction). The picker now closes only when the
                        // threshold is met — decided by the panel's
                        // `Message::Updated(Ok)` handler — so a sub-threshold
                        // signature should leave it visible to add more.
                        self.display_modal = true;
                        let daemon = daemon.clone();
                        if let Err(e) =
                            merge_signatures_for_chain(self.wallet.chain, &mut tx.psbt, &psbt)
                        {
                            // Nothing was merged and nothing is persisted: a
                            // signature the adapter refuses (conflicting or
                            // ambiguous encoding) must not reach the daemon.
                            let err_msg = replay::unsafe_legacy_policy_copy(&e)
                                .map(str::to_owned)
                                .unwrap_or_else(|| {
                                    crate::user_error::report(&Error::Unexpected(format!(
                                        "Couldn't merge the signature from {fingerprint}: {e}"
                                    )))
                                });
                            let e = Error::Unexpected(format!(
                                "Couldn't merge the signature from {fingerprint}: {e}"
                            ));
                            self.error = Some(e);
                            return Task::done(Message::View(view::Message::ShowError(err_msg)));
                        }
                        self.signed.insert(fingerprint);
                        // Persist the *merged* PSBT (not the lone single-signer
                        // result) so the daemon's stored copy always matches the
                        // desktop's in-memory set. Otherwise a later re-read of
                        // the tx from the daemon (list refresh / reopen) can
                        // surface a partially-signed psbt and drop signatures
                        // collected earlier in the same session.
                        let merged = tx.psbt.clone();
                        if self.is_saved {
                            return Task::perform(
                                async move {
                                    daemon.update_spend_tx(&merged).await.map_err(|e| e.into())
                                },
                                Message::Updated,
                            );
                        // If the spend transaction was never saved before, then both the psbt and
                        // labels attached to it must be updated.
                        } else {
                            let mut labels = HashMap::<LabelItem, Option<String>>::new();
                            for (item, label) in tx.labels() {
                                if !label.is_empty() {
                                    labels.insert(label_item_from_str(item), Some(label.clone()));
                                }
                            }
                            return Task::perform(
                                async move {
                                    daemon.update_spend_tx(&merged).await?;
                                    daemon.update_labels(&labels).await.map_err(|e| e.into())
                                },
                                Message::Updated,
                            );
                        }
                    }
                }
            }
            Message::Updated(res) => match res {
                Ok(()) => match replay::spend_info_for_chain(
                    self.wallet.chain,
                    match self.wallet.inheritance_descriptor() {
                        Some(descriptor) => descriptor,
                        None => {
                            return Task::done(Message::View(view::Message::ShowError(
                                "Use the imported wallet signing screen for this PSBT.".into(),
                            )));
                        }
                    },
                    &tx.psbt,
                ) {
                    Ok(sigs) => tx.sigs = sigs,
                    Err(e) => {
                        // Keep the descriptor error as itself rather than
                        // flattening it into `Unexpected(String)`: `Desc` has
                        // copy that names the failing part of the descriptor,
                        // and flattening threw that away along with the class.
                        let e = Error::Desc(e);
                        let err_msg = crate::user_error::report(&e);
                        self.error = Some(e);
                        return Task::done(Message::View(view::Message::ShowError(err_msg)));
                    }
                },
                Err(e) => {
                    let err_msg = crate::user_error::report(&e);
                    self.error = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },

            Message::HardwareWallets(msg) => match self.hws.update(msg) {
                Ok(cmd) => {
                    return cmd.map(Message::HardwareWallets);
                }
                Err(e) => {
                    let e: Error = e.into();
                    let err_msg = crate::user_error::report(&e);
                    self.error = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },
            // Expand/collapse an inactive spending-path card (keyed by path:
            // None = primary, Some(seq) = recovery).
            Message::View(view::Message::Spend(view::SpendTxMessage::ToggleSpendPath(path))) => {
                if !self.expanded_paths.remove(&path) {
                    self.expanded_paths.insert(path);
                }
            }
            // Forward all Keychain traffic to the nested modal: its own
            // async results (`KeychainSign(_)`) plus the per-row user actions
            // routed through the unified picker. `KeychainSignModal::update`
            // runs the merge+persist and the dismissed-drain choke point.
            Message::KeychainSign(_)
            | Message::View(view::Message::Spend(
                view::SpendTxMessage::SelectKeychainSigner(_)
                | view::SpendTxMessage::RequestFromEveryone
                | view::SpendTxMessage::RetryKeychainSigner(_)
                | view::SpendTxMessage::CancelKeychainSign,
            )) => {
                let requests_signature = matches!(
                    message,
                    Message::View(view::Message::Spend(
                        view::SpendTxMessage::SelectKeychainSigner(_)
                            | view::SpendTxMessage::RequestFromEveryone
                            | view::SpendTxMessage::RetryKeychainSigner(_),
                    ))
                );
                if requests_signature {
                    if let Some(refused) = self.refuse_before_dispatch(&tx.psbt) {
                        return refused;
                    }
                }
                if let Some(k) = self.keychain.as_mut() {
                    return k.update(daemon, message, tx);
                }
            }
            _ => {}
        }

        // Use global toast overlay instead of local toast
        Task::none()
    }

    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        // Use global toast overlay instead of local toast
        if self.display_modal {
            if let Some(recon) = &self.border_wallet_recon {
                modal::Modal::new(content, view::vault::psbt::border_wallet_recon_view(recon))
                    .on_blur(Some(view::Message::Spend(
                        view::SpendTxMessage::BorderWalletRecon(BorderWalletReconMessage::Cancel),
                    )))
                    .into()
            } else {
                let paths = self.signing_paths();
                let notices = self.sign_notices(&paths);
                let error = self
                    .signer_notice
                    .clone()
                    .or_else(|| self.error.as_ref().map(crate::user_error::report));
                modal::Modal::new(
                    content,
                    view::vault::psbt::sign_action(
                        self.wallet.chain,
                        self.action_scope,
                        paths,
                        notices,
                        error,
                    ),
                )
                .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
                .into()
            }
        } else {
            content
        }
    }
}

fn merge_signatures(psbt: &mut Psbt, signed_psbt: &Psbt) {
    // Signatures are only meaningful for the transaction they were made for.
    // Copying them by input index onto a different unsigned transaction would
    // attach them to outputs the signer did not see.
    if psbt.unsigned_tx != signed_psbt.unsigned_tx {
        return;
    }
    for i in 0..signed_psbt.inputs.len() {
        let psbtin = match psbt.inputs.get_mut(i) {
            Some(psbtin) => psbtin,
            None => continue,
        };
        let signed_psbtin = match signed_psbt.inputs.get(i) {
            Some(signed_psbtin) => signed_psbtin,
            None => continue,
        };
        psbtin
            .partial_sigs
            .extend(&mut signed_psbtin.partial_sigs.iter());
        psbtin
            .tap_script_sigs
            .extend(&mut signed_psbtin.tap_script_sigs.iter());
        if let Some(sig) = signed_psbtin.tap_key_sig {
            psbtin.tap_key_sig = Some(sig);
        }
    }
}

/// Merge the signatures of `signed_psbt` into `psbt`, keyed on the chain.
///
/// Bitcoin family: [`merge_signatures`], the prior behaviour, unchanged and
/// infallible (last write wins on a key). Bitcoin Blake2b: the unified
/// adapter merge ([`coincube_core::psbt_unified::merge_signatures`]) run
/// against `psbt` **as it is** — never after the prior copy, which would
/// have overwritten a stored signature before the adapter could compare it —
/// carries `partial_sigs` and the proprietary unified records across and
/// **refuses**, leaving `psbt` untouched, a conflicting signature for a key,
/// a key that would end up with both a unified and a legacy signature, or a
/// PSBT for another transaction, or a signature that does not verify (checked
/// on the merged result, so a signer's result need not carry prevouts).
/// Mirrors the daemon's `update_spend` so the desktop never holds a PSBT the
/// daemon would reject. (A Blake2b Vault
/// is native P2WSH — Taproot is not offered on that chain — so the adapter's
/// ECDSA-only view is the whole picture there.)
fn merge_signatures_for_chain(
    chain: ChainId,
    psbt: &mut Psbt,
    signed_psbt: &Psbt,
) -> Result<(), UnifiedFinalizeError> {
    if !chain.is_blake2b() {
        merge_signatures(psbt, signed_psbt);
        return Ok(());
    }
    let mut destination = UnifiedPsbt::from_psbt(psbt.clone())?;
    let delta = UnifiedPsbt::from_psbt(signed_psbt.clone())?;
    coincube_core::psbt_unified::merge_signatures(&mut destination, &delta)?;
    // The adapter validates representation, not validity: a signer result
    // carrying a signature that does not verify (wrong digest, ANYONECANPAY)
    // must not enter the in-memory PSBT — the daemon would refuse to store
    // it, and a later correct signature for the same key would then read as
    // a conflict. Verified on the **merged** PSBT, as the daemon verifies the
    // whole PSBT it is handed: the destination carries the prevouts and
    // witness scripts verification needs, whereas a signer's result (a device
    // returning only its signatures, or a Keychain's return over the API
    // rail, whose contents this repository does not control) need not. `psbt`
    // is assigned only after the merged result verifies.
    coincube_core::unified_finalize::verify_all_signatures(
        &destination,
        &secp256k1::Secp256k1::verification_only(),
    )?;
    coincube_core::unified_finalize::ensure_no_unsafe_legacy_alternative(
        &destination,
        &secp256k1::Secp256k1::verification_only(),
    )?;
    *psbt = destination.psbt().clone();
    Ok(())
}

fn enforce_legacy_signature_policy(
    chain: ChainId,
    psbt: &Psbt,
) -> Result<(), UnifiedFinalizeError> {
    if !chain.is_blake2b() {
        return Ok(());
    }
    let unified = UnifiedPsbt::from_psbt(psbt.clone())?;
    coincube_core::unified_finalize::ensure_no_unsafe_legacy_alternative(
        &unified,
        &secp256k1::Secp256k1::verification_only(),
    )
}

/// [`merge_signatures_for_chain`] for the Keychain sign flow, so every merge
/// site shares one definition.
pub(crate) fn merge_signatures_pub(
    chain: ChainId,
    psbt: &mut Psbt,
    signed_psbt: &Psbt,
) -> Result<(), UnifiedFinalizeError> {
    merge_signatures_for_chain(chain, psbt, signed_psbt)
}

/// Reuse the application's signature-only merge in the opt-in integration runner.
#[cfg(feature = "integration-tests")]
pub fn merge_testnet4_integration_signatures(
    psbt: &mut Psbt,
    signed_psbt: &Psbt,
) -> Result<(), UnifiedFinalizeError> {
    merge_signatures_for_chain(ChainId::from(Network::Testnet4), psbt, signed_psbt)
}

/// Sign with the Vault's hot key, keyed on the chain: `SIGHASH_ALL` into
/// `partial_sigs` on the Bitcoin family (unchanged), unified signatures into
/// the proprietary records on Bitcoin Blake2b.
#[cfg(feature = "integration-tests")]
pub(crate) fn sign_recovery_fixture_for_chain(
    chain: ChainId,
    signer: &crate::signer::Signer,
    psbt: Psbt,
) -> Result<Psbt, Error> {
    sign_with_master_signer_for_chain(chain, signer, psbt)
}

fn sign_with_master_signer_for_chain(
    chain: ChainId,
    signer: &crate::signer::Signer,
    psbt: Psbt,
) -> Result<Psbt, Error> {
    if !chain.is_blake2b() {
        return signer.sign_psbt(psbt).map_err(|e| {
            WalletError::MasterSigner(format!("Master signer failed to sign psbt: {}", e)).into()
        });
    }
    let unified = UnifiedPsbt::from_psbt(psbt).map_err(|e| {
        Error::from(WalletError::MasterSigner(format!(
            "Master signer refused the PSBT: {}",
            e
        )))
    })?;
    let signed = signer.sign_psbt_unified(&unified).map_err(|e| {
        Error::from(WalletError::MasterSigner(format!(
            "Master signer failed to sign psbt: {}",
            e
        )))
    })?;
    Ok(signed.psbt().clone())
}

/// Border Wallet signing keyed on the chain, same split as
/// [`sign_with_master_signer_for_chain`].
fn sign_border_wallet_for_chain(
    chain: ChainId,
    mnemonic: coincube_core::bip39::Mnemonic,
    fingerprint: Fingerprint,
    network: Network,
    psbt: Psbt,
) -> Result<(Fingerprint, Psbt), coincube_core::border_wallet::BorderWalletError> {
    if !chain.is_blake2b() {
        return sign_psbt_with_border_wallet(mnemonic, fingerprint, network, psbt);
    }
    let unified = UnifiedPsbt::from_psbt(psbt).map_err(|e| {
        coincube_core::border_wallet::BorderWalletError::SigningFailed(e.to_string())
    })?;
    let (fingerprint, signed) =
        sign_psbt_with_border_wallet_unified(mnemonic, fingerprint, network, &unified)?;
    Ok((fingerprint, signed.psbt().clone()))
}

async fn sign_psbt_with_master_signer(
    wallet: Arc<Wallet>,
    psbt: Psbt,
) -> (Fingerprint, Result<Psbt, Error>) {
    if let Some(signer) = &wallet.signer {
        let res = sign_with_master_signer_for_chain(wallet.chain, signer, psbt);
        (signer.fingerprint(), res)
    } else {
        (
            Fingerprint::default(),
            Err(WalletError::MasterSigner("Master signer not loaded".to_string()).into()),
        )
    }
}

async fn sign_psbt(
    wallet: Arc<Wallet>,
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    mut psbt: Psbt,
) -> Result<Psbt, Error> {
    // Sign against a copy pruned to the active spending path. Some signers
    // (e.g. the BitBox02) only produce a signature for a single key per Script,
    // so an unpruned PSBT — in which a signer's key appears on more than one
    // spending path (a primary `multi(...)` key and a recovery `pkh(...)` key) —
    // can lead them to sign the wrong path's key, which then can't satisfy the
    // path this transaction actually spends. Pruning removes the BIP32
    // derivations for the inactive paths so every signer signs the right key(s).
    // Applied to all devices, not just the BitBox02, so a future single-key
    // signer is safe by construction; multi-key signers are unaffected (they
    // just sign the active-path key instead of also signing an unused
    // recovery-path key).
    //
    // We prune a *clone* and merge the returned signatures back into the
    // original PSBT, so its full derivations are preserved in the daemon's
    // stored copy.
    let mut pruned_psbt = wallet
        .inheritance_descriptor()
        .ok_or_else(|| {
            Error::Unexpected("Use the imported wallet signer for this account.".into())
        })?
        .prune_bip32_derivs_last_avail(psbt.clone())
        .map_err(Error::Desc)?;
    hw.sign_tx(&mut pruned_psbt).await.map_err(Error::from)?;
    for (i, psbt_in) in psbt.inputs.iter_mut().enumerate() {
        if let Some(pruned_psbt_in) = pruned_psbt.inputs.get_mut(i) {
            psbt_in
                .partial_sigs
                .append(&mut pruned_psbt_in.partial_sigs);
            if let Some(tap_key_sig) = pruned_psbt_in.tap_key_sig {
                psbt_in.tap_key_sig = Some(tap_key_sig);
            }
            psbt_in
                .tap_script_sigs
                .append(&mut pruned_psbt_in.tap_script_sigs);
        } else {
            log::error!(
                "Not all PSBT inputs are present in the pruned psbt. Pruned psbt: '{}'.",
                pruned_psbt
            );
        }
    }
    Ok(psbt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{
            cache::Cache,
            state::vault::test_support::{empty_psbt, tokens},
            state::PsbtsPanel,
        },
        daemon::client::{Coincubed, Request},
        utils::{mock::Daemon, sandbox::Sandbox},
    };

    use coincube_core::descriptors::CoincubeDescriptor;
    use serde_json::json;
    use std::{path::PathBuf, str::FromStr};

    const DESC: &str = "wsh(or_d(multi(2,[f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<0;1>/*,[2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<0;1>/*),and_v(v:thresh(1,pkh([f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<2;3>/*),a:pkh([2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<2;3>/*)),older(65535))))#9s8ekrce";
    const GRID_PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn fingerprint(hex: &str) -> Fingerprint {
        Fingerprint::from_str(hex).expect("fingerprint")
    }

    fn wallet() -> Wallet {
        Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap())
    }

    fn focus_after_restore(
        task: Task<Message>,
        target: iced::advanced::widget::Id,
        mounted: bool,
        focused_elsewhere: bool,
    ) -> Option<iced::advanced::widget::Id> {
        use iced::advanced::{
            layout,
            renderer::Headless,
            widget::{operation, operation::focusable, Operation, Tree},
            Layout,
        };
        use iced::futures::StreamExt;
        for bytes in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let elsewhere = iced::advanced::widget::Id::new("other.sheet.input");
        let control = |id| {
            coincube_ui::component::button::secondary_action(
                id,
                None,
                "Control",
                view::Message::FocusAction,
            )
            .on_press(view::Message::Spend(view::SpendTxMessage::Cancel))
        };
        let mut column = iced::widget::Column::new().push(control(elsewhere.clone()));
        if mounted {
            column = column.push(control(target.clone()));
        }
        let mut element: Element<'_, view::Message> = column.into();
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, iced::Size::new(760.0, 600.0)),
        );
        let mut apply = |mut op: Box<dyn Operation<()>>| loop {
            element.as_widget_mut().operate(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &mut operation::black_box(op.as_mut()),
            );
            match op.finish() {
                operation::Outcome::Chain(next) => op = next,
                _ => break,
            }
        };
        if focused_elsewhere {
            apply(Box::new(focusable::focus(elsewhere)));
        }
        if let Some(mut stream) = iced_runtime::task::into_stream(task) {
            while let Some(action) = iced::futures::executor::block_on(stream.next()) {
                if let iced_runtime::Action::Widget(op) = action {
                    apply(op);
                    break;
                }
            }
        }
        let mut find: Box<dyn Operation<iced::advanced::widget::Id>> =
            Box::new(focusable::find_focused());
        loop {
            element.as_widget_mut().operate(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &mut operation::black_box(find.as_mut()),
            );
            match find.finish() {
                operation::Outcome::Some(id) => return Some(id),
                operation::Outcome::None => return None,
                operation::Outcome::Chain(next) => find = next,
            }
        }
    }

    #[test]
    fn psbt_focus_is_scoped_and_async_restore_respects_other_focus() {
        let fixture = crate::app::state::vault::test_support::unified::fixture();
        let make_state = || {
            let wallet = Arc::new(Wallet::new(fixture.descriptor.clone()));
            let tx = SpendTx::new(
                None,
                fixture.psbt.clone(),
                Vec::new(),
                &fixture.descriptor,
                &secp256k1::Secp256k1::verification_only(),
                Network::Signet,
            );
            PsbtState::new(wallet, tx, false)
        };
        let first = make_state();
        let second = make_state();
        let target = first.action_id("overview.sign");
        assert_ne!(target, second.action_id("overview.sign"));
        assert_eq!(
            focus_after_restore(
                restore_psbt_focus(target.clone()),
                target.clone(),
                true,
                false
            ),
            Some(target.clone())
        );
        assert_eq!(
            focus_after_restore(
                restore_psbt_focus(target.clone()),
                target.clone(),
                false,
                false
            ),
            None
        );
        assert_eq!(
            focus_after_restore(
                restore_psbt_focus(target.clone()),
                second.action_id("overview.sign"),
                true,
                false
            ),
            None
        );
        assert_eq!(
            focus_after_restore(restore_psbt_focus(target.clone()), target, true, true),
            Some(iced::advanced::widget::Id::new("other.sheet.input"))
        );
    }

    fn enter_phrase_words(state: &mut BorderWalletReconstructionState, phrase: &str) {
        for (i, word) in phrase.split_whitespace().enumerate() {
            state.update(BorderWalletReconMessage::PhraseWordEdited(
                i,
                word.to_string(),
            ));
        }
    }

    #[test]
    fn persisted_connect_identity_counts_as_available_session() {
        let cache = Cache {
            connect_email: Some("alice@example.com".to_string()),
            connect_tokens: Some(tokens()),
            ..Cache::default()
        };

        assert!(connect_session_available(&cache));
    }

    #[test]
    fn connect_session_available_accepts_live_or_persisted_session_markers() {
        assert!(!connect_session_available(&Cache::default()));

        assert!(connect_session_available(&Cache {
            connect_authenticated: true,
            ..Cache::default()
        }));
        assert!(connect_session_available(&Cache {
            has_connect_session: true,
            ..Cache::default()
        }));
        assert!(connect_session_available(&Cache {
            connect_email: Some("alice@example.com".to_string()),
            connect_tokens: Some(tokens()),
            ..Cache::default()
        }));

        assert!(!connect_session_available(&Cache {
            connect_email: Some("alice@example.com".to_string()),
            ..Cache::default()
        }));
        assert!(!connect_session_available(&Cache {
            connect_tokens: Some(tokens()),
            ..Cache::default()
        }));
    }

    #[test]
    fn keychain_connect_missing_reports_each_required_field() {
        assert_eq!(
            keychain_connect_missing(&Cache::default()),
            vec![
                "connect_grpc_url",
                "connect_tokens",
                "connect_device_id",
                "current_cube_server_id",
            ]
        );

        let ready = Cache {
            connect_grpc_url: Some("https://grpc.example.test".to_string()),
            connect_tokens: Some(tokens()),
            connect_device_id: Some("device-1".to_string()),
            current_cube_server_id: Some(42),
            ..Cache::default()
        };
        assert!(keychain_connect_missing(&ready).is_empty());
    }

    #[test]
    fn build_keychain_if_ready_requires_connect_fields() {
        let wallet = Arc::new(wallet());
        let psbt = empty_psbt();

        let (modal, _task) = build_keychain_if_ready(&Cache::default(), &wallet, &psbt);
        assert!(modal.is_none());

        let ready = Cache {
            connect_grpc_url: Some("https://grpc.example.test".to_string()),
            connect_tokens: Some(tokens()),
            connect_device_id: Some("device-1".to_string()),
            current_cube_server_id: Some(42),
            cube_id: "cube-local".to_string(),
            ..Cache::default()
        };
        let (modal, _task) = build_keychain_if_ready(&ready, &wallet, &psbt);
        assert!(modal.is_some());
    }

    #[test]
    fn counted_signers_drive_the_signed_row_state() {
        use view::vault::psbt::SigningKeyState;

        // Regression: a per-key row must read "Signed" only when its signature
        // is actually counted (present in the merged PSBT / `tx.sigs`), never
        // from an optimistic flag alone. `reconcile_and_maybe_close` feeds the
        // counted set here via `set_counted_signers`.
        let mut modal = SignModal::new(
            HashSet::new(),
            Arc::new(wallet()),
            CoincubeDirectory::new(PathBuf::new()),
            Network::Signet,
            false,
            None,
            None,
            false,
        );

        let signer = fingerprint("f714c228");
        let other = fingerprint("2522f23c");

        let primary_before = modal
            .signing_paths()
            .into_iter()
            .find(|p| p.is_primary)
            .unwrap();
        let row_before = primary_before
            .keys
            .iter()
            .find(|r| r.fingerprint == signer)
            .expect("signer is a primary key");
        assert!(
            !matches!(row_before.state, SigningKeyState::Signed),
            "uncounted key must not render as Signed"
        );

        modal.set_counted_signers(HashSet::from([signer]));

        let primary_after = modal
            .signing_paths()
            .into_iter()
            .find(|p| p.is_primary)
            .unwrap();
        let signer_row = primary_after
            .keys
            .iter()
            .find(|r| r.fingerprint == signer)
            .unwrap();
        assert!(
            matches!(signer_row.state, SigningKeyState::Signed),
            "counted key must render as Signed"
        );
        let other_row = primary_after
            .keys
            .iter()
            .find(|r| r.fingerprint == other)
            .unwrap();
        assert!(
            !matches!(other_row.state, SigningKeyState::Signed),
            "uncounted key must stay un-Signed"
        );
    }

    #[tokio::test]
    async fn master_signer_reports_error_when_not_loaded() {
        let (fingerprint, result) =
            sign_psbt_with_master_signer(Arc::new(wallet()), empty_psbt()).await;

        assert_eq!(fingerprint, Fingerprint::default());
        assert!(matches!(
            result,
            Err(Error::Wallet(WalletError::MasterSigner(_)))
        ));
    }

    #[test]
    fn border_wallet_reconstruction_rejects_bad_phrase_and_moves_to_grid_on_valid_phrase() {
        let mut state =
            BorderWalletReconstructionState::new(fingerprint("f714c228"), Network::Bitcoin);

        enter_phrase_words(
            &mut state,
            "not a valid mnemonic with twelve words total surely now extra words",
        );
        assert!(state.phrase_valid);
        assert!(state.update(BorderWalletReconMessage::Next).is_none());
        assert_eq!(state.step, ReconStep::RecoveryPhrase);
        assert!(state
            .error
            .as_deref()
            .is_some_and(|e| e.contains("Invalid recovery phrase")));

        let mut state =
            BorderWalletReconstructionState::new(fingerprint("f714c228"), Network::Bitcoin);
        enter_phrase_words(&mut state, GRID_PHRASE);
        assert!(state.phrase_valid);
        assert!(state.update(BorderWalletReconMessage::Next).is_none());
        assert_eq!(state.step, ReconStep::Grid);
        assert!(state.grid.is_some());
        assert!(state.pattern.is_empty());
        assert!(state.error.is_none());
    }

    fn sign_modal_for(wallet: Wallet) -> SignModal {
        SignModal::new(
            HashSet::new(),
            Arc::new(wallet),
            CoincubeDirectory::new(PathBuf::new()),
            Network::Signet,
            false,
            None,
            None,
            true,
        )
    }

    /// A Border Wallet key enrolled inside a Cube seeds its grid from a seed
    /// that Cube holds, so the reconstruction step must arrive with the twelve
    /// words already in it rather than asking the user to copy out something
    /// the Cube can derive. On a passkey Cube that seed comes back from the
    /// WebAuthn assertion at unlock, so there is nothing to type.
    #[test]
    fn border_wallet_reconstruction_prefills_the_derived_grid_phrase() {
        let signer = crate::signer::Signer::generate(Network::Bitcoin).unwrap();
        // The same call the installer's wizard makes on "Generate" — the two
        // have to agree exactly or the prefill is twelve wrong words.
        let expected = signer.derive_grid_recovery_phrase().unwrap();

        let mut state =
            BorderWalletReconstructionState::new(fingerprint("f714c228"), Network::Bitcoin);
        assert!(state.phrase_words.iter().all(|w| w.value.is_empty()));
        assert!(!state.phrase_prefilled);

        state.prefill_phrase(&expected);

        let filled = state
            .phrase_words
            .iter()
            .map(|w| w.value.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(filled, expected.as_str());
        assert!(state.phrase_valid);
        assert!(state.phrase_prefilled);

        // The grid seed is not the key. The pattern is still the user's to
        // supply, so `Next` only opens the grid.
        assert!(state.update(BorderWalletReconMessage::Next).is_none());
        assert_eq!(state.step, ReconStep::Grid);
        assert!(state.grid.is_some());
        assert!(state.pattern.is_empty());
        assert!(state.error.is_none());
    }

    /// The step may only claim a derivation while the words still *are* the
    /// derived ones. Editing any of the twelve makes that claim false, and a
    /// stale "re-derived from the seed it was created with" reads as
    /// confirmation that whatever is now in the fields is correct.
    ///
    /// `phrase_valid` cannot stand in for this: it means "all twelve fields are
    /// non-empty", so it is still true after an edit.
    #[test]
    fn editing_a_prefilled_word_drops_the_derived_claim() {
        let signer = crate::signer::Signer::generate(Network::Bitcoin).unwrap();
        let phrase = signer.derive_grid_recovery_phrase().unwrap();

        let mut state =
            BorderWalletReconstructionState::new(fingerprint("f714c228"), Network::Bitcoin);
        state.prefill_phrase(&phrase);
        assert!(state.phrase_prefilled);

        state.update(BorderWalletReconMessage::PhraseWordEdited(
            4,
            "satoshi".to_string(),
        ));
        assert!(
            !state.phrase_prefilled,
            "the words are no longer the ones we derived"
        );
        assert!(
            state.phrase_valid,
            "all twelve fields are still non-empty, which is why phrase_valid \
             cannot gate the claim"
        );
    }

    /// A Vault with no signer on this machine — every key a hardware wallet, a
    /// Keychain cosigner or a Contact's — has nothing to derive from, so no
    /// phrase is offered and the user types it, as they always did.
    #[test]
    fn no_grid_phrase_is_offered_without_a_signer() {
        let modal = sign_modal_for(wallet());
        assert!(modal.wallet.signer.is_none());
        assert!(modal
            .border_wallet_grid_phrase(&fingerprint("f714c228"))
            .is_none());
    }

    /// "No credential was tried" and "the credential was rejected" are
    /// different facts and must not share a message.
    ///
    /// They shared one set once, so a Vault loaded without a session PIN — the
    /// state every Recovery-Kit restore landed in on its first run — told the
    /// user their PIN had failed when no PIN had been attempted. That sends
    /// them to re-check a credential that was never tested and hides the actual
    /// remedy.
    #[test]
    fn an_untried_seed_is_not_reported_as_a_rejected_one() {
        use view::vault::psbt::SigningKeyState as St;

        let rejected = fingerprint("f714c228");
        let untried = fingerprint("2522f23c");

        let mut wallet = wallet();
        wallet.unopenable_seed_keys.insert(rejected);
        wallet.locked_seed_keys.insert(untried);
        let modal = sign_modal_for(wallet);

        let message = |fp| match modal.classify_signing_key(fp, true).1 {
            St::Disabled(m) => m,
            _ => panic!("expected a disabled row for a seed we hold but cannot use"),
        };

        let rejected_msg = message(rejected);
        assert!(
            rejected_msg.contains("didn't unlock it"),
            "{}",
            rejected_msg
        );

        let untried_msg = message(untried);
        assert!(
            untried_msg.contains("Reopen this Cube"),
            "the remedy is to unlock, not to doubt the PIN: {}",
            untried_msg
        );
        assert!(
            !untried_msg.contains("didn't unlock"),
            "nothing was tried, so nothing failed: {}",
            untried_msg
        );
    }

    /// Having *a* signer is not enough — it has to be the one this key was
    /// enrolled from.
    ///
    /// Covers all four shapes the provenance can take, including the two the
    /// loaded-hot-signer heuristic got wrong: a key whose grid seed was typed
    /// by hand, and a key derived from some *other* seed (a Vault reusing an
    /// earlier Vault's key), where deriving off `Wallet::signer` would produce
    /// twelve confident, wrong words.
    #[test]
    fn a_grid_phrase_is_only_offered_for_the_seed_it_was_enrolled_from() {
        use crate::app::settings::GridSeedSource;

        let hot = crate::signer::Signer::generate(Network::Signet).unwrap();
        let hot_fg = hot.fingerprint();
        let expected = hot.derive_grid_recovery_phrase().unwrap();

        let from_hot = fingerprint("f714c228");
        let from_elsewhere = fingerprint("2522f23c");
        let hand_typed = fingerprint("8a64f2a9");
        let unrecorded = fingerprint("8cb54888");
        // A seed this machine does not hold — no hot signer, no session.
        let absent_seed = fingerprint("deadbeef");

        let mut wallet = wallet().with_signer(hot);
        wallet.border_wallet_fingerprints.extend([
            from_hot,
            from_elsewhere,
            hand_typed,
            unrecorded,
        ]);
        wallet.border_wallet_grid_seed.extend([
            (
                from_hot,
                GridSeedSource::MasterDerived {
                    fingerprint: hot_fg,
                },
            ),
            (
                from_elsewhere,
                GridSeedSource::MasterDerived {
                    fingerprint: absent_seed,
                },
            ),
            (hand_typed, GridSeedSource::Independent),
        ]);
        // `unrecorded` gets no entry on purpose: enrolled before the wizard
        // tracked provenance.

        let modal = sign_modal_for(wallet);

        assert_eq!(
            modal
                .border_wallet_grid_phrase(&from_hot)
                .map(|p| p.as_str().to_string()),
            Some(expected.as_str().to_string()),
            "the seed named at enrolment is loaded, so the phrase is exactly reproducible"
        );
        assert!(
            modal.border_wallet_grid_phrase(&from_elsewhere).is_none(),
            "a different seed derived this key; deriving off the loaded one would be wrong words"
        );
        assert!(
            modal.border_wallet_grid_phrase(&hand_typed).is_none(),
            "a random or hand-typed grid seed is not ours to guess"
        );
        assert_eq!(
            modal
                .border_wallet_grid_phrase(&unrecorded)
                .map(|p| p.as_str().to_string()),
            Some(expected.as_str().to_string()),
            "unrecorded provenance is not a recorded 'no' — it predates the field"
        );

        // And a refusal really does leave the form empty.
        let mut state = BorderWalletReconstructionState::new(hand_typed, Network::Signet);
        if let Some(phrase) = modal.border_wallet_grid_phrase(&hand_typed) {
            state.prefill_phrase(&phrase);
        }
        assert!(state.phrase_words.iter().all(|w| w.value.is_empty()));
        assert!(!state.phrase_prefilled);
    }

    #[test]
    fn a_grid_source_uses_only_its_recorded_cube_session_seed() {
        use crate::app::{session, settings::GridSeedSource};
        use coincube_core::{miniscript::bitcoin, signer::MasterSigner};
        let _guard = session::test_guard();
        session::close();
        let cube_id = "synthetic-legacy-grid-761";
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let source = MasterSigner::generate(Network::Signet).unwrap();
        let source_fp = source.fingerprint(&secp);
        let expected = GridRecoveryPhrase::from_master_signer(&source, &secp).unwrap();
        let border_key = fingerprint("f714c228");
        // An unrelated imported/historical hot key must never replace the
        // recorded Cube master as the grid's BIP-85 source.
        let hot = crate::signer::Signer::generate(Network::Signet).unwrap();
        let mut wallet = wallet().with_signer(hot);
        wallet.border_wallet_grid_seed.insert(
            border_key,
            GridSeedSource::MasterDerived {
                fingerprint: source_fp,
            },
        );
        let modal = sign_modal_for(wallet);
        assert!(modal.border_wallet_grid_phrase(&border_key).is_none());
        session::open_without_pin(cube_id);
        session::store_unlocked_signer(cube_id, source_fp, source);
        assert!(modal
            .border_wallet_grid_phrase(&border_key)
            .is_some_and(|phrase| phrase.as_str() == expected.as_str()));
        let other = MasterSigner::generate(Network::Signet).unwrap();
        session::store_unlocked_signer(cube_id, other.fingerprint(&secp), other);
        assert!(modal.border_wallet_grid_phrase(&border_key).is_none());
        session::close();
        assert!(modal.border_wallet_grid_phrase(&border_key).is_none());
    }

    #[test]
    fn border_wallet_pattern_messages_update_checksum_and_errors() {
        let mut state =
            BorderWalletReconstructionState::new(fingerprint("f714c228"), Network::Bitcoin);
        enter_phrase_words(&mut state, GRID_PHRASE);
        state.update(BorderWalletReconMessage::Next);

        state.update(BorderWalletReconMessage::Next);
        assert!(state
            .error
            .as_deref()
            .is_some_and(|e| e.contains("Please select exactly")));

        state.update(BorderWalletReconMessage::ToggleCell(0, 0));
        assert_eq!(state.pattern.len(), 1);
        assert!(state.error.is_none());
        assert!(state.checksum_word.is_none());

        state.update(BorderWalletReconMessage::ToggleCell(0, 0));
        assert!(state.pattern.is_empty());
        assert!(state.checksum_word.is_none());

        for row in 0..PATTERN_LENGTH as u16 {
            state.update(BorderWalletReconMessage::ToggleCell(row, 0));
        }
        assert!(state.pattern.is_complete());
        assert!(state.checksum_word.is_some());

        state.update(BorderWalletReconMessage::UndoLastCell);
        assert_eq!(state.pattern.len(), PATTERN_LENGTH - 1);
        assert!(state.checksum_word.is_none());

        state.update(BorderWalletReconMessage::ClearPattern);
        assert!(state.pattern.is_empty());

        state.update(BorderWalletReconMessage::Previous);
        assert_eq!(state.step, ReconStep::RecoveryPhrase);
    }

    #[test]
    fn signing_paths_classify_active_signed_border_and_inactive_keys() {
        use view::vault::psbt::{SigningKeyAction, SigningKeyKind, SigningKeyState};

        let primary_fp = fingerprint("f714c228");
        let border_fp = fingerprint("2522f23c");
        let mut wallet = wallet();
        wallet
            .keys_aliases
            .insert(primary_fp, "Primary alias".to_string());
        wallet.border_wallet_fingerprints.insert(border_fp);

        let modal = SignModal::new(
            HashSet::from([primary_fp]),
            Arc::new(wallet),
            CoincubeDirectory::new(PathBuf::new()),
            Network::Signet,
            false,
            None,
            None,
            true,
        );

        let paths = modal.signing_paths();
        assert_eq!(paths.len(), 2);

        let primary = paths.iter().find(|p| p.is_primary).unwrap();
        assert!(primary.active);
        assert!(primary.expanded);
        assert_eq!(primary.threshold, 2);
        assert_eq!(primary.total, 2);
        assert_eq!(primary.collected, 1);

        let primary_row = primary
            .keys
            .iter()
            .find(|row| row.fingerprint == primary_fp)
            .unwrap();
        assert_eq!(primary_row.label, "Primary alias");
        assert!(matches!(&primary_row.state, SigningKeyState::Signed));

        let border_row = primary
            .keys
            .iter()
            .find(|row| row.fingerprint == border_fp)
            .unwrap();
        assert!(matches!(&border_row.kind, SigningKeyKind::BorderWallet));
        assert!(matches!(
            &border_row.state,
            SigningKeyState::Available(SigningKeyAction::BorderWallet)
        ));

        let recovery = paths.iter().find(|p| !p.is_primary).unwrap();
        assert!(!recovery.active);
        assert!(!recovery.expanded);
        assert_eq!(recovery.sequence, Some(65535));
        let signed_recovery_row = recovery
            .keys
            .iter()
            .find(|row| row.fingerprint == primary_fp)
            .unwrap();
        assert!(matches!(
            &signed_recovery_row.state,
            SigningKeyState::Signed
        ));
        let unsigned_recovery_row = recovery
            .keys
            .iter()
            .find(|row| row.fingerprint == border_fp)
            .unwrap();
        assert!(matches!(
            &unsigned_recovery_row.state,
            SigningKeyState::Disabled(reason) if reason.contains("isn't available")
        ));
    }

    #[test]
    fn signing_paths_prompt_unknown_keys_to_sign_in_only_when_keychain_is_enabled() {
        use view::vault::psbt::SigningKeyState;

        let keychain_enabled = SignModal::new(
            HashSet::new(),
            Arc::new(wallet()),
            CoincubeDirectory::new(PathBuf::new()),
            Network::Signet,
            false,
            None,
            None,
            true,
        );
        let primary = keychain_enabled
            .signing_paths()
            .into_iter()
            .find(|p| p.is_primary)
            .unwrap();
        assert!(primary.keys.iter().all(|row| matches!(
            &row.state,
            SigningKeyState::NeedsSignIn(reason) if reason.contains("Connect a device")
        )));

        let local_only = SignModal::new(
            HashSet::new(),
            Arc::new(wallet()),
            CoincubeDirectory::new(PathBuf::new()),
            Network::Signet,
            false,
            None,
            None,
            false,
        );
        let primary = local_only
            .signing_paths()
            .into_iter()
            .find(|p| p.is_primary)
            .unwrap();
        assert!(primary.keys.iter().all(|row| matches!(
            &row.state,
            SigningKeyState::Disabled(reason) if reason.contains("Connect this signing device")
        )));
    }

    #[test]
    fn signing_paths_ask_a_signed_in_non_owner_to_switch_accounts() {
        use view::vault::psbt::SigningKeyState;

        let primary_states = |status: ConnectAccountStatus| {
            SignModal::new(
                HashSet::new(),
                Arc::new(wallet()),
                CoincubeDirectory::new(PathBuf::new()),
                Network::Signet,
                false,
                None,
                None,
                true,
            )
            .with_connect_account(status)
            .signing_paths()
            .into_iter()
            .find(|p| p.is_primary)
            .unwrap()
            .keys
            .into_iter()
            .map(|row| row.state)
            .collect::<Vec<_>>()
        };

        // Never names the owner — only that it's a different account.
        let other = primary_states(ConnectAccountStatus::OtherAccount);
        assert!(other.iter().all(|s| matches!(
            s,
            SigningKeyState::NeedsAccountSwitch(reason)
                if reason.contains("different Connect account") && !reason.contains('@')
        )));

        let unknown = primary_states(ConnectAccountStatus::SignedInUnknownOwner);
        assert!(unknown
            .iter()
            .all(|s| matches!(s, SigningKeyState::NeedsAccountSwitch(_))));

        // Signed out, or the owner while Keychain is still coming up: keep the
        // "Sign in to Connect" affordance rather than a dead-end hint.
        for status in [ConnectAccountStatus::SignedOut, ConnectAccountStatus::Owner] {
            assert!(primary_states(status)
                .iter()
                .all(|s| matches!(s, SigningKeyState::NeedsSignIn(_))));
        }
    }

    #[test]
    fn connect_account_status_compares_the_session_to_the_cube_owner() {
        let owner = crate::app::settings::ConnectOwner {
            user_id: 1,
            email: "owner@example.com".to_string(),
        };
        let cache =
            |owner: Option<crate::app::settings::ConnectOwner>, email: Option<&str>| Cache {
                has_connect_session: email.is_some(),
                connect_email: email.map(str::to_string),
                cube_connect_owner: owner,
                ..Cache::default()
            };

        assert_eq!(
            ConnectAccountStatus::from_cache(&cache(Some(owner.clone()), None)),
            ConnectAccountStatus::SignedOut
        );
        assert_eq!(
            ConnectAccountStatus::from_cache(&cache(
                Some(owner.clone()),
                Some("OWNER@example.com")
            )),
            ConnectAccountStatus::Owner
        );
        assert_eq!(
            ConnectAccountStatus::from_cache(&cache(
                Some(owner.clone()),
                Some("other@example.com")
            )),
            ConnectAccountStatus::OtherAccount
        );
        assert_eq!(
            ConnectAccountStatus::from_cache(&cache(None, Some("other@example.com"))),
            ConnectAccountStatus::SignedInUnknownOwner
        );

        // Session flags that outlived a sign-out, with no account email: this
        // is signed out, not the owner (it used to drop the sign-in button).
        let stale = Cache {
            has_connect_session: true,
            ..cache(Some(owner), None)
        };
        assert_eq!(
            ConnectAccountStatus::from_cache(&stale),
            ConnectAccountStatus::SignedOut
        );

        // The server's refusal wins, even with no recorded owner.
        let refused = Cache {
            cube_owned_by_other_account: true,
            ..cache(None, Some("other@example.com"))
        };
        assert_eq!(
            ConnectAccountStatus::from_cache(&refused),
            ConnectAccountStatus::OtherAccount
        );
    }

    /// Answers with a fixed fingerprint; the picker tests never let it sign.
    #[derive(Debug)]
    struct StubDevice(Fingerprint);

    #[async_trait::async_trait]
    impl async_hwi::HWI for StubDevice {
        fn device_kind(&self) -> async_hwi::DeviceKind {
            async_hwi::DeviceKind::Specter
        }
        async fn get_version(&self) -> Result<async_hwi::Version, async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
        async fn get_master_fingerprint(&self) -> Result<Fingerprint, async_hwi::Error> {
            Ok(self.0)
        }
        async fn get_extended_pubkey(
            &self,
            _path: &coincube_core::miniscript::bitcoin::bip32::DerivationPath,
        ) -> Result<coincube_core::miniscript::bitcoin::bip32::Xpub, async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
        async fn register_wallet(
            &self,
            _name: &str,
            _policy: &str,
        ) -> Result<Option<[u8; 32]>, async_hwi::Error> {
            Ok(None)
        }
        async fn is_wallet_registered(
            &self,
            _name: &str,
            _policy: &str,
        ) -> Result<bool, async_hwi::Error> {
            Ok(true)
        }
        async fn display_address(
            &self,
            _script: &async_hwi::AddressScript,
        ) -> Result<(), async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
        async fn sign_tx(&self, _psbt: &mut Psbt) -> Result<(), async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
    }

    /// A device-list entry for `fp` under `id` — `phone-…` for a Keychain
    /// phone on the local network, a vendor prefix for a USB wallet.
    fn device(id: &str, fp: Fingerprint) -> HardwareWallet {
        HardwareWallet::Supported {
            id: id.to_string(),
            device: Arc::new(StubDevice(fp)),
            kind: async_hwi::DeviceKind::Specter,
            fingerprint: fp,
            version: None,
            registered: Some(true),
            alias: None,
        }
    }

    const PHONE_FP: &str = "f714c228";
    const USB_FP: &str = "2522f23c";

    /// The DESC primary path with a Keychain phone on the LAN holding
    /// `PHONE_FP` (device index 0) and a USB wallet holding `USB_FP` (index 1).
    /// `connect` is the Connect session for the phone's key, if Connect
    /// resolved it.
    fn picker_with_phone(
        connect: Option<super::super::keychain_sign::PendingSessionStatus>,
    ) -> SignModal {
        let wallet = Arc::new(wallet());
        let keychain = connect.map(|status| {
            super::super::keychain_sign::KeychainSignModal::resolved_for_test(
                wallet.clone(),
                &[(fingerprint(PHONE_FP), status)],
            )
        });
        let mut modal = SignModal::new(
            HashSet::new(),
            wallet,
            CoincubeDirectory::new(PathBuf::new()),
            Network::Signet,
            false,
            None,
            keychain,
            true,
        );
        modal.hws.list = vec![
            device(
                &format!("{}0a0b0c0d", crate::hw::LAN_PHONE_ID_PREFIX),
                fingerprint(PHONE_FP),
            ),
            device("ledger-usb-1", fingerprint(USB_FP)),
        ];
        modal
    }

    #[test]
    fn psbt_device_selection_uses_fingerprint_after_list_reorders() {
        let mut modal = picker_with_phone(None);
        modal.hws.list.swap(0, 1);
        let (daemon, mut tx) = update_args();
        let request = Message::View(view::Message::SelectPsbtHardwareWallet(fingerprint(USB_FP)));
        let _ = modal.update(daemon, request, &mut tx);
        assert!(modal.signing.contains(&fingerprint(USB_FP)));
        assert!(!modal.signing.contains(&fingerprint(PHONE_FP)));
    }

    #[test]
    fn missing_signing_device_keeps_the_explanation_in_the_modal() {
        for request in [
            view::Message::SelectHardwareWallet(0),
            view::Message::SelectPsbtHardwareWallet(fingerprint(USB_FP)),
        ] {
            let mut modal = picker_with_phone(None);
            modal.hws.list.clear();
            let (daemon, mut tx) = update_args();
            let _ = modal.update(daemon, Message::View(request), &mut tx);
            assert!(modal
                .signer_notice
                .as_deref()
                .is_some_and(|notice| notice.contains("Reconnect or unlock it")));
            assert!(modal.signing.is_empty());
        }
    }

    /// HW-1 item 7: on a Bitcoin Blake2b Vault, the picker shows the Split
    /// flow's device copy before a listed hardware device signs. A Bitcoin
    /// Vault, a fork Vault with no device listed, and a Keychain phone on
    /// the local network get no such copy.
    #[test]
    fn device_copy_shows_only_when_a_device_can_sign_a_bitcoin_blake2b_spend() {
        use crate::split_hardware::sign::DEVICE_SHOWS_BITCOIN_WARNING;
        let picker = |chain: ChainId, devices: Vec<HardwareWallet>| {
            let wallet = Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap()).with_chain(chain);
            let mut modal = sign_modal_for(wallet);
            modal.hws.list = devices;
            modal
        };
        let notices = |modal: &SignModal| modal.sign_notices(&modal.signing_paths());
        let usb = || device("ledger-usb-1", fingerprint(USB_FP));
        let phone = || {
            device(
                &format!("{}0a0b0c0d", crate::hw::LAN_PHONE_ID_PREFIX),
                fingerprint(PHONE_FP),
            )
        };

        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let with_device = picker(chain, vec![usb()]);
            assert_eq!(
                notices(&with_device),
                vec![DEVICE_SHOWS_BITCOIN_WARNING.to_string()],
                "{:?}",
                chain
            );
            assert!(
                notices(&picker(chain, Vec::new())).is_empty(),
                "{:?}",
                chain
            );
            assert!(
                notices(&picker(chain, vec![phone()])).is_empty(),
                "{:?}",
                chain
            );
        }
        for chain in [ChainId::Bitcoin, ChainId::Testnet4, ChainId::Signet] {
            assert!(
                notices(&picker(chain, vec![usb()])).is_empty(),
                "{:?}",
                chain
            );
        }
        assert!(DEVICE_SHOWS_BITCOIN_WARNING
            .starts_with("Your hardware wallet will call this a Bitcoin transaction."));
    }

    fn primary_row(
        modal: &SignModal,
        fp: &str,
    ) -> (
        view::vault::psbt::SigningKeyKind,
        view::vault::psbt::SigningKeyState,
    ) {
        let row = modal
            .signing_paths()
            .into_iter()
            .find(|p| p.is_primary)
            .unwrap()
            .keys
            .into_iter()
            .find(|row| row.fingerprint == fingerprint(fp))
            .unwrap();
        (row.kind, row.state)
    }

    /// What `SignModal::update` needs besides the message. Neither is touched
    /// on the paths these tests drive (a failure, or a device dispatch).
    fn update_args() -> (Arc<dyn crate::daemon::Daemon + Sync + Send>, SpendTx) {
        let f = crate::app::state::vault::test_support::unified::fixture();
        let tx = SpendTx::new(
            None,
            f.psbt.clone(),
            Vec::new(),
            &f.descriptor,
            &coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new(),
            Network::Signet,
        );
        (Arc::new(Coincubed::new(Daemon::new(vec![]).run())), tx)
    }

    /// Dispatch the device holding `fp` (its index in the device list), then
    /// deliver `error` as that attempt's result — the order the picker sees.
    fn fail_signing(modal: &mut SignModal, fp: &str, error: async_hwi::Error) {
        let i = modal
            .hws
            .list
            .iter()
            .position(|hw| hw.fingerprint() == Some(fingerprint(fp)))
            .expect("a device holds this key");
        let (daemon, mut tx) = update_args();
        let _ = modal.update(
            daemon.clone(),
            Message::View(view::Message::SelectHardwareWallet(i)),
            &mut tx,
        );
        let _ = modal.update(
            daemon,
            Message::Signed(fingerprint(fp), Err(Error::HardwareWallet(error))),
            &mut tx,
        );
    }

    /// A phone whose connection died has usually left the device list by the
    /// time its failure lands. That is still a Wi-Fi failure: while the phone
    /// is gone its row is Connect's, and when it reappears the row offers
    /// Connect instead of silently retrying the same road.
    #[test]
    fn a_phone_that_drops_off_the_list_still_counts_as_a_wifi_failure() {
        use super::super::keychain_sign::PendingSessionStatus;
        use view::vault::psbt::{
            SigningKeyAction as Act, SigningKeyKind as Kind, SigningKeyState as St,
        };

        let mut modal = picker_with_phone(Some(PendingSessionStatus::Idle));
        let (daemon, mut tx) = update_args();
        let _ = modal.update(
            daemon.clone(),
            Message::View(view::Message::SelectHardwareWallet(0)),
            &mut tx,
        );
        let phone = modal.hws.list.remove(0);
        let _ = modal.update(
            daemon,
            Message::Signed(
                fingerprint(PHONE_FP),
                Err(Error::HardwareWallet(async_hwi::Error::DeviceDisconnected)),
            ),
            &mut tx,
        );
        assert_eq!(modal.lan_failures.get(&fingerprint(PHONE_FP)), Some(&false));
        let (kind, state) = primary_row(&modal, PHONE_FP);
        assert!(matches!(kind, Kind::Keychain));
        assert!(matches!(state, St::Available(Act::Keychain)));

        modal.hws.list.insert(0, phone);
        assert!(matches!(
            primary_row(&modal, PHONE_FP).1,
            St::LanFailed {
                retry: Some(0),
                connect: true,
                ..
            }
        ));
    }

    /// The phone is a Keychain key reached over Wi-Fi — never labelled a
    /// hardware wallet — and signs through its device-list entry. A USB
    /// wallet beside it keeps its hardware label.
    #[test]
    fn a_lan_phone_reads_as_keychain_and_signs_over_the_local_network() {
        use super::super::keychain_sign::PendingSessionStatus;
        use view::vault::psbt::{
            SigningKeyAction as Act, SigningKeyKind as Kind, SigningKeyState as St,
        };

        for connect in [None, Some(PendingSessionStatus::Idle)] {
            let modal = picker_with_phone(connect);
            let (kind, state) = primary_row(&modal, PHONE_FP);
            assert!(matches!(kind, Kind::LanKeychain), "{:?}", connect);
            assert!(
                matches!(state, St::Available(Act::Hardware(0))),
                "{:?}",
                connect
            );

            let (kind, state) = primary_row(&modal, USB_FP);
            assert!(matches!(kind, Kind::Hardware));
            assert!(matches!(state, St::Available(Act::Hardware(1))));
        }
    }

    /// Once a Connect session for the key is under way, Connect owns the row:
    /// the user sees its progress and nothing is also offered over Wi-Fi.
    #[test]
    fn a_connect_session_under_way_owns_the_phone_row() {
        use super::super::keychain_sign::PendingSessionStatus;
        use view::vault::psbt::{SigningKeyKind as Kind, SigningKeyState as St};

        let modal = picker_with_phone(Some(PendingSessionStatus::Pending));
        let (kind, state) = primary_row(&modal, PHONE_FP);
        assert!(matches!(kind, Kind::Keychain));
        assert!(matches!(state, St::InProgress(_)));

        let modal = picker_with_phone(Some(PendingSessionStatus::Rejected));
        let (kind, state) = primary_row(&modal, PHONE_FP);
        assert!(matches!(kind, Kind::Keychain));
        assert!(matches!(state, St::Retry(0)));
    }

    /// A failed Wi-Fi attempt offers Connect (when it resolved the key) and
    /// another Wi-Fi attempt — except when the phone asked to be paired
    /// again, where retrying the same road can't work. Trying Wi-Fi again
    /// clears the failure.
    #[test]
    fn a_failed_wifi_attempt_offers_connect_instead() {
        use super::super::keychain_sign::PendingSessionStatus;
        use view::vault::psbt::{SigningKeyKind as Kind, SigningKeyState as St};

        let mut modal = picker_with_phone(Some(PendingSessionStatus::Idle));
        fail_signing(
            &mut modal,
            PHONE_FP,
            async_hwi::Error::Device("sign_tx timeout".into()),
        );
        let (kind, state) = primary_row(&modal, PHONE_FP);
        assert!(matches!(kind, Kind::LanKeychain));
        assert!(matches!(
            state,
            St::LanFailed {
                retry: Some(0),
                connect: true,
                ..
            }
        ));

        fail_signing(
            &mut modal,
            PHONE_FP,
            async_hwi::Error::Device(
                "pair_again: This LAN pairing has no current exact signer identity.".into(),
            ),
        );
        let (_, state) = primary_row(&modal, PHONE_FP);
        assert!(matches!(
            state,
            St::LanFailed { retry: None, connect: true, ref reason } if reason.contains("paired again")
        ));

        let (daemon, mut tx) = update_args();
        let _ = modal.update(
            daemon,
            Message::View(view::Message::SelectHardwareWallet(0)),
            &mut tx,
        );
        let (kind, state) = primary_row(&modal, PHONE_FP);
        assert!(matches!(kind, Kind::LanKeychain));
        assert!(
            matches!(state, St::InProgress(_)),
            "retry clears the failure"
        );
    }

    /// Without Connect there is nothing to fall back to: the row offers only
    /// another Wi-Fi attempt, or just the reason when the phone needs pairing.
    #[test]
    fn without_connect_a_failed_wifi_attempt_offers_no_fallback() {
        use view::vault::psbt::SigningKeyState as St;

        let mut modal = picker_with_phone(None);
        fail_signing(&mut modal, PHONE_FP, async_hwi::Error::DeviceDisconnected);
        assert!(matches!(
            primary_row(&modal, PHONE_FP).1,
            St::LanFailed {
                retry: Some(0),
                connect: false,
                ..
            }
        ));

        fail_signing(
            &mut modal,
            PHONE_FP,
            async_hwi::Error::Device("pair_again: stale".into()),
        );
        assert!(matches!(
            primary_row(&modal, PHONE_FP).1,
            St::LanFailed {
                retry: None,
                connect: false,
                ..
            }
        ));
    }

    /// A USB wallet's failure is not a Wi-Fi failure: its row is unchanged.
    #[test]
    fn a_usb_failure_does_not_offer_the_connect_fallback() {
        use view::vault::psbt::{SigningKeyAction as Act, SigningKeyState as St};

        let mut modal = picker_with_phone(None);
        fail_signing(&mut modal, USB_FP, async_hwi::Error::DeviceDisconnected);
        assert!(modal.lan_failures.is_empty());
        assert!(matches!(
            primary_row(&modal, USB_FP).1,
            St::Available(Act::Hardware(1))
        ));
    }

    #[tokio::test]
    async fn test_update_psbt() {
        let daemon = Daemon::new(vec![
            (
                Some(json!({"method": "getinfo", "params": Option::<Request>::None})),
                Ok(json!({
                    "version": "",
                    "network": "signet",
                    "block_height": 1000,
                    "sync": 1.0,
                    "descriptors": { "main": CoincubeDescriptor::from_str(DESC).unwrap() },
                    "receive_index": 4,
                    "change_index": 3,
                    "timestamp": 1000,
                })),
            ),
            (
                Some(json!({"method": "listspendtxs", "params": Option::<Request>::None})),
                Ok(json!({ "spend_txs": [{
                    "psbt": "cHNidP8BAIkCAAAAAc0x/jtWvFugrl8zc34KVIlWCugXT6JNtgir6UqX+Vv6AQAAAAD9////AkBCDwAAAAAAIgAgtQu/fA/8rQhJ0I6wUoBDO0vNa3lgsEpEIj7rTOMnBcXuIEkBAAAAACIAIOdCiXh7yL2V/f6S6KMTOzgqKkqyIXgmFuwDnmXbIiosAAAAAAABAP04AQIAAAAAAQKYYriMs/PtSqm6LPNWWFYskTL6nWZegJdwxYcVCRn8vwEAAAAA/f///87D7dkdgMd1Laj/v6xspNRtrQXGP+8BPFMLqkeBb6MRAQAAAAD9////AuGQDgAAAAAAIlEg7DgdNxI7WybaPUZXcMCh+uN1E4X8E5DzJIlj83S+tIMQZFgBAAAAACIAIJZAn7j5iOen7xo2sKzjMc24llTZIuS+RpdwcLHtE6ufAUCksqYUJBbHB9x8eHdoRvRqiGzG4wQXpmY96vh14zAJEM2CS/oZaNVC4Wj8rY2cdjAvZj9dlVZFPbOxx9g5tFxUAUA24s2KJ7sjSHUAcUSd4yqRK/G3CZM8qhkhyHhGDSS0zZvZaIcgoqOPe23gH32wAI9Aax1gJUDv4kKOqOx64ltg9BADAAEBKxBkWAEAAAAAIgAglkCfuPmI56fvGjawrOMxzbiWVNki5L5Gl3Bwse0Tq58BBYZSIQIeYxzruE4/cvi6zbRmB1asJO0bMfUutoH0bpubw1zAZSEDLZSmORZKW/k5A+4QxJR2/H+vcV8U0WPX9SvS+MRMffNSrnNkdqkUmNf1mL657o/oxxnHkIrtdNkbge+IrGt2qRSIigBO15eaB9dj93ihNpAX9HHDuoisbJNRiAP//wCyaCIGAh5jHOu4Tj9y+LrNtGYHVqwk7Rsx9S62gfRum5vDXMBlHPcUwigwAACAAQAAgAAAAIACAACAAAAAAAAAAAAiBgIr7HqsyKEvERWQsmsv6FleMuXThpI77+TVkQ3TSOOLURz3FMIoMAAAgAEAAIAAAACAAgAAgAIAAAAAAAAAIgYDLZSmORZKW/k5A+4QxJR2/H+vcV8U0WPX9SvS+MRMffMcJSLyPDAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA/h0pUXGHq1+kSuTYVTO8RHKfQLJlhfNtm+qdcIIr09jHCUi8jwwAACAAQAAgAAAAIACAACAAgAAAAAAAAAAIgICGAO/4xFiX/S5DXTV6uARFTcMwP1hto8BtPkdn3gIjf0c9xTCKDAAAIABAACAAAAAgAIAAIACAAAAAgAAACICAuNOSbsNRv31XkF2ygwCOuCnsJNRLhV0isJ/VRdj1k7IHPcUwigwAACAAQAAgAAAAIACAACAAAAAAAIAAAAiAgOpBJHEchNOeXuQwuLHlwOfkAyfoGvrYfb4pCFLKEPw2hwlIvI8MAAAgAEAAIAAAACAAgAAgAIAAAACAAAAIgIDyLkJiZTjLCysDOQotYs9us5CEYev4kyTYW2uL2r5H1McJSLyPDAAAIABAACAAAAAgAIAAIAAAAAAAgAAAAAiAgIlvGBvHRPmmVP6sn9g/akW2VJAvbJagMnZ/24gLdITsxz3FMIoMAAAgAEAAIAAAACAAgAAgAMAAAADAAAAIgIDNmVQOMMezQgABjk1zjfc3I2eKFJ4xLqT55jG4BP4p0Ec9xTCKDAAAIABAACAAAAAgAIAAIABAAAAAwAAACICA4Subm7T6yYCMWLgDtMy92hOgjanJefukbCOSVEHlX0IHCUi8jwwAACAAQAAgAAAAIACAACAAQAAAAMAAAAiAgPpsETw12nxLEM6OSOPfxp4YYj8NtRcLdqBpi3S4/BTuRwlIvI8MAAAgAEAAIAAAACAAgAAgAMAAAADAAAAAA==",
                }]})),
            ),
            (
                Some(
                    json!({"method": "listcoins", "params": vec![Vec::new(), vec!["fa5bf9974ae9ab08b64da24f17e80a5689540a7e73335faea05bbc563bfe31cd:1"]]}),
                ),
                Ok(json!({ "coins": [{
                    "amount": 10000,
                    "outpoint": "fa5bf9974ae9ab08b64da24f17e80a5689540a7e73335faea05bbc563bfe31cd:1",
                    "address": "TB1QJEQFLW8E3RN60MC6X6C2ECE3EKUFV4XEYTJTU35HWPCTRMGN4W0S3DCXH5",
                    "block_height": 200949,
                    "derivation_index": 0,
                    "is_immature": false,
                    "is_change": false,
                    "is_from_self": false,

                }]})),
            ),
            (
                Some(json!({"method": "getlabels", "params": vec![vec![
                    "4bc07e8fe753f7314b69da02a7cfbedc3e4e0d5fbee316a048240ae87b8aaa58",
                    "4bc07e8fe753f7314b69da02a7cfbedc3e4e0d5fbee316a048240ae87b8aaa58:0",
                    "4bc07e8fe753f7314b69da02a7cfbedc3e4e0d5fbee316a048240ae87b8aaa58:1",
                    "fa5bf9974ae9ab08b64da24f17e80a5689540a7e73335faea05bbc563bfe31cd:1",
                    "tb1qjeqflw8e3rn60mc6x6c2ece3ekufv4xeytjtu35hwpctrmgn4w0s3dcxh5",
                    "tb1qk59m7lq0ljkssjws36c99qzr8d9u66mevzcy53pz8m45ece8qhzs6alndx",
                    "tb1quapgj7rmez7etl07jt52xyem8q4z5j4jy9uzv9hvqw0xtkez9gkqaw7rgr",
                ]]})),
                Ok(json!({ "labels": {}})),
            ),
            (
                Some(json!({"method": "updatespend", "params": vec![vec![json!({})]]})),
                Ok(json!({})),
            ),
        ]);
        let wallet = Arc::new(Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap()));
        let sandbox: Sandbox<PsbtsPanel> = Sandbox::new(PsbtsPanel::new(wallet.clone()));
        let client = Arc::new(Coincubed::new(daemon.run()));
        let cache = Cache::default();
        let sandbox = sandbox
            .load(client.clone(), &Cache::default(), wallet)
            .await;
        let _sandbox = sandbox
            .update(
                client.clone(),
                &cache,
                Message::View(view::Message::Select(0)),
            )
            .await
            .update(
                client.clone(),
                &cache,
                Message::View(view::Message::Spend(view::SpendTxMessage::EditPsbt)),
            )
            .await
            .update(
                client.clone(),
                &cache,
                Message::View(view::Message::ImportSpend(
                    view::ImportSpendMessage::PsbtEdited("panic".to_string()),
                )),
            )
            .await
            .update(
                client.clone(),
                &cache,
                Message::View(view::Message::ImportSpend(
                    view::ImportSpendMessage::Confirm,
                )),
            )
            .await;
    }

    /// Bitcoin keeps its signing and merge formats and has no replay review.
    /// Its completion/broadcast gates additionally verify the final witness.
    mod bitcoin_paths {
        use super::super::*;
        use crate::app::state::vault::{
            replay,
            test_support::unified::{fixture, legacy, signer, unified},
        };
        use crate::utils::mock::Daemon as MockDaemon;
        use coincube_core::{
            border_wallet::sign_psbt_with_border_wallet, miniscript::bitcoin::secp256k1,
        };
        use std::path::PathBuf;

        const BITCOIN_FAMILY: [ChainId; 5] = [
            ChainId::Bitcoin,
            ChainId::Testnet,
            ChainId::Testnet4,
            ChainId::Signet,
            ChainId::Regtest,
        ];

        #[test]
        fn master_signer_bytes_are_the_prior_sign_psbt() {
            let f = fixture();
            let hot = crate::signer::Signer::new(signer(21));
            let expected = hot.sign_psbt(f.psbt.clone()).unwrap().serialize();
            assert!(!expected.is_empty());
            for chain in BITCOIN_FAMILY {
                let got = sign_with_master_signer_for_chain(chain, &hot, f.psbt.clone())
                    .unwrap()
                    .serialize();
                assert_eq!(got, expected, "{:?}", chain);
            }
            // And the prior path never produced a unified record.
            let signed = hot.sign_psbt(f.psbt.clone()).unwrap();
            assert!(signed.inputs[0].proprietary.is_empty());
            assert_eq!(signed.inputs[0].partial_sigs.len(), 1);
        }

        #[test]
        fn border_wallet_bytes_are_the_prior_sign_psbt_with_border_wallet() {
            let f = fixture();
            let secp = secp256k1::Secp256k1::new();
            let mnemonic = coincube_core::bip39::Mnemonic::from_entropy(&[22; 16]).unwrap();
            let fingerprint = signer(22).fingerprint(&secp);
            let (fp, expected) = sign_psbt_with_border_wallet(
                mnemonic.clone(),
                fingerprint,
                Network::Bitcoin,
                f.psbt.clone(),
            )
            .unwrap();
            for chain in BITCOIN_FAMILY {
                let (got_fp, got) = sign_border_wallet_for_chain(
                    chain,
                    mnemonic.clone(),
                    fingerprint,
                    Network::Bitcoin,
                    f.psbt.clone(),
                )
                .unwrap();
                assert_eq!(got_fp, fp);
                assert_eq!(got.serialize(), expected.serialize(), "{:?}", chain);
            }
        }

        #[test]
        fn merge_is_the_prior_merge_and_never_fails() {
            let f = fixture();
            let a = legacy(&f.psbt, &f.signers[0]);
            let b = legacy(&f.psbt, &f.signers[1]);
            let mut expected = a.clone();
            merge_signatures(&mut expected, &b);
            for chain in BITCOIN_FAMILY {
                let mut got = a.clone();
                merge_signatures_for_chain(chain, &mut got, &b).unwrap();
                assert_eq!(got.serialize(), expected.serialize(), "{:?}", chain);
                // A conflicting signature on the Bitcoin path is merged the way
                // it always was (last write wins) — the adapter's refusal is
                // BTCB2-only behaviour.
                let conflicting = b.clone();
                let (pk, sig) = conflicting.inputs[0]
                    .partial_sigs
                    .iter()
                    .map(|(pk, sig)| (*pk, *sig))
                    .next()
                    .unwrap();
                let mut other = a.clone();
                other.inputs[0].partial_sigs.insert(pk, sig);
                let mut prior = other.clone();
                merge_signatures(&mut prior, &conflicting);
                let mut keyed = other.clone();
                assert_eq!(
                    merge_signatures_for_chain(chain, &mut keyed, &conflicting),
                    Ok(())
                );
                assert_eq!(keyed.serialize(), prior.serialize());
            }
        }

        #[test]
        fn export_does_not_apply_the_blake2b_legacy_policy() {
            let f = fixture();
            // This exact retained set is refused on Bitcoin Blake2b: a
            // unified-bearing witness and a complete legacy alternative are
            // both available. A Bitcoin-family wallet keeps its historical
            // opaque PSBT export behavior.
            let psbt = legacy(
                &legacy(&unified(&f.psbt, &f.signers[0]), &f.signers[1]),
                &f.signers[2],
            );
            let wallet = Arc::new(Wallet::new(f.descriptor.clone()).with_chain(ChainId::Bitcoin));
            let tx = SpendTx::new(
                None,
                psbt.clone(),
                Vec::new(),
                &f.descriptor,
                &secp256k1::Secp256k1::new(),
                Network::Bitcoin,
            );
            let mut state = PsbtState::new(wallet, tx, true);
            let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
                crate::daemon::client::Coincubed::new(MockDaemon::new(vec![]).run()),
            );
            let _task = state.update(
                daemon,
                &Cache::default(),
                Message::View(view::Message::ExportPsbt),
            );
            assert!(state.modal.is_some(), "Bitcoin export modal must open");
            assert_eq!(state.tx.psbt.serialize(), psbt.serialize());
        }

        #[test]
        fn a_bitcoin_wallet_has_no_replay_review_and_requires_verified_finalization() {
            let f = fixture();
            for chain in BITCOIN_FAMILY {
                let wallet = Arc::new(Wallet::new(f.descriptor.clone()).with_chain(chain));
                let secp = secp256k1::Secp256k1::new();
                let tx = SpendTx::new(
                    None,
                    legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]),
                    Vec::new(),
                    &f.descriptor,
                    &secp,
                    Network::Bitcoin,
                );
                let state = PsbtState::new(wallet, tx, true);
                assert!(state.replay.is_none(), "{:?}", chain);
                assert!(state.tx.path_ready().is_some());
                assert!(state.broadcast_ready(&Cache::default()));
                assert!(state.replay_presentation(&Cache::default()).is_none());
                assert_eq!(
                    state.tx.sigs,
                    f.descriptor.partial_spend_info(&state.tx.psbt).unwrap()
                );
            }
            let _ = replay::REPLAYABLE_ACKNOWLEDGEMENT;
        }

        #[tokio::test]
        async fn tampered_bitcoin_signatures_never_enable_broadcast_or_close_the_picker() {
            use iced::advanced::{
                layout,
                renderer::Headless,
                widget::{Id, Operation, Tree},
                Layout,
            };
            #[derive(Default)]
            struct Labels(Vec<String>);
            impl Operation for Labels {
                fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                    operate(self);
                }
                fn text(&mut self, _: Option<&Id>, _: iced::Rectangle, text: &str) {
                    self.0.push(text.to_owned());
                }
            }
            let renderer = <iced::Renderer as Headless>::new(
                iced::Font::DEFAULT,
                iced::Pixels(16.0),
                Some("tiny-skia"),
            )
            .await
            .expect("software renderer available for view regression");
            let labels = |state: &PsbtState| {
                let cache = Cache::default();
                let mut element = state.view(&cache);
                let mut tree = Tree::new(element.as_widget());
                let node = element.as_widget_mut().layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(iced::Size::ZERO, iced::Size::new(1200.0, 1600.0)),
                );
                let mut labels = Labels::default();
                element.as_widget_mut().operate(
                    &mut tree,
                    Layout::new(&node),
                    &renderer,
                    &mut labels,
                );
                labels.0
            };
            use crate::app::state::vault::test_support::unified::taproot_fixture;
            for f in [fixture(), taproot_fixture()] {
                let signed = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
                let wallet =
                    Arc::new(Wallet::new(f.descriptor.clone()).with_chain(ChainId::Bitcoin));
                let tx = SpendTx::new(
                    None,
                    signed.clone(),
                    Vec::new(),
                    &f.descriptor,
                    &secp256k1::Secp256k1::new(),
                    Network::Bitcoin,
                );
                let mut state = PsbtState::new(wallet, tx, true);
                // This signing fixture has no wallet coin registry. Exercise the
                // pending-spend controls independently of coin availability.
                state.tx.status = crate::daemon::model::SpendStatus::Pending;
                assert!(state.broadcast_ready(&Cache::default()));
                // Keep every claimed signer present, but invalidate their digests.
                state.tx.psbt.unsigned_tx.output[0].value =
                    coincube_core::miniscript::bitcoin::Amount::from_sat(40_001);
                assert!(
                    state.tx.path_ready().is_some(),
                    "claimed threshold remains complete"
                );
                assert!(
                    !state.broadcast_ready(&Cache::default()),
                    "cached validity must be invalidated"
                );
                let english = state.not_ready_reason(&Cache::default()).unwrap();
                // Handling a message runs in the window's language, so the
                // toast is composed in Spanish for a Spanish window.
                let spanish =
                    coincube_ui::locale::render_in(coincube_ui::locale::Language::Spanish, || {
                        state.not_ready_reason(&Cache::default())
                    })
                    .unwrap();
                assert_ne!(spanish, english);
                assert!(
                    coincube_ui::locale::english_leaks([spanish.as_str()]).is_empty(),
                    "{}",
                    spanish
                );
                let rendered = labels(&state);
                assert!(
                    rendered.iter().any(|s| s == "Sign"),
                    "labels: {:?}; status: {:?}",
                    rendered,
                    state.tx.status
                );
                assert!(!rendered.iter().any(|s| s == "Ready" || s == "Broadcast"));
                state.modal = Some(PsbtModal::Sign(SignModal::new(
                    HashSet::new(),
                    state.wallet.clone(),
                    CoincubeDirectory::new(PathBuf::new()),
                    Network::Bitcoin,
                    false,
                    None,
                    None,
                    false,
                )));
                let _ = state.reconcile_and_maybe_close(&Cache::default());
                assert!(matches!(state.modal, Some(PsbtModal::Sign(_))));
                state.tx.psbt = signed;
                let _ = state.reconcile_and_maybe_close(&Cache::default());
                assert!(
                    state.modal.is_none(),
                    "valid signatures complete the picker"
                );
                assert!(state.broadcast_ready(&Cache::default()));
                let rendered = labels(&state);
                assert!(rendered.iter().any(|s| s == "Ready"));
                assert!(rendered.iter().any(|s| s == "Broadcast"));
                assert!(!rendered.iter().any(|s| s == "Sign"));
            }
        }

        #[test]
        fn anyonecanpay_is_not_refused_on_bitcoin() {
            // Prior behaviour: the picker dispatches whatever the PSBT asks
            // for; the refusal is BTCB2-only. (Without a loaded signer the
            // async task reports "not loaded" later — what matters here is
            // that the synchronous refusal did not fire.)
            let f = fixture();
            let mut asked = f.psbt.clone();
            asked.inputs[0].sighash_type = Some(
                coincube_core::miniscript::bitcoin::sighash::EcdsaSighashType::AllPlusAnyoneCanPay
                    .into(),
            );
            let wallet = Arc::new(Wallet::new(f.descriptor.clone()).with_chain(ChainId::Bitcoin));
            let mut modal = SignModal::new(
                HashSet::new(),
                wallet,
                CoincubeDirectory::new(PathBuf::new()),
                Network::Bitcoin,
                false,
                None,
                None,
                false,
            );
            assert!(modal.refuse_before_dispatch(&asked).is_none());
            assert!(modal.error.is_none());
        }
    }

    /// The Bitcoin Blake2b side of the same entry points.
    mod blake2b_paths {
        use super::super::*;
        use super::focus_after_restore;
        use crate::app::state::vault::{
            replay::{ReplayStatus, UnknownReason},
            test_support::unified::{fixture, legacy, signer, unified},
        };
        use crate::utils::mock::Daemon as MockDaemon;
        use coincube_core::{
            miniscript::bitcoin::secp256k1,
            psbt_unified::{unified_signatures, UnifiedPsbt},
        };
        use std::path::PathBuf;
        use std::str::FromStr;

        /// The generation of the re-check currently in flight, or a panic.
        fn in_flight_generation(state: &PsbtState) -> u64 {
            match &state.entangled_check {
                EntangledCheck::InFlight { generation, .. } => *generation,
                other => panic!("no re-check in flight: {:?}", other),
            }
        }

        /// A lookup answer observed now.
        fn observed(
            txid: Txid,
            answer: crate::services::entangled::Entanglement,
        ) -> crate::services::entangled::LookupAnswer {
            crate::services::entangled::LookupAnswer {
                txid,
                answer,
                observed_at: std::time::Instant::now(),
            }
        }

        /// The origin a re-check issued by the screen under `cache` carries.
        fn origin_of(cache: &Cache) -> crate::app::cache::LookupOrigin {
            crate::app::cache::LookupOrigin {
                app: cache.app_generation,
                chain: ChainId::BitcoinBlake2b,
            }
        }

        fn wallet_with_hot_signer(
            f: &crate::app::state::vault::test_support::unified::Fixture,
        ) -> Arc<Wallet> {
            let mut wallet = Wallet::new(f.descriptor.clone()).with_chain(ChainId::BitcoinBlake2b);
            wallet.signer = Some(Arc::new(crate::signer::Signer::new(signer(21))));
            Arc::new(wallet)
        }

        #[test]
        fn master_signer_produces_unified_records_not_partial_sigs() {
            let f = fixture();
            let hot = crate::signer::Signer::new(signer(21));
            let signed =
                sign_with_master_signer_for_chain(ChainId::BitcoinBlake2b, &hot, f.psbt.clone())
                    .unwrap();
            assert!(signed.inputs[0].partial_sigs.is_empty());
            let records =
                unified_signatures(&UnifiedPsbt::from_psbt(signed.clone()).unwrap()).unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].signature.last(), Some(&0x21));
            // Verified by the same verifier the status reads.
            assert_eq!(
                replay::replay_status(&signed, &secp256k1::Secp256k1::verification_only()),
                ReplayStatus::Unknown(UnknownReason::Incomplete)
            );
        }

        #[test]
        fn border_wallet_produces_unified_records() {
            let f = fixture();
            let secp = secp256k1::Secp256k1::new();
            let mnemonic = coincube_core::bip39::Mnemonic::from_entropy(&[22; 16]).unwrap();
            let fingerprint = signer(22).fingerprint(&secp);
            let (fp, signed) = sign_border_wallet_for_chain(
                ChainId::BitcoinBlake2b,
                mnemonic,
                fingerprint,
                Network::Bitcoin,
                f.psbt.clone(),
            )
            .unwrap();
            assert_eq!(fp, fingerprint);
            assert!(signed.inputs[0].partial_sigs.is_empty());
            assert_eq!(
                unified_signatures(&UnifiedPsbt::from_psbt(signed).unwrap())
                    .unwrap()
                    .len(),
                1
            );
        }

        #[test]
        fn merge_carries_unified_records_and_refuses_conflicts() {
            let f = fixture();
            let a = unified(&f.psbt, &f.signers[0]);
            let b = unified(&f.psbt, &f.signers[1]);
            let mut merged = a.clone();
            merge_signatures_for_chain(ChainId::BitcoinBlake2b, &mut merged, &b).unwrap();
            assert_eq!(
                unified_signatures(&UnifiedPsbt::from_psbt(merged.clone()).unwrap())
                    .unwrap()
                    .len(),
                2
            );
            // Legacy from a third signer rides along in `partial_sigs`.
            let c = legacy(&f.psbt, &f.signers[2]);
            merge_signatures_for_chain(ChainId::BitcoinBlake2b, &mut merged, &c).unwrap();
            assert_eq!(merged.inputs[0].partial_sigs.len(), 2);

            // A key with both encodings is refused and the destination is
            // untouched.
            let both = legacy(&f.psbt, &f.signers[0]);
            let before = merged.serialize();
            let err = merge_signatures_for_chain(ChainId::BitcoinBlake2b, &mut merged, &both)
                .unwrap_err();
            let err_text = err.to_string();
            assert!(
                err_text.contains("both")
                    || err_text.contains("ambiguous")
                    || err_text.contains("Ambiguous"),
                "{}",
                err
            );
            assert_eq!(merged.serialize(), before);

            // A conflicting *legacy* signature — the same key, a different
            // signature — is refused and the destination is byte-unchanged.
            // The prior copy is last-write-wins, so running it before the
            // adapter would have replaced the stored signature and hidden the
            // conflict. Two shapes: key B's signature assigned to key A (a
            // parseable signature that does not verify — refused by the
            // signature check now, "does not verify"), and a *second valid*
            // signature by key A over the same digest with different nonce
            // data (verifies, differs — the adapter's "conflict").
            let stored = legacy(&f.psbt, &f.signers[0]);
            let stored_bytes = stored.serialize();
            let key_a = *stored.inputs[0].partial_sigs.keys().next().unwrap();
            let sig_b = *legacy(&f.psbt, &f.signers[1]).inputs[0]
                .partial_sigs
                .values()
                .next()
                .unwrap();
            let mut unverifiable = f.psbt.clone();
            unverifiable.inputs[0].partial_sigs.insert(key_a, sig_b);
            // Against a destination that already holds A's valid signature it
            // is a conflict (the merge runs first; the merged result is what
            // is verified)…
            let mut destination = stored.clone();
            let err = merge_signatures_for_chain(
                ChainId::BitcoinBlake2b,
                &mut destination,
                &unverifiable,
            )
            .unwrap_err();
            assert!(
                err.to_string().to_lowercase().contains("conflict"),
                "{}",
                err
            );
            assert_eq!(destination.serialize(), stored_bytes);
            // …and against an unsigned destination it is refused because the
            // merged result does not verify — nothing enters.
            let mut unsigned = f.psbt.clone();
            let unsigned_bytes = unsigned.serialize();
            let err =
                merge_signatures_for_chain(ChainId::BitcoinBlake2b, &mut unsigned, &unverifiable)
                    .unwrap_err();
            assert!(err.to_string().contains("does not verify"), "{}", err);
            assert_eq!(unsigned.serialize(), unsigned_bytes);
            {
                use coincube_core::miniscript::bitcoin::{
                    bip32::DerivationPath, hashes::Hash, sighash::SighashCache,
                };
                let secp_all = secp256k1::Secp256k1::new();
                let witness_script = f.psbt.inputs[0].witness_script.clone().unwrap();
                let value = f.psbt.inputs[0].witness_utxo.as_ref().unwrap().value;
                let digest = SighashCache::new(&f.psbt.unsigned_tx)
                    .p2wsh_signature_hash(
                        0,
                        &witness_script,
                        value,
                        coincube_core::miniscript::bitcoin::sighash::EcdsaSighashType::All,
                    )
                    .unwrap();
                let secret = f.signers[0]
                    .xpriv_at(
                        &DerivationPath::from_str("m/48'/0'/0/3").unwrap(),
                        &secp_all,
                    )
                    .private_key;
                assert_eq!(
                    coincube_core::miniscript::bitcoin::PublicKey::new(
                        secp256k1::PublicKey::from_secret_key(&secp_all, &secret)
                    ),
                    key_a,
                    "the derivation reaches key A"
                );
                let second = secp_all.sign_ecdsa_with_noncedata(
                    &secp256k1::Message::from_digest(digest.to_byte_array()),
                    &secret,
                    &[7u8; 32],
                );
                assert_ne!(second, stored.inputs[0].partial_sigs[&key_a].signature);
                let mut conflicting = f.psbt.clone();
                conflicting.inputs[0].partial_sigs.insert(
                    key_a,
                    coincube_core::miniscript::bitcoin::ecdsa::Signature {
                        signature: second,
                        sighash_type:
                            coincube_core::miniscript::bitcoin::sighash::EcdsaSighashType::All,
                    },
                );
                // Verifies on its own…
                assert!(coincube_core::unified_finalize::verify_all_signatures(
                    &UnifiedPsbt::from_psbt(conflicting.clone()).unwrap(),
                    &secp_all
                )
                .is_ok());
                // …and is refused as a conflict, destination untouched.
                let err = merge_signatures_for_chain(
                    ChainId::BitcoinBlake2b,
                    &mut destination,
                    &conflicting,
                )
                .unwrap_err();
                assert!(
                    err.to_string().to_lowercase().contains("conflict"),
                    "{}",
                    err
                );
                assert_eq!(destination.serialize(), stored_bytes);
            }
            // Merging the same signature again is a no-op, not a conflict.
            merge_signatures_for_chain(ChainId::BitcoinBlake2b, &mut destination, &stored).unwrap();
            assert_eq!(destination.serialize(), stored_bytes);

            // A sighash *request* the chain does not serve is refused at the
            // merge boundary too, not only at finalisation: the spend never
            // reads "apparently collected" and then fails at Broadcast.
            let mut asks = legacy(&f.psbt, &f.signers[1]);
            asks.inputs[0].sighash_type =
                Some(coincube_core::miniscript::bitcoin::psbt::PsbtSighashType::from_u32(0x81));
            let err = merge_signatures_for_chain(ChainId::BitcoinBlake2b, &mut destination, &asks)
                .unwrap_err();
            assert!(err.to_string().contains("0x81"), "{}", err);
            assert_eq!(destination.serialize(), stored_bytes);

            // Signatures that pass the adapter's representation checks but do
            // not verify are refused at the merge too, destination untouched:
            // an ANYONECANPAY legacy signature, a legacy signature for another
            // transaction, a unified signature for another transaction.
            let mut other_tx = f.psbt.clone();
            other_tx.unsigned_tx.output[0].value =
                coincube_core::miniscript::bitcoin::Amount::from_sat(40_001);
            let mut acp = legacy(&f.psbt, &f.signers[1]);
            for sig in acp.inputs[0].partial_sigs.values_mut() {
                sig.sighash_type =
                    coincube_core::miniscript::bitcoin::sighash::EcdsaSighashType::AllPlusAnyoneCanPay;
            }
            let mut wrong_legacy = f.psbt.clone();
            wrong_legacy.inputs[0].partial_sigs = legacy(&other_tx, &f.signers[1]).inputs[0]
                .partial_sigs
                .clone();
            let mut wrong_unified = f.psbt.clone();
            wrong_unified.inputs[0].proprietary = unified(&other_tx, &f.signers[1]).inputs[0]
                .proprietary
                .clone();
            for (name, bad, representation_accepts) in [
                // The ANYONECANPAY flag is a representation rule (the adapter
                // refuses it alone); the wrong-digest ones pass the adapter
                // and are caught only by signature verification.
                ("legacy ANYONECANPAY", acp, false),
                ("legacy for another tx", wrong_legacy, true),
                ("unified for another tx", wrong_unified, true),
            ] {
                assert_eq!(
                    UnifiedPsbt::from_psbt(bad.clone()).is_ok(),
                    representation_accepts,
                    "{}",
                    name
                );
                let err =
                    merge_signatures_for_chain(ChainId::BitcoinBlake2b, &mut destination, &bad)
                        .unwrap_err();
                assert!(!err.to_string().is_empty(), "{}", name);
                assert_eq!(destination.serialize(), stored_bytes, "{}", name);
            }
            // …and the correct signature for the same key still merges after.
            merge_signatures_for_chain(
                ChainId::BitcoinBlake2b,
                &mut destination,
                &legacy(&f.psbt, &f.signers[1]),
            )
            .unwrap();
            assert_eq!(destination.inputs[0].partial_sigs.len(), 2);

            // The merged *candidate* is what is verified, not each side alone:
            // a destination whose input asks for `SIGHASH_ALL` plus a
            // signer's unified record is a PSBT the verifier refuses
            // (`IncompatibleSighash`), so the merge is refused and the
            // destination untouched — never a PSBT the daemon would reject.
            let mut asks_all = f.psbt.clone();
            asks_all.inputs[0].sighash_type =
                Some(coincube_core::miniscript::bitcoin::psbt::PsbtSighashType::from_u32(0x01));
            let asks_all_bytes = asks_all.serialize();
            let incoming_unified = unified(&f.psbt, &f.signers[0]);
            assert!(coincube_core::unified_finalize::verify_all_signatures(
                &UnifiedPsbt::from_psbt(incoming_unified.clone()).unwrap(),
                &secp256k1::Secp256k1::verification_only()
            )
            .is_ok());
            let err = merge_signatures_for_chain(
                ChainId::BitcoinBlake2b,
                &mut asks_all,
                &incoming_unified,
            )
            .unwrap_err();
            assert!(err.to_string().contains("sighash"), "{}", err);
            assert_eq!(asks_all.serialize(), asks_all_bytes);

            // A signer's result that carries **no prevout data at all** — a
            // device returning only its signatures, or a sparse return over
            // the Keychain API rail — still merges: verification runs on the merged
            // destination, which always carries the prevouts and witness
            // scripts, never on the delta.
            let mut bare = legacy(&f.psbt, &f.signers[0]);
            bare.inputs[0].non_witness_utxo = None;
            bare.inputs[0].witness_utxo = None;
            bare.inputs[0].witness_script = None;
            bare.inputs[0].bip32_derivation.clear();
            assert!(
                coincube_core::unified_finalize::verify_all_signatures(
                    &UnifiedPsbt::from_psbt(bare.clone()).unwrap(),
                    &secp256k1::Secp256k1::verification_only()
                )
                .is_err(),
                "on its own the delta cannot be verified"
            );
            let mut full = f.psbt.clone();
            merge_signatures_for_chain(ChainId::BitcoinBlake2b, &mut full, &bare).unwrap();
            assert_eq!(full.inputs[0].partial_sigs.len(), 1);
            assert!(full.inputs[0].non_witness_utxo.is_some());
            // …and a bare delta with a *bad* signature is still refused,
            // through the merged result.
            let mut bare_bad = bare.clone();
            let (bad_key, bad_sig) = {
                let other_signed = legacy(&other_tx, &f.signers[0]);
                other_signed.inputs[0]
                    .partial_sigs
                    .iter()
                    .map(|(k, v)| (*k, *v))
                    .next()
                    .unwrap()
            };
            bare_bad.inputs[0].partial_sigs.clear();
            bare_bad.inputs[0].partial_sigs.insert(bad_key, bad_sig);
            let mut untouched = f.psbt.clone();
            let untouched_bytes = untouched.serialize();
            let err =
                merge_signatures_for_chain(ChainId::BitcoinBlake2b, &mut untouched, &bare_bad)
                    .unwrap_err();
            assert!(err.to_string().contains("does not verify"), "{}", err);
            assert_eq!(untouched.serialize(), untouched_bytes);

            // Taproot signature data in a signer's result never enters the
            // destination: the adapter merge carries signatures only, so the
            // merged (verified) PSBT has none — the dispatch guard is the
            // door that refuses such a PSBT before a signer sees it, and the
            // daemon's boundary refuses one handed to it directly.
            let mut with_tap = legacy(&f.psbt, &f.signers[1]);
            let tap_secp = secp256k1::Secp256k1::new();
            let tap_keypair = secp256k1::Keypair::from_secret_key(
                &tap_secp,
                &secp256k1::SecretKey::from_slice(&[5u8; 32]).unwrap(),
            );
            with_tap.inputs[0].tap_key_sig =
                Some(coincube_core::miniscript::bitcoin::taproot::Signature {
                    signature: tap_secp.sign_schnorr_no_aux_rand(
                        &secp256k1::Message::from_digest([4u8; 32]),
                        &tap_keypair,
                    ),
                    sighash_type:
                        coincube_core::miniscript::bitcoin::TapSighashType::AllPlusAnyoneCanPay,
                });
            let mut plain = f.psbt.clone();
            merge_signatures_for_chain(ChainId::BitcoinBlake2b, &mut plain, &with_tap).unwrap();
            assert_eq!(plain.inputs[0].partial_sigs.len(), 1);
            assert!(plain.inputs[0].tap_key_sig.is_none());
            assert!(plain.inputs[0].tap_script_sigs.is_empty());
            // Handed to the verifier as a whole, the same PSBT is refused.
            assert!(matches!(
                coincube_core::unified_finalize::verify_all_signatures(
                    &UnifiedPsbt::from_psbt(with_tap).unwrap(),
                    &secp256k1::Secp256k1::verification_only()
                ),
                Err(
                    coincube_core::unified_finalize::UnifiedFinalizeError::Signing(
                        coincube_core::unified_signing::UnifiedSigningError::TaprootSignatureData {
                            input: 0
                        }
                    )
                )
            ));
        }

        #[tokio::test]
        async fn surplus_legacy_is_refused_atomically_and_cannot_be_exported_after_reload() {
            let f = fixture();
            let mut safe = unified(&f.psbt, &f.signers[0]);
            merge_signatures_for_chain(
                ChainId::BitcoinBlake2b,
                &mut safe,
                &legacy(&f.psbt, &f.signers[1]),
            )
            .expect("one legacy signature is required by the mixed witness");
            let safe_bytes = safe.serialize();
            let refusal = merge_signatures_for_chain(
                ChainId::BitcoinBlake2b,
                &mut safe,
                &legacy(&f.psbt, &f.signers[2]),
            )
            .unwrap_err();
            assert!(
                matches!(
                    refusal,
                    UnifiedFinalizeError::UnsafeLegacyAlternative { input: 0, .. }
                ),
                "{}",
                refusal
            );
            assert_eq!(safe.serialize(), safe_bytes, "merge must be atomic");

            // Simulate an unsafe row created by an older build: reload must not
            // show Protected or permit export even though the selected witness
            // contains a unified signature.
            let mut unsafe_psbt = unified(&f.psbt, &f.signers[0]);
            for signer in [&f.signers[1], &f.signers[2]] {
                unsafe_psbt.inputs[0]
                    .partial_sigs
                    .extend(legacy(&f.psbt, signer).inputs[0].partial_sigs.clone());
            }
            let wallet = wallet_with_hot_signer(&f);
            let tx = SpendTx::new(
                None,
                unsafe_psbt,
                Vec::new(),
                &f.descriptor,
                &secp256k1::Secp256k1::new(),
                Network::Bitcoin,
            );
            let mut state = PsbtState::new(wallet, tx, true);
            assert!(matches!(
                state.replay.as_ref().map(|review| review.status()),
                Some(ReplayStatus::Unknown(UnknownReason::Refused(reason)))
                    if reason.contains("independently satisfy")
            ));
            assert!(!state.broadcast_ready(&Cache::default()));
            let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
                crate::daemon::client::Coincubed::new(MockDaemon::new(vec![]).run()),
            );
            let messages = drive(state.update(
                daemon,
                &Cache::default(),
                Message::View(view::Message::ExportPsbt),
            ))
            .await;
            assert!(
                shows_error(&messages, "independently form a Bitcoin-valid witness"),
                "{:?}",
                messages
            );
            assert!(state.modal.is_none(), "unsafe export modal must not open");
        }

        // Gandalf's probes from the review of 15a26267 (WORK_LOGS/LAUNCH_GA/
        // B1/B1.2/GANDALF_15a26267/PROBES.patch), kept as regressions: they
        // assert the fixed behaviour and failed on that head. Adapted only
        // where the repair changed an API (`set_acknowledged`,
        // `record_entanglement`, the chain parameter of `classify_signers`,
        // `refuse_before_dispatch`).

        #[test]
        fn gandalf_probe_entangled_legacy_ack_is_not_sufficient() {
            let f = fixture();
            let wallet = wallet_with_hot_signer(&f);
            let secp = secp256k1::Secp256k1::new();
            let signed = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
            let tx = SpendTx::new(
                None,
                signed,
                Vec::new(),
                &f.descriptor,
                &secp,
                Network::Bitcoin,
            );
            let mut state = PsbtState::new(wallet, tx, true);
            let mut cache = Cache::default();
            cache.record_entanglement(
                f.psbt.unsigned_tx.input[0].previous_output.txid,
                crate::services::entangled::Entanglement::Entangled,
                std::time::Instant::now(),
            );
            state.replay.as_mut().unwrap().set_acknowledged(true);
            let pill = state.replay_presentation(&cache).unwrap();
            assert_eq!(
                pill.entangled,
                vec![(0, crate::services::entangled::Entanglement::Entangled)]
            );
            println!(
                "GANDALF entangled legacy-only acknowledged broadcast ready: {}",
                pill.broadcast_ready
            );
            assert!(
                !pill.broadcast_ready,
                "I13 requires a verified unified signature or positive split evidence"
            );
        }

        #[test]
        fn gandalf_probe_gui_legacy_conflict_is_atomic() {
            let f = fixture();
            let mut stored = legacy(&f.psbt, &f.signers[0]);
            let other = legacy(&f.psbt, &f.signers[1]);
            let key = *stored.inputs[0].partial_sigs.keys().next().unwrap();
            let bad_sig = *other.inputs[0].partial_sigs.values().next().unwrap();
            let mut incoming = stored.clone();
            incoming.inputs[0].partial_sigs.insert(key, bad_sig);
            let before = stored.serialize();
            let result =
                merge_signatures_for_chain(ChainId::BitcoinBlake2b, &mut stored, &incoming);
            println!(
                "GANDALF GUI conflict accepted: {}; mutated: {}",
                result.is_ok(),
                before != stored.serialize()
            );
            assert!(
                result.is_err(),
                "BTCB2 legacy conflict must be rejected before overwrite"
            );
            assert_eq!(stored.serialize(), before);
        }

        #[test]
        fn gandalf_probe_classification_counts_existing_unified_signature() {
            let f = fixture();
            let wallet = wallet_with_hot_signer(&f);
            let signed = unified(&f.psbt, &f.signers[0]);
            let caps = crate::app::state::vault::signers::ReplayCapabilities::from_wallet(&wallet);
            // The BTCB2 classification of the raw PSBT must agree with the
            // Bitcoin classification of the counting projection: the record
            // counts.
            let raw = crate::app::state::vault::signers::classify_signers(
                ChainId::BitcoinBlake2b,
                &signed,
                &f.descriptor,
                &HashMap::new(),
                &HashMap::new(),
                &caps,
            )
            .unwrap();
            let projected = replay::counting_projection(&signed);
            let checked = crate::app::state::vault::signers::classify_signers(
                ChainId::Bitcoin,
                &projected,
                &f.descriptor,
                &HashMap::new(),
                &HashMap::new(),
                &caps,
            )
            .unwrap();
            println!(
                "GANDALF remaining signer rows raw={} projected={}",
                raw.len(),
                checked.len()
            );
            assert_eq!(
                raw.len(),
                checked.len(),
                "BTCB2 classification must count unified signatures"
            );
            assert_eq!(raw.len(), 1);
        }

        #[test]
        fn gandalf_probe_dispatch_rejects_reserved_anyonecanpay() {
            let f = fixture();
            let mut signed = unified(&f.psbt, &f.signers[0]);
            for bytes in signed.inputs[0].proprietary.values_mut() {
                *bytes.last_mut().unwrap() = 0xa1;
            }
            assert!(UnifiedPsbt::from_psbt(signed.clone()).is_err());
            let result = replay::refuse_before_dispatch(&signed);
            println!(
                "GANDALF reserved ANYONECANPAY dispatch guard accepts: {}",
                result.is_ok()
            );
            assert!(
                result.is_err(),
                "invalid reserved records must not be dispatched"
            );
        }

        #[test]
        fn psbt_state_reads_the_verified_witness_and_gates_broadcast() {
            let f = fixture();
            let wallet = wallet_with_hot_signer(&f);
            let secp = secp256k1::Secp256k1::new();
            let tx = SpendTx::new(
                None,
                f.psbt.clone(),
                Vec::new(),
                &f.descriptor,
                &secp,
                Network::Bitcoin,
            );
            let mut state = PsbtState::new(wallet.clone(), tx, true);
            assert_eq!(
                state.replay.as_ref().map(|r| r.status()),
                Some(ReplayStatus::Unknown(UnknownReason::NotYetChecked))
            );
            assert!(!state.broadcast_ready(&Cache::default()));

            // Two unified signatures: protected, ready, and the picker's
            // signature count sees both (the daemon's analysis alone would
            // show zero).
            state.tx.psbt = unified(&unified(&f.psbt, &f.signers[0]), &f.signers[1]);
            let _ = state.reconcile_and_maybe_close(&Cache::default());
            assert_eq!(
                state.replay.as_ref().unwrap().status(),
                ReplayStatus::Protected
            );
            assert_eq!(state.tx.sigs.primary_path().sigs_count, 2);
            assert!(state.broadcast_ready(&Cache::default()));
            let pill = state.replay_presentation(&Cache::default()).unwrap();
            assert!(pill.broadcast_ready);
            // No lookup has run: every input is "not yet checked", never safe.
            assert_eq!(
                pill.entangled,
                vec![(0, crate::services::entangled::Entanglement::Unknown)]
            );

            // Legacy only: replayable, and Broadcast waits for the
            // acknowledgement, given through the view message.
            state.tx.psbt = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
            let _ = state.reconcile_and_maybe_close(&Cache::default());
            assert_eq!(
                state.replay.as_ref().unwrap().status(),
                ReplayStatus::Replayable { inputs: vec![0] }
            );
            assert!(!state.broadcast_ready(&Cache::default()));
            let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
                crate::daemon::client::Coincubed::new(MockDaemon::new(vec![]).run()),
            );
            let _ = state.update(
                daemon.clone(),
                &Cache::default(),
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::AcknowledgeReplay(true),
                )),
            );
            assert!(state.broadcast_ready(&Cache::default()));
            // A new signature resets the acknowledgement.
            state.tx.psbt = legacy(&state.tx.psbt, &f.signers[2]);
            let _ = state.reconcile_and_maybe_close(&Cache::default());
            assert!(!state.replay.as_ref().unwrap().acknowledged());
            assert!(!state.broadcast_ready(&Cache::default()));
        }

        /// A split key marked when the PSBT opened counts only while the
        /// cache still holds its proof. Once the proof is gone (a reorg, a
        /// reload, a twin sighting) the spend is replayable again and the
        /// entangled-coin rules apply, at render and at broadcast.
        #[test]
        fn a_split_key_without_a_current_proof_is_not_split() {
            let f = fixture();
            let wallet = wallet_with_hot_signer(&f);
            let secp = secp256k1::Secp256k1::new();
            let signed = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
            let key = (0, signed.unsigned_tx.input[0].previous_output);
            let tx = SpendTx::new(
                None,
                signed,
                Vec::new(),
                &f.descriptor,
                &secp,
                Network::Bitcoin,
            );
            let mut state = PsbtState::new(wallet, tx, true);
            let review = state.replay.take().unwrap().with_split_key(Some(key));
            assert_eq!(review.status(), ReplayStatus::Split, "as opened");
            state.replay = Some(review);

            let cache = Cache::default();
            let current = state.current_review(&cache).unwrap();
            assert_eq!(current.split_key(), None);
            assert_eq!(
                current.status(),
                ReplayStatus::Replayable { inputs: vec![0] }
            );
            assert!(
                !state.broadcast_ready(&cache),
                "no Split bypass once the key's proof is gone"
            );
            assert_eq!(
                state.replay_presentation(&cache).unwrap().review.status(),
                ReplayStatus::Replayable { inputs: vec![0] }
            );
        }

        /// A split prepared with a verified key, whose live ancestry then
        /// changes before it is signed (a reorg, a twin-chain change). The
        /// cache still holds the proof checked when the split was built and
        /// nothing refreshes it, so the screen must not take it on trust:
        /// the key counts only with a fresh re-qualification, and a failed or
        /// lapsed one refuses Sign, Broadcast and the *Split* status.
        #[tokio::test]
        async fn a_split_key_whose_ancestry_changed_after_prepare_is_refused() {
            use coincube_core::claim_ancestry::{retained::RetainedPath, search::OwnedLink};
            use coincube_core::miniscript::bitcoin::consensus::serialize;
            let f = fixture();
            let wallet = wallet_with_hot_signer(&f);
            let secp = secp256k1::Secp256k1::new();
            let signed = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
            let spend = signed.unsigned_tx.compute_txid();
            let key = signed.unsigned_tx.input[0].previous_output;
            let funding = signed.inputs[0].non_witness_utxo.clone().unwrap();
            let path = RetainedPath::new(
                key,
                vec![OwnedLink {
                    transaction: serialize(&funding),
                    parent_input: None,
                }],
            )
            .unwrap();

            // As `split_coins::prepare` leaves it: the key verified in the
            // cache and the split recorded in the journal.
            let mut cache = Cache::default();
            cache.chain_proofs.insert(
                key,
                crate::services::chain_status::ProofEntry::Verified(path.clone()),
            );
            cache
                .split_records
                .push(crate::services::split_coins::SplitRecord {
                    txid: spend,
                    chain: ChainId::BitcoinBlake2b,
                    method: crate::services::split_coins::Method::SplitKey,
                    inputs: Vec::new(),
                    key: Some(key),
                    key_path: Some(hex::encode(path.encode())),
                    created_at: 0,
                    confirmations: 0,
                });
            let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
                crate::daemon::client::Coincubed::new(MockDaemon::new(vec![]).run()),
            );
            let sign = || Message::View(view::Message::Spend(view::SpendTxMessage::Sign));
            let broadcast = || Message::View(view::Message::Spend(view::SpendTxMessage::Broadcast));
            let in_flight = |state: &PsbtState| match &state.split_key_check {
                SplitKeyCheck::InFlight { generation, key: k } => {
                    assert_eq!(*k, key);
                    *generation
                }
                other => panic!("no split-key check in flight: {:?}", other),
            };
            let reply = |generation, result| Message::SplitKeyChecked {
                spend,
                key,
                generation,
                result,
            };

            let mut state = PsbtState::new(
                wallet,
                SpendTx::new(
                    None,
                    signed,
                    Vec::new(),
                    &f.descriptor,
                    &secp,
                    Network::Bitcoin,
                ),
                true,
            );
            // Opening the screen asks for the key; until the answer lands
            // the cached proof admits nothing.
            let asked = drive(state.update(daemon.clone(), &cache, Message::Reconcile)).await;
            let first = in_flight(&state);
            assert!(asked.iter().any(|m| matches!(
                m,
                Message::SplitKeyCheck { spend: s, key: k, generation }
                    if *s == spend && *k == key && *generation == first
            )));
            assert!(!state.broadcast_ready(&cache));
            assert_eq!(
                state.current_review(&cache).unwrap().status(),
                ReplayStatus::Replayable { inputs: vec![0] }
            );
            let refused = drive(state.update(daemon.clone(), &cache, sign())).await;
            assert!(state.modal.is_none(), "Sign waits for the key check");
            assert!(shows_error(&refused, SPLIT_KEY_CHECKING_COPY));

            // The key re-qualifies: Split, and the split may be broadcast.
            let _ = drive(state.update(daemon.clone(), &cache, reply(first, Ok(())))).await;
            assert_eq!(
                state.current_review(&cache).unwrap().status(),
                ReplayStatus::Split
            );
            assert!(state.broadcast_ready(&cache));

            // Time passes and the key's ancestry changes. The cache still
            // says verified; the lapsed answer does not.
            if let SplitKeyCheck::Done { at, .. } = &mut state.split_key_check {
                *at = std::time::Instant::now()
                    .checked_sub(SPLIT_KEY_CHECK_LIFETIME)
                    .unwrap();
            }
            assert!(matches!(
                cache.chain_proofs.get(&key),
                Some(crate::services::chain_status::ProofEntry::Verified(_))
            ));
            assert!(!state.broadcast_ready(&cache));
            assert_eq!(
                state.not_ready_reason(&cache).as_deref(),
                Some(SPLIT_KEY_STALE_COPY)
            );
            let refused = drive(state.update(daemon.clone(), &cache, broadcast())).await;
            assert!(shows_error(&refused, SPLIT_KEY_CHECKING_COPY));
            assert!(state.modal.is_none(), "no Broadcast dialog");
            let second = in_flight(&state);
            assert!(second > first, "Broadcast asked again");

            // The fresh check fails against the changed chains, and a late
            // positive reply for the old check is ignored.
            let changed = "The split key couldn't be verified right now: reorg".to_string();
            let failed =
                drive(state.update(daemon.clone(), &cache, reply(second, Err(changed.clone()))))
                    .await;
            assert!(shows_error(&failed, &changed));
            let _ = drive(state.update(daemon.clone(), &cache, reply(first, Ok(())))).await;
            assert!(!state.broadcast_ready(&cache));
            assert_eq!(
                state.current_review(&cache).unwrap().status(),
                ReplayStatus::Replayable { inputs: vec![0] }
            );
            assert_eq!(state.not_ready_reason(&cache), Some(changed.clone()));

            // Sign is refused too, and asks again. The empty mock daemon
            // panics on any RPC, so no path reached the daemon.
            let refused = drive(state.update(daemon.clone(), &cache, sign())).await;
            assert!(state.modal.is_none(), "no signer picker");
            assert!(shows_error(&refused, SPLIT_KEY_CHECKING_COPY));
            assert!(in_flight(&state) > second);
        }

        /// A journaled split-key split of the unsigned fixture after a
        /// restart: only the journal holds the key's proof.
        fn journaled_split() -> (PsbtState, Cache, Txid, OutPoint) {
            use coincube_core::claim_ancestry::{retained::RetainedPath, search::OwnedLink};
            use coincube_core::miniscript::bitcoin::consensus::serialize;
            let f = fixture();
            let wallet = wallet_with_hot_signer(&f);
            let secp = secp256k1::Secp256k1::new();
            let spend = f.psbt.unsigned_tx.compute_txid();
            let key = f.psbt.unsigned_tx.input[0].previous_output;
            let funding = f.psbt.inputs[0].non_witness_utxo.clone().unwrap();
            let path = RetainedPath::new(
                key,
                vec![OwnedLink {
                    transaction: serialize(&funding),
                    parent_input: None,
                }],
            )
            .unwrap();
            let mut cache = Cache::default();
            cache
                .split_records
                .push(crate::services::split_coins::SplitRecord {
                    txid: spend,
                    chain: ChainId::BitcoinBlake2b,
                    method: crate::services::split_coins::Method::SplitKey,
                    inputs: Vec::new(),
                    key: Some(key),
                    key_path: Some(hex::encode(path.encode())),
                    created_at: 0,
                    confirmations: 0,
                });
            assert_eq!(
                cache.split_key_path(spend, key).map(|p| p.encode()),
                Some(path.encode())
            );
            let state = PsbtState::new(
                wallet,
                SpendTx::new(
                    None,
                    f.psbt.clone(),
                    Vec::new(),
                    &f.descriptor,
                    &secp,
                    Network::Bitcoin,
                ),
                true,
            );
            (state, cache, spend, key)
        }

        fn split_key_generation(state: &PsbtState) -> u64 {
            match &state.split_key_check {
                SplitKeyCheck::InFlight { generation, .. } => *generation,
                other => panic!("no split-key check in flight: {:?}", other),
            }
        }

        /// Open the picker of [`journaled_split`] with its key freshly
        /// re-qualified.
        fn open_fresh_picker(
            state: &mut PsbtState,
            cache: &Cache,
            spend: Txid,
            key: OutPoint,
            daemon: &Arc<dyn Daemon + Sync + Send>,
        ) {
            let _ = state.update(daemon.clone(), cache, Message::Reconcile);
            let generation = split_key_generation(state);
            let _ = state.update(
                daemon.clone(),
                cache,
                Message::SplitKeyChecked {
                    spend,
                    key,
                    generation,
                    result: Ok(()),
                },
            );
            let _ = state.update(
                daemon.clone(),
                cache,
                Message::View(view::Message::Spend(view::SpendTxMessage::Sign)),
            );
            assert!(
                matches!(state.modal, Some(PsbtModal::Sign(_))),
                "fresh: opens"
            );
        }

        /// The picker was opened while the key was fresh; the key's answer
        /// then lapses and its re-qualification fails. No signer pick may
        /// sign, and the picker comes down.
        #[tokio::test]
        async fn a_signer_pick_in_an_open_picker_needs_a_fresh_split_key() {
            let (mut state, cache, spend, key) = journaled_split();
            let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
                crate::daemon::client::Coincubed::new(MockDaemon::new(vec![]).run()),
            );
            let master = || {
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::SelectMasterSigner,
                ))
            };
            open_fresh_picker(&mut state, &cache, spend, key, &daemon);

            // The answer lapses while the picker is open.
            if let SplitKeyCheck::Done { at, .. } = &mut state.split_key_check {
                *at = std::time::Instant::now()
                    .checked_sub(SPLIT_KEY_CHECK_LIFETIME)
                    .unwrap();
            }
            let refused = drive(state.update(daemon.clone(), &cache, master())).await;
            assert!(shows_error(&refused, SPLIT_KEY_CHECKING_COPY));
            assert!(
                !refused.iter().any(|m| matches!(m, Message::Signed(..))),
                "no signature while the key is unchecked"
            );
            let second = split_key_generation(&state);
            assert!(
                matches!(state.modal, Some(PsbtModal::Sign(_))),
                "checking: stays"
            );

            // The fresh check fails: the picker comes down, and a pick that
            // still arrives signs nothing.
            let failed = "The split key couldn't be verified right now: reorg".to_string();
            let reply = Message::SplitKeyChecked {
                spend,
                key,
                generation: second,
                result: Err(failed),
            };
            let _ = drive(state.update(daemon.clone(), &cache, reply)).await;
            assert!(state.modal.is_none(), "the picker comes down");
            let refused = drive(state.update(daemon.clone(), &cache, master())).await;
            assert!(!refused.iter().any(|m| matches!(m, Message::Signed(..))));
            assert!(state.tx.psbt.inputs[0].partial_sigs.is_empty());
        }

        /// After a refusal the picker's Keychain sessions are cancelled
        /// exactly once: also when the picker was only hidden (Cancel while a
        /// device signed) with sessions still live, and never again while the
        /// dismissed picker drains them.
        #[tokio::test]
        async fn a_refused_split_cancels_its_keychain_sessions_once() {
            use crate::app::state::vault::keychain_sign::tests::{
                all_cancel_requested, modal_with_live_session,
            };
            let (mut state, cache, spend, key) = journaled_split();
            let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
                crate::daemon::client::Coincubed::new(MockDaemon::new(vec![]).run()),
            );
            open_fresh_picker(&mut state, &cache, spend, key, &daemon);
            // Hidden by Cancel while a device signed: not dismissed, and its
            // phone session is still live.
            if let Some(PsbtModal::Sign(sign)) = state.modal.as_mut() {
                sign.set_keychain(Some(modal_with_live_session()));
                sign.display_modal = false;
            }
            state.split_key_check = SplitKeyCheck::Done {
                key,
                at: std::time::Instant::now(),
                result: Err("reorg".into()),
            };
            assert!(state.split_key_refusal(&cache).is_some());

            let _ = state.close_picker_if_split_key_refused(&cache);
            let Some(PsbtModal::Sign(sign)) = &state.modal else {
                panic!("mounted to drain its sessions");
            };
            let keychain = sign.keychain.as_ref().unwrap();
            assert!(keychain.is_dismissed(), "dismissed now");
            assert!(
                all_cancel_requested(keychain),
                "the live session is cancelled"
            );

            // Draining: left alone, nothing sent again.
            for _ in 0..2 {
                let again = state.close_picker_if_split_key_refused(&cache);
                assert_eq!(again.units(), 0, "no cancel sent again");
                assert!(matches!(&state.modal, Some(PsbtModal::Sign(sign)) if !sign.display_modal));
            }
        }

        #[test]
        fn closing_picker_with_complete_replay_signatures_focuses_enabled_copy_action() {
            let f = fixture();
            let wallet = wallet_with_hot_signer(&f);
            let tx = SpendTx::new(
                None,
                legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]),
                Vec::new(),
                &f.descriptor,
                &secp256k1::Secp256k1::new(),
                Network::Bitcoin,
            );
            let mut state = PsbtState::new(wallet, tx, true);
            // The synthetic fixture omits its Coin, so SpendTx otherwise
            // classifies it as Deprecated and hides both signing actions.
            state.tx.status = SpendStatus::Pending;
            let cache = Cache::default();
            assert!(state
                .replay
                .as_ref()
                .unwrap()
                .signatures_complete(&state.entangled_inputs(&cache)));
            assert!(!state.broadcast_ready(&cache));

            state.modal = Some(PsbtModal::Sign(SignModal::new(
                state.tx.signers(),
                state.wallet.clone(),
                cache.datadir_path.clone(),
                cache.network,
                state.saved,
                state.tx.recovery_timelock(),
                None,
                false,
            )));
            let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
                crate::daemon::client::Coincubed::new(MockDaemon::new(vec![]).run()),
            );
            let task = state.update(
                daemon,
                &cache,
                Message::View(view::Message::Spend(view::SpendTxMessage::Cancel)),
            );
            assert!(state.modal.is_none());
            let copy = state.action_id("overview.copy-txid");
            assert_eq!(state.overview_focus_id(&cache), copy);
            assert_eq!(
                focus_after_restore(task, copy.clone(), true, false),
                Some(copy)
            );

            state.replay.as_mut().unwrap().set_acknowledged(true);
            assert!(state.broadcast_ready(&cache));
            assert_eq!(
                state.overview_focus_id(&cache),
                state.action_id("overview.broadcast")
            );
        }

        fn pending_focus_state(psbt: Psbt) -> PsbtState {
            let f = fixture();
            let tx = SpendTx::new(
                None,
                psbt,
                Vec::new(),
                &f.descriptor,
                &secp256k1::Secp256k1::new(),
                Network::Bitcoin,
            );
            let mut state = PsbtState::new(wallet_with_hot_signer(&f), tx, true);
            // The synthetic fixture omits its Coin; see the Cancel test above.
            state.tx.status = SpendStatus::Pending;
            state
        }

        fn focus_daemon() -> Arc<dyn Daemon + Sync + Send> {
            Arc::new(crate::daemon::client::Coincubed::new(
                MockDaemon::new(vec![]).run(),
            ))
        }

        fn picker_close_id(state: &PsbtState) -> iced::advanced::widget::Id {
            match &state.modal {
                Some(PsbtModal::Sign(sign)) if sign.display_modal => {
                    view::vault::psbt::action_id(sign.action_scope, "picker.close")
                }
                _ => panic!("the signer picker is not displayed"),
            }
        }

        #[test]
        fn opening_the_picker_through_update_focuses_its_close_action() {
            let mut state = pending_focus_state(fixture().psbt);
            let task = state.update(
                focus_daemon(),
                &Cache::default(),
                Message::View(view::Message::Spend(view::SpendTxMessage::Sign)),
            );
            let close = picker_close_id(&state);
            assert_ne!(close, state.action_id("overview.sign"));
            assert_eq!(
                focus_after_restore(task, close.clone(), true, false),
                Some(close)
            );
        }

        #[test]
        fn a_direct_signer_through_update_keeps_focus_in_the_open_picker() {
            let mut state = pending_focus_state(fixture().psbt);
            let cache = Cache::default();
            let _ = state.update(
                focus_daemon(),
                &cache,
                Message::View(view::Message::Spend(view::SpendTxMessage::Sign)),
            );
            let task = state.update(
                focus_daemon(),
                &cache,
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::SelectMasterSigner,
                )),
            );
            assert!(state.signer_focus_return);
            let close = picker_close_id(&state);
            assert_eq!(
                focus_after_restore(task, close.clone(), true, false),
                Some(close)
            );
        }

        #[test]
        fn a_keychain_signer_without_connect_focuses_the_unavailable_connect_action() {
            let mut state = pending_focus_state(fixture().psbt);
            let cache = Cache::default();
            let _ = state.update(
                focus_daemon(),
                &cache,
                Message::View(view::Message::Spend(view::SpendTxMessage::Sign)),
            );
            let task = state.update(
                focus_daemon(),
                &cache,
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::RequestFromEveryone,
                )),
            );
            assert!(matches!(
                state.modal,
                Some(PsbtModal::KeychainUnavailable(_))
            ));
            // The handoff replaces the picker; it never returns to the overview.
            assert!(!state.signer_focus_return);
            let connect = state.action_id("keychain-unavailable.connect");
            assert_eq!(
                focus_after_restore(task, connect.clone(), true, false),
                Some(connect)
            );
        }

        #[test]
        fn the_picker_closing_after_signing_through_update_focuses_the_overview() {
            let f = fixture();
            let mut state = pending_focus_state(f.psbt.clone());
            let cache = Cache::default();
            let _ = state.update(
                focus_daemon(),
                &cache,
                Message::View(view::Message::Spend(view::SpendTxMessage::Sign)),
            );
            let _ = state.update(
                focus_daemon(),
                &cache,
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::SelectMasterSigner,
                )),
            );
            assert!(state.signer_focus_return);

            // A signature merge that does not satisfy the path leaves the
            // picker open and never initiates a restore.
            state.tx.psbt = unified(&f.psbt, &f.signers[0]);
            let task = state.update(focus_daemon(), &cache, Message::Reconcile);
            let close = picker_close_id(&state);
            assert_eq!(focus_after_restore(task, close, true, false), None);
            assert!(state.signer_focus_return);

            // The satisfying merge closes the picker and returns to Broadcast.
            state.tx.psbt = unified(&state.tx.psbt, &f.signers[1]);
            let task = state.update(focus_daemon(), &cache, Message::Reconcile);
            assert!(state.modal.is_none());
            assert!(!state.signer_focus_return);
            assert!(state.broadcast_ready(&cache));
            let broadcast = state.action_id("overview.broadcast");
            assert_eq!(state.overview_focus_id(&cache), broadcast);
            assert_eq!(
                focus_after_restore(task, broadcast.clone(), true, false),
                Some(broadcast)
            );
        }

        /// `#276` I13 through the state: a legacy-only spend of a deposit
        /// Connect has confirmed on Bitcoin is not broadcastable — not through
        /// the handler's gate, not through the view's button, and not after
        /// the acknowledgement — until a verified unified signature is in the
        /// witness. The entangled set is read from the cache at every check,
        /// so a lookup that lands after the last signature still tightens the
        /// gate. `Unknown` and `NotEntangled` keep the acknowledgement path.
        #[test]
        fn a_known_entangled_input_requires_a_unified_signature_through_state_and_view() {
            use crate::services::entangled::Entanglement;
            let f = fixture();
            let wallet = wallet_with_hot_signer(&f);
            let secp = secp256k1::Secp256k1::new();
            let legacy_only = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
            let tx = SpendTx::new(
                None,
                legacy_only.clone(),
                Vec::new(),
                &f.descriptor,
                &secp,
                Network::Bitcoin,
            );
            let mut state = PsbtState::new(wallet, tx, true);
            let txid = f.psbt.unsigned_tx.input[0].previous_output.txid;
            let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
                crate::daemon::client::Coincubed::new(MockDaemon::new(vec![]).run()),
            );

            // Before any lookup: Unknown, amber, acknowledgeable.
            let unchecked = Cache::default();
            let pill = state.replay_presentation(&unchecked).unwrap();
            assert_eq!(pill.entangled, vec![(0, Entanglement::Unknown)]);
            assert!(pill.review.signatures_complete(&pill.entangled));
            assert!(!pill.broadcast_ready);
            let _ = state.update(
                daemon.clone(),
                &unchecked,
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::AcknowledgeReplay(true),
                )),
            );
            assert!(state.broadcast_ready(&unchecked));
            assert!(
                state
                    .replay_presentation(&unchecked)
                    .unwrap()
                    .broadcast_ready
            );

            // The lookup lands *after* the acknowledgement: the same review,
            // read against the new cache, is no longer ready — through the
            // state gate and through the pill the view renders.
            let mut entangled = Cache::default();
            entangled.record_entanglement(txid, Entanglement::Entangled, std::time::Instant::now());
            assert!(state.replay.as_ref().unwrap().acknowledged());
            assert!(!state.broadcast_ready(&entangled));
            let pill = state.replay_presentation(&entangled).unwrap();
            assert!(!pill.broadcast_ready);
            assert!(!pill.review.signatures_complete(&pill.entangled));
            assert_eq!(
                replay::blocked_entangled_inputs(&pill.review.status(), &pill.entangled),
                vec![0]
            );
            let (label, _) = replay::pill_copy(&pill.review.status(), &pill.entangled);
            assert_eq!(
                label,
                "Replayable — no replay-capable signature on input 0 \
                 (also exists on Bitcoin — signature required)"
            );
            assert!(replay::blocked_entangled_copy(&[0]).is_some());
            // Ticking again changes nothing.
            let _ = state.update(
                daemon.clone(),
                &entangled,
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::AcknowledgeReplay(true),
                )),
            );
            assert!(!state.broadcast_ready(&entangled));
            // The picker would not close on this set of signatures either.
            state.modal = Some(PsbtModal::Sign(SignModal::new(
                HashSet::new(),
                state.wallet.clone(),
                CoincubeDirectory::new(PathBuf::new()),
                Network::Bitcoin,
                true,
                None,
                None,
                false,
            )));
            let _ = state.reconcile_and_maybe_close(&entangled);
            assert!(
                state.modal.is_some(),
                "a blocked input keeps the picker open"
            );
            // …but a lookup answering NotEntangled lets the ordinary
            // acknowledgement path close it.
            let mut not_entangled = Cache::default();
            not_entangled.record_entanglement(
                txid,
                Entanglement::NotEntangled,
                std::time::Instant::now(),
            );
            let _ = state.reconcile_and_maybe_close(&not_entangled);
            assert!(state.modal.is_none());
            assert!(state
                .replay_presentation(&not_entangled)
                .unwrap()
                .entangled
                .is_empty());

            // Adding a unified signature after a complete legacy threshold
            // would leave two complete alternatives, so the reloaded-state
            // gate refuses it and tells the user to recreate the spend.
            state.tx.psbt = legacy(
                &legacy(&unified(&f.psbt, &f.signers[2]), &f.signers[0]),
                &f.signers[1],
            );
            let _ = state.reconcile_and_maybe_close(&entangled);
            assert!(matches!(
                &state.replay.as_ref().unwrap().status(),
                ReplayStatus::Unknown(UnknownReason::Refused(reason))
                    if reason.contains("independently satisfy")
            ));
            assert!(!state.broadcast_ready(&entangled));

            // Recreated in the safe order: unified first, then only the one
            // legacy signature the mixed witness needs.
            state.tx.psbt = legacy(&unified(&f.psbt, &f.signers[0]), &f.signers[1]);
            let _ = state.reconcile_and_maybe_close(&entangled);
            assert_eq!(
                state.replay.as_ref().unwrap().status(),
                ReplayStatus::Protected
            );
            assert!(state.broadcast_ready(&entangled));
            let pill = state.replay_presentation(&entangled).unwrap();
            assert!(pill.broadcast_ready);
            assert!(
                replay::blocked_entangled_inputs(&pill.review.status(), &pill.entangled).is_empty()
            );

            // Forward ordering: the lookup is already known when the legacy
            // threshold is reached. The picker stays open at the threshold,
            // the view keeps offering Sign (`signatures_complete` is what the
            // Sign/Broadcast switch reads), and the hot key's unified
            // signature then completes it.
            let tx = SpendTx::new(
                None,
                legacy_only.clone(),
                Vec::new(),
                &f.descriptor,
                &secp,
                Network::Bitcoin,
            );
            let mut state = PsbtState::new(state.wallet.clone(), tx, true);
            state.modal = Some(PsbtModal::Sign(SignModal::new(
                HashSet::new(),
                state.wallet.clone(),
                CoincubeDirectory::new(PathBuf::new()),
                Network::Bitcoin,
                true,
                None,
                None,
                false,
            )));
            let _ = state.reconcile_and_maybe_close(&entangled);
            assert!(
                state.modal.is_some(),
                "picker stays open at the legacy threshold"
            );
            let pill = state.replay_presentation(&entangled).unwrap();
            assert!(
                !pill.review.signatures_complete(&pill.entangled),
                "Sign stays offered"
            );
            assert!(!pill.broadcast_ready);
            state.tx.psbt = legacy(
                &legacy(&unified(&f.psbt, &f.signers[2]), &f.signers[0]),
                &f.signers[1],
            );
            let _ = state.reconcile_and_maybe_close(&entangled);
            assert!(
                state.modal.is_some(),
                "unsafe retained alternatives do not close the picker"
            );
            assert!(matches!(
                &state.replay.as_ref().unwrap().status(),
                ReplayStatus::Unknown(UnknownReason::Refused(_))
            ));
            state.tx.psbt = legacy(&unified(&f.psbt, &f.signers[0]), &f.signers[1]);
            let _ = state.reconcile_and_maybe_close(&entangled);
            assert!(
                state.modal.is_none(),
                "safe recreated spend closes the picker"
            );
            let pill = state.replay_presentation(&entangled).unwrap();
            assert!(pill.review.signatures_complete(&pill.entangled));
            assert!(pill.broadcast_ready);
        }

        #[test]
        fn anyonecanpay_is_refused_before_any_signer_is_dispatched() {
            let f = fixture();
            let mut asked = f.psbt.clone();
            asked.inputs[0].sighash_type = Some(
                coincube_core::miniscript::bitcoin::sighash::EcdsaSighashType::AllPlusAnyoneCanPay
                    .into(),
            );
            let wallet = wallet_with_hot_signer(&f);
            let mut modal = SignModal::new(
                HashSet::new(),
                wallet,
                CoincubeDirectory::new(PathBuf::new()),
                Network::Bitcoin,
                false,
                None,
                None,
                false,
            );
            assert!(modal.refuse_before_dispatch(&asked).is_some());
            assert!(modal
                .error
                .as_ref()
                .map(|e| e.to_string())
                .unwrap_or_default()
                .contains("ANYONECANPAY"));
            assert!(modal.signing.is_empty());
            // A clean PSBT is not refused.
            modal.error = None;
            assert!(modal.refuse_before_dispatch(&f.psbt).is_none());
            assert!(modal.error.is_none());
        }

        /// The spend screen re-checks a replayable spend's inputs at the
        /// moment it matters (`#276` I13, cache lifecycle): once per set of
        /// signatures; Broadcast is disabled and says it is checking while the
        /// answer is in flight (the tick does not override that); `Entangled`
        /// closes the gate; `Unknown` leaves the acknowledgement path open
        /// with the "could not check" copy; no Connect session means the
        /// check cannot run and says so. A reply for another spend is ignored.
        #[test]
        fn a_replayable_spend_rechecks_its_inputs_before_it_can_be_acknowledged() {
            use crate::app::state::vault::test_support::tokens;
            use crate::services::entangled::Entanglement;
            let f = fixture();
            let wallet = wallet_with_hot_signer(&f);
            let secp = secp256k1::Secp256k1::new();
            let legacy_only = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
            let txid = f.psbt.unsigned_tx.input[0].previous_output.txid;
            let spend = f.psbt.unsigned_tx.compute_txid();
            let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
                crate::daemon::client::Coincubed::new(MockDaemon::new(vec![]).run()),
            );
            let ack = || {
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::AcknowledgeReplay(true),
                ))
            };
            let new_state = |psbt: &Psbt| {
                PsbtState::new(
                    wallet.clone(),
                    SpendTx::new(
                        None,
                        psbt.clone(),
                        Vec::new(),
                        &f.descriptor,
                        &secp,
                        Network::Bitcoin,
                    ),
                    true,
                )
            };

            // No Connect session: the check cannot run; it says so and the
            // acknowledgement path stays open.
            let mut state = new_state(&legacy_only);
            assert_eq!(state.entangled_check, EntangledCheck::Idle);
            let no_session = Cache::default();
            let _ = state.update(daemon.clone(), &no_session, ack());
            assert_eq!(
                state.entangled_check,
                EntangledCheck::Done {
                    unresolved: vec![txid]
                }
            );
            let pill = state.replay_presentation(&no_session).unwrap();
            assert_eq!(pill.unresolved, vec![0]);
            assert!(!pill.checking);
            assert!(
                state.broadcast_ready(&no_session),
                "acknowledged, check could not run"
            );

            // With a session: in flight on the first message, Broadcast
            // disabled even though the tick was given.
            let session = Cache {
                connect_tokens: Some(tokens()),
                ..Cache::default()
            };
            let mut state = new_state(&legacy_only);
            let _ = state.update(daemon.clone(), &session, ack());
            assert!(
                matches!(&state.entangled_check, EntangledCheck::InFlight { txids, .. } if *txids == vec![txid])
            );
            assert!(state.replay.as_ref().unwrap().acknowledged());
            assert!(!state.broadcast_ready(&session));
            let pill = state.replay_presentation(&session).unwrap();
            assert!(pill.checking);
            assert!(!pill.broadcast_ready);
            // Once per signature set: another message does not restart it.
            let _ = state.update(daemon.clone(), &session, ack());
            assert!(
                matches!(&state.entangled_check, EntangledCheck::InFlight { txids, .. } if *txids == vec![txid])
            );

            // A reply for another spend is ignored, even with the right
            // generation.
            let generation = in_flight_generation(&state);
            let _ = state.update(
                daemon.clone(),
                &session,
                Message::EntangledRevalidated {
                    origin: origin_of(&session),
                    spend: Txid::from_str(
                        "0000000000000000000000000000000000000000000000000000000000000001",
                    )
                    .unwrap(),
                    generation,
                    answers: vec![observed(txid, Entanglement::Entangled)],
                },
            );
            assert!(
                matches!(&state.entangled_check, EntangledCheck::InFlight { txids, .. } if *txids == vec![txid])
            );

            // `Entangled` comes back (the app has cached it before routing):
            // the requirement gate closes, no acknowledgement helps.
            let mut entangled = Cache {
                connect_tokens: Some(tokens()),
                ..Cache::default()
            };
            entangled.record_entanglement(txid, Entanglement::Entangled, std::time::Instant::now());
            let _ = state.update(
                daemon.clone(),
                &entangled,
                Message::EntangledRevalidated {
                    origin: origin_of(&entangled),
                    spend,
                    generation,
                    answers: vec![observed(txid, Entanglement::Entangled)],
                },
            );
            assert_eq!(
                state.entangled_check,
                EntangledCheck::Done { unresolved: vec![] }
            );
            assert!(!state.broadcast_ready(&entangled));
            assert!(!state.replay_presentation(&entangled).unwrap().checking);

            // `Unknown` comes back: acknowledgement path open, copy says the
            // check could not complete.
            let mut state = new_state(&legacy_only);
            let _ = state.update(daemon.clone(), &session, ack());
            let generation = in_flight_generation(&state);
            let _ = state.update(
                daemon.clone(),
                &session,
                Message::EntangledRevalidated {
                    origin: origin_of(&session),
                    spend,
                    generation,
                    answers: vec![observed(txid, Entanglement::Unknown)],
                },
            );
            assert_eq!(
                state.entangled_check,
                EntangledCheck::Done {
                    unresolved: vec![txid]
                }
            );
            assert!(state.broadcast_ready(&session));
            assert_eq!(
                state.replay_presentation(&session).unwrap().unresolved,
                vec![0]
            );

            // A new signature is a new set: the re-check runs again, and the
            // tick from the previous set is gone.
            state.tx.psbt = legacy(&legacy_only, &f.signers[2]);
            let _ = state.reconcile_and_maybe_close(&session);
            assert!(
                matches!(&state.entangled_check, EntangledCheck::InFlight { txids, .. } if *txids == vec![txid])
            );
            assert!(!state.replay.as_ref().unwrap().acknowledged());

            // A protected spend checks nothing.
            let protected = unified(&unified(&f.psbt, &f.signers[0]), &f.signers[1]);
            let mut state = new_state(&protected);
            let _ = state.update(daemon.clone(), &session, ack());
            assert_eq!(state.entangled_check, EntangledCheck::Idle);
            assert!(state.broadcast_ready(&session));
        }

        /// Drive a task to completion, collecting the messages it emits. A
        /// `broadcast_spend_tx` against the empty mock daemon would panic the
        /// mock's thread and this future ("Mock Daemon must have all requests
        /// mocked"), so a completed drive is the proof that no RPC was made.
        async fn drive(task: Task<Message>) -> Vec<Message> {
            use iced::futures::StreamExt;
            use iced_runtime::{task::into_stream, Action};
            let mut out = Vec::new();
            if let Some(mut stream) = into_stream(task) {
                while let Some(action) = stream.next().await {
                    if let Action::Output(message) = action {
                        out.push(message);
                    }
                }
            }
            out
        }

        fn shows_error(messages: &[Message], needle: &str) -> bool {
            messages.iter().any(|m| {
                matches!(m, Message::View(view::Message::ShowError(text)) if text.contains(needle))
            })
        }

        /// Final Confirm is gated on the **current** cache (`#276` I13): a
        /// positive lookup landing after the Broadcast dialog was created, or
        /// between the gated click and the dialog's creation, or a re-check in
        /// flight, means Confirm dispatches nothing — the dialog closes and the
        /// reason is on screen. The empty mock daemon turns any dispatch into a
        /// panic, so the drive completing is the zero-call assertion.
        #[tokio::test]
        async fn confirm_is_gated_on_the_current_cache_in_every_ordering() {
            use crate::services::entangled::Entanglement;
            let f = fixture();
            let wallet = wallet_with_hot_signer(&f);
            let secp = secp256k1::Secp256k1::new();
            let legacy_only = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
            let txid = f.psbt.unsigned_tx.input[0].previous_output.txid;
            let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
                crate::daemon::client::Coincubed::new(MockDaemon::new(vec![]).run()),
            );
            let confirm = || Message::View(view::Message::Spend(view::SpendTxMessage::Confirm));
            let ack = || {
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::AcknowledgeReplay(true),
                ))
            };
            let open_dialog = || Message::BroadcastModal(Ok(HashSet::new()));
            let new_state = || {
                PsbtState::new(
                    wallet.clone(),
                    SpendTx::new(
                        None,
                        legacy_only.clone(),
                        Vec::new(),
                        &f.descriptor,
                        &secp,
                        Network::Bitcoin,
                    ),
                    true,
                )
            };
            // No Connect session: the re-check cannot run, the spend is
            // acknowledgeable, and the dialog opens.
            let unchecked = Cache::default();
            let mut entangled = Cache::default();
            entangled.record_entanglement(txid, Entanglement::Entangled, std::time::Instant::now());

            // 1. Lookup lands *after* the dialog is created.
            let mut state = new_state();
            let _ = drive(state.update(daemon.clone(), &unchecked, ack())).await;
            assert!(state.broadcast_ready(&unchecked));
            let _ = drive(state.update(daemon.clone(), &unchecked, open_dialog())).await;
            assert!(
                matches!(&state.modal, Some(PsbtModal::Broadcast(d)) if d.awaiting_confirmation())
            );
            let out = drive(state.update(daemon.clone(), &entangled, confirm())).await;
            assert!(
                state.modal.is_none(),
                "dialog closed back to the spend screen"
            );
            assert!(
                shows_error(&out, "also exists on Bitcoin"),
                "{:?}",
                out.len()
            );
            assert!(!state.broadcast_ready(&entangled));

            // 2. Lookup lands *before* the dialog is created (inside the
            //    `list_coins` window): the dialog never opens.
            let mut state = new_state();
            let _ = drive(state.update(daemon.clone(), &unchecked, ack())).await;
            let out = drive(state.update(daemon.clone(), &entangled, open_dialog())).await;
            assert!(state.modal.is_none(), "an unready dialog is never opened");
            assert!(shows_error(&out, "also exists on Bitcoin"));
            // …and a Confirm that somehow arrives anyway dispatches nothing.
            let _ = drive(state.update(daemon.clone(), &entangled, confirm())).await;
            assert!(state.modal.is_none());

            // 3. A re-check in flight when Confirm arrives: nothing dispatched,
            //    the checking copy is the reason.
            let mut state = new_state();
            let _ = drive(state.update(daemon.clone(), &unchecked, ack())).await;
            let _ = drive(state.update(daemon.clone(), &unchecked, open_dialog())).await;
            assert!(matches!(&state.modal, Some(PsbtModal::Broadcast(_))));
            state.entangled_check = EntangledCheck::InFlight {
                generation: u64::MAX,
                txids: vec![txid],
            };
            let out = drive(state.update(daemon.clone(), &unchecked, confirm())).await;
            assert!(state.modal.is_none());
            assert!(shows_error(&out, replay::CHECKING_COPY));

            // 4. A dialog open on a spend whose acknowledgement was dropped by
            //    a new signature (status equal, content changed) comes down on
            //    the next message too.
            let mut state = new_state();
            let _ = drive(state.update(daemon.clone(), &unchecked, ack())).await;
            let _ = drive(state.update(daemon.clone(), &unchecked, open_dialog())).await;
            state.tx.psbt = legacy(&legacy_only, &f.signers[2]);
            let out = drive(state.update(daemon.clone(), &unchecked, Message::Reconcile)).await;
            assert!(state.modal.is_none());
            assert!(shows_error(&out, replay::REPLAYABLE_ACKNOWLEDGEMENT));

            // Control: a ready spend's Confirm reaches the dialog, which
            // dispatches — the mock has no `broadcastspend` scripted, so the
            // drive is not attempted; the dialog's own state shows the
            // dispatch happened.
            let mut state = new_state();
            let _ = drive(state.update(daemon.clone(), &unchecked, ack())).await;
            let _ = drive(state.update(daemon.clone(), &unchecked, open_dialog())).await;
            let _task = state.update(daemon.clone(), &unchecked, confirm());
            assert!(
                matches!(&state.modal, Some(PsbtModal::Broadcast(d)) if !d.awaiting_confirmation())
            );
        }

        /// A re-check reply is accepted only for the generation currently in
        /// flight: a reply from an earlier instance of the screen, or from
        /// before a signature was added, never clears the current claim — and
        /// the message is applied before any new check is kicked, so a pass
        /// cannot start a check and resolve it with an older answer. A stale
        /// reply's positive still lands in the cache (the app does that before
        /// routing) and closes the gate.
        #[tokio::test]
        async fn a_stale_recheck_reply_never_clears_the_current_claim() {
            use crate::app::state::vault::test_support::tokens;
            use crate::services::entangled::Entanglement;
            let f = fixture();
            let wallet = wallet_with_hot_signer(&f);
            let secp = secp256k1::Secp256k1::new();
            let legacy_only = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
            let txid = f.psbt.unsigned_tx.input[0].previous_output.txid;
            let spend = f.psbt.unsigned_tx.compute_txid();
            let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
                crate::daemon::client::Coincubed::new(MockDaemon::new(vec![]).run()),
            );
            let ack = || {
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::AcknowledgeReplay(true),
                ))
            };
            let session = Cache {
                connect_tokens: Some(tokens()),
                ..Cache::default()
            };
            let new_state = |psbt: &Psbt| {
                PsbtState::new(
                    wallet.clone(),
                    SpendTx::new(
                        None,
                        psbt.clone(),
                        Vec::new(),
                        &f.descriptor,
                        &secp,
                        Network::Bitcoin,
                    ),
                    true,
                )
            };
            let reply = |generation: u64, answer: Entanglement| Message::EntangledRevalidated {
                origin: origin_of(&session),
                spend,
                generation,
                answers: vec![observed(txid, answer)],
            };

            // Screen instance A starts a check and is closed with it in flight.
            let mut a = new_state(&legacy_only);
            let _ = a.update(daemon.clone(), &session, ack());
            let stale = in_flight_generation(&a);
            drop(a);

            // Instance B reopens the same spend. A's reply arrives first, on a
            // fresh state: it must not both kick B's check and resolve it.
            let mut b = new_state(&legacy_only);
            let _ = b.update(
                daemon.clone(),
                &session,
                reply(stale, Entanglement::NotEntangled),
            );
            let current = in_flight_generation(&b);
            assert_ne!(current, stale, "generations are process-wide, never reused");
            assert!(!b.broadcast_ready(&session), "still checking");
            // The stale reply again, now with B in flight: ignored.
            let _ = b.update(
                daemon.clone(),
                &session,
                reply(stale, Entanglement::NotEntangled),
            );
            assert_eq!(in_flight_generation(&b), current);
            // B's own reply resolves it.
            let _ = b.update(
                daemon.clone(),
                &session,
                reply(current, Entanglement::NotEntangled),
            );
            assert_eq!(
                b.entangled_check,
                EntangledCheck::Done { unresolved: vec![] }
            );

            // A signature added while a reply is in flight: the new set gets a
            // new generation; the old reply is ignored, the new one lands.
            let mut c = new_state(&legacy_only);
            let _ = c.update(daemon.clone(), &session, ack());
            let first = in_flight_generation(&c);
            c.tx.psbt = legacy(&legacy_only, &f.signers[2]);
            let _ = c.update(daemon.clone(), &session, Message::Reconcile);
            let second = in_flight_generation(&c);
            assert_ne!(second, first);
            let _ = c.update(
                daemon.clone(),
                &session,
                reply(first, Entanglement::Unknown),
            );
            assert_eq!(in_flight_generation(&c), second, "stale reply ignored");
            let _ = c.update(
                daemon.clone(),
                &session,
                reply(second, Entanglement::Unknown),
            );
            assert_eq!(
                c.entangled_check,
                EntangledCheck::Done {
                    unresolved: vec![txid]
                }
            );

            // The cache is not the screen's to write: neither a stale nor an
            // accepted reply hands anything back to the app. The app records
            // every resolved answer under its own observation instant before
            // routing, and the cache is monotonic in that instant
            // (`Cache::record_entanglement`,
            // `entangled_cache_tests::a_stale_negative_cannot_re_stamp_a_newer_one`),
            // which is what keeps a stale negative from re-stamping a newer
            // one — the screen's generation filter only guards the claim.
            let mut e = new_state(&legacy_only);
            let _ = e.update(daemon.clone(), &session, ack());
            let live = in_flight_generation(&e);
            let stale_out = drive(e.update(
                daemon.clone(),
                &session,
                reply(stale, Entanglement::NotEntangled),
            ))
            .await;
            assert!(
                stale_out.is_empty(),
                "a stale reply emits nothing: {:?}",
                stale_out
            );
            assert_eq!(in_flight_generation(&e), live);
            let live_out = drive(e.update(
                daemon.clone(),
                &session,
                reply(live, Entanglement::NotEntangled),
            ))
            .await;
            assert!(
                live_out.is_empty(),
                "an accepted reply clears the claim and emits nothing: {:?}",
                live_out
            );
            assert_eq!(
                e.entangled_check,
                EntangledCheck::Done { unresolved: vec![] }
            );

            // A stale reply carrying `Entangled`: the claim is untouched, but
            // the answer is in the cache (as the app records it before
            // routing) and the gate is closed by it.
            let mut d = new_state(&legacy_only);
            let _ = d.update(daemon.clone(), &session, ack());
            let live = in_flight_generation(&d);
            let mut cached = Cache {
                connect_tokens: Some(tokens()),
                ..Cache::default()
            };
            cached.record_entanglement(txid, Entanglement::Entangled, std::time::Instant::now());
            let _ = d.update(
                daemon.clone(),
                &cached,
                reply(stale, Entanglement::Entangled),
            );
            assert_eq!(in_flight_generation(&d), live, "claim untouched");
            assert!(!d.broadcast_ready(&cached));
            let _ = d.update(
                daemon.clone(),
                &cached,
                reply(live, Entanglement::Entangled),
            );
            assert_eq!(
                d.entangled_check,
                EntangledCheck::Done { unresolved: vec![] }
            );
            assert!(!d.broadcast_ready(&cached), "gate closed by the positive");
            assert!(!d.replay_presentation(&cached).unwrap().broadcast_ready);
        }

        /// A reserved unified record ending `0xa1` reaches no device and no
        /// phone: the hardware arm and the Keychain-request arm of the picker
        /// both refuse through `update` before anything is dispatched, with
        /// the error set and no signer marked as signing.
        #[test]
        fn a_malformed_reserved_record_never_reaches_a_device_or_a_phone() {
            let f = fixture();
            let mut tx = SpendTx::new(
                None,
                unified(&f.psbt, &f.signers[0]),
                Vec::new(),
                &f.descriptor,
                &secp256k1::Secp256k1::new(),
                Network::Bitcoin,
            );
            let key = tx.psbt.inputs[0]
                .proprietary
                .keys()
                .next()
                .cloned()
                .unwrap();
            *tx.psbt.inputs[0]
                .proprietary
                .get_mut(&key)
                .unwrap()
                .last_mut()
                .unwrap() = 0xa1;
            let wallet = wallet_with_hot_signer(&f);
            let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(
                crate::daemon::client::Coincubed::new(MockDaemon::new(vec![]).run()),
            );
            for message in [
                // Hardware: index 0 of an (empty) device list — the refusal
                // fires before the list is even consulted.
                Message::View(view::Message::SelectHardwareWallet(0)),
                // Hot key.
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::SelectMasterSigner,
                )),
                // Keychain request (would be forwarded to the nested flow).
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::RequestFromEveryone,
                )),
                Message::View(view::Message::Spend(
                    view::SpendTxMessage::SelectKeychainSigner(
                        f.signers[1].fingerprint(&secp256k1::Secp256k1::new()),
                    ),
                )),
            ] {
                let mut modal = SignModal::new(
                    HashSet::new(),
                    wallet.clone(),
                    CoincubeDirectory::new(PathBuf::new()),
                    Network::Bitcoin,
                    true,
                    None,
                    None,
                    true,
                );
                let _ = modal.update(daemon.clone(), message, &mut tx);
                let error = modal
                    .error
                    .as_ref()
                    .map(|e| e.to_string())
                    .unwrap_or_default();
                assert!(error.contains("refusing to sign"), "{}", error);
                assert!(modal.signing.is_empty(), "nothing was dispatched");
            }
            // The same PSBT with a well-formed record passes the boundary.
            let clean = unified(&f.psbt, &f.signers[0]);
            let mut modal = SignModal::new(
                HashSet::new(),
                wallet,
                CoincubeDirectory::new(PathBuf::new()),
                Network::Bitcoin,
                true,
                None,
                None,
                true,
            );
            assert!(modal.refuse_before_dispatch(&clean).is_none());
        }
    }
}
