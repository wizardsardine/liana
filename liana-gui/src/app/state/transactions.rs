use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use iced::Task;
use liana::{
    miniscript::bitcoin::{OutPoint, Txid},
    spend::{SpendCreationError, MAX_FEERATE},
};
use liana_ui::{
    component::{form, modal::Modal},
    widget::*,
};
use lianad::commands::CoinStatus;

pub const HISTORY_EVENT_PAGE_SIZE: u64 = 20;

use crate::{
    app::{
        cache::Cache,
        error::Error,
        message::Message,
        state::{label::LabelsEdited, State},
        view,
        wallet::Wallet,
    },
    daemon::model::{self, LabelsLoader},
    export::{ExportMessage, ExportType},
};

use crate::daemon::{
    model::{CreateSpendResult, HistoryTransaction, LabelItem, Labelled},
    Daemon,
};

use super::export::ExportModal;

#[derive(Debug)]
pub enum TransactionsModal {
    CreateRbf(CreateRbfModal),
    Export(ExportModal),
    None,
}

pub struct TransactionsPanel {
    wallet: Arc<Wallet>,
    txs: Vec<HistoryTransaction>,
    labels_edited: LabelsEdited,
    selected_tx: Option<HistoryTransaction>,
    warning: Option<Error>,
    modal: TransactionsModal,
    is_last_page: bool,
    processing: bool,
}

impl TransactionsPanel {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self {
            wallet,
            selected_tx: None,
            txs: Vec::new(),
            labels_edited: LabelsEdited::default(),
            warning: None,
            modal: TransactionsModal::None,
            is_last_page: false,
            processing: false,
        }
    }

    pub fn preselect(&mut self, tx: HistoryTransaction) {
        self.selected_tx = Some(tx);
        self.warning = None;
        self.modal = TransactionsModal::None;
    }
}

impl State for TransactionsPanel {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(tx) = self.selected_tx.as_ref() {
            let content = view::transactions::tx_view(
                cache,
                tx,
                self.labels_edited.cache(),
                self.warning.as_ref(),
            );
            match &self.modal {
                TransactionsModal::CreateRbf(rbf) => rbf.view(content),
                _ => content,
            }
        } else {
            let content = view::transactions::transactions_view(
                cache,
                &self.txs,
                self.warning.as_ref(),
                self.is_last_page,
                self.processing,
            );
            match &self.modal {
                TransactionsModal::Export(export) => export.view(content),
                _ => content,
            }
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::HistoryTransactions(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(txs) => {
                    self.warning = None;
                    self.txs = txs;
                    self.is_last_page = (self.txs.len() as u64) < HISTORY_EVENT_PAGE_SIZE;
                }
            },
            Message::HistoryTransactionsExtension(res) => match res {
                Err(e) => self.warning = Some(e),
                Ok(txs) => {
                    self.processing = false;
                    self.warning = None;
                    self.is_last_page = (txs.len() as u64) < HISTORY_EVENT_PAGE_SIZE;
                    if let Some(tx) = txs.first() {
                        if let Some(position) = self.txs.iter().position(|tx2| tx2.txid == tx.txid)
                        {
                            let len = self.txs.len();
                            for tx in txs {
                                if !self.txs[position..len]
                                    .iter()
                                    .any(|tx2| tx2.txid == tx.txid)
                                {
                                    self.txs.push(tx);
                                }
                            }
                        } else {
                            self.txs.extend(txs);
                        }
                    }
                }
            },
            Message::RbfModal(tx, is_cancel, res) => match res {
                Ok(descendant_txids) => {
                    let modal = CreateRbfModal::new(*tx, is_cancel, descendant_txids);
                    self.modal = TransactionsModal::CreateRbf(modal);
                }
                Err(e) => {
                    self.warning = e.into();
                }
            },
            Message::View(view::Message::Reload) | Message::View(view::Message::Close) => {
                return self.reload(daemon, self.wallet.clone());
            }
            Message::View(view::Message::Select(i)) => {
                self.selected_tx = self.txs.get(i).cloned();
                // Clear modal if it's for a different tx.
                if let TransactionsModal::CreateRbf(modal) = &self.modal {
                    if Some(modal.tx.tx.compute_txid())
                        != self
                            .selected_tx
                            .as_ref()
                            .map(|selected| selected.tx.compute_txid())
                    {
                        self.modal = TransactionsModal::None;
                    }
                }
            }
            Message::View(view::Message::CreateRbf(view::CreateRbfMessage::Cancel)) => {
                self.modal = TransactionsModal::None;
            }
            Message::View(view::Message::CreateRbf(view::CreateRbfMessage::New(is_cancel))) => {
                if let Some(tx) = &self.selected_tx {
                    if tx.fee_amount.is_some() {
                        let tx = tx.clone();
                        let outpoints: Vec<_> = (0..tx.tx.output.len())
                            .map(|vout| {
                                OutPoint::new(
                                    tx.tx.compute_txid(),
                                    vout.try_into()
                                        .expect("number of transaction outputs must fit in u32"),
                                )
                            })
                            .collect();
                        return Task::perform(
                            async move {
                                let res = daemon
                                    .list_coins(&[CoinStatus::Spending], &outpoints)
                                    .await
                                    .map(|res| {
                                        res.coins
                                            .iter()
                                            .filter_map(|c| c.spend_info.map(|info| info.txid))
                                            .collect()
                                    })
                                    .map_err(|e| e.into());
                                (Box::new(tx), is_cancel, res)
                            },
                            |(tx, is_cancel, res)| Message::RbfModal(tx, is_cancel, res),
                        );
                    }
                }
            }
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                match self.labels_edited.update(
                    daemon,
                    message,
                    self.txs
                        .iter_mut()
                        .map(|tx| tx as &mut dyn LabelsLoader)
                        .chain(
                            self.selected_tx
                                .iter_mut()
                                .map(|tx| tx as &mut dyn LabelsLoader),
                        ),
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
                    self.processing = true;
                    return Task::perform(
                        async move {
                            let mut limit = HISTORY_EVENT_PAGE_SIZE;
                            let mut txs =
                                daemon.list_history_txs(0_u32, last_tx_date, limit).await?;

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
                                limit += HISTORY_EVENT_PAGE_SIZE;
                                txs = daemon.list_history_txs(0, last_tx_date, limit).await?;
                            }
                            txs.sort_by(|a, b| a.compare(b));
                            Ok(txs)
                        },
                        Message::HistoryTransactionsExtension,
                    );
                }
            }
            Message::View(view::Message::Export(ExportMessage::Open)) => {
                if let TransactionsModal::None = &self.modal {
                    self.modal = TransactionsModal::Export(ExportModal::new(
                        daemon,
                        ExportType::Transactions,
                    ));
                    if let TransactionsModal::Export(m) = &self.modal {
                        return m.launch();
                    }
                }
            }
            Message::View(view::Message::Export(ExportMessage::Close)) => {
                if let TransactionsModal::Export(_) = &self.modal {
                    self.modal = TransactionsModal::None;
                }
            }
            ref msg => {
                return match &mut self.modal {
                    TransactionsModal::CreateRbf(modal) => modal.update(daemon, _cache, message),
                    TransactionsModal::Export(modal) => {
                        if let Message::View(view::Message::Export(m)) = msg {
                            modal.update(m.clone())
                        } else {
                            Task::none()
                        }
                    }
                    TransactionsModal::None => Task::none(),
                };
            }
        };
        Task::none()
    }

    fn reload(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.selected_tx = None;
        let now: u32 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .try_into()
            .unwrap();
        Task::batch(vec![Task::perform(
            async move {
                let mut txs = daemon
                    .list_history_txs(0, now, HISTORY_EVENT_PAGE_SIZE)
                    .await?;
                txs.sort_by(|a, b| a.compare(b));

                let mut pending_txs = daemon.list_pending_txs().await?;
                pending_txs.extend(txs);
                Ok(pending_txs)
            },
            Message::HistoryTransactions,
        )])
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        if let TransactionsModal::Export(modal) = &self.modal {
            if let Some(sub) = modal.subscription() {
                return sub.map(|m| {
                    Message::View(view::Message::Export(ExportMessage::ExportProgress(m)))
                });
            }
        }
        iced::Subscription::none()
    }
}

impl From<TransactionsPanel> for Box<dyn State> {
    fn from(s: TransactionsPanel) -> Box<dyn State> {
        Box::new(s)
    }
}

#[derive(Debug)]
pub struct CreateRbfModal {
    /// Transaction to replace.
    tx: model::HistoryTransaction,
    /// Whether to cancel or bump fee.
    is_cancel: bool,
    /// Min feerate required for RBF.
    min_feerate_vb: u64,
    /// IDs of any transactions from this wallet that are direct descendants of
    /// the transaction to be replaced.
    descendant_txids: HashSet<Txid>,
    /// Feerate form value.
    feerate_val: form::Value<String>,
    /// Parsed feerate.
    feerate_vb: Option<u64>,
    /// Replacement transaction ID.
    replacement_txid: Option<Txid>,

    processing: bool,
    warning: Option<Error>,
}

impl CreateRbfModal {
    fn new(
        tx: model::HistoryTransaction,
        is_cancel: bool,
        descendant_txids: HashSet<Txid>,
    ) -> Self {
        let prev_feerate_vb = tx
            .fee_amount
            .expect("rbf should only be used on a transaction with fee amount set")
            .to_sat()
            .checked_div(tx.tx.vsize().try_into().expect("vsize must fit in u64"))
            .expect("transaction vsize must be positive");
        let min_feerate_vb = prev_feerate_vb.checked_add(1).unwrap();
        Self {
            tx,
            is_cancel,
            min_feerate_vb,
            descendant_txids,
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
            replacement_txid: None,
            warning: None,
            processing: false,
        }
    }

    fn update(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(view::Message::CreateRbf(view::CreateRbfMessage::FeerateEdited(s))) => {
                self.warning = None;
                if let Ok(value) = s.parse::<u64>() {
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
                self.feerate_val.value = s; // save form value even if it cannot be parsed
            }
            Message::RbfPsbt(res) => {
                self.processing = false;
                match res {
                    Ok(txid) => {
                        self.replacement_txid = Some(txid);
                    }
                    Err(e) => self.warning = Some(e),
                }
            }
            Message::View(view::Message::CreateRbf(view::CreateRbfMessage::Confirm)) => {
                self.warning = None;
                self.processing = true;
                return Task::perform(
                    rbf(daemon, self.tx.clone(), self.is_cancel, self.feerate_vb),
                    Message::RbfPsbt,
                );
            }
            _ => {}
        }
        Task::none()
    }
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<view::Message> {
        let modal = Modal::new(
            content,
            view::transactions::create_rbf_modal(
                self.is_cancel,
                &self.descendant_txids,
                &self.feerate_val,
                self.replacement_txid,
                self.warning.as_ref(),
            ),
        );
        if self.processing {
            modal
        } else {
            modal.on_blur(Some(view::Message::CreateRbf(
                view::CreateRbfMessage::Cancel,
            )))
        }
        .into()
    }
}

async fn rbf(
    daemon: Arc<dyn Daemon + Sync + Send>,
    previous_tx: model::HistoryTransaction,
    is_cancel: bool,
    feerate_vb: Option<u64>,
) -> Result<Txid, Error> {
    let previous_txid = previous_tx.tx.compute_txid();
    let psbt = match daemon
        .rbf_psbt(&previous_txid, is_cancel, feerate_vb)
        .await?
    {
        CreateSpendResult::Success { psbt, .. } => psbt,
        CreateSpendResult::InsufficientFunds { missing } => {
            return Err(
                SpendCreationError::CoinSelection(liana::spend::InsufficientFunds { missing })
                    .into(),
            );
        }
    };

    if !is_cancel {
        let mut labels = HashMap::<LabelItem, Option<String>>::new();
        let new_txid = psbt.unsigned_tx.compute_txid();
        for item in previous_tx.labelled() {
            if let Some(label) = previous_tx.labels.get(&item.to_string()) {
                match item {
                    LabelItem::Txid(_) => {
                        labels.insert(new_txid.into(), Some(label.to_string()));
                    }
                    LabelItem::OutPoint(o) => {
                        if let Some(previous_output) = previous_tx.tx.output.get(o.vout as usize) {
                            for (vout, output) in psbt.unsigned_tx.output.iter().enumerate() {
                                if output.script_pubkey == previous_output.script_pubkey {
                                    labels.insert(
                                        LabelItem::OutPoint(OutPoint {
                                            txid: new_txid,
                                            vout: vout as u32,
                                        }),
                                        Some(label.to_string()),
                                    );
                                }
                            }
                        }
                    }
                    // Address label is already in database
                    LabelItem::Address(_) => {}
                }
            }
        }

        daemon.update_labels(&labels).await?;
    }

    daemon.update_spend_tx(&psbt).await?;
    Ok(psbt.unsigned_tx.compute_txid())
}
