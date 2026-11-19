//! Read-only history from the wallets in the currently open Cube.
//! Source records remain attached to each row so detail actions can hand off to
//! the owning wallet panel without guessing from a display string.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use coincube_spark_protocol::PaymentSummary;
use iced::Task;

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        message::Message,
        view::{
            self,
            activity::{ActivityMessage, RecentScope, WalletFilter},
        },
        wallet::Wallet,
        wallets::SparkBackend,
    },
    daemon::{
        model::{HistoryTransaction, SpendStatus, SpendTx},
        Daemon,
    },
    utils::now,
};

use super::State;

const HISTORY_LIMIT: u64 = 100;
/// The label editors' and the RPC daemon's limit (`vault/label.rs`).
const LABEL_MAX_LEN: usize = 100;

#[derive(Debug, Clone)]
pub enum ActivityTarget {
    Vault(HistoryTransaction),
    Psbt(SpendTx),
    Spark(PaymentSummary),
}

pub struct Activity {
    spark_backend: Option<Arc<SparkBackend>>,
    vault: Vec<HistoryTransaction>,
    psbts: Vec<SpendTx>,
    spark: Vec<PaymentSummary>,
    pub filters: view::activity::Filters,
    pub selected_id: Option<String>,
    /// The row an Overview row opened ([`ActivityMessage::Open`]), as the
    /// Overview listed it. While it is still the selected row it owns the
    /// detail pane even when Activity doesn't list it: the Overview lists the
    /// newest rows without [`Self::complete_through`]'s cutoff, and the
    /// reload on entry clears the rows, so a failed read would otherwise
    /// leave nothing to show. A fresh copy of the row wins once it loads.
    pub opened: Option<view::activity::Entry>,
    pub vault_loading: bool,
    pub psbt_loading: bool,
    pub spark_loading: bool,
    pub vault_error: Option<String>,
    pub psbt_error: Option<String>,
    pub spark_error: Option<String>,
    wallet: Option<Arc<Wallet>>,
    pub vault_has_more: bool,
    pub spark_has_more: bool,
    /// A wallet page asked for fresh rows while that source was loading:
    /// refresh again once the answer lands, so a payment seen meanwhile
    /// still reaches "Recent activity".
    vault_stale: bool,
    spark_stale: bool,
    /// Spark refreshes asked for (Spark's bridge has no test double).
    #[cfg(test)]
    pub spark_refresh_requests: u64,
    vault_limit: u64,
    generation: u64,
    /// The selected row's unsaved label: its entry id and text.
    label_draft: Option<(String, String)>,
    /// Label saves in flight: request number, entry id and the text sent.
    /// Their own numbers, not the list generation, so a reload cannot
    /// strand one; rows save independently.
    label_saves: Vec<(u64, String, String)>,
    label_requests: u64,
    /// Why an entry's last save failed, shown under that entry until it is
    /// edited or saved.
    label_errors: HashMap<String, String>,
    /// The entry whose rail details are open; a new selection closes them.
    details_open: Option<String>,
}

impl Activity {
    pub fn new(spark_backend: Option<Arc<SparkBackend>>) -> Self {
        Self {
            spark_backend,
            vault: Vec::new(),
            psbts: Vec::new(),
            spark: Vec::new(),
            filters: Default::default(),
            selected_id: None,
            opened: None,
            vault_loading: false,
            psbt_loading: false,
            spark_loading: false,
            vault_error: None,
            psbt_error: None,
            spark_error: None,
            wallet: None,
            vault_has_more: false,
            spark_has_more: false,
            vault_stale: false,
            spark_stale: false,
            #[cfg(test)]
            spark_refresh_requests: 0,
            vault_limit: HISTORY_LIMIT,
            generation: 0,
            label_draft: None,
            label_saves: Vec::new(),
            label_requests: 0,
            label_errors: HashMap::new(),
            details_open: None,
        }
    }

    /// Whether the Vault screen offers Bump fee and Cancel for this wallet
    /// (`state/vault/transactions.rs`).
    pub fn vault_supports_replacement(&self) -> bool {
        self.wallet
            .as_ref()
            .is_some_and(|wallet| wallet.supports_replacement())
    }

    #[cfg(test)]
    pub fn generation_for_tests(&self) -> u64 {
        self.generation
    }

    pub fn label_saving_for(&self, entry_id: &str) -> bool {
        self.label_saves.iter().any(|(_, id, _)| id == entry_id)
    }

    pub fn label_error_for(&self, entry_id: &str) -> Option<&str> {
        self.label_errors.get(entry_id).map(String::as_str)
    }

    pub fn label_draft_for(&self, entry_id: &str) -> Option<&str> {
        self.label_draft
            .as_ref()
            .filter(|(id, _)| id == entry_id)
            .map(|(_, draft)| draft.as_str())
    }

    pub fn details_open_for(&self, entry_id: &str) -> bool {
        self.details_open.as_deref() == Some(entry_id)
    }

    /// One wallet's rows, newest first: the Vault and Spark pages' "Recent
    /// activity" reads the same rows and labels as this page.
    pub fn wallet_entries(&self, wallet: WalletFilter) -> Vec<view::activity::Entry> {
        self.entries()
            .into_iter()
            .filter(|entry| entry.wallet == wallet)
            .collect()
    }

    /// The rows a "Recent activity" table in `scope` lists: its newest
    /// [`RECENT_ACTIVITY_ROWS`], read from the same rows as this page.
    ///
    /// [`RECENT_ACTIVITY_ROWS`]: view::activity::RECENT_ACTIVITY_ROWS
    pub fn recent_entries(&self, scope: RecentScope) -> Vec<view::activity::Entry> {
        self.entries()
            .into_iter()
            .filter(|entry| scope.includes(entry.wallet))
            .take(view::activity::RECENT_ACTIVITY_ROWS)
            .collect()
    }

    /// No row selected: a wallet page opens without a detail pane.
    pub fn clear_selection(&mut self) {
        self.selected_id = None;
        self.opened = None;
        self.details_open = None;
        self.label_draft = None;
    }

    /// Re-read one wallet's newest page for its own page's "Recent activity".
    /// Unlike [`State::reload`] the loaded rows stay on screen until the
    /// answer replaces them, so a background refresh never flashes "Loading".
    /// A source already loading is left to finish.
    pub fn refresh_wallet(
        &mut self,
        source: WalletFilter,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        if wallet.is_some() {
            self.wallet = wallet;
        }
        let generation = self.generation;
        let mut tasks = Vec::new();
        match source {
            WalletFilter::Vault => {
                let Some(daemon) = daemon else {
                    return Task::none();
                };
                if self.vault_loading {
                    self.vault_stale = true;
                } else {
                    self.vault_loading = true;
                    self.vault_limit = HISTORY_LIMIT;
                    tasks.push(vault_history(
                        daemon.clone(),
                        self.wallet.clone(),
                        HISTORY_LIMIT,
                        generation,
                    ));
                }
                if !self.psbt_loading && self.vault_supports_replacement() {
                    self.psbt_loading = true;
                    tasks.push(vault_proposals(daemon, generation));
                }
            }
            WalletFilter::Spark => {
                #[cfg(test)]
                {
                    self.spark_refresh_requests += 1;
                }
                if self.spark_loading {
                    self.spark_stale = true;
                } else if let Some(spark) = self.spark_backend.clone() {
                    self.spark_loading = true;
                    tasks.push(spark_payments(spark, 0, move |result| {
                        Message::ActivitySparkRefreshed(generation, result)
                    }));
                }
            }
            WalletFilter::All => {}
        }
        Task::batch(tasks)
    }

    /// [`Self::refresh_wallet`] for each wallet a "Recent activity" table in
    /// `scope` reads.
    pub fn refresh_scope(
        &mut self,
        scope: RecentScope,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        let tasks: Vec<_> = scope
            .sources()
            .iter()
            .map(|source| self.refresh_wallet(*source, daemon.clone(), wallet.clone()))
            .collect();
        Task::batch(tasks)
    }

    /// The label `entry` edits, read from the entry's own row: other rows
    /// can carry the same key (a spend loads its inputs' outpoint labels).
    pub fn stored_label(&self, entry: &view::activity::Entry) -> &str {
        let Some(key) = entry.label_key.as_ref().map(ToString::to_string) else {
            return "";
        };
        // The row an Overview row opened is read from its kept copy while
        // the loaded rows lack it (a failed read).
        let opened = self.opened.as_ref().map(|opened| &opened.target);
        let labels = match &entry.target {
            ActivityTarget::Vault(tx) => self
                .vault
                .iter()
                .find(|row| row.txid == tx.txid)
                .map(|row| &row.labels)
                .or(match opened {
                    Some(ActivityTarget::Vault(row)) if row.txid == tx.txid => Some(&row.labels),
                    _ => None,
                }),
            ActivityTarget::Psbt(tx) => {
                let txid = tx.psbt.unsigned_tx.compute_txid();
                self.psbts
                    .iter()
                    .find(|row| row.psbt.unsigned_tx.compute_txid() == txid)
                    .map(|row| &row.labels)
                    .or(match opened {
                        Some(ActivityTarget::Psbt(row))
                            if row.psbt.unsigned_tx.compute_txid() == txid =>
                        {
                            Some(&row.labels)
                        }
                        _ => None,
                    })
            }
            _ => None,
        };
        labels
            .and_then(|labels| labels.get(&key))
            .map_or("", String::as_str)
    }

    /// Keep a saved label in the loaded rows, so titles follow at once: every
    /// row already holding the key (a spend carries its inputs' labels), and
    /// the row that owns it. A txid key, or an outpoint key (`txid:vout`),
    /// belongs to that txid.
    fn apply_label(&mut self, key: &str, label: Option<&str>) {
        let owns = |txid: String| {
            key == txid
                || key
                    .strip_prefix(&txid)
                    .is_some_and(|rest| rest.starts_with(':'))
        };
        // The opened row's kept copy follows too, so its pane and title do.
        let opened = self
            .opened
            .as_mut()
            .and_then(|opened| match &mut opened.target {
                ActivityTarget::Vault(tx) => Some((tx.txid.to_string(), &mut tx.labels)),
                ActivityTarget::Psbt(tx) => Some((
                    tx.psbt.unsigned_tx.compute_txid().to_string(),
                    &mut tx.labels,
                )),
                _ => None,
            });
        let rows = self
            .vault
            .iter_mut()
            .map(|tx| (tx.txid.to_string(), &mut tx.labels))
            .chain(self.psbts.iter_mut().map(|tx| {
                (
                    tx.psbt.unsigned_tx.compute_txid().to_string(),
                    &mut tx.labels,
                )
            }))
            .chain(opened);
        for (txid, labels) in rows {
            if !owns(txid) && !labels.contains_key(key) {
                continue;
            }
            match label {
                Some(label) => labels.insert(key.to_owned(), label.to_owned()),
                None => labels.remove(key),
            };
        }
        if let Some(opened) = self.opened.as_mut() {
            let rebuilt = match &opened.target {
                ActivityTarget::Vault(tx) => Some(view::activity::Entry::vault(tx.clone())),
                ActivityTarget::Psbt(tx) => Some(view::activity::Entry::psbt(tx.clone())),
                _ => None,
            };
            if let Some(mut rebuilt) = rebuilt {
                rebuilt.id = std::mem::take(&mut opened.id);
                *opened = rebuilt;
            }
        }
    }

    fn finish_vault_load(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        result: Result<(Vec<HistoryTransaction>, bool), String>,
    ) -> Task<Message> {
        self.vault_loading = false;
        match result {
            Ok((rows, more)) => {
                self.vault = rows;
                self.vault_has_more = more;
                self.vault_error = None;
            }
            Err(error) => self.vault_error = Some(error),
        }
        if std::mem::take(&mut self.vault_stale) {
            return self.refresh_wallet(WalletFilter::Vault, daemon, None);
        }
        Task::none()
    }

    fn finish_label_save(
        &mut self,
        request: u64,
        result: Result<(String, String, Option<String>), String>,
    ) {
        // Only this request's own answer ends its "Saving…"; an
        // answer from before a reload still updates the rows.
        let current = self
            .label_saves
            .iter()
            .position(|(pending, _, _)| *pending == request)
            .map(|index| self.label_saves.remove(index));
        match result {
            Ok((entry_id, key, label)) => {
                self.apply_label(&key, label.as_deref());
                self.label_errors.remove(&entry_id);
                if self.label_draft.as_ref().is_some_and(|(id, draft)| {
                    id == &entry_id && draft.trim() == label.as_deref().unwrap_or("")
                }) {
                    self.label_draft = None;
                }
            }
            Err(_) => {
                if let Some((_, entry_id, text)) = current {
                    // Give the text back, unless another row's edit
                    // holds the draft now.
                    if self
                        .label_draft
                        .as_ref()
                        .is_none_or(|(id, _)| id == &entry_id)
                    {
                        self.label_draft = Some((entry_id.clone(), text));
                    }
                    self.label_errors.insert(
                        entry_id,
                        "The label couldn't be saved. Press Enter to retry.".to_owned(),
                    );
                }
            }
        }
    }

    fn update_activity_view(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        event: ActivityMessage,
    ) -> Task<Message> {
        match event {
            ActivityMessage::Wallet(value) => self.filters.wallet = value,
            ActivityMessage::Direction(value) => self.filters.direction = value,
            ActivityMessage::Status(value) => self.filters.status = value,
            ActivityMessage::DateRange(value) => self.filters.date_range = value,
            ActivityMessage::Search(value) => self.filters.search = value,
            ActivityMessage::Clear => self.filters = Default::default(),
            ActivityMessage::Select(id) => {
                if self.selected_id.as_deref() != Some(id.as_str()) {
                    self.details_open = None;
                }
                // One draft at a time, kept for its own row (a failed
                // save gives its text back); a row's save error stays.
                if self
                    .label_draft
                    .as_ref()
                    .is_some_and(|(draft_id, _)| draft_id != &id)
                {
                    self.label_draft = None;
                }
                self.opened = None;
                self.selected_id = Some(id);
            }
            ActivityMessage::ToggleDetails(open) => {
                self.details_open = open.then(|| self.selected_id.clone()).flatten();
            }
            ActivityMessage::LabelEdited(value) => {
                if let Some(id) = self.selected_id.clone() {
                    self.label_errors.remove(&id);
                    self.label_draft = Some((id, value));
                }
            }
            ActivityMessage::SaveLabel => return self.save_label(daemon),
            ActivityMessage::CloseDetail => {
                self.details_open = None;
                self.label_draft = None;
                self.opened = None;
                let selected = self.selected_id.take();
                return selected.map_or_else(Task::none, |id| {
                    iced::widget::operation::focus(view::activity::row_focus_id(&id))
                });
            }
            ActivityMessage::Retry => return self.reload(daemon, self.wallet.clone()),
            ActivityMessage::RetryWallet(wallet) => {
                return self.refresh_wallet(wallet, daemon, None)
            }
            ActivityMessage::LoadMore => return self.load_more(daemon),
            ActivityMessage::OpenDetail(target) => {
                return Task::done(Message::ActivityOpenDetail(target))
            }
            ActivityMessage::ViewAll(wallet) => {
                // App switches to Activity; it opens on this wallet alone.
                self.clear_selection();
                self.filters = view::activity::Filters {
                    wallet,
                    ..Default::default()
                };
            }
            ActivityMessage::Open(id) => {
                // App switches to Activity; it opens unfiltered with this
                // row selected. The id is the row's own, not its place in
                // a list, so the selection holds while the page reloads.
                // App keeps the row as the Overview listed it
                // (`Self::opened`), read before the reload cleared it.
                let opened = self.entries().into_iter().find(|entry| entry.id == id);
                self.clear_selection();
                self.filters = Default::default();
                self.opened = opened;
                self.selected_id = Some(id);
            }
        }
        Task::none()
    }

    fn save_label(&mut self, daemon: Option<Arc<dyn Daemon + Sync + Send>>) -> Task<Message> {
        let Some((entry_id, draft)) = self.label_draft.clone() else {
            return Task::none();
        };
        if self.label_saving_for(&entry_id) {
            return Task::none();
        }
        // The opened row's kept copy saves too while the loaded rows lack it.
        let Some(entry) = self
            .entries()
            .into_iter()
            .find(|entry| entry.id == entry_id)
            .or_else(|| self.opened.clone().filter(|opened| opened.id == entry_id))
        else {
            self.label_draft = None;
            return Task::none();
        };
        let Some(key) = entry.label_key else {
            return Task::none();
        };
        let label = Some(draft.trim().to_owned()).filter(|label| !label.is_empty());
        // The daemon's own limit; the embedded one does not check it.
        if label
            .as_ref()
            .is_some_and(|label| label.len() > LABEL_MAX_LEN)
        {
            self.label_errors.insert(
                entry_id,
                format!("Labels are limited to {LABEL_MAX_LEN} characters."),
            );
            return Task::none();
        }
        let Some(daemon) = daemon else {
            self.label_errors.insert(
                entry_id,
                "The Vault isn't running, so the label can't be saved yet.".to_owned(),
            );
            return Task::none();
        };
        self.label_requests = self.label_requests.wrapping_add(1);
        let request = self.label_requests;
        self.label_saves.push((request, entry_id.clone(), draft));
        self.label_errors.remove(&entry_id);
        Task::perform(
            async move {
                let stored = key.to_string();
                daemon
                    .update_labels(&HashMap::from([(key, label.clone())]))
                    .await
                    .map(|()| (entry_id, stored, label))
                    .map_err(|e| e.to_string())
            },
            move |result| Message::ActivityLabelSaved(request, result),
        )
    }

    pub fn entries(&self) -> Vec<view::activity::Entry> {
        let mut entries =
            Vec::with_capacity(self.vault.len() + self.psbts.len() + self.spark.len());
        entries.extend(self.vault.iter().cloned().map(view::activity::Entry::vault));
        entries.extend(
            self.psbts
                .iter()
                .filter(|tx| tx.status == SpendStatus::Pending)
                .cloned()
                .map(view::activity::Entry::psbt),
        );
        entries.extend(self.spark.iter().cloned().map(view::activity::Entry::spark));
        entries.sort_by(|a, b| {
            let priority = |timestamp: u64| if timestamp == 0 { u64::MAX } else { timestamp };
            priority(b.timestamp)
                .cmp(&priority(a.timestamp))
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut duplicate_counts = HashMap::<String, usize>::new();
        for entry in &mut entries {
            let count = duplicate_counts.entry(entry.id.clone()).or_default();
            if *count > 0 {
                entry.id.push_str(&format!(":duplicate-{count}"));
            }
            *count += 1;
        }
        entries
    }

    /// Older rows are incomplete while any paginated source still has history.
    /// The last loaded timestamp from that source is an exclusive boundary:
    /// another page may contain rows with the same timestamp.
    pub fn complete_through(&self) -> Option<u64> {
        [
            (self.vault_has_more
                && matches!(
                    self.filters.wallet,
                    view::activity::WalletFilter::All | view::activity::WalletFilter::Vault
                ))
            .then(|| {
                self.vault
                    .iter()
                    .filter_map(|tx| tx.time.map(u64::from))
                    .min()
                    .unwrap_or(u64::MAX)
            }),
            (self.spark_has_more
                && matches!(
                    self.filters.wallet,
                    view::activity::WalletFilter::All | view::activity::WalletFilter::Spark
                ))
            .then(|| {
                self.spark
                    .iter()
                    .map(|row| row.timestamp)
                    .filter(|time| *time > 0)
                    .min()
                    .unwrap_or(u64::MAX)
            }),
        ]
        .iter()
        .flatten()
        .max()
        .copied()
    }

    fn load_more(&mut self, daemon: Option<Arc<dyn Daemon + Sync + Send>>) -> Task<Message> {
        let generation = self.generation;
        let mut tasks = Vec::new();
        if self.vault_has_more && !self.vault_loading {
            if let Some(daemon) = daemon {
                self.vault_limit += HISTORY_LIMIT;
                let limit = self.vault_limit;
                let wallet = self.wallet.clone();
                self.vault_loading = true;
                tasks.push(vault_history(daemon, wallet, limit, generation));
            }
        }
        if self.spark_has_more && !self.spark_loading {
            if let Some(spark) = self.spark_backend.clone() {
                self.spark_loading = true;
                let offset = self.spark.len() as u32;
                tasks.push(spark_payments(spark, offset, move |result| {
                    Message::ActivitySparkLoaded(generation, result)
                }));
            }
        }
        Task::batch(tasks)
    }
}

/// The Vault's newest `limit` history rows, pending first, as Activity lists
/// them.
fn vault_history(
    daemon: Arc<dyn Daemon + Sync + Send>,
    wallet: Option<Arc<Wallet>>,
    limit: u64,
    generation: u64,
) -> Task<Message> {
    Task::perform(
        async move {
            let upper = now().as_secs().min(u32::MAX as u64) as u32;
            let mut rows = daemon
                .list_history_txs(0, upper, limit + 1)
                .await
                .map_err(|e| e.to_string())?;
            let more = rows.len() as u64 > limit;
            rows.truncate(limit as usize);
            let mut pending = daemon.list_pending_txs().await.map_err(|e| e.to_string())?;
            if let Some(wallet) = wallet {
                let known: HashSet<_> = pending.iter().map(|tx| tx.txid).collect();
                pending.extend(wallet.synthesized_pending_history_txs(&known));
            }
            let known: HashSet<_> = pending.iter().map(|tx| tx.txid).collect();
            pending.extend(rows.into_iter().filter(|tx| !known.contains(&tx.txid)));
            Ok::<_, String>((pending, more))
        },
        move |result| Message::ActivityVaultLoaded(generation, result),
    )
}

fn vault_proposals(daemon: Arc<dyn Daemon + Sync + Send>, generation: u64) -> Task<Message> {
    Task::perform(
        async move {
            daemon
                .list_spend_transactions(None)
                .await
                .map_err(|e| e.to_string())
        },
        move |result| Message::ActivityPsbtLoaded(generation, result),
    )
}

type SparkPage = Result<(Vec<PaymentSummary>, bool), String>;

/// One page of Spark payments from `offset`, answered by `done`.
fn spark_payments(
    spark: Arc<SparkBackend>,
    offset: u32,
    done: impl FnOnce(SparkPage) -> Message + Send + 'static,
) -> Task<Message> {
    Task::perform(
        async move {
            spark
                .list_payments(Some(HISTORY_LIMIT as u32 + 1), Some(offset))
                .await
                .map(|page| {
                    let mut rows = page.payments;
                    let more = rows.len() as u64 > HISTORY_LIMIT;
                    rows.truncate(HISTORY_LIMIT as usize);
                    (rows, more)
                })
                .map_err(|e| e.to_string())
        },
        done,
    )
}

impl State for Activity {
    fn view<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
    ) -> coincube_ui::widget::Element<'a, view::Message> {
        view::dashboard_activity(menu, cache, view::activity::render(self, cache))
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::ActivityVaultLoaded(generation, result) if generation == self.generation => {
                return self.finish_vault_load(daemon, result);
            }
            Message::ActivityPsbtLoaded(generation, result) if generation == self.generation => {
                self.psbt_loading = false;
                match result {
                    Ok(rows) => {
                        self.psbts = rows;
                        self.psbt_error = None;
                    }
                    Err(error) => self.psbt_error = Some(error),
                }
            }
            Message::ActivitySparkLoaded(generation, result) if generation == self.generation => {
                self.spark_loading = false;
                match result {
                    Ok((rows, more)) => {
                        if self.spark.is_empty() {
                            self.spark = rows;
                        } else {
                            self.spark.extend(rows);
                        }
                        self.spark_has_more = more;
                        self.spark_error = None;
                    }
                    Err(error) => self.spark_error = Some(error),
                }
                if std::mem::take(&mut self.spark_stale) {
                    return self.refresh_wallet(WalletFilter::Spark, None, None);
                }
            }
            Message::ActivitySparkRefreshed(generation, result)
                if generation == self.generation =>
            {
                self.spark_loading = false;
                match result {
                    Ok((rows, more)) => {
                        self.spark = rows;
                        self.spark_has_more = more;
                        self.spark_error = None;
                    }
                    Err(error) => self.spark_error = Some(error),
                }
                if std::mem::take(&mut self.spark_stale) {
                    return self.refresh_wallet(WalletFilter::Spark, None, None);
                }
            }
            Message::ActivityLabelSaved(request, result) => self.finish_label_save(request, result),
            Message::View(view::Message::Activity(event)) => {
                return self.update_activity_view(daemon, event)
            }
            _ => {}
        }
        Task::none()
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        if wallet.is_some() {
            self.wallet = wallet.clone();
        }
        self.generation = self.generation.wrapping_add(1);
        // A save's answer may have gone to another panel while this one was
        // away; never leave a field waiting on it. Unsaved text stays.
        self.label_saves.clear();
        self.vault_limit = HISTORY_LIMIT;
        self.vault_has_more = false;
        self.spark_has_more = false;
        self.vault_stale = false;
        self.spark_stale = false;
        self.vault.clear();
        self.psbts.clear();
        self.spark.clear();
        let generation = self.generation;
        self.vault_loading = daemon.is_some();
        self.psbt_loading = daemon.is_some() && wallet.is_some();
        self.spark_loading = self.spark_backend.is_some();
        self.vault_error = None;
        self.psbt_error = None;
        self.spark_error = None;
        let mut tasks = Vec::new();
        if let Some(daemon) = daemon {
            if self.psbt_loading {
                tasks.push(vault_proposals(daemon.clone(), generation));
            }
            tasks.push(vault_history(daemon, wallet, HISTORY_LIMIT, generation));
        } else {
            self.vault.clear();
        }
        if let Some(spark) = self.spark_backend.clone() {
            tasks.push(spark_payments(spark, 0, move |result| {
                Message::ActivitySparkLoaded(generation, result)
            }));
        } else {
            self.spark.clear();
        }
        Task::batch(tasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payment(id: &str, timestamp: u64) -> PaymentSummary {
        PaymentSummary {
            id: id.into(),
            amount_sat: 1,
            fees_sat: 0,
            token_amount: None,
            token_decimals: None,
            token_ticker: None,
            timestamp,
            status: "completed".into(),
            direction: "receive".into(),
            method: "lightning".into(),
            description: None,
            details: Default::default(),
        }
    }

    #[test]
    fn incomplete_source_hides_older_cross_wallet_days() {
        let mut activity = Activity::new(None);
        activity.spark = vec![payment("new", 300), payment("oldest-loaded", 200)];
        activity.spark_has_more = true;
        assert_eq!(activity.complete_through(), Some(200));
        assert!(activity
            .entries()
            .iter()
            .any(|entry| entry.timestamp == 300));
        assert!(!activity
            .entries()
            .iter()
            .filter(|entry| {
                activity
                    .complete_through()
                    .is_none_or(|cutoff| entry.timestamp > cutoff)
            })
            .any(|entry| entry.timestamp == 200));
        activity.spark.extend([payment("older", 100)]);
        activity.spark_has_more = false;
        assert_eq!(activity.complete_through(), None);
        activity.spark_has_more = true;
        activity.filters.wallet = view::activity::WalletFilter::Vault;
        assert_eq!(activity.complete_through(), None);
    }
}
