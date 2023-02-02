use std::sync::Arc;

use iced::{Command, Element};
use liana::{
    descriptors::LianaDescInfo,
    miniscript::bitcoin::{
        consensus,
        util::{bip32::Fingerprint, psbt::Psbt},
    },
};

use crate::{
    app::{
        cache::Cache, error::Error, message::Message, view, view::spend::detail, wallet::Wallet,
    },
    daemon::{
        model::{SpendStatus, SpendTx},
        Daemon,
    },
    hw::{list_hardware_wallets, HardwareWallet},
    ui::component::{form, modal},
};

trait Action {
    fn warning(&self) -> Option<&Error> {
        None
    }
    fn load(&self, _wallet: &Wallet, _daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        Command::none()
    }
    fn update(
        &mut self,
        _wallet: &Wallet,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _message: Message,
        _tx: &mut SpendTx,
    ) -> Command<Message> {
        Command::none()
    }
    fn view(&self) -> Element<view::Message>;
}

pub struct SpendTxState {
    wallet: Arc<Wallet>,
    desc_info: LianaDescInfo,
    tx: SpendTx,
    saved: bool,
    action: Option<Box<dyn Action>>,
}

impl SpendTxState {
    pub fn new(wallet: Arc<Wallet>, tx: SpendTx, saved: bool) -> Self {
        Self {
            desc_info: wallet.main_descriptor.info(),
            wallet,
            action: None,
            tx,
            saved,
        }
    }

    pub fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        if let Some(action) = &self.action {
            action.load(&self.wallet, daemon)
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
                    self.action = Some(Box::new(DeleteAction::default()));
                }
                view::SpendTxMessage::Sign => {
                    let action = SignAction::new();
                    let cmd = action.load(&self.wallet, daemon);
                    self.action = Some(Box::new(action));
                    return cmd;
                }
                view::SpendTxMessage::EditPsbt => {
                    let action = UpdateAction::new(self.tx.psbt.to_string());
                    let cmd = action.load(&self.wallet, daemon);
                    self.action = Some(Box::new(action));
                    return cmd;
                }
                view::SpendTxMessage::Broadcast => {
                    self.action = Some(Box::new(BroadcastAction::default()));
                }
                view::SpendTxMessage::Save => {
                    self.action = Some(Box::new(SaveAction::default()));
                }
                _ => {
                    if let Some(action) = self.action.as_mut() {
                        return action.update(&self.wallet, daemon.clone(), message, &mut self.tx);
                    }
                }
            },
            Message::Updated(Ok(_)) => {
                self.saved = true;
                if let Some(action) = self.action.as_mut() {
                    return action.update(&self.wallet, daemon.clone(), message, &mut self.tx);
                }
            }
            _ => {
                if let Some(action) = self.action.as_mut() {
                    return action.update(&self.wallet, daemon.clone(), message, &mut self.tx);
                }
            }
        };
        Command::none()
    }

    pub fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = detail::spend_view(
            &self.tx,
            self.saved,
            &self.desc_info,
            &self.wallet.keys_aliases,
            cache.network,
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
        _wallet: &Wallet,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::Confirm)) => {
                let daemon = daemon.clone();
                let psbt = tx.psbt.clone();
                return Command::perform(
                    async move { daemon.update_spend_tx(&psbt).map_err(|e| e.into()) },
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
        detail::save_action(self.error.as_ref(), self.saved)
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
        _wallet: &Wallet,
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
        detail::broadcast_action(self.error.as_ref(), self.broadcast)
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
        _wallet: &Wallet,
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
        detail::delete_action(self.error.as_ref(), self.deleted)
    }
}

pub struct SignAction {
    chosen_hw: Option<usize>,
    processing: bool,
    hws: Vec<HardwareWallet>,
    error: Option<Error>,
    signed: Vec<Fingerprint>,
}

impl SignAction {
    pub fn new() -> Self {
        Self {
            chosen_hw: None,
            processing: false,
            hws: Vec::new(),
            error: None,
            signed: Vec::new(),
        }
    }
}

impl Action for SignAction {
    fn warning(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    fn load(&self, wallet: &Wallet, _daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let wallet = wallet.clone();
        Command::perform(list_hws(wallet), Message::ConnectedHardwareWallets)
    }
    fn update(
        &mut self,
        wallet: &Wallet,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
        tx: &mut SpendTx,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::Spend(view::SpendTxMessage::SelectHardwareWallet(i))) => {
                if let Some(hw) = self.hws.get(i) {
                    let device = hw.device.clone();
                    self.chosen_hw = Some(i);
                    self.processing = true;
                    let psbt = tx.psbt.clone();
                    return Command::perform(
                        sign_psbt(device, hw.fingerprint, psbt),
                        Message::Signed,
                    );
                }
            }
            Message::Signed(res) => match res {
                Err(e) => self.error = Some(e),
                Ok((psbt, fingerprint)) => {
                    self.error = None;
                    self.signed.push(fingerprint);
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
                    tx.sigs = wallet.main_descriptor.partial_spend_info(&tx.psbt).unwrap();
                }
                Err(e) => self.error = Some(e),
            },
            // We add the new hws without dropping the reference of the previous ones.
            Message::ConnectedHardwareWallets(hws) => {
                for h in hws {
                    if !self.hws.iter().any(|hw| hw.fingerprint == h.fingerprint) {
                        self.hws.push(h);
                    }
                }
            }
            Message::View(view::Message::Reload) => {
                return self.load(wallet, daemon);
            }
            _ => {}
        };
        Command::none()
    }
    fn view(&self) -> Element<view::Message> {
        view::spend::detail::sign_action(
            self.error.as_ref(),
            &self.hws,
            self.processing,
            self.chosen_hw,
            &self.signed,
        )
    }
}

async fn list_hws(wallet: Wallet) -> Vec<HardwareWallet> {
    list_hardware_wallets(
        &wallet.hardware_wallets,
        Some((&wallet.name, &wallet.main_descriptor.to_string())),
    )
    .await
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
    psbt: String,
    updated: form::Value<String>,
    processing: bool,
    error: Option<Error>,
    success: bool,
}

impl UpdateAction {
    pub fn new(psbt: String) -> Self {
        Self {
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
            view::spend::detail::update_spend_success_view()
        } else {
            view::spend::detail::update_spend_view(
                self.psbt.clone(),
                &self.updated,
                self.error.as_ref(),
                self.processing,
            )
        }
    }

    fn update(
        &mut self,
        wallet: &Wallet,
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
                        let psbt = consensus::encode::deserialize::<Psbt>(
                            &base64::decode(&self.updated.value).unwrap(),
                        )
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
                        tx.sigs = wallet.main_descriptor.partial_spend_info(&tx.psbt).unwrap();
                    }
                    Err(e) => self.error = e.into(),
                }
            }
            Message::View(view::Message::ImportSpend(view::ImportSpendMessage::PsbtEdited(s))) => {
                self.updated.value = s;
                if let Some(psbt) = base64::decode(&self.updated.value)
                    .ok()
                    .and_then(|bytes| consensus::encode::deserialize::<Psbt>(&bytes).ok())
                {
                    self.updated.valid = tx.psbt.unsigned_tx.txid() == psbt.unsigned_tx.txid();
                }
            }
            Message::View(view::Message::ImportSpend(view::ImportSpendMessage::Confirm)) => {
                if self.updated.valid {
                    self.processing = true;
                    self.error = None;
                    let updated: Psbt = consensus::encode::deserialize(
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
