mod step;
use std::sync::Arc;

use iced::{pure::Element, Command};

use super::{redirect, State};
use crate::{
    app::{cache::Cache, error::Error, menu::Menu, message::Message, view},
    daemon::{
        model::{Coin, SpendTx},
        Daemon,
    },
};

pub struct SpendPanel {
    selected_tx: Option<usize>,
    spend_txs: Vec<SpendTx>,
    warning: Option<Error>,
}

impl SpendPanel {
    pub fn new(_coins: &[Coin], spend_txs: &[SpendTx]) -> Self {
        Self {
            spend_txs: spend_txs.to_vec(),
            warning: None,
            selected_tx: None,
        }
    }
}

impl State for SpendPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::dashboard(
            &Menu::Spend,
            cache,
            self.warning.as_ref(),
            view::spend::spend_view(&self.spend_txs),
        )
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
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
            Message::View(view::Message::Select(i)) => {
                self.selected_tx = Some(i);
            }
            _ => {}
        }
        Command::none()
    }

    fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let daemon = daemon.clone();
        Command::perform(
            async move {
                daemon
                    .list_spend_txs()
                    .map(|res| res.spend_txs)
                    .map_err(|e| e.into())
            },
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
    pub fn new(coins: &[Coin]) -> Self {
        Self {
            draft: step::TransactionDraft::default(),
            current: 0,
            steps: vec![
                Box::new(step::ChooseRecipients::default()),
                Box::new(step::ChooseCoins::new(coins.to_vec())),
                Box::new(step::ChooseFeerate::default()),
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

            if self.steps.get(self.current + 1).is_some() {
                self.current += 1;
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
