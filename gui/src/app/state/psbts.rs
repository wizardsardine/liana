use std::str::FromStr;
use std::sync::Arc;

use iced::{Command, Subscription};

use liana::miniscript::bitcoin::psbt::Psbt;
use liana_ui::{
    component::{form, modal},
    widget::Element,
};

use super::{psbt, State};
use crate::{
    app::{cache::Cache, error::Error, menu::Menu, message::Message, view, wallet::Wallet},
    daemon::{model::SpendTx, Daemon},
};

pub struct PsbtsPanel {
    wallet: Arc<Wallet>,
    selected_tx: Option<psbt::PsbtState>,
    spend_txs: Vec<SpendTx>,
    warning: Option<Error>,
    import_tx: Option<ImportPsbtModal>,
}

impl PsbtsPanel {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self {
            wallet,
            spend_txs: Vec::new(),
            warning: None,
            selected_tx: None,
            import_tx: None,
        }
    }

    pub fn preselect(&mut self, spend_tx: SpendTx) {
        let psbt_state = psbt::PsbtState::new(self.wallet.clone(), spend_tx, true);
        self.selected_tx = Some(psbt_state);
        self.warning = None;
        self.import_tx = None;
    }
}

impl State for PsbtsPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(tx) = &self.selected_tx {
            tx.view(cache)
        } else {
            let list_view = view::dashboard(
                &Menu::PSBTs,
                cache,
                self.warning.as_ref(),
                view::psbts::psbts_view(&self.spend_txs),
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
            Message::View(view::Message::Reload) | Message::View(view::Message::Close) => {
                return self.reload(daemon);
            }
            Message::SpendTxs(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(txs) => {
                    self.warning = None;
                    self.spend_txs = txs;
                }
            },
            Message::View(view::Message::ImportSpend(view::ImportSpendMessage::Import)) => {
                if self.import_tx.is_none() {
                    self.import_tx = Some(ImportPsbtModal::new());
                }
            }
            Message::View(view::Message::Select(i)) => {
                if let Some(tx) = self.spend_txs.get(i) {
                    let tx = psbt::PsbtState::new(self.wallet.clone(), tx.clone(), true);
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

    fn subscription(&self) -> Subscription<Message> {
        if let Some(psbt) = &self.selected_tx {
            psbt.subscription()
        } else {
            Subscription::none()
        }
    }

    fn reload(&mut self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        self.selected_tx = None;
        self.import_tx = None;
        let daemon = daemon.clone();
        Command::perform(
            async move { daemon.list_spend_transactions(None).map_err(|e| e.into()) },
            Message::SpendTxs,
        )
    }
}

impl From<PsbtsPanel> for Box<dyn State> {
    fn from(s: PsbtsPanel) -> Box<dyn State> {
        Box::new(s)
    }
}

pub struct ImportPsbtModal {
    imported: form::Value<String>,
    processing: bool,
    error: Option<Error>,
    success: bool,
}

impl ImportPsbtModal {
    pub fn new() -> Self {
        Self {
            imported: form::Value::default(),
            processing: false,
            error: None,
            success: false,
        }
    }
}

impl ImportPsbtModal {
    fn view<'a>(&self) -> Element<'a, view::Message> {
        if self.success {
            view::psbts::import_psbt_success_view()
        } else {
            view::psbts::import_psbt_view(&self.imported, self.error.as_ref(), self.processing)
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
                self.imported.valid = Psbt::from_str(&self.imported.value).ok().is_some();
            }
            Message::View(view::Message::ImportSpend(view::ImportSpendMessage::Confirm)) => {
                if self.imported.valid {
                    self.processing = true;
                    self.error = None;
                    let imported = Psbt::from_str(&self.imported.value).expect("Already checked");
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
