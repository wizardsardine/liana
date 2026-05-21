use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
    sync::Arc,
};

use coincube_core::{
    miniscript::bitcoin::{OutPoint, Txid},
    spend::{SpendCreationError, MAX_FEERATE},
};
use coincube_ui::{
    component::form,
    widget::{modal::Modal, Element},
};
use coincubed::commands::CoinStatus;
use iced::Task;

/// Bump to a larger value (e.g. 50) once Prev/Next pagination is verified
/// end-to-end on real wallets. Kept low during rollout so QA can exercise
/// pagination without needing 50+ transactions in a single wallet. Matches
/// the PAGE_SIZE used by the Spark and Liquid Transactions panels.
pub const HISTORY_EVENT_PAGE_SIZE: u64 = 10;

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        message::Message,
        state::{vault::label::LabelsEdited, State},
        view,
        wallet::Wallet,
    },
    daemon::model::{self, LabelsLoader},
    export::{ImportExportMessage, ImportExportType},
    utils::now,
};

use crate::daemon::{
    model::{CreateSpendResult, HistoryTransaction, LabelItem, Labelled},
    Daemon,
};

use super::export::VaultExportModal;

#[derive(Debug)]
pub enum VaultTransactionsModal {
    CreateRbf(CreateRbfModal),
    Export(VaultExportModal),
    None,
}

pub struct VaultTransactionsPanel {
    wallet: Arc<Wallet>,
    /// Cached pages keyed by page index. `page_cache[i]` holds the
    /// transactions for page `i` (Prev navigation re-reads from cache so
    /// there's no need to invert the daemon's blocktime cursor). Cleared
    /// on `reload`.
    page_cache: Vec<Vec<HistoryTransaction>>,
    /// Pending (unconfirmed) txs from `list_pending_txs()`, fetched at
    /// reload time and only shown on page 0.
    pending_txs: Vec<HistoryTransaction>,
    /// Materialised view of the current page (pending + cache[0] on page
    /// 0, just cache[N] otherwise). Recomputed via `refresh_displayed`
    /// whenever the displayed page or its underlying data changes, so the
    /// view function can borrow it directly with `&self`'s lifetime.
    displayed_txs: Vec<HistoryTransaction>,
    labels_edited: LabelsEdited,
    selected_tx: Option<HistoryTransaction>,
    warning: Option<Error>,
    modal: VaultTransactionsModal,
    /// Index of the final page, once discovered. `None` means we don't yet
    /// know where history ends. Stored as an index (rather than a per-page
    /// bool derived from row count) because the daemon's blocktime cursor
    /// is inclusive: a fetch can return a full page from the server yet,
    /// after stripping the rows that overlap the previous page, leave a
    /// short Vec — so row count is not a reliable end-of-history signal.
    last_page_index: Option<u32>,
    processing: bool,
    current_page: u32,
    /// Target page of an in-flight `VaultNextPage` extension fetch. Set on
    /// dispatch, cleared on the result *or* by `reload`. The extension
    /// handler ignores any response that arrives once this is `None`, so a
    /// reload mid-fetch can't let a stale page advance `current_page`.
    pending_page: Option<u32>,
}

impl VaultTransactionsPanel {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self {
            wallet,
            selected_tx: None,
            page_cache: Vec::new(),
            pending_txs: Vec::new(),
            displayed_txs: Vec::new(),
            labels_edited: LabelsEdited::default(),
            warning: None,
            modal: VaultTransactionsModal::None,
            last_page_index: None,
            // Starts true: the panel always fires a `reload` fetch the
            // moment it's shown, so the first render should display the
            // loading state rather than briefly flashing "No transactions".
            processing: true,
            current_page: 0,
            pending_page: None,
        }
    }

    /// True when the current page is the last page of history. Derived
    /// from `last_page_index` rather than the displayed row count.
    fn is_last_page(&self) -> bool {
        self.last_page_index == Some(self.current_page)
    }

    /// Recompute `displayed_txs` from the current page + pending list.
    /// Call after any change to `current_page`, `page_cache`, or
    /// `pending_txs`.
    fn refresh_displayed(&mut self) {
        let page = self
            .page_cache
            .get(self.current_page as usize)
            .cloned()
            .unwrap_or_default();
        self.displayed_txs = if self.current_page == 0 {
            let mut combined = self.pending_txs.clone();
            combined.extend(page);
            combined
        } else {
            page
        };
    }

    pub fn preselect(&mut self, tx: HistoryTransaction) {
        self.selected_tx = Some(tx);
        self.warning = None;
        self.modal = VaultTransactionsModal::None;
    }
}

impl State for VaultTransactionsPanel {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Some(tx) = self.selected_tx.as_ref() {
            let content = view::vault::transactions::transaction_detail_view(
                menu,
                cache,
                tx,
                self.labels_edited.cache(),
                cache.bitcoin_unit,
            );
            match &self.modal {
                VaultTransactionsModal::CreateRbf(rbf) => rbf.view(content),
                _ => content,
            }
        } else {
            let content = view::vault::transactions::transactions_view(
                menu,
                cache,
                &self.displayed_txs,
                self.current_page,
                self.is_last_page(),
                self.processing,
            );
            match &self.modal {
                VaultTransactionsModal::Export(export) => export.view(content),
                _ => content,
            }
        }
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let Some(daemon) = daemon else {
            tracing::warn!("VaultTransactionsPanel update called without daemon");
            return Task::none();
        };
        match message {
            Message::HistoryTransactions(res) => match res {
                Err(e) => {
                    // Must clear `processing` here: `reload` sets it true, so
                    // a failed initial fetch would otherwise leave the panel
                    // stuck on the loading indicator with Prev/Next disabled.
                    self.processing = false;
                    let err_msg = e.to_string();
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
                Ok((pending_txs, page_txs)) => {
                    self.warning = None;
                    self.pending_txs = pending_txs;
                    // Page 0 is fetched with a flat limit and no dedup, so a
                    // short response here genuinely means end-of-history.
                    if (page_txs.len() as u64) < HISTORY_EVENT_PAGE_SIZE {
                        self.last_page_index = Some(0);
                    } else {
                        self.last_page_index = None;
                    }
                    self.page_cache = vec![page_txs];
                    self.current_page = 0;
                    self.processing = false;
                    self.refresh_displayed();
                }
            },
            Message::HistoryTransactionsExtension(res) => match res {
                Err(e) => {
                    // Stale-response guard (see the Ok arm): if `pending_page`
                    // is already cleared a `reload` superseded this fetch —
                    // discard the late error without touching `processing`,
                    // which the reload's own fetch now owns.
                    if self.pending_page.take().is_none() {
                        return Task::none();
                    }
                    self.processing = false;
                    let err_msg = e.to_string();
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
                Ok((mut txs, server_exhausted)) => {
                    // Stale-response guard: this extension result is only
                    // valid if `pending_page` still records the NextPage
                    // fetch that produced it. A `reload` between dispatch and
                    // arrival clears `pending_page`, so a late response is
                    // discarded rather than advancing `current_page` and
                    // injecting stale rows into `page_cache`.
                    let Some(target) = self.pending_page.take() else {
                        return Task::none();
                    };
                    self.processing = false;
                    self.warning = None;
                    // The cursor we used (last tx's blocktime) is inclusive,
                    // so the response can repeat txs that already appeared on
                    // the current page. Drop those by txid before storing the
                    // next page. NOTE: `server_exhausted` is derived in the
                    // fetch task from the *raw* response length vs. the
                    // request limit — it must not be re-derived from the
                    // post-dedup `txs.len()`, which can be short even when
                    // more history exists.
                    if let Some(prev_page) = self.page_cache.get(self.current_page as usize) {
                        let prev_ids: std::collections::HashSet<_> =
                            prev_page.iter().map(|t| t.tx.compute_txid()).collect();
                        txs.retain(|t| !prev_ids.contains(&t.tx.compute_txid()));
                    }
                    if txs.is_empty() {
                        // Every row overlapped the current page — there is
                        // nothing new beyond it, so the current page is the
                        // last. Don't advance onto an empty page.
                        self.last_page_index = Some(self.current_page);
                    } else {
                        self.current_page = target;
                        if server_exhausted {
                            self.last_page_index = Some(self.current_page);
                        }
                        if (target as usize) < self.page_cache.len() {
                            self.page_cache[target as usize] = txs;
                        } else {
                            self.page_cache.push(txs);
                        }
                    }
                    self.refresh_displayed();
                }
            },
            Message::RbfModal(tx, is_cancel, res) => match res {
                Ok(descendant_txids) => {
                    let modal = CreateRbfModal::new(*tx, is_cancel, descendant_txids);
                    self.modal = VaultTransactionsModal::CreateRbf(modal);
                }
                Err(e) => {
                    let err: Error = e;
                    let err_msg = err.to_string();
                    self.warning = Some(err);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
            },
            Message::View(view::Message::Reload) | Message::View(view::Message::Close) => {
                return self.reload(Some(daemon), Some(self.wallet.clone()));
            }
            Message::View(view::Message::Select(i)) => {
                self.selected_tx = self.displayed_txs.get(i).cloned();
                // Clear modal if it's for a different tx.
                if let VaultTransactionsModal::CreateRbf(modal) = &self.modal {
                    if Some(modal.tx.tx.compute_txid())
                        != self
                            .selected_tx
                            .as_ref()
                            .map(|selected| selected.tx.compute_txid())
                    {
                        self.modal = VaultTransactionsModal::None;
                    }
                }
            }
            Message::View(view::Message::CreateRbf(view::CreateRbfMessage::Cancel)) => {
                self.modal = VaultTransactionsModal::None;
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
                    self.pending_txs
                        .iter_mut()
                        .chain(self.page_cache.iter_mut().flat_map(|p| p.iter_mut()))
                        .map(|tx| tx as &mut dyn LabelsLoader)
                        .chain(
                            self.selected_tx
                                .iter_mut()
                                .map(|tx| tx as &mut dyn LabelsLoader),
                        ),
                ) {
                    Ok(cmd) => {
                        // `labels_edited.update` mutates labels in-place on
                        // `pending_txs` / `page_cache`, but the view renders
                        // the `displayed_txs` clone — rematerialise it so the
                        // edited label is visible immediately rather than only
                        // after a page change or reload.
                        self.refresh_displayed();
                        return cmd;
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        self.warning = Some(e);
                        return Task::done(Message::View(view::Message::ShowError(err_msg)));
                    }
                };
            }
            Message::View(view::Message::VaultPrevPage) => {
                if self.current_page > 0 && !self.processing {
                    self.current_page -= 1;
                    self.refresh_displayed();
                }
            }
            Message::View(view::Message::VaultNextPage) => {
                if self.is_last_page() || self.processing {
                    return Task::none();
                }
                let next_page = (self.current_page as usize) + 1;
                // Already cached — jump straight there. Avoids a redundant
                // round-trip when the user pages back and forth.
                if next_page < self.page_cache.len() {
                    self.current_page += 1;
                    self.refresh_displayed();
                    return Task::none();
                }
                // Need to fetch. Cursor is the blocktime of the last
                // confirmed tx on the current page (mirrors the original
                // "See more" forward-scan logic, including the
                // duplicate-blocktime overflow handling — see comment in
                // the async block).
                let current_page_txs = self
                    .page_cache
                    .get(self.current_page as usize)
                    .cloned()
                    .unwrap_or_default();
                let Some(last) = current_page_txs.last() else {
                    return Task::none();
                };
                let Some(last_tx_date) = last.time else {
                    return Task::none();
                };
                let daemon = daemon.clone();
                self.pending_page = Some(self.current_page + 1);
                self.processing = true;
                return Task::perform(
                    async move {
                        let mut limit = HISTORY_EVENT_PAGE_SIZE;
                        let mut txs = daemon.list_history_txs(0_u32, last_tx_date, limit).await?;

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
                            return Ok((txs, true));
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
                        // The server is exhausted when it returned fewer rows
                        // than the (possibly bumped) limit we asked for. This
                        // is measured on the raw response — the caller must
                        // not re-derive it from the post-dedup length.
                        let server_exhausted = (txs.len() as u64) < limit;
                        txs.sort_by(|a, b| a.compare(b));
                        Ok((txs, server_exhausted))
                    },
                    Message::HistoryTransactionsExtension,
                );
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Open)) => {
                if let VaultTransactionsModal::None = &self.modal {
                    self.modal = VaultTransactionsModal::Export(VaultExportModal::new(
                        Some(daemon),
                        ImportExportType::Transactions,
                    ));
                    if let VaultTransactionsModal::Export(m) = &self.modal {
                        return m.launch(true);
                    }
                }
            }
            Message::View(view::Message::ImportExport(ImportExportMessage::Close)) => {
                if let VaultTransactionsModal::Export(_) = &self.modal {
                    self.modal = VaultTransactionsModal::None;
                }
            }
            ref msg => {
                return match &mut self.modal {
                    VaultTransactionsModal::CreateRbf(modal) => {
                        modal.update(daemon, _cache, message)
                    }
                    VaultTransactionsModal::Export(modal) => {
                        if let Message::View(view::Message::ImportExport(m)) = msg {
                            modal.update::<Message>(m.clone())
                        } else {
                            Task::none()
                        }
                    }
                    VaultTransactionsModal::None => Task::none(),
                };
            }
        }
        Task::none()
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        let Some(daemon) = daemon else {
            tracing::warn!("VaultTransactionsPanel reload called without daemon");
            return Task::none();
        };
        self.selected_tx = None;
        self.current_page = 0;
        self.last_page_index = None;
        self.processing = true;
        // Drop any in-flight NextPage fetch: its result must not advance
        // pages once this reload has reset pagination back to page 0.
        self.pending_page = None;
        self.page_cache.clear();
        self.pending_txs.clear();
        self.displayed_txs.clear();
        let now: u32 = now().as_secs().try_into().unwrap();
        Task::batch(vec![Task::perform(
            async move {
                let mut txs = daemon
                    .list_history_txs(0, now, HISTORY_EVENT_PAGE_SIZE)
                    .await?;
                txs.sort_by(|a, b| a.compare(b));

                let pending_txs = daemon.list_pending_txs().await?;
                Ok((pending_txs, txs))
            },
            Message::HistoryTransactions,
        )])
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        if let VaultTransactionsModal::Export(modal) = &self.modal {
            if let Some(sub) = modal.subscription() {
                return sub.map(|m| {
                    Message::View(view::Message::ImportExport(ImportExportMessage::Progress(
                        m,
                    )))
                });
            }
        }
        iced::Subscription::none()
    }
}

impl From<VaultTransactionsPanel> for Box<dyn State> {
    fn from(s: VaultTransactionsPanel) -> Box<dyn State> {
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
                warning: None,
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
                    Err(e) => {
                        let err_msg = e.to_string();
                        self.warning = Some(e);
                        return Task::done(Message::View(view::Message::ShowError(err_msg)));
                    }
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
    fn view<'a>(&'a self, content: Element<'a, view::Message>) -> Element<'a, view::Message> {
        let modal = Modal::new(
            content,
            view::vault::transactions::create_rbf_modal(
                self.is_cancel,
                &self.descendant_txids,
                &self.feerate_val,
                self.replacement_txid,
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
            return Err(SpendCreationError::CoinSelection(
                coincube_core::spend::InsufficientFunds { missing },
            )
            .into());
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
