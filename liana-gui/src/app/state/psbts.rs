use std::sync::Arc;

use iced::{Subscription, Task};

use liana_ui::widget::Element;

use super::{export::ExportModal, psbt, State};
use crate::{
    app::{cache::Cache, error::Error, menu::Menu, message::Message, view, wallet::Wallet},
    daemon::{model::SpendTx, Daemon},
    export::{ImportExportMessage, ImportExportType},
};

pub struct PsbtsPanel {
    wallet: Arc<Wallet>,
    selected_tx: Option<psbt::PsbtState>,
    spend_txs: Vec<SpendTx>,
    warning: Option<Error>,
    modal: Option<ExportModal>,
}

impl PsbtsPanel {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self {
            wallet,
            spend_txs: Vec::new(),
            warning: None,
            selected_tx: None,
            modal: None,
        }
    }

    pub fn preselect(&mut self, spend_tx: SpendTx) {
        let psbt_state = psbt::PsbtState::new(self.wallet.clone(), spend_tx, true, None);
        self.selected_tx = Some(psbt_state);
        self.warning = None;
        self.modal = None;
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
            if let Some(modal) = &self.modal {
                modal.view(list_view)
            } else {
                list_view
            }
        }
    }

    fn interrupt(&mut self) {
        self.selected_tx = None;
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::Reload) | Message::View(view::Message::Close) => {
                return self.reload(daemon, self.wallet.clone());
            }
            Message::SpendTxs(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(txs) => {
                    self.warning = None;
                    self.spend_txs = txs;
                    if let Some(tx) = &self.selected_tx {
                        if let Some(tx) = self.spend_txs.iter().find(|spend_tx| {
                            spend_tx.psbt.unsigned_tx.compute_txid()
                                == tx.tx.psbt.unsigned_tx.compute_txid()
                        }) {
                            let tx =
                                psbt::PsbtState::new(self.wallet.clone(), tx.clone(), true, None);
                            let cmd = tx.load(daemon);
                            self.selected_tx = Some(tx);
                            return cmd;
                        }
                    }
                }
            },
            Message::View(view::Message::ImportPsbt) => {
                if let Some(tx) = &mut self.selected_tx {
                    return tx.update(daemon, cache, message);
                } else if self.modal.is_none() {
                    let modal =
                        ExportModal::new(Some(daemon.clone()), ImportExportType::ImportPsbt(None));
                    let launch = modal.launch(false);
                    self.modal = Some(modal);
                    return launch;
                }
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Close)) => {
                if let Some(tx) = &mut self.selected_tx {
                    return tx.update(daemon, cache, message);
                } else if self.modal.is_some() {
                    self.modal = None;
                    return Task::perform(async {}, |_| Message::View(view::Message::Reload));
                }
            }
            Message::View(view::Message::ImportExport(m)) => {
                let m = m.clone();
                if let Some(tx) = &mut self.selected_tx {
                    let message = Message::View(view::Message::ImportExport(m));
                    return tx.update(daemon, cache, message);
                } else if let Some(modal) = self.modal.as_mut() {
                    return modal.update(m.clone());
                }
            }
            Message::View(view::Message::Select(i)) => {
                if let Some(tx) = self.spend_txs.get(i) {
                    let tx = psbt::PsbtState::new(self.wallet.clone(), tx.clone(), true, None);
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
        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        if let Some(psbt) = &self.selected_tx {
            psbt.subscription()
        } else if let Some(modal) = &self.modal {
            modal
                .subscription()
                .map(|s| {
                    s.map(|m| {
                        Message::View(view::Message::ImportExport(ImportExportMessage::Progress(
                            m,
                        )))
                    })
                })
                .unwrap_or(Subscription::none())
        } else {
            Subscription::none()
        }
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.wallet = wallet;
        self.selected_tx = None;
        self.modal = None;
        let daemon = daemon.clone();
        Task::perform(
            async move {
                daemon
                    .list_spend_transactions(None)
                    .await
                    .map_err(|e| e.into())
            },
            Message::SpendTxs,
        )
    }
}

impl From<PsbtsPanel> for Box<dyn State> {
    fn from(s: PsbtsPanel) -> Box<dyn State> {
        Box::new(s)
    }
}
