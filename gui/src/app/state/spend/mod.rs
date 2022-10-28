mod detail;
mod step;
use std::sync::Arc;

use iced::{pure::Element, Command};

use super::{redirect, State};
use crate::{
    app::{cache::Cache, config::Config, error::Error, menu::Menu, message::Message, view},
    daemon::{
        model::{Coin, SpendTx},
        Daemon,
    },
};

pub struct SpendPanel {
    config: Config,
    selected_tx: Option<detail::SpendTxState>,
    spend_txs: Vec<SpendTx>,
    warning: Option<Error>,
}

impl SpendPanel {
    pub fn new(config: Config, spend_txs: &[SpendTx]) -> Self {
        Self {
            config,
            spend_txs: spend_txs.to_vec(),
            warning: None,
            selected_tx: None,
        }
    }
}

impl State for SpendPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(tx) = &self.selected_tx {
            tx.view(cache)
        } else {
            view::dashboard(
                &Menu::Spend,
                cache,
                self.warning.as_ref(),
                view::spend::spend_view(&self.spend_txs),
            )
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
            Message::View(view::Message::Close) => {
                if self.selected_tx.is_some() {
                    self.selected_tx = None;
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
    pub fn new(config: Config, coins: &[Coin]) -> Self {
        Self {
            draft: step::TransactionDraft::default(),
            current: 0,
            steps: vec![
                Box::new(step::ChooseRecipients::default()),
                Box::new(step::ChooseCoins::new(coins.to_vec())),
                Box::new(step::ChooseFeerate::default()),
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

        if matches!(message, Message::View(view::Message::Previous)) {
            if self.steps.get(self.current - 1).is_some() {
                self.current -= 1;
            }
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
