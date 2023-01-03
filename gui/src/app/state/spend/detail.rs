use std::sync::Arc;

use iced::{Command, Element};
use liana::miniscript::bitcoin::util::{bip32::Fingerprint, psbt::Psbt};

use crate::{
    app::{
        cache::Cache, config::Config, error::Error, message::Message, view, view::spend::detail,
    },
    daemon::{
        model::{SpendStatus, SpendTx},
        Daemon,
    },
    hw::{list_hardware_wallets, HardwareWallet},
    ui::component::modal,
};

trait Action {
    fn warning(&self) -> Option<&Error> {
        None
    }
    fn load(&self, _daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        Command::none()
    }
    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        _message: Message,
        _tx: &mut SpendTx,
    ) -> Command<Message> {
        Command::none()
    }
    fn view(&self) -> Element<view::Message>;
}

pub struct SpendTxState {
    config: Config,
    tx: SpendTx,
    saved: bool,
    action: Option<Box<dyn Action>>,
}

impl SpendTxState {
    pub fn new(config: Config, tx: SpendTx, saved: bool) -> Self {
        Self {
            action: None,
            config,
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
        cache: &Cache,
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
                    let action = SignAction::new(self.config.clone());
                    let cmd = action.load(daemon);
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
                        return action.update(daemon.clone(), cache, message, &mut self.tx);
                    }
                }
            },
            Message::Updated(Ok(_)) => {
                self.saved = true;
                if let Some(action) = self.action.as_mut() {
                    return action.update(daemon.clone(), cache, message, &mut self.tx);
                }
            }
            _ => {
                if let Some(action) = self.action.as_mut() {
                    return action.update(daemon.clone(), cache, message, &mut self.tx);
                }
            }
        };
        Command::none()
    }

    pub fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        let content = detail::spend_view(&self.tx, self.saved, cache.network);
        if let Some(action) = &self.action {
            modal::Modal::new(content, action.view())
                .on_blur(view::Message::Spend(view::SpendTxMessage::Cancel))
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
        _cache: &Cache,
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
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
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
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
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
    config: Config,
    chosen_hw: Option<usize>,
    processing: bool,
    hws: Vec<HardwareWallet>,
    error: Option<Error>,
    signed: Vec<Fingerprint>,
}

impl SignAction {
    pub fn new(config: Config) -> Self {
        Self {
            config,
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

    fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let config = self.config.clone();
        let desc = daemon.config().main_descriptor.to_string();
        Command::perform(
            list_hws(config, "Liana".to_string(), desc),
            Message::ConnectedHardwareWallets,
        )
    }
    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
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
                Ok(()) => self.processing = false,
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
                return self.load(daemon);
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

async fn list_hws(config: Config, wallet_name: String, descriptor: String) -> Vec<HardwareWallet> {
    list_hardware_wallets(&config.hardware_wallets, Some((&wallet_name, &descriptor))).await
}

async fn sign_psbt(
    hw: std::sync::Arc<dyn async_hwi::HWI + Send + Sync>,
    fingerprint: Fingerprint,
    mut psbt: Psbt,
) -> Result<(Psbt, Fingerprint), Error> {
    hw.sign_tx(&mut psbt).await.map_err(Error::from)?;
    Ok((psbt, fingerprint))
}
