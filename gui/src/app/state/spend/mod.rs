pub mod detail;
mod step;
use std::sync::Arc;

use iced::Command;

use liana_ui::widget::Element;

use super::{redirect, State};
use crate::{
    app::{cache::Cache, menu::Menu, message::Message, view, wallet::Wallet},
    daemon::{model::Coin, Daemon},
};

pub struct CreateSpendPanel {
    draft: step::TransactionDraft,
    current: usize,
    steps: Vec<Box<dyn step::Step>>,
}

impl CreateSpendPanel {
    pub fn new(wallet: Arc<Wallet>, coins: &[Coin], blockheight: u32) -> Self {
        let descriptor = wallet.main_descriptor.clone();
        let timelock = descriptor.first_timelock_value();
        Self {
            draft: step::TransactionDraft::default(),
            current: 0,
            steps: vec![
                Box::new(step::DefineSpend::new(
                    descriptor,
                    coins.to_vec(),
                    timelock,
                    blockheight,
                )),
                Box::new(step::SaveSpend::new(wallet)),
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
            return redirect(Menu::PSBTs);
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
            return step.update(daemon, cache, message);
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
