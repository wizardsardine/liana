use std::sync::Arc;

use iced::pure::Element;
use iced::Command;
use minisafe::miniscript::bitcoin::util::{bip32::Fingerprint, psbt::Psbt};

use crate::{
    app::{
        cache::Cache, config::Config, error::Error, message::Message, view, view::spend::detail,
    },
    daemon::{
        model::{SpendStatus, SpendTx},
        Daemon,
    },
    hw::{list_hardware_wallets, HardwareWallet},
};

trait Action {
    fn warning(&self) -> Option<&Error> {
        None
    }
    fn updated(&self) -> bool {
        false
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
    action: Box<dyn Action>,
}

impl SpendTxState {
    pub fn new(config: Config, tx: SpendTx, saved: bool) -> Self {
        Self {
            action: choose_action(&config, saved, &tx),
            config,
            tx,
            saved,
        }
    }

    pub fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        self.action.load(daemon)
    }

    pub fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        let cmd = match &message {
            Message::View(view::Message::Spend(msg)) => match msg {
                view::SpendTxMessage::Cancel => {
                    self.action = choose_action(&self.config, self.saved, &self.tx);
                    self.action.load(daemon.clone())
                }
                view::SpendTxMessage::Delete => {
                    self.action = Box::new(DeleteAction::default());
                    self.action.load(daemon.clone())
                }
                _ => self
                    .action
                    .update(daemon.clone(), cache, message, &mut self.tx),
            },
            _ => self
                .action
                .update(daemon.clone(), cache, message, &mut self.tx),
        };
        if self.action.updated() {
            self.saved = true;
            self.action = choose_action(&self.config, self.saved, &self.tx);
            self.action.load(daemon)
        } else {
            cmd
        }
    }

    pub fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        detail::spend_view(
            self.action.warning(),
            &self.tx,
            self.action.view(),
            self.saved,
            cache.network,
        )
    }
}

fn choose_action(config: &Config, saved: bool, tx: &SpendTx) -> Box<dyn Action> {
    if saved {
        match tx.status {
            SpendStatus::Deprecated | SpendStatus::Broadcasted => {
                return Box::new(NoAction::default());
            }
            _ => {}
        }

        if !tx.psbt.inputs.first().unwrap().partial_sigs.is_empty() {
            return Box::new(BroadcastAction::default());
        } else {
            return Box::new(SignAction::new(config.clone()));
        }
    }
    Box::new(SaveAction::default())
}

#[derive(Default)]
pub struct SaveAction {
    saved: bool,
    error: Option<Error>,
}

impl Action for SaveAction {
    fn warning(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    fn updated(&self) -> bool {
        self.saved
    }

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
        detail::save_action(self.saved)
    }
}

#[derive(Default)]
pub struct BroadcastAction {
    broadcasted: bool,
    error: Option<Error>,
}

impl Action for BroadcastAction {
    fn warning(&self) -> Option<&Error> {
        self.error.as_ref()
    }
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
                Ok(()) => self.broadcasted = true,
                Err(e) => self.error = Some(e),
            },
            _ => {}
        }
        Command::none()
    }
    fn view(&self) -> Element<view::Message> {
        detail::broadcast_action(self.broadcasted)
    }
}

#[derive(Default)]
pub struct DeleteAction {
    deleted: bool,
    error: Option<Error>,
}

impl Action for DeleteAction {
    fn warning(&self) -> Option<&Error> {
        self.error.as_ref()
    }

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
        detail::delete_action(self.deleted)
    }
}

pub struct SignAction {
    config: Config,
    chosen_hw: Option<usize>,
    processing: bool,
    hws: Vec<HardwareWallet>,
    error: Option<Error>,
    signed: Vec<Fingerprint>,
    updated: bool,
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
            updated: false,
        }
    }
}

impl Action for SignAction {
    fn warning(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    fn updated(&self) -> bool {
        self.updated
    }

    fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let config = self.config.clone();
        let desc = daemon.config().main_descriptor.to_string();
        Command::perform(
            list_hws(config, "Minisafe".to_string(), desc),
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
                Ok(()) => self.updated = true,
                Err(e) => self.error = Some(e),
            },
            Message::ConnectedHardwareWallets(hws) => {
                self.hws = hws;
            }
            Message::View(view::Message::Reload) => {
                return self.load(daemon);
            }
            _ => {}
        };
        Command::none()
    }
    fn view(&self) -> Element<view::Message> {
        view::spend::detail::sign_action(&self.hws, self.processing, self.chosen_hw, &self.signed)
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

#[derive(Default)]
pub struct NoAction {}

impl Action for NoAction {
    fn view(&self) -> Element<view::Message> {
        iced::pure::column().into()
    }
}
