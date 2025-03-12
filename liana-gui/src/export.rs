use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
    str::FromStr,
    sync::{
        mpsc::{channel, sync_channel, Receiver, Sender, SyncSender},
        Arc, Mutex,
    },
    time,
};

use async_hwi::bitbox::api::btc::Fingerprint;
use chrono::{DateTime, Duration, Utc};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{Amount, Psbt, Txid},
};
use lianad::{
    bip329::{error::ExportError, Labels},
    commands::LabelItem,
};
use tokio::{
    task::{JoinError, JoinHandle},
    time::sleep,
};

use iced::futures::{SinkExt, Stream};

use crate::{
    app::{
        settings::{KeySetting, Settings},
        view,
    },
    backup::{self, Backup},
    daemon::{
        model::{HistoryTransaction, Labelled},
        Daemon, DaemonBackend, DaemonError,
    },
    lianalite::client::backend::api::DEFAULT_LIMIT,
};

const DUMP_LABELS_LIMIT: u32 = 100;

macro_rules! send_error {
    ($sender:ident, $error:ident) => {
        if let Err(e) = $sender.send(Progress::Error(Error::$error)) {
            tracing::error!("Import/Export fail to send msg: {}", e);
        }
    };
    ($sender:ident, $error:expr) => {
        if let Err(e) = $sender.send(Progress::Error($error)) {
            tracing::error!("Import/Export fail to send msg: {}", e);
        }
    };
}

macro_rules! send_progress {
    ($sender:ident, $progress:ident) => {
        if let Err(e) = $sender.send(Progress::$progress) {
            tracing::error!("ImportExport fail to send msg: {}", e);
        }
    };
    ($sender:ident, $progress:ident($val:expr)) => {
        if let Err(e) = $sender.send(Progress::$progress($val)) {
            tracing::error!("ImportExport fail to send msg: {}", e);
        }
    };
}

macro_rules! open_file_write {
    ($path:ident, $sender:ident) => {{
        let dir = match $path.parent() {
            Some(dir) => dir,
            None => {
                send_error!($sender, NoParentDir);
                return;
            }
        };
        if !dir.exists() {
            if let Err(e) = fs::create_dir_all(dir) {
                send_error!($sender, e.into());
                return;
            }
        }
        match File::create($path.as_path()) {
            Ok(f) => f,
            Err(e) => {
                send_error!($sender, e.into());
                return;
            }
        }
    }};
}

macro_rules! open_file_read {
    ($path:ident, $sender:ident) => {{
        match File::open($path.as_path()) {
            Ok(f) => f,
            Err(e) => {
                send_error!($sender, e.into());
                return;
            }
        }
    }};
}

#[derive(Debug, Clone)]
pub enum ImportExportMessage {
    Open,
    Progress(Progress),
    TimedOut,
    UserStop,
    Path(Option<PathBuf>),
    Close,
    Overwrite,
    Ignore,
    UpdateAliases(HashMap<Fingerprint, String>),
}

impl From<ImportExportMessage> for view::Message {
    fn from(value: ImportExportMessage) -> Self {
        Self::ImportExport(value)
    }
}

#[derive(Debug, PartialEq)]
pub enum ImportExportState {
    Init,
    ChoosePath,
    Path(PathBuf),
    Started,
    Progress(f32),
    TimedOut,
    Aborted,
    Ended,
    Closed,
}

#[derive(Debug, Clone)]
pub enum Error {
    Io(String),
    HandleLost,
    UnexpectedEnd,
    JoinError(String),
    ChannelLost,
    NoParentDir,
    Daemon(String),
    TxTimeMissing,
    DaemonMissing,
    ParsePsbt,
    ParseDescriptor,
    Bip329Export(String),
    BackupImport(String),
    Backup(backup::Error),
}

#[derive(Debug, Clone)]
pub enum ImportExportType {
    Transactions,
    ExportPsbt(String),
    ExportBackup(String),
    ImportBackup(
        Option<SyncSender<bool>>, /*overwrite_labels*/
        Option<SyncSender<bool>>, /*overwrite_aliases*/
    ),
    Descriptor(LianaDescriptor),
    ExportLabels,
    ImportPsbt,
    ImportDescriptor,
}

impl ImportExportType {
    pub fn end_message(&self) -> &str {
        match self {
            ImportExportType::Transactions
            | ImportExportType::ExportPsbt(_)
            | ImportExportType::ExportBackup(_)
            | ImportExportType::Descriptor(_)
            | ImportExportType::ExportLabels => "Export successful!",
            ImportExportType::ImportBackup(_, _)
            | ImportExportType::ImportPsbt
            | ImportExportType::ImportDescriptor => "Import successful",
        }
    }
}

impl PartialEq for ImportExportType {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ExportPsbt(l0), Self::ExportPsbt(r0)) => l0 == r0,
            (Self::ExportBackup(l0), Self::ExportBackup(r0)) => l0 == r0,
            (Self::ImportBackup(l0, l1), Self::ImportBackup(r0, r1)) => {
                l0.is_some() == r0.is_some() && l1.is_some() == r1.is_some()
            }
            (Self::Descriptor(l0), Self::Descriptor(r0)) => l0 == r0,
            _ => core::mem::discriminant(self) == core::mem::discriminant(other),
        }
    }
}

impl From<JoinError> for Error {
    fn from(value: JoinError) -> Self {
        Error::JoinError(format!("{:?}", value))
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Error::Io(format!("{:?}", value))
    }
}

impl From<DaemonError> for Error {
    fn from(value: DaemonError) -> Self {
        Error::Daemon(format!("{:?}", value))
    }
}

impl From<ExportError> for Error {
    fn from(value: ExportError) -> Self {
        Error::Bip329Export(format!("{:?}", value))
    }
}

#[derive(Debug)]
pub enum Status {
    Init,
    Running,
    Stopped,
}

#[derive(Debug, Clone)]
pub enum Progress {
    Started(Arc<Mutex<JoinHandle<()>>>),
    Progress(f32),
    Ended,
    Finished,
    Error(Error),
    None,
    Psbt(Psbt),
    Descriptor(LianaDescriptor),
    LabelsConflict(SyncSender<bool>),
    KeyAliasesConflict(SyncSender<bool>),
    UpdateAliases(HashMap<Fingerprint, String>),
}

pub struct Export {
    pub receiver: Receiver<Progress>,
    pub sender: Option<Sender<Progress>>,
    pub handle: Option<Arc<Mutex<JoinHandle<()>>>>,
    pub daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    pub path: Box<PathBuf>,
    pub export_type: ImportExportType,
}

impl Export {
    pub fn new(
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        path: Box<PathBuf>,
        export_type: ImportExportType,
    ) -> Self {
        let (sender, receiver) = channel();
        Export {
            receiver,
            sender: Some(sender),
            handle: None,
            daemon,
            path,
            export_type,
        }
    }

    pub async fn export_logic(
        export_type: ImportExportType,
        sender: Sender<Progress>,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        path: PathBuf,
    ) {
        match export_type {
            ImportExportType::Transactions => export_transactions(sender, daemon, path).await,
            ImportExportType::ExportPsbt(str) => export_string(sender, path, str),
            ImportExportType::Descriptor(descriptor) => export_descriptor(sender, path, descriptor),
            ImportExportType::ExportLabels => export_labels(sender, daemon, path).await,
            ImportExportType::ImportPsbt => import_psbt(sender, path),
            ImportExportType::ImportDescriptor => import_descriptor(sender, path),
            ImportExportType::ExportBackup(str) => export_string(sender, path, str),
            ImportExportType::ImportBackup(..) => import_backup(sender, path, daemon).await,
        };
    }

    pub async fn start(&mut self) {
        if let (true, Some(sender)) = (self.handle.is_none(), self.sender.take()) {
            let daemon = self.daemon.clone();
            let path = self.path.clone();

            let cloned_sender = sender.clone();
            let export_type = self.export_type.clone();
            let handle = tokio::spawn(async move {
                Self::export_logic(export_type, cloned_sender, daemon, *path).await;
            });
            let handle = Arc::new(Mutex::new(handle));

            let cloned_sender = sender.clone();
            // we send the handle to the GUI so we can kill the thread on timeout
            // or user cancel action
            send_progress!(cloned_sender, Started(handle.clone()));
            self.handle = Some(handle);
        } else {
            tracing::error!("ExportState can start only once!");
        }
    }
    pub fn state(&self) -> Status {
        match (&self.sender, &self.handle) {
            (Some(_), None) => Status::Init,
            (None, Some(_)) => Status::Running,
            (None, None) => Status::Stopped,
            _ => unreachable!(),
        }
    }
}

pub fn export_subscription(
    daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    path: PathBuf,
    export_type: ImportExportType,
) -> impl Stream<Item = Progress> {
    iced::stream::channel(100, move |mut output| async move {
        let mut state = Export::new(daemon, Box::new(path), export_type);
        loop {
            match state.state() {
                Status::Init => {
                    state.start().await;
                }
                Status::Stopped => {
                    break;
                }
                Status::Running => {}
            }
            let msg = state.receiver.try_recv();
            let disconnected = match msg {
                Ok(m) => {
                    if let Err(e) = output.send(m).await {
                        tracing::error!("export_subscription() fail to send message: {}", e);
                    }
                    continue;
                }
                Err(e) => match e {
                    std::sync::mpsc::TryRecvError::Empty => false,
                    std::sync::mpsc::TryRecvError::Disconnected => true,
                },
            };

            let handle = match state.handle.take() {
                Some(h) => h,
                None => {
                    if let Err(e) = output.send(Progress::Error(Error::HandleLost)).await {
                        tracing::error!("export_subscription() fail to send message: {}", e);
                    }
                    continue;
                }
            };
            let msg = {
                let h = handle.lock().expect("should not fail");
                if h.is_finished() {
                    Some(Progress::Finished)
                } else if disconnected {
                    Some(Progress::Error(Error::ChannelLost))
                } else {
                    None
                }
            };
            if let Some(msg) = msg {
                if let Err(e) = output.send(msg).await {
                    tracing::error!("export_subscription() fail to send message: {}", e);
                }
                continue;
            }
            state.handle = Some(handle);

            sleep(time::Duration::from_millis(100)).await;
        }
    })
}

pub async fn export_transactions(
    sender: Sender<Progress>,
    daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    path: PathBuf,
) {
    async move {
        let daemon = match daemon {
            Some(d) => d,
            None => {
                send_error!(sender, Error::DaemonMissing);
                return;
            }
        };
        let mut file = open_file_write!(path, sender);

        let header = "Date,Label,Value,Fee,Txid,Block\n".to_string();
        if let Err(e) = file.write_all(header.as_bytes()) {
            send_error!(sender, e.into());
            return;
        }

        // look 2 hour forward
        // https://github.com/bitcoin/bitcoin/blob/62bd61de110b057cbfd6e31e4d0b727d93119c72/src/chain.h#L29
        let mut end = ((Utc::now() + Duration::hours(2)).timestamp()) as u32;
        let total_txs = daemon.list_confirmed_txs(0, end, u32::MAX as u64).await;
        let total_txs = match total_txs {
            Ok(r) => r.transactions.len(),
            Err(e) => {
                send_error!(sender, e.into());
                return;
            }
        };

        if total_txs == 0 {
            send_progress!(sender, Ended);
        } else {
            send_progress!(sender, Progress(5.0));
        }

        let max = match daemon.backend() {
            DaemonBackend::RemoteBackend => DEFAULT_LIMIT as u64,
            _ => u32::MAX as u64,
        };

        // store txs in a map to avoid duplicates
        let mut map = HashMap::<Txid, HistoryTransaction>::new();
        let mut limit = max;

        loop {
            let history = daemon.list_history_txs(0, end, limit).await;
            let history_txs = match history {
                Ok(h) => h,
                Err(e) => {
                    send_error!(sender, e.into());
                    return;
                }
            };
            let dl = map.len() + history_txs.len();
            if dl > 0 {
                let progress = (dl as f32) / (total_txs as f32) * 80.0;
                send_progress!(sender, Progress(progress));
            }
            // all txs have been fetched
            if history_txs.is_empty() {
                break;
            }
            if history_txs.len() == limit as usize {
                let first = if let Some(t) = history_txs.first().expect("checked").time {
                    t
                } else {
                    send_error!(sender, TxTimeMissing);
                    return;
                };
                let last = if let Some(t) = history_txs.last().expect("checked").time {
                    t
                } else {
                    send_error!(sender, TxTimeMissing);
                    return;
                };
                // limit too low, all tx are in the same timestamp
                // we must increase limit and retry
                if first == last {
                    limit += DEFAULT_LIMIT as u64;
                    continue;
                } else {
                    // add txs to map
                    for tx in history_txs {
                        let txid = tx.txid;
                        map.insert(txid, tx);
                    }
                    limit = max;
                    end = first.min(last);
                    continue;
                }
            } else
            /* history_txs.len() < limit */
            {
                // add txs to map
                for tx in history_txs {
                    let txid = tx.txid;
                    map.insert(txid, tx);
                }
                break;
            }
        }

        let mut txs: Vec<_> = map.into_values().collect();
        txs.sort_by(|a, b| b.compare(a));

        for mut tx in txs {
            let date_time = tx
                .time
                .map(|t| {
                    let mut str = DateTime::from_timestamp(t as i64, 0)
                        .expect("bitcoin timestamp")
                        .to_rfc3339();
                    //str has the form `1996-12-19T16:39:57-08:00`
                    //                            ^        ^^^^^^
                    //          replace `T` by ` `|           | drop this part
                    str = str.replace("T", " ");
                    str[0..(str.len() - 6)].to_string()
                })
                .unwrap_or("".to_string());

            let txid = tx.txid.clone().to_string();
            let txid_label = tx.labels().get(&txid).cloned();
            let mut label = if let Some(txid) = txid_label {
                txid
            } else {
                "".to_string()
            };
            if !label.is_empty() {
                label = format!("\"{}\"", label);
            }
            let txid = tx.txid.to_string();
            let fee = tx.fee_amount.unwrap_or(Amount::ZERO).to_sat() as i128;
            let mut inputs_amount = 0;
            tx.coins.iter().for_each(|(_, coin)| {
                inputs_amount += coin.amount.to_sat() as i128;
            });
            let value = tx.incoming_amount.to_sat() as i128 - inputs_amount;
            let value = value as f64 / 100_000_000.0;
            let fee = fee as f64 / 100_000_000.0;
            let block = tx.height.map(|h| h.to_string()).unwrap_or("".to_string());
            let fee = if fee != 0.0 {
                fee.to_string()
            } else {
                "".into()
            };

            let line = format!(
                "{},{},{},{},{},{}\n",
                date_time, label, value, fee, txid, block
            );
            if let Err(e) = file.write_all(line.as_bytes()) {
                send_error!(sender, e.into());
                return;
            }
        }
        send_progress!(sender, Progress(100.0));
        send_progress!(sender, Ended);
    }
    .await;
}

pub fn export_descriptor(sender: Sender<Progress>, path: PathBuf, descriptor: LianaDescriptor) {
    let mut file = open_file_write!(path, sender);

    let descr_string = descriptor.to_string();
    if let Err(e) = file.write_all(descr_string.as_bytes()) {
        send_error!(sender, e.into());
        return;
    }
    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Ended);
}

pub fn export_string(sender: Sender<Progress>, path: PathBuf, psbt: String) {
    let mut file = open_file_write!(path, sender);

    if let Err(e) = file.write_all(psbt.as_bytes()) {
        send_error!(sender, e.into());
        return;
    }
    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Ended);
}

pub fn import_psbt(sender: Sender<Progress>, path: PathBuf) {
    let mut file = open_file_read!(path, sender);

    let mut psbt_str = String::new();
    if let Err(e) = file.read_to_string(&mut psbt_str) {
        send_error!(sender, e.into());
        return;
    }

    let psbt = match Psbt::from_str(&psbt_str) {
        Ok(psbt) => psbt,
        Err(_) => {
            send_error!(sender, Error::ParsePsbt);
            return;
        }
    };

    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Psbt(psbt));
}

pub fn import_descriptor(sender: Sender<Progress>, path: PathBuf) {
    let mut file = open_file_read!(path, sender);

    let mut descr_str = String::new();
    if let Err(e) = file.read_to_string(&mut descr_str) {
        send_error!(sender, e.into());
        return;
    }

    let descriptor = match LianaDescriptor::from_str(&descr_str) {
        Ok(psbt) => psbt,
        Err(_) => {
            send_error!(sender, Error::ParseDescriptor);
            return;
        }
    };

    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Descriptor(descriptor));
}

/// Import a backup in an already existing wallet:
///    - Load backup from file
///    - check if networks matches
///    - check if descriptors matches
///    - check if labels can be imported w/o conflict, if conflic ask user to ACK
///    - check if aliases can be imported w/o conflict, if conflict ask user to ACK
///    - update receive and change indexes
///    - parse psbt from backup
///    - import PSBTs
///    - import labels if no conflict or user ACK
///    - update aliases if no conflict or user ACK
pub async fn import_backup(
    sender: Sender<Progress>,
    path: PathBuf,
    daemon: Option<Arc<dyn Daemon + Sync + Send>>,
) {
    let daemon = match daemon {
        Some(d) => d,
        None => {
            send_error!(sender, Error::DaemonMissing);
            return;
        }
    };

    // TODO: drop after support for restore to liana-connect
    if matches!(daemon.backend(), DaemonBackend::RemoteBackend) {
        send_error!(
            sender,
            Error::BackupImport("Restore to a Liana-connect backend is not yet supported!".into())
        );
        return;
    }

    // Load backup from file
    let mut file = open_file_read!(path, sender);

    let mut backup_str = String::new();
    if let Err(e) = file.read_to_string(&mut backup_str) {
        send_error!(sender, e.into());
        return;
    }

    let backup: Result<Backup, _> = serde_json::from_str(&backup_str);
    let backup = match backup {
        Ok(psbt) => psbt,
        Err(e) => {
            send_error!(sender, Error::BackupImport(format!("{:?}", e)));
            return;
        }
    };

    // get backend info
    let info = match daemon.get_info().await {
        Ok(info) => info,
        Err(e) => {
            send_error!(sender, Error::Daemon(format!("{e:?}")));
            return;
        }
    };

    // check if networks matches
    let network = info.network;
    if backup.network != network {
        send_error!(
            sender,
            Error::BackupImport("The network of the backup don't match the wallet network!".into())
        );
        return;
    }

    // check if descriptors matches
    let descriptor = info.descriptors.main;
    let account = match backup.accounts.len() {
        0 => {
            send_error!(
                sender,
                Error::BackupImport("There is no account in the backup!".into())
            );
            return;
        }
        1 => backup.accounts.first().expect("already checked"),
        _ => {
            send_error!(
                sender,
                Error::BackupImport(
                    "Liana is actually not supporting import of backup with several accounts!"
                        .into()
                )
            );
            return;
        }
    };

    let backup_descriptor = match LianaDescriptor::from_str(&account.descriptor) {
        Ok(d) => d,
        Err(_) => {
            send_error!(
                sender,
                Error::BackupImport(
                    "The backup descriptor is not a valid Liana descriptor!".into()
                )
            );
            return;
        }
    };

    if backup_descriptor != descriptor {
        send_error!(
            sender,
            Error::BackupImport("The backup descriptor do not match this wallet!".into())
        );
        return;
    }

    // TODO: check if timestamp matches?

    // check if labels can be imported w/o conflict
    let mut write_labels = true;
    let backup_labels = if let Some(labels) = account.labels.clone() {
        let db_labels = match daemon.get_labels_bip329(0, u32::MAX).await {
            Ok(l) => l,
            Err(_) => {
                send_error!(sender, Error::BackupImport("Fail to dump DB labels".into()));
                return;
            }
        };

        let labels_map = db_labels.clone().into_map();
        let backup_labels_map = labels.clone().into_map();

        // if there is a conflict, we ask user to ACK before overwrite
        let (ack_sender, ack_receiver) = sync_channel(0);
        let mut conflict = false;
        for (k, l) in &backup_labels_map {
            if let Some(lab) = labels_map.get(k) {
                if lab != l {
                    send_progress!(sender, LabelsConflict(ack_sender));
                    conflict = true;
                    break;
                }
            }
        }
        if conflict {
            write_labels = match ack_receiver.recv() {
                Ok(b) => b,
                Err(_) => {
                    send_error!(
                        sender,
                        Error::BackupImport("Fail to receive labels ACK".into())
                    );
                    return;
                }
            }
        }

        labels.into_vec()
    } else {
        Vec::new()
    };

    let datadir = match daemon.config() {
        Some(c) => match &c.data_dir {
            Some(dd) => dd,
            None => {
                send_error!(
                    sender,
                    Error::BackupImport("Fail to get Daemon config".into())
                );
                return;
            }
        },
        None => {
            send_error!(
                sender,
                Error::BackupImport("Fail to get Daemon config".into())
            );
            return;
        }
    };

    // check if key aliases can be imported w/o conflict
    let mut write_aliases = true;
    let settings = if !account.keys.is_empty() {
        let settings = match Settings::from_file(datadir.to_path_buf(), network) {
            Ok(s) => s,
            Err(_) => {
                send_error!(
                    sender,
                    Error::BackupImport("Fail to get App Settings".into())
                );
                return;
            }
        };

        let settings_aliases: HashMap<_, _> = match settings.wallets.len() {
            1 => settings
                .wallets
                .first()
                .expect("already checked")
                .keys
                .clone()
                .into_iter()
                .map(|s| (s.master_fingerprint, s))
                .collect(),
            _ => {
                send_error!(
                    sender,
                    Error::BackupImport("Settings.wallets.len() is not 1".into())
                );
                return;
            }
        };

        let (ack_sender, ack_receiver) = sync_channel(0);
        let mut conflict = false;
        for (fg, key) in &account.keys {
            if let Some(k) = settings_aliases.get(fg) {
                let ks = k.to_backup();
                if ks != *key {
                    send_progress!(sender, KeyAliasesConflict(ack_sender));
                    conflict = true;
                    break;
                }
            }
        }
        if conflict {
            // wait for the user ACK/NACK
            write_aliases = match ack_receiver.recv() {
                Ok(a) => a,
                Err(_) => {
                    send_error!(
                        sender,
                        Error::BackupImport("Fail to receive aliases ACK".into())
                    );
                    return;
                }
            };
        }

        Some((settings, settings_aliases))
    } else {
        None
    };

    // update receive & change index
    let db_receive = info.receive_index;
    let i = account.receive_index.unwrap_or(0);
    let receive = if db_receive < i { Some(i) } else { None };

    let db_change = info.change_index;
    let i = account.change_index.unwrap_or(0);
    let change = if db_change < i { Some(i) } else { None };

    if daemon.update_deriv_indexes(receive, change).await.is_err() {
        send_error!(
            sender,
            Error::BackupImport("Fail to update derivation indexes".into())
        );
        return;
    }

    // parse PSBTs
    let mut psbts = Vec::new();
    for psbt_str in &account.psbts {
        match Psbt::from_str(psbt_str) {
            Ok(p) => {
                psbts.push(p);
            }
            Err(_) => {
                send_error!(sender, Error::BackupImport("Fail to parse PSBT".into()));
                return;
            }
        }
    }

    // import PSBTs
    for psbt in psbts {
        if daemon.update_spend_tx(&psbt).await.is_err() {
            send_error!(sender, Error::BackupImport("Fail to store PSBT".into()));
            return;
        }
    }

    // import labels if no conflict or user ACK
    if write_labels {
        let labels: HashMap<LabelItem, Option<String>> = backup_labels
            .into_iter()
            .filter_map(|l| {
                if let Some((item, label)) = LabelItem::from_bip329(&l, network) {
                    Some((item, Some(label)))
                } else {
                    None
                }
            })
            .collect();
        if daemon.update_labels(&labels).await.is_err() {
            send_error!(sender, Error::BackupImport("Fail to import labels".into()));
            return;
        }
    }

    // update aliases if no conflict or user ACK
    if let (true, Some((mut settings, mut settings_aliases))) = (write_aliases, settings) {
        for (k, v) in &account.keys {
            if let Some(ks) = KeySetting::from_backup(
                v.alias.clone().unwrap_or("".into()),
                *k,
                v.role.clone(),
                v.key_type.clone(),
                v.metadata.clone(),
            ) {
                settings_aliases.insert(*k, ks);
            }
        }

        settings.wallets.get_mut(0).expect("already checked").keys =
            settings_aliases.into_values().collect();
        if settings.to_file(datadir.to_path_buf(), network).is_err() {
            send_error!(
                sender,
                Error::BackupImport("Fail to import keys aliases".into())
            );
            return;
        }
    }

    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Ended);
}

pub async fn export_labels(
    sender: Sender<Progress>,
    daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    path: PathBuf,
) {
    let daemon = match daemon {
        Some(d) => d,
        None => {
            send_error!(sender, Error::DaemonMissing);
            return;
        }
    };
    let mut labels = Labels::new(Vec::new());
    let mut offset = 0u32;
    loop {
        let mut fetched = match daemon.get_labels_bip329(offset, DUMP_LABELS_LIMIT).await {
            Ok(l) => l,
            Err(e) => {
                send_error!(sender, e.into());
                return;
            }
        }
        .into_vec();
        let fetch_len = fetched.len() as u32;
        labels.append(&mut fetched);
        if fetch_len < DUMP_LABELS_LIMIT {
            break;
        } else {
            offset += DUMP_LABELS_LIMIT;
        }
    }
    let json = match labels.export() {
        Ok(j) => j,
        Err(e) => {
            send_error!(sender, e.into());
            return;
        }
    };
    let mut file = open_file_write!(path, sender);

    if let Err(e) = file.write_all(json.as_bytes()) {
        send_error!(sender, e.into());
        return;
    }
    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Ended);
}

pub async fn get_path(filename: String, write: bool) -> Option<PathBuf> {
    if write {
        rfd::AsyncFileDialog::new()
            .set_title("Choose a location to export...")
            .set_file_name(filename)
            .save_file()
            .await
            .map(|fh| fh.path().to_path_buf())
    } else {
        rfd::AsyncFileDialog::new()
            .set_title("Choose a file to import...")
            .set_file_name(filename)
            .pick_file()
            .await
            .map(|fh| fh.path().to_path_buf())
    }
}
