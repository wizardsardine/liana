use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use iced::Subscription;

use iced::Command;
use liana::{
    descriptors::LianaPolicy,
    miniscript::bitcoin::{bip32::Fingerprint, psbt::Psbt, Network},
};

use liana_ui::component::toast;
use liana_ui::{
    component::{form, modal},
    widget::Element,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        message::Message,
        state::label::{label_item_from_str, LabelsEdited},
        view,
        wallet::{Wallet, WalletError},
    },
    daemon::{
        model::{LabelItem, Labelled, SpendStatus, SpendTx},
        Daemon,
    },
    hw::{HardwareWallet, HardwareWallets},
};

pub trait Action {
    fn warning(&self) -> Option<&Error> {
        None
    }
    fn load(&self, _daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        Command::none()
    }
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }
    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _message: Message,
        _tx: &mut SpendTx,
    ) -> Command<Message> {
        Command::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message>;
}

pub enum PsbtAction {
    Save(SaveAction),
    Sign(SignAction),
    Update(UpdateAction),
    Broadcast(BroadcastAction),
    Delete(DeleteAction),
}

impl<'a> AsRef<dyn Action + 'a> for PsbtAction {
    fn as_ref(&self) -> &(dyn Action + 'a) {
        match &self {
            Self::Save(a) => a,
            Self::Sign(a) => a,
            Self::Update(a) => a,
            Self::Broadcast(a) => a,
            Self::Delete(a) => a,
        }
    }
}

impl<'a> AsMut<dyn Action + 'a> for PsbtAction {
    fn as_mut(&mut self) -> &mut (dyn Action + 'a) {
        match self {
            Self::Save(a) => a,
            Self::Sign(a) => a,
            Self::Update(a) => a,
            Self::Broadcast(a) => a,
            Self::Delete(a) => a,
        }
    }
}

pub struct PsbtState {
    pub wallet: Arc<Wallet>,
    pub desc_policy: LianaPolicy,
    pub tx: SpendTx,
    pub saved: bool,
    pub warning: Option<Error>,
    pub labels_edited: LabelsEdited,
    pub action: Option<PsbtAction>,
}

impl PsbtState {
    pub fn new(wallet: Arc<Wallet>, tx: SpendTx, saved: bool) -> Self {
        Self {
            desc_policy: wallet.main_descriptor.policy(),
            wallet,
            labels_edited: LabelsEdited::default(),
            warning: None,
            action: None,
            tx,
            saved,
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        if let Some(action) = &self.action {
            action.as_ref().subscription()
        } else {
            Subscription::none()
        }
    }

    pub fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        if let Some(action) = &self.action {
            action.as_ref().load(daemon)
        } else {
            Command::none()
        }
    }

    pub fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match &message {
            Message::View(view::Message::Spend(msg)) => match msg {
                view::SpendTxMessage::Cancel => {
                    if let Some(PsbtAction::Sign(SignAction { display_modal, .. })) =
                        &mut self.action
                    {
                        *display_modal = false;
                        return Command::none();
                    }

                    self.action = None;
                }
                view::SpendTxMessage::Delete => {
                    self.action = Some(PsbtAction::Delete(DeleteAction::default()));
                }
                view::SpendTxMessage::Sign => {
                    if let Some(PsbtAction::Sign(SignAction { display_modal, .. })) =
                        &mut self.action
                    {
                        *display_modal = true;
                        return Command::none();
                    }

                    let action = SignAction::new(
                        self.tx.signers(),
                        self.wallet.clone(),
                        cache.datadir_path.clone(),
                        cache.network,
                        self.saved,
                    );
                    let cmd = action.load(daemon);
                    self.action = Some(PsbtAction::Sign(action));
                    return cmd;
                }
                view::SpendTxMessage::EditPsbt => {
                    let action = UpdateAction::new(self.wallet.clone(), self.tx.psbt.to_string());
                    let cmd = action.load(daemon);
                    self.action = Some(PsbtAction::Update(action));
                    return cmd;
                }
                view::SpendTxMessage::Broadcast => {
                    self.action = Some(PsbtAction::Broadcast(BroadcastAction::default()));
                }
                view::SpendTxMessage::Save => {
                    self.action = Some(PsbtAction::Save(SaveAction::default()));
                }
                _ => {
                    if let Some(action) = self.action.as_mut() {
                        return action
                            .as_mut()
                            .update(daemon.clone(), message, &mut self.tx);
                    }
                }
            },
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    std::iter::once(&mut self.tx).map(|tx| tx as &mut dyn Labelled),
                ) {
                    Ok(cmd) => {
                        return cmd;
                    }
                    Err(e) => {
                        self.warning = Some(e);
                    }
                };
            }
            Message::Updated(Ok(_)) => {
                self.saved = true;
                if let Some(action) = self.action.as_mut() {
                    return action
                        .as_mut()
                        .update(daemon.clone(), message, &mut self.tx);
                }
            }
            _ => {
                if let Some(action) = self.action.as_mut() {
                    return action
                        .as_mut()
                        .update(daemon.clone(), message, &mut self.tx);
                }
            }
        };
        Command::none()
    }

    pub fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = view::psbt::psbt_view(
            cache,
            &self.tx,
            self.saved,
            &self.desc_policy,
            &self.wallet.keys_aliases,
            self.labels_edited.cache(),
            cache.network,
            self.warning.as_ref(),
        );
        if let Some(action) = &self.action {
            action.as_ref().view(content)
        } else {
            content
        }
    }
}

#[derive(Default)]
pub struct SaveAction {
    saved: bool,
    error: Option<Error>,
}

impl Action for SaveAction {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Command<Message> {
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
                return Command::perform(
                    async move {
                        daemon.update_spend_tx(&psbt)?;
                        daemon.update_labels(&labels).map_err(|e| e.into())
                    },
                    Message::Updated,
                );
            }
            Message::Updated(res) => match res {
                Ok(()) => self.saved = true,
                Err(e) => self.error = Some(e),
            },
            _ => {}
        }
        Command::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(
            content,
            view::psbt::save_action(self.error.as_ref(), self.saved),
        )
        .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
        .into()
    }
}

#[derive(Default)]
pub struct BroadcastAction {
    broadcast: bool,
    error: Option<Error>,
}

impl Action for BroadcastAction {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::Confirm)) => {
                let daemon = daemon.clone();
                let psbt = tx.psbt.clone();
                self.error = None;
                return Command::perform(
                    async move {
                        daemon
                            .broadcast_spend_tx(&psbt.unsigned_tx.txid())
                            .map_err(|e| e.into())
                    },
                    Message::Updated,
                );
            }
            Message::Updated(res) => match res {
                Ok(()) => {
                    tx.status = SpendStatus::Broadcast;
                    self.broadcast = true;
                }
                Err(e) => self.error = Some(e),
            },
            _ => {}
        }
        Command::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(
            content,
            view::psbt::broadcast_action(self.error.as_ref(), self.broadcast),
        )
        .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
        .into()
    }
}

#[derive(Default)]
pub struct DeleteAction {
    deleted: bool,
    error: Option<Error>,
}

impl Action for DeleteAction {
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::Confirm)) => {
                let daemon = daemon.clone();
                let psbt = tx.psbt.clone();
                self.error = None;
                return Command::perform(
                    async move {
                        daemon
                            .delete_spend_tx(&psbt.unsigned_tx.txid())
                            .map_err(|e| e.into())
                    },
                    Message::Updated,
                );
            }
            Message::Updated(res) => match res {
                Ok(()) => self.deleted = true,
                Err(e) => self.error = Some(e),
            },
            _ => {}
        }
        Command::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(
            content,
            view::psbt::delete_action(self.error.as_ref(), self.deleted),
        )
        .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
        .into()
    }
}

pub struct SignAction {
    wallet: Arc<Wallet>,
    hws: HardwareWallets,
    error: Option<Error>,
    signing: HashSet<Fingerprint>,
    signed: HashSet<Fingerprint>,
    is_saved: bool,
    display_modal: bool,
}

impl SignAction {
    pub fn new(
        signed: HashSet<Fingerprint>,
        wallet: Arc<Wallet>,
        datadir_path: PathBuf,
        network: Network,
        is_saved: bool,
    ) -> Self {
        Self {
            signing: HashSet::new(),
            hws: HardwareWallets::new(datadir_path, network).with_wallet(wallet.clone()),
            wallet,
            error: None,
            signed,
            is_saved,
            display_modal: true,
        }
    }
}

impl Action for SignAction {
    fn warning(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    fn subscription(&self) -> Subscription<Message> {
        self.hws.refresh().map(Message::HardwareWallets)
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::SelectHardwareWallet(i)) => {
                if let Some(HardwareWallet::Supported {
                    fingerprint,
                    device,
                    ..
                }) = self.hws.list.get(i)
                {
                    self.display_modal = false;
                    self.signing.insert(*fingerprint);
                    let psbt = tx.psbt.clone();
                    let fingerprint = *fingerprint;
                    return Command::perform(
                        sign_psbt(self.wallet.clone(), device.clone(), psbt),
                        move |res| Message::Signed(fingerprint, res),
                    );
                }
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::SelectHotSigner)) => {
                return Command::perform(
                    sign_psbt_with_hot_signer(self.wallet.clone(), tx.psbt.clone()),
                    |(fg, res)| Message::Signed(fg, res),
                );
            }
            Message::Signed(fingerprint, res) => {
                self.signing.remove(&fingerprint);
                match res {
                    Err(e) => self.error = Some(e),
                    Ok(psbt) => {
                        self.error = None;
                        self.signed.insert(fingerprint);
                        let daemon = daemon.clone();
                        merge_signatures(&mut tx.psbt, &psbt);
                        if self.is_saved {
                            return Command::perform(
                                async move { daemon.update_spend_tx(&psbt).map_err(|e| e.into()) },
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
                            return Command::perform(
                                async move {
                                    daemon.update_spend_tx(&psbt)?;
                                    daemon.update_labels(&labels).map_err(|e| e.into())
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
                    Err(e) => self.error = Some(Error::Unexpected(e.to_string())),
                },
                Err(e) => self.error = Some(e),
            },

            Message::HardwareWallets(msg) => match self.hws.update(msg) {
                Ok(cmd) => {
                    return cmd.map(Message::HardwareWallets);
                }
                Err(e) => {
                    self.error = Some(e.into());
                }
            },
            _ => {}
        };
        Command::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        let content = toast::Manager::new(
            content,
            view::psbt::sign_action_toasts(&self.hws.list, &self.signing),
        )
        .into();
        if self.display_modal {
            modal::Modal::new(
                content,
                view::psbt::sign_action(
                    self.error.as_ref(),
                    &self.hws.list,
                    self.wallet.signer.as_ref().map(|s| s.fingerprint()),
                    self.wallet
                        .signer
                        .as_ref()
                        .and_then(|signer| self.wallet.keys_aliases.get(&signer.fingerprint)),
                    &self.signed,
                    &self.signing,
                ),
            )
            .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
            .into()
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
    }
}

async fn sign_psbt_with_hot_signer(
    wallet: Arc<Wallet>,
    psbt: Psbt,
) -> (Fingerprint, Result<Psbt, Error>) {
    if let Some(signer) = &wallet.signer {
        let res = signer
            .sign_psbt(psbt)
            .map_err(|e| WalletError::HotSigner(format!("Hot signer failed to sign psbt: {}", e)))
            .map_err(|e| e.into());
        (signer.fingerprint(), res)
    } else {
        (
            Fingerprint::default(),
            Err(WalletError::HotSigner("Hot signer not loaded".to_string()).into()),
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

pub struct UpdateAction {
    wallet: Arc<Wallet>,
    psbt: String,
    updated: form::Value<String>,
    processing: bool,
    error: Option<Error>,
    success: bool,
}

impl UpdateAction {
    pub fn new(wallet: Arc<Wallet>, psbt: String) -> Self {
        Self {
            wallet,
            psbt,
            updated: form::Value::default(),
            processing: false,
            error: None,
            success: false,
        }
    }
}

impl Action for UpdateAction {
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        modal::Modal::new(
            content,
            if self.success {
                view::psbt::update_spend_success_view()
            } else {
                view::psbt::update_spend_view(
                    self.psbt.clone(),
                    &self.updated,
                    self.error.as_ref(),
                    self.processing,
                )
            },
        )
        .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
        .into()
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Command<Message> {
        match message {
            Message::Updated(res) => {
                self.processing = false;
                match res {
                    Ok(()) => {
                        self.success = true;
                        self.error = None;
                        let psbt = Psbt::deserialize(&base64::decode(&self.updated.value).unwrap())
                            .expect("Already checked");
                        for (i, input) in tx.psbt.inputs.iter_mut().enumerate() {
                            if tx
                                .psbt
                                .unsigned_tx
                                .input
                                .get(i)
                                .map(|tx_in| tx_in.previous_output)
                                != psbt
                                    .unsigned_tx
                                    .input
                                    .get(i)
                                    .map(|tx_in| tx_in.previous_output)
                            {
                                continue;
                            }
                            if let Some(updated_input) = psbt.inputs.get(i) {
                                input
                                    .partial_sigs
                                    .extend(updated_input.partial_sigs.clone().into_iter());
                            }
                        }
                        tx.sigs = self
                            .wallet
                            .main_descriptor
                            .partial_spend_info(&tx.psbt)
                            .unwrap();
                    }
                    Err(e) => self.error = e.into(),
                }
            }
            Message::View(view::Message::ImportSpend(view::ImportSpendMessage::PsbtEdited(s))) => {
                self.updated.value = s;
                if let Some(psbt) = base64::decode(&self.updated.value)
                    .ok()
                    .and_then(|bytes| Psbt::deserialize(&bytes).ok())
                {
                    self.updated.valid = tx.psbt.unsigned_tx.txid() == psbt.unsigned_tx.txid();
                }
            }
            Message::View(view::Message::ImportSpend(view::ImportSpendMessage::Confirm)) => {
                if self.updated.valid {
                    self.processing = true;
                    self.error = None;
                    let updated = Psbt::deserialize(
                        &base64::decode(&self.updated.value).expect("Already checked"),
                    )
                    .unwrap();
                    return Command::perform(
                        async move { daemon.update_spend_tx(&updated).map_err(|e| e.into()) },
                        Message::Updated,
                    );
                }
            }
            _ => {}
        }

        Command::none()
    }
}
