use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use iced::Subscription;

use coincube_core::{
    border_wallet::{
        build_mnemonic, sign_psbt_with_border_wallet, CellRef, GridRecoveryPhrase, OrderedPattern,
        WordGrid, PATTERN_LENGTH,
    },
    descriptors::CoincubePolicy,
    miniscript::bitcoin::{bip32::Fingerprint, psbt::Psbt, Network, Txid},
};
use coincubed::commands::CoinStatus;
use iced::Task;
use zeroize::{Zeroize, Zeroizing};

use coincube_ui::component::form;
use coincube_ui::{widget::modal, widget::Element};

use crate::daemon::model::LabelsLoader;
use crate::export::{ImportExportMessage, ImportExportType, Progress};
use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::{Menu, VaultSubMenu},
        message::Message,
        state::vault::label::{label_item_from_str, LabelsEdited},
        view,
        view::BorderWalletReconMessage,
        wallet::{Wallet, WalletError},
    },
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

pub struct PsbtState {
    pub wallet: Arc<Wallet>,
    pub desc_policy: CoincubePolicy,
    pub tx: SpendTx,
    pub saved: bool,
    pub warning: Option<Error>,
    pub labels_edited: LabelsEdited,
    pub modal: Option<PsbtModal>,
}

impl PsbtState {
    pub fn new(wallet: Arc<Wallet>, tx: SpendTx, saved: bool) -> Self {
        Self {
            desc_policy: wallet.main_descriptor.policy(),
            wallet,
            labels_edited: LabelsEdited::default(),
            warning: None,
            modal: None,
            tx,
            saved,
        }
    }

    pub fn interrupt(&mut self) {
        self.modal = None;
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
        match message {
            Message::View(view::Message::ExportPsbt) => {
                if self.modal.is_none() {
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
                );
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
                    }));
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
            Message::View(view::Message::Spend(view::SpendTxMessage::Broadcast)) => {
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
                        let err_msg = e.to_string();
                        self.warning = Some(e);
                        return Task::done(Message::View(view::Message::ShowError(err_msg)));
                    }
                };
            }
            Message::Updated(Ok(_)) => {
                self.saved = true;
                if let Some(modal) = self.modal.as_mut() {
                    let cmd = modal.as_mut().update(daemon.clone(), message, &mut self.tx);
                    // Recompute path_ready / sigs against the newly merged
                    // PSBT so the close decision below — and the outer view's
                    // Sign→Broadcast swap — reflect the just-added signature.
                    // The signature may have come from a local device
                    // (`SignModal`'s own Updated arm) or a Keychain session
                    // (which re-emits `Updated(Ok)` through here after its
                    // merge+persist), so recompute unconditionally.
                    if let Ok(sigs) = self
                        .wallet
                        .main_descriptor
                        .partial_spend_info(&self.tx.psbt)
                    {
                        self.tx.sigs = sigs;
                    }
                    // Close the unified picker only when the spending path
                    // threshold is met, or when it was dismissed mid-flight
                    // and its Keychain sessions have finished draining. A
                    // single local/keychain signature that doesn't meet the
                    // threshold keeps the picker open so more signers can be
                    // added.
                    let close = match modal {
                        PsbtModal::Sign(sign) => {
                            self.tx.path_ready().is_some() || sign.should_close_after_dismiss()
                        }
                        _ => false,
                    };
                    if close {
                        // Best-effort: cancel any Keychain sessions still in
                        // flight so they don't outlive the closed picker.
                        let extra = if let PsbtModal::Sign(sign) = modal {
                            sign.cancel_keychain_if_active()
                        } else {
                            Task::none()
                        };
                        self.modal = None;
                        return Task::batch([cmd, extra]);
                    }
                    return cmd;
                }
            }
            Message::BroadcastModal(res) => match res {
                Ok(conflicting_txids) => {
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
                    let amount_display =
                        display_amount.to_formatted_string_with_unit(cache.bitcoin_unit);
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
                        is_self_transfer,
                        wallet: self.wallet.clone(),
                    }));
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },
            Message::Export(ImportExportMessage::Progress(Progress::Psbt(psbt))) => {
                merge_signatures(&mut self.tx.psbt, &psbt);
                self.tx.sigs = self
                    .wallet
                    .main_descriptor
                    .partial_spend_info(&self.tx.psbt)
                    .expect("already check in psbt import logic");
            }
            _ => {
                if let Some(modal) = self.modal.as_mut() {
                    return modal.as_mut().update(daemon.clone(), message, &mut self.tx);
                }
            }
        }
        Task::none()
    }

    pub fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = view::vault::psbt::psbt_view(
            cache,
            &self.tx,
            self.saved,
            &self.desc_policy,
            &self.wallet.keys_aliases,
            self.labels_edited.cache(),
            cache.network,
            if let Some(PsbtModal::Sign(m)) = &self.modal {
                m.is_signing()
            } else {
                false
            },
            cache.bitcoin_unit,
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
                    let err_msg = e.to_string();
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
    /// True when every output of the tx returns to the wallet's own change
    /// addresses. Used by the celebration view to render the right phrasing.
    is_self_transfer: bool,
    /// Wallet handle used to register a successful broadcast in
    /// `Wallet::recently_broadcast` so the other panels (Transactions,
    /// Overview balance, Send) can optimistically reflect the spend
    /// before the daemon's mempool poller catches up.
    wallet: Arc<Wallet>,
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
                        let err_msg = e.to_string();
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
        modal::Modal::new(
            content,
            view::vault::psbt::broadcast_action(
                &self.conflicting_txids,
                self.broadcast,
                self.broadcasting,
                self.error.as_ref().map(|e| e.to_string()),
                &self.spend_amount_display,
                &self.sent_quote,
                &self.sent_image_handle,
                self.is_self_transfer,
            ),
        )
        .on_blur(on_blur)
        .into()
    }
}

/// "Keychain signing needs Connect" dialog. Pure UI — its two actions
/// (sign in / pair a phone) are handled in `PsbtState::update`, which
/// clears this modal and emits the matching navigation message.
pub struct KeychainUnavailableModal {
    /// Whether the user already has Connect tokens. When true the blocker
    /// is device/stream readiness rather than a missing sign-in, which the
    /// view uses to adjust its wording.
    pub signed_in: bool,
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
    let descriptor_id = wallet.main_descriptor.to_string();
    let modal = super::keychain_sign::KeychainSignModal::new(
        wallet.clone(),
        coincube_client,
        tokens,
        grpc_url,
        device_id,
        cube_server_id,
        cache.cube_id.clone(),
        descriptor_id,
        psbt.clone(),
    );
    let launch = modal.launch();
    (Some(modal), launch)
}

impl Modal for KeychainUnavailableModal {
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(
            content,
            view::vault::psbt::keychain_unavailable_action(self.signed_in),
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
                    let err_msg = e.to_string();
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
            error: None,
        }
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
                            self.error = Some(format!(
                                "Please select exactly {} cells. Currently selected: {}",
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
                                    self.error =
                                        Some(format!("Mnemonic construction failed: {:?}", e));
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
                        Err(e) => self.error = Some(format!("{:?}", e)),
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
    }
}

pub struct SignModal {
    wallet: Arc<Wallet>,
    hws: HardwareWallets,
    network: Network,
    error: Option<Error>,
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
    /// Spending-path identities the user has expanded, keyed the same way as
    /// `ToggleSpendPath`: `None` = primary, `Some(seq)` = a recovery path.
    /// Inactive cards default to collapsed.
    expanded_paths: HashSet<Option<u16>>,
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
            signing: HashSet::new(),
            hws: HardwareWallets::new(datadir_path, network).with_wallet(wallet.clone()),
            wallet,
            network,
            error: None,
            signed,
            is_saved,
            display_modal: true,
            recovery_timelock,
            border_wallet_recon: None,
            keychain,
            keychain_enabled,
            expanded_paths: HashSet::new(),
        }
    }

    pub fn is_signing(&self) -> bool {
        !self.signing.is_empty()
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
        let keep = k.has_undrained_sessions();
        if keep {
            k.mark_dismissed();
            self.display_modal = false;
        }
        (cancel, keep)
    }

    /// True once the picker was dismissed (hidden) and its keychain sessions
    /// have all drained — the signal the panel uses to finally drop it.
    pub fn should_close_after_dismiss(&self) -> bool {
        !self.display_modal
            && self
                .keychain
                .as_ref()
                .is_none_or(|k| !k.has_undrained_sessions())
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
        let policy = self.wallet.main_descriptor.policy();
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
        // Keychain session for this key, only once the flow has resolved.
        let kc = self
            .keychain
            .as_ref()
            .filter(|k| k.is_resolved())
            .and_then(|k| {
                k.pending()
                    .iter()
                    .enumerate()
                    .find(|(_, p)| p.fingerprint == fp)
                    .map(|(i, p)| {
                        (
                            i,
                            p.status,
                            p.status.is_terminal_success() && p.signed_psbt_persisted,
                        )
                    })
            });
        let kind = self.signing_key_kind(fp, master_fp, kc.is_some());

        // Signature already collected (local or keychain).
        if self.signed.contains(&fp) || kc.map(|(_, _, s)| s).unwrap_or(false) {
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
            .position(|hw| hw.fingerprint() == Some(fp))
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
        // A Connect-resolved Keychain signer.
        if let Some((idx, status, _)) = kc {
            return match status {
                PendingSessionStatus::Idle => (Kind::Keychain, St::Available(Act::Keychain)),
                PendingSessionStatus::Rejected
                | PendingSessionStatus::Expired
                | PendingSessionStatus::Failed => (Kind::Keychain, St::Retry(idx)),
                other => (Kind::Keychain, St::InProgress(other.label().to_string())),
            };
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
            return (
                Kind::Unknown,
                St::NeedsSignIn("Connect a device to sign with this key.".to_string()),
            );
        }
        (
            Kind::Unknown,
            St::Disabled("Connect this signing device to sign.".to_string()),
        )
    }

    /// Icon kind for a descriptor key: master / border / connected-hardware /
    /// resolved-keychain, else unknown.
    fn signing_key_kind(
        &self,
        fp: Fingerprint,
        master_fp: Option<Fingerprint>,
        is_keychain: bool,
    ) -> view::vault::psbt::SigningKeyKind {
        use view::vault::psbt::SigningKeyKind as Kind;
        if master_fp == Some(fp) {
            Kind::Master
        } else if self.wallet.border_wallet_fingerprints.contains(&fp) {
            Kind::BorderWallet
        } else if self.hws.list.iter().any(|hw| hw.fingerprint() == Some(fp)) {
            Kind::Hardware
        } else if is_keychain {
            Kind::Keychain
        } else {
            Kind::Unknown
        }
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

    /// Keychain-flow banners for the unified picker (errors, degraded stream,
    /// unaddressable signers) — surfaced above the signer list. Empty when no
    /// keychain flow is active or everything is healthy.
    fn keychain_notices(&self) -> Vec<String> {
        let Some(k) = self.keychain.as_ref() else {
            return Vec::new();
        };
        let mut notices = Vec::new();
        if let Some(err) = k.error() {
            notices.push(format!("Couldn't start Keychain signing: {}", err));
        }
        if let Some(banner) = k.stream_health_banner() {
            notices.push(banner);
        }
        for u in k.unresolved() {
            notices.push(format!(
                "Can't sign with {} — this signer has no registered device.",
                u
            ));
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
                if let Some(HardwareWallet::Supported {
                    fingerprint,
                    device,
                    ..
                }) = self.hws.list.get(i)
                {
                    // Keep the modal open (as the master-signer path below
                    // does) so the selected device shows its "Processing… /
                    // Please check your device" state while we wait for the
                    // signature, rather than the modal vanishing with no
                    // indication that the device is awaiting confirmation.
                    self.signing.insert(*fingerprint);
                    let psbt = tx.psbt.clone();
                    let fingerprint = *fingerprint;
                    return Task::perform(
                        sign_psbt(self.wallet.clone(), device.clone(), psbt),
                        move |res| Message::Signed(fingerprint, res),
                    );
                }
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::SelectMasterSigner)) => {
                if let Some(fingerprint) = self.wallet.signer.as_ref().map(|s| s.fingerprint()) {
                    self.signing.insert(fingerprint);
                }
                return Task::perform(
                    sign_psbt_with_master_signer(self.wallet.clone(), tx.psbt.clone()),
                    |(fg, res)| Message::Signed(fg, res),
                );
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::SelectBorderWallet(fg))) => {
                let network = self.network;
                self.border_wallet_recon = Some(BorderWalletReconstructionState::new(fg, network));
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
                        let psbt = tx.psbt.clone();
                        // Clear reconstruction state (zeroizes phrase words via Drop).
                        self.border_wallet_recon = None;
                        self.display_modal = false;
                        self.signing.insert(fingerprint);
                        return Task::perform(
                            async move {
                                let result = sign_psbt_with_border_wallet(
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
                match res {
                    Err(e) => {
                        self.display_modal = true;
                        if !matches!(e, Error::HardwareWallet(async_hwi::Error::UserRefused)) {
                            let err_msg = e.to_string();
                            self.error = Some(e);
                            return Task::done(Message::View(view::Message::ShowError(err_msg)));
                        }
                    }
                    Ok(psbt) => {
                        self.error = None;
                        // Bring the picker back into view after a local sign
                        // (the border-wallet path hides it during the async
                        // reconstruction). The picker now closes only when the
                        // threshold is met — decided by the panel's
                        // `Message::Updated(Ok)` handler — so a sub-threshold
                        // signature should leave it visible to add more.
                        self.display_modal = true;
                        self.signed.insert(fingerprint);
                        let daemon = daemon.clone();
                        merge_signatures(&mut tx.psbt, &psbt);
                        if self.is_saved {
                            return Task::perform(
                                async move { daemon.update_spend_tx(&psbt).await.map_err(|e| e.into()) },
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
                                    daemon.update_spend_tx(&psbt).await?;
                                    daemon.update_labels(&labels).await.map_err(|e| e.into())
                                },
                                Message::Updated,
                            );
                        }
                    }
                }
            }
            Message::Updated(res) => match res {
                Ok(()) => match self.wallet.main_descriptor.partial_spend_info(&tx.psbt) {
                    Ok(sigs) => tx.sigs = sigs,
                    Err(e) => {
                        let err_msg = e.to_string();
                        self.error = Some(Error::Unexpected(err_msg.clone()));
                        return Task::done(Message::View(view::Message::ShowError(err_msg)));
                    }
                },
                Err(e) => {
                    let err_msg = e.to_string();
                    self.error = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },

            Message::HardwareWallets(msg) => match self.hws.update(msg) {
                Ok(cmd) => {
                    return cmd.map(Message::HardwareWallets);
                }
                Err(e) => {
                    let err_msg = e.to_string();
                    self.error = Some(e.into());
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
                let keychain_notices = self.keychain_notices();
                modal::Modal::new(
                    content,
                    view::vault::psbt::sign_action(paths, keychain_notices),
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

/// Merge BIP32 / Tapscript signatures from `signed_psbt` into `psbt`.
/// Public so the Keychain sign flow can reuse the same merge semantics.
pub(crate) fn merge_signatures_pub(psbt: &mut Psbt, signed_psbt: &Psbt) {
    merge_signatures(psbt, signed_psbt)
}

async fn sign_psbt_with_master_signer(
    wallet: Arc<Wallet>,
    psbt: Psbt,
) -> (Fingerprint, Result<Psbt, Error>) {
    if let Some(signer) = &wallet.signer {
        let res = signer
            .sign_psbt(psbt)
            .map_err(|e| {
                WalletError::MasterSigner(format!("Master signer failed to sign psbt: {}", e))
            })
            .map_err(|e| e.into());
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
    // The BitBox02 is only going to produce a signature for a single key in the Script. In order
    // to make sure it doesn't sign for a public key from another spending path we remove the BIP32
    // derivation for the other paths.
    if matches!(hw.device_kind(), async_hwi::DeviceKind::BitBox02) {
        // We need to make sure we don't prune the BIP32 derivations from the original PSBT (which
        // would end up being updated in the daemon's database and erase the previously unpruned
        // one). To this end we create a new, pruned, psbt we use for signing and then merge its
        // signatures back into the original PSBT.
        let mut pruned_psbt = wallet
            .main_descriptor
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
                    &pruned_psbt
                );
            }
        }
    } else {
        hw.sign_tx(&mut psbt).await.map_err(Error::from)?;
    }
    Ok(psbt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{cache::Cache, state::PsbtsPanel},
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

    fn empty_psbt() -> Psbt {
        use coincube_core::miniscript::bitcoin::{absolute, transaction, Transaction};

        Psbt::from_unsigned_tx(Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
            input: vec![],
            output: vec![],
        })
        .expect("empty unsigned tx is psbt-compatible")
    }

    fn tokens(
    ) -> Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>> {
        Arc::new(tokio::sync::RwLock::new(
            crate::services::connect::client::auth::AccessTokenResponse {
                access_token: "access".to_string(),
                refresh_token: "refresh".to_string(),
                expires_at: i64::MAX,
            },
        ))
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
}
