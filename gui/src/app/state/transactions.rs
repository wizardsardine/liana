use std::{
    convert::TryInto,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use iced::Command;
use liana::{miniscript::bitcoin::Txid, spend::MAX_FEERATE};
use liana_ui::{
    component::{form, modal::Modal},
    widget::*,
};

use crate::app::{
    cache::Cache,
    error::Error,
    message::Message,
    state::{label::LabelsEdited, State},
    view,
};

use crate::daemon::{
    model::{HistoryTransaction, Labelled},
    Daemon,
};

#[derive(Default)]
pub struct TransactionsPanel {
    pending_txs: Vec<HistoryTransaction>,
    txs: Vec<HistoryTransaction>,
    labels_edited: LabelsEdited,
    selected_tx: Option<usize>,
    warning: Option<Error>,
    create_rbf_modal: Option<CreateRbfModal>,
}

impl TransactionsPanel {
    pub fn new() -> Self {
        Self {
            selected_tx: None,
            txs: Vec::new(),
            pending_txs: Vec::new(),
            labels_edited: LabelsEdited::default(),
            warning: None,
            create_rbf_modal: None,
        }
    }
}

impl State for TransactionsPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(i) = self.selected_tx {
            let tx = if i < self.pending_txs.len() {
                &self.pending_txs[i]
            } else {
                &self.txs[i - self.pending_txs.len()]
            };
            let content = view::transactions::tx_view(
                cache,
                tx,
                self.labels_edited.cache(),
                self.warning.as_ref(),
            );
            if let Some(modal) = &self.create_rbf_modal {
                Modal::new(content, modal.view())
                    .on_blur(Some(view::Message::CreateRbf(
                        view::CreateRbfMessage::Cancel,
                    )))
                    .into()
            } else {
                content
            }
        } else {
            view::transactions::transactions_view(
                cache,
                &self.pending_txs,
                &self.txs,
                self.warning.as_ref(),
            )
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::HistoryTransactions(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(txs) => {
                    self.warning = None;
                    for tx in txs {
                        if !self.txs.iter().any(|other| other.tx == tx.tx) {
                            self.txs.push(tx);
                        }
                    }
                }
            },
            Message::PendingTransactions(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(txs) => {
                    self.warning = None;
                    for tx in txs {
                        if !self.pending_txs.iter().any(|other| other.tx == tx.tx) {
                            self.pending_txs.push(tx);
                        }
                    }
                }
            },
            Message::View(view::Message::Close) => {
                self.selected_tx = None;
            }
            Message::View(view::Message::Select(i)) => {
                self.selected_tx = Some(i);
            }
            Message::View(view::Message::CreateRbf(view::CreateRbfMessage::Cancel)) => {
                self.create_rbf_modal = None;
            }
            Message::View(view::Message::CreateRbf(view::CreateRbfMessage::New(is_cancel))) => {
                if let Some(idx) = self.selected_tx {
                    if let Some(tx) = self.pending_txs.get(idx) {
                        if let Some(fee_amount) = tx.fee_amount {
                            let prev_feerate_vb = fee_amount
                                .to_sat()
                                .checked_div(tx.tx.vsize().try_into().unwrap())
                                .unwrap();
                            let modal =
                                CreateRbfModal::new(tx.tx.txid(), is_cancel, prev_feerate_vb);
                            self.create_rbf_modal = Some(modal);
                        }
                    }
                }
            }
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    self.pending_txs
                        .iter_mut()
                        .map(|tx| tx as &mut dyn Labelled)
                        .chain(self.txs.iter_mut().map(|tx| tx as &mut dyn Labelled)),
                ) {
                    Ok(cmd) => {
                        return cmd;
                    }
                    Err(e) => {
                        self.warning = Some(e);
                    }
                };
            }
            Message::View(view::Message::Next) => {
                if let Some(last) = self.txs.last() {
                    let daemon = daemon.clone();
                    let last_tx_date = last.time.unwrap();
                    return Command::perform(
                        async move {
                            let mut limit = view::home::HISTORY_EVENT_PAGE_SIZE;
                            let mut txs = daemon.list_history_txs(0_u32, last_tx_date, limit)?;

                            // because gethistory cursor is inclusive and use blocktime
                            // multiple txs can occur in the same block.
                            // If there is more tx in the same block that the
                            // HISTORY_EVENT_PAGE_SIZE they can not be retrieved by changing
                            // the cursor value (blocktime) but by increasing the limit.
                            //
                            // 1. Check if the txs retrieved have all the same blocktime
                            let blocktime = if let Some(tx) = txs.first() {
                                tx.time
                            } else {
                                return Ok(txs);
                            };

                            // 2. Retrieve a larger batch of tx with the same cursor but
                            //    a larger limit.
                            while !txs.iter().any(|evt| evt.time != blocktime)
                                && txs.len() as u64 == limit
                            {
                                // increments of the equivalent of one page more.
                                limit += view::home::HISTORY_EVENT_PAGE_SIZE;
                                txs = daemon.list_history_txs(0, last_tx_date, limit)?;
                            }
                            Ok(txs)
                        },
                        Message::HistoryTransactions,
                    );
                }
            }
            _ => {
                if let Some(modal) = &mut self.create_rbf_modal {
                    return modal.update(daemon, _cache, message);
                }
            }
        };
        Command::none()
    }

    fn load(&self, daemon: Arc<dyn Daemon + Sync + Send>) -> Command<Message> {
        let daemon1 = daemon.clone();
        let daemon2 = daemon.clone();
        let daemon3 = daemon.clone();
        let now: u32 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .try_into()
            .unwrap();
        Command::batch(vec![
            Command::perform(
                async move { daemon3.list_pending_txs().map_err(|e| e.into()) },
                Message::PendingTransactions,
            ),
            Command::perform(
                async move {
                    daemon1
                        .list_history_txs(0, now, view::home::HISTORY_EVENT_PAGE_SIZE)
                        .map_err(|e| e.into())
                },
                Message::HistoryTransactions,
            ),
            Command::perform(
                async move {
                    daemon2
                        .list_coins()
                        .map(|res| res.coins)
                        .map_err(|e| e.into())
                },
                Message::Coins,
            ),
        ])
    }
}

impl From<TransactionsPanel> for Box<dyn State> {
    fn from(s: TransactionsPanel) -> Box<dyn State> {
        Box::new(s)
    }
}

pub struct CreateRbfModal {
    /// Transaction to replace.
    txid: Txid,
    /// Whether to cancel or bump fee.
    is_cancel: bool,
    /// Min feerate required for RBF.
    min_feerate_vb: u64,
    /// Feerate form value.
    feerate_val: form::Value<String>,
    /// Parsed feerate.
    feerate_vb: Option<u64>,
    warning: Option<Error>,
    /// Replacement transaction ID.
    replacement_txid: Option<Txid>,
}

impl CreateRbfModal {
    fn new(txid: Txid, is_cancel: bool, prev_feerate_vb: u64) -> Self {
        let min_feerate_vb = prev_feerate_vb.checked_add(1).unwrap();
        Self {
            txid,
            is_cancel,
            min_feerate_vb,
            feerate_val: form::Value {
                valid: true,
                value: min_feerate_vb.to_string(),
            },
            // For cancel, we let `rbfpsbt` set the feerate.
            feerate_vb: if is_cancel {
                None
            } else {
                Some(min_feerate_vb)
            },
            warning: None,
            replacement_txid: None,
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Command<Message> {
        match message {
            Message::View(view::Message::CreateRbf(view::CreateRbfMessage::FeerateEdited(s))) => {
                self.warning = None;
                if let Ok(value) = s.parse::<u64>() {
                    self.feerate_val.value = s;
                    self.feerate_val.valid = value >= self.min_feerate_vb && value <= MAX_FEERATE;
                    if self.feerate_val.valid {
                        self.feerate_vb = Some(value);
                    }
                } else {
                    self.feerate_val.valid = false;
                }
                if !self.feerate_val.valid {
                    self.feerate_vb = None;
                }
            }
            Message::View(view::Message::CreateRbf(view::CreateRbfMessage::Confirm)) => {
                self.warning = None;

                let psbt = match daemon.rbf_psbt(&self.txid, self.is_cancel, self.feerate_vb) {
                    Ok(res) => res.psbt,
                    Err(e) => {
                        self.warning = Some(e.into());
                        return Command::none();
                    }
                };
                if let Err(e) = daemon.update_spend_tx(&psbt) {
                    self.warning = Some(e.into());
                    return Command::none();
                }
                self.replacement_txid = Some(psbt.unsigned_tx.txid());
            }
            _ => {}
        }
        Command::none()
    }
    fn view(&self) -> Element<view::Message> {
        view::transactions::create_rbf_modal(
            self.is_cancel,
            &self.feerate_val,
            self.replacement_txid,
            self.warning.as_ref(),
        )
    }
}
