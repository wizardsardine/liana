mod detail;
mod step;
use std::sync::Arc;

use iced::{Command, Element};

use liana::{
    descriptors::MultipathDescriptor,
    miniscript::bitcoin::{consensus, util::psbt::Psbt},
};

use super::{redirect, State};
use crate::{
    app::{cache::Cache, config::Config, error::Error, menu::Menu, message::Message, view},
    daemon::{
        model::{Coin, SpendTx},
        Daemon,
    },
    ui::component::{form, modal},
};

pub struct SpendPanel {
    config: Config,
    selected_tx: Option<detail::SpendTxState>,
    spend_txs: Vec<SpendTx>,
    warning: Option<Error>,
    import_tx: Option<ImportSpendState>,
}

impl SpendPanel {
    pub fn new(config: Config, spend_txs: &[SpendTx]) -> Self {
        Self {
            config,
            spend_txs: spend_txs.to_vec(),
            warning: None,
            selected_tx: None,
            import_tx: None,
        }
    }
}

impl State for SpendPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(tx) = &self.selected_tx {
            tx.view(cache)
        } else {
            let list_view = view::dashboard(
                &Menu::Spend,
                cache,
                self.warning.as_ref(),
                view::spend::spend_view(&self.spend_txs),
            );
            if let Some(import_tx) = &self.import_tx {
                modal::Modal::new(list_view, import_tx.view())
                    .on_blur(if import_tx.processing {
                        None
                    } else {
                        Some(view::Message::Close)
                    })
                    .into()
            } else {
                list_view
            }
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::SpendTxs(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(txs) => {
                    self.warning = None;
                    self.spend_txs = txs;
                }
            },
            Message::View(view::Message::ImportSpend(view::ImportSpendMessage::Import)) => {
                if self.import_tx.is_none() {
                    self.import_tx = Some(ImportSpendState::new());
                }
            }
            Message::View(view::Message::Close) => {
                if self.selected_tx.is_some() {
                    self.selected_tx = None;
                    return self.load(daemon);
                }
                if self.import_tx.is_some() {
                    self.import_tx = None;
                    return self.load(daemon);
                }
            }
            Message::View(view::Message::Select(i)) => {
                if let Some(tx) = self.spend_txs.get(i) {
                    let tx = detail::SpendTxState::new(self.config.clone(), tx.clone(), true);
                    let cmd = tx.load(daemon);
                    self.selected_tx = Some(tx);
                    return cmd;
                }
            }
            _ => {
                if let Some(tx) = &mut self.selected_tx {
                    return tx.update(daemon, cache, message);
                }

                if let Some(import_tx) = &mut self.import_tx {
                    return import_tx.update(daemon, cache, message);
                }
            }
        }
        Command::none()
    }

    fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let daemon = daemon.clone();
        Command::perform(
            async move { daemon.list_spend_transactions().map_err(|e| e.into()) },
            Message::SpendTxs,
        )
    }
}

impl From<SpendPanel> for Box<dyn State> {
    fn from(s: SpendPanel) -> Box<dyn State> {
        Box::new(s)
    }
}

pub struct CreateSpendPanel {
    draft: step::TransactionDraft,
    current: usize,
    steps: Vec<Box<dyn step::Step>>,
}

impl CreateSpendPanel {
    pub fn new(
        config: Config,
        descriptor: MultipathDescriptor,
        coins: &[Coin],
        timelock: u32,
        blockheight: u32,
    ) -> Self {
        Self {
            draft: step::TransactionDraft::default(),
            current: 0,
            steps: vec![
                Box::new(step::ChooseRecipients::new(coins)),
                Box::new(step::ChooseCoins::new(
                    descriptor,
                    coins.to_vec(),
                    timelock,
                    blockheight,
                )),
                Box::new(step::SaveSpend::new(config)),
            ],
        }
    }
}

impl State for CreateSpendPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        self.steps.get(self.current).unwrap().view(cache)
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        if matches!(message, Message::View(view::Message::Close)) {
            return redirect(Menu::Spend);
        }

        if matches!(message, Message::View(view::Message::Next)) {
            if let Some(step) = self.steps.get(self.current) {
                step.apply(&mut self.draft);
            }

            if let Some(step) = self.steps.get_mut(self.current + 1) {
                self.current += 1;
                step.load(&self.draft);
            }
        }

        if matches!(message, Message::View(view::Message::Previous))
            && self.steps.get(self.current - 1).is_some()
        {
            self.current -= 1;
        }

        if let Some(step) = self.steps.get_mut(self.current) {
            return step.update(daemon, cache, &self.draft, message);
        }

        Command::none()
    }

    fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let daemon = daemon.clone();
        Command::perform(
            async move {
                daemon
                    .list_coins()
                    .map(|res| res.coins)
                    .map_err(|e| e.into())
            },
            Message::Coins,
        )
    }
}

impl From<CreateSpendPanel> for Box<dyn State> {
    fn from(s: CreateSpendPanel) -> Box<dyn State> {
        Box::new(s)
    }
}

pub struct ImportSpendState {
    imported: form::Value<String>,
    processing: bool,
    error: Option<Error>,
    success: bool,
}

impl ImportSpendState {
    pub fn new() -> Self {
        Self {
            imported: form::Value::default(),
            processing: false,
            error: None,
            success: false,
        }
    }
}

impl ImportSpendState {
    fn view<'a>(&self) -> Element<'a, view::Message> {
        if self.success {
            view::spend::import_spend_success_view()
        } else {
            view::spend::import_spend_view(&self.imported, self.error.as_ref(), self.processing)
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::Updated(res) => {
                self.processing = false;
                match res {
                    Ok(()) => {
                        self.success = true;
                        self.error = None;
                    }
                    Err(e) => self.error = e.into(),
                }
            }
            Message::View(view::Message::ImportSpend(view::ImportSpendMessage::PsbtEdited(s))) => {
                self.imported.value = s;
                self.imported.valid = base64::decode(&self.imported.value)
                    .ok()
                    .and_then(|bytes| consensus::encode::deserialize::<Psbt>(&bytes).ok())
                    .is_some();
            }
            Message::View(view::Message::ImportSpend(view::ImportSpendMessage::Confirm)) => {
                if self.imported.valid {
                    self.processing = true;
                    self.error = None;
                    let imported: Psbt = consensus::encode::deserialize(
                        &base64::decode(&self.imported.value).expect("Already checked"),
                    )
                    .unwrap();
                    return Command::perform(
                        async move { daemon.update_spend_tx(&imported).map_err(|e| e.into()) },
                        Message::Updated,
                    );
                }
            }
            _ => {}
        }

        Command::none()
    }
}
