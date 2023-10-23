use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use iced::Command;
use liana::{
    descriptors::LianaPolicy,
    miniscript::bitcoin::{bip32::Fingerprint, psbt::Psbt},
};

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
    hw::{list_hardware_wallets, HardwareWallet},
};

pub trait Action {
    fn warning(&self) -> Option<&Error> {
        None
    }
    fn load(&self, _daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        Command::none()
    }
    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _message: Message,
        _tx: &mut SpendTx,
    ) -> Command<Message> {
        Command::none()
    }
    fn view(&self) -> Element<view::Message>;
}

pub struct PsbtState {
    pub wallet: Arc<Wallet>,
    pub desc_policy: LianaPolicy,
    pub tx: SpendTx,
    pub saved: bool,
    pub warning: Option<Error>,
    pub labels_edited: LabelsEdited,
    pub action: Option<Box<dyn Action>>,
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

    pub fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        if let Some(action) = &self.action {
            action.load(daemon)
        } else {
            Command::none()
        }
    }

    pub fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match &message {
            Message::View(view::Message::Spend(msg)) => match msg {
                view::SpendTxMessage::Cancel => {
                    self.action = None;
                }
                view::SpendTxMessage::Delete => {
                    self.action = Some(Box::<DeleteAction>::default());
                }
                view::SpendTxMessage::Sign => {
                    let action = SignAction::new(self.tx.signers(), self.wallet.clone());
                    let cmd = action.load(daemon);
                    self.action = Some(Box::new(action));
                    return cmd;
                }
                view::SpendTxMessage::EditPsbt => {
                    let action = UpdateAction::new(self.wallet.clone(), self.tx.psbt.to_string());
                    let cmd = action.load(daemon);
                    self.action = Some(Box::new(action));
                    return cmd;
                }
                view::SpendTxMessage::Broadcast => {
                    self.action = Some(Box::<BroadcastAction>::default());
                }
                view::SpendTxMessage::Save => {
                    self.action = Some(Box::<SaveAction>::default());
                }
                _ => {
                    if let Some(action) = self.action.as_mut() {
                        return action.update(daemon.clone(), message, &mut self.tx);
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
                    return action.update(daemon.clone(), message, &mut self.tx);
                }
            }
            _ => {
                if let Some(action) = self.action.as_mut() {
                    return action.update(daemon.clone(), message, &mut self.tx);
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
            modal::Modal::new(content, action.view())
                .on_blur(Some(view::Message::Spend(view::SpendTxMessage::Cancel)))
                .into()
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
    fn view(&self) -> Element<view::Message> {
        view::psbt::save_action(self.error.as_ref(), self.saved)
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
    fn view(&self) -> Element<view::Message> {
        view::psbt::broadcast_action(self.error.as_ref(), self.broadcast)
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
    fn view(&self) -> Element<view::Message> {
        view::psbt::delete_action(self.error.as_ref(), self.deleted)
    }
}

pub struct SignAction {
    wallet: Arc<Wallet>,
    chosen_hw: Option<usize>,
    processing: bool,
    hws: Vec<HardwareWallet>,
    error: Option<Error>,
    signed: HashSet<Fingerprint>,
}

impl SignAction {
    pub fn new(signed: HashSet<Fingerprint>, wallet: Arc<Wallet>) -> Self {
        Self {
            wallet,
            chosen_hw: None,
            processing: false,
            hws: Vec::new(),
            error: None,
            signed,
        }
    }
}

impl Action for SignAction {
    fn warning(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    fn load(&self, _daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let wallet = self.wallet.clone();
        Command::perform(
            async move { list_hardware_wallets(&wallet).await },
            Message::ConnectedHardwareWallets,
        )
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
                }) = self.hws.get(i)
                {
                    self.chosen_hw = Some(i);
                    self.processing = true;
                    let psbt = tx.psbt.clone();
                    return Command::perform(
                        sign_psbt(device.clone(), *fingerprint, psbt),
                        Message::Signed,
                    );
                }
            }
            Message::View(view::Message::Spend(view::SpendTxMessage::SelectHotSigner)) => {
                self.processing = true;
                return Command::perform(
                    sign_psbt_with_hot_signer(self.wallet.clone(), tx.psbt.clone()),
                    Message::Signed,
                );
            }
            Message::Signed(res) => match res {
                Err(e) => self.error = Some(e),
                Ok((psbt, fingerprint)) => {
                    self.error = None;
                    self.signed.insert(fingerprint);
                    let daemon = daemon.clone();
                    tx.psbt = psbt.clone();
                    return Command::perform(
                        async move { daemon.update_spend_tx(&psbt).map_err(|e| e.into()) },
                        Message::Updated,
                    );
                }
            },
            Message::Updated(res) => match res {
                Ok(()) => {
                    self.processing = false;
                    tx.sigs = self
                        .wallet
                        .main_descriptor
                        .partial_spend_info(&tx.psbt)
                        .unwrap();
                }
                Err(e) => self.error = Some(e),
            },
            // We add the new hws without dropping the reference of the previous ones.
            Message::ConnectedHardwareWallets(hws) => {
                for h in hws {
                    if !self
                        .hws
                        .iter()
                        .any(|hw| hw.fingerprint() == hw.fingerprint() && hw.kind() == h.kind())
                    {
                        self.hws.push(h);
                    }
                }
            }
            Message::View(view::Message::Reload) => {
                self.hws = Vec::new();
                self.chosen_hw = None;
                self.error = None;
                return self.load(daemon);
            }
            _ => {}
        };
        Command::none()
    }
    fn view(&self) -> Element<view::Message> {
        view::psbt::sign_action(
            self.error.as_ref(),
            &self.hws,
            self.wallet.signer.as_ref().map(|s| s.fingerprint()),
            self.wallet
                .signer
                .as_ref()
                .and_then(|signer| self.wallet.keys_aliases.get(&signer.fingerprint)),
            self.processing,
            self.chosen_hw,
            &self.signed,
        )
    }
}

async fn sign_psbt_with_hot_signer(
    wallet: Arc<Wallet>,
    psbt: Psbt,
) -> Result<(Psbt, Fingerprint), Error> {
    if let Some(signer) = &wallet.signer {
        let psbt = signer.sign_psbt(psbt).map_err(|e| {
            WalletError::HotSigner(format!("Hot signer failed to sign psbt: {}", e))
        })?;
        Ok((psbt, signer.fingerprint()))
    } else {
        Err(WalletError::HotSigner("Hot signer not loaded".to_string()).into())
    }
}

async fn sign_psbt(
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    mut psbt: Psbt,
) -> Result<(Psbt, Fingerprint), Error> {
    hw.sign_tx(&mut psbt).await.map_err(Error::from)?;
    Ok((psbt, fingerprint))
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
    fn view(&self) -> Element<view::Message> {
        if self.success {
            view::psbt::update_spend_success_view()
        } else {
            view::psbt::update_spend_view(
                self.psbt.clone(),
                &self.updated,
                self.error.as_ref(),
                self.processing,
            )
        }
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
