use std::{
    collections::HashMap,
    fmt::Display,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
    time,
};

use tokio::sync::mpsc::{channel, unbounded_channel, Sender, UnboundedReceiver, UnboundedSender};

use async_hwi::bitbox::api::btc::Fingerprint;
use chrono::{DateTime, Duration, Utc};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::{
        bitcoin::{Amount, Network, Psbt, Txid},
        DescriptorPublicKey,
    },
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
        cache::Cache,
        settings::{self, update_settings_file, KeySetting, WalletSettings},
        view,
        wallet::Wallet,
        Config,
    },
    backup::{self, Backup},
    daemon::{
        model::{HistoryTransaction, Labelled},
        Daemon, DaemonBackend, DaemonError,
    },
    dir::{LianaDirectory, NetworkDirectory},
    node::bitcoind::Bitcoind,
    services::connect::client::backend::api::DEFAULT_LIMIT,
};

const DUMP_LABELS_LIMIT: u32 = 100;

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

async fn open_file_write(path: &Path) -> Result<File, Error> {
    let dir = path.parent().ok_or(Error::NoParentDir)?;
    if !dir.exists() {
        fs::create_dir_all(dir)?;
    }
    let file = File::create(path)?;
    Ok(file)
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
    UpdateAliases(HashMap<Fingerprint, settings::KeySetting>),
    Xpub(String),
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
    ParseXpub,
    XpubNetwork,
    TxidNotMatch,
    InsanePsbt,
    OutpointNotOwned,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "ImportExport Io Error: {e}"),
            Error::HandleLost => write!(f, "ImportExport: subprocess handle lost"),
            Error::UnexpectedEnd => write!(f, "ImportExport: unexpected end of the process"),
            Error::JoinError(e) => write!(f, "ImportExport fail to handle.join(): {e} "),
            Error::ChannelLost => write!(f, "ImportExport: the channel have been closed"),
            Error::NoParentDir => write!(f, "ImportExport: there is no parent dir"),
            Error::Daemon(e) => write!(f, "ImportExport daemon error: {e}"),
            Error::TxTimeMissing => write!(f, "ImportExport: transaction block height missing"),
            Error::DaemonMissing => write!(f, "ImportExport: the daemon is missing"),
            Error::ParsePsbt => write!(f, "ImportExport: fail to parse PSBT"),
            Error::ParseDescriptor => write!(f, "ImportExport: fail to parse descriptor"),
            Error::Bip329Export(e) => write!(f, "Bip329Export: {e}"),
            Error::BackupImport(e) => write!(f, "BackupImport: {e}"),
            Error::Backup(e) => write!(f, "Backup: {e}"),
            Error::ParseXpub => write!(f, "Failed to parse Xpub from file"),
            Error::XpubNetwork => write!(f, "Xpub is for another network"),
            Error::TxidNotMatch => write!(f, "The imported PSBT txid doesn't match this PSBT"),
            Error::InsanePsbt => write!(f, "The Psbt is not sane"),
            Error::OutpointNotOwned => write!(
                f,
                "Import failed. The PSBT either doesn't belong to the wallet or has already been spent."
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ImportExportType {
    Transactions,
    ExportPsbt(String),
    ExportXpub(String),
    ExportBackup(String),
    ExportProcessBackup(LianaDirectory, Network, Arc<Config>, Arc<Wallet>),
    ImportBackup {
        network_dir: NetworkDirectory,
        wallet: Arc<Wallet>,
        overwrite_labels: Option<Sender<bool>>,
        overwrite_aliases: Option<Sender<bool>>,
    },
    WalletFromBackup,
    Descriptor(LianaDescriptor),
    ExportLabels,
    ImportPsbt(Option<Txid>),
    ImportXpub(Network),
    ImportDescriptor,
}

impl ImportExportType {
    pub fn end_message(&self) -> &str {
        match self {
            ImportExportType::Transactions
            | ImportExportType::ExportPsbt(_)
            | ImportExportType::ExportBackup(_)
            | ImportExportType::Descriptor(_)
            | ImportExportType::ExportProcessBackup(..)
            | ImportExportType::ExportXpub(_)
            | ImportExportType::ExportLabels => "Export successful!",
            ImportExportType::ImportBackup { .. }
            | ImportExportType::ImportPsbt(_)
            | ImportExportType::ImportXpub(_)
            | ImportExportType::WalletFromBackup
            | ImportExportType::ImportDescriptor => "Import successful",
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
    Xpub(String),
    LabelsConflict(Sender<bool>),
    KeyAliasesConflict(Sender<bool>),
    UpdateAliases(HashMap<Fingerprint, settings::KeySetting>),
    WalletFromBackup(
        (
            LianaDescriptor,
            Network,
            HashMap<Fingerprint, settings::KeySetting>,
            Backup,
        ),
    ),
}

pub struct Export {
    pub receiver: UnboundedReceiver<Progress>,
    pub sender: Option<UnboundedSender<Progress>>,
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
        let (sender, receiver) = unbounded_channel();
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
        sender: UnboundedSender<Progress>,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        path: PathBuf,
    ) {
        if let Err(e) = match export_type {
            ImportExportType::Transactions => export_transactions(&sender, daemon, path).await,
            ImportExportType::ExportPsbt(str) => export_string(&sender, path, str).await,
            ImportExportType::Descriptor(descriptor) => {
                export_descriptor(&sender, path, descriptor).await
            }
            ImportExportType::ExportLabels => export_labels(&sender, daemon, path).await,
            ImportExportType::ImportPsbt(txid) => import_psbt(daemon, &sender, path, txid).await,
            ImportExportType::ImportXpub(network) => import_xpub(&sender, path, network).await,
            ImportExportType::ImportDescriptor => import_descriptor(&sender, path).await,
            ImportExportType::ExportBackup(str) => export_string(&sender, path, str).await,
            ImportExportType::ExportXpub(xpub_str) => export_string(&sender, path, xpub_str).await,
            ImportExportType::ExportProcessBackup(datadir, network, config, wallet) => {
                app_backup_export(
                    datadir,
                    network,
                    config,
                    wallet,
                    daemon.clone().expect("cannot fail"),
                    path,
                    &sender,
                )
                .await
            }
            ImportExportType::ImportBackup {
                network_dir,
                wallet,
                ..
            } => import_backup(&network_dir, wallet, &sender, path, daemon).await,
            ImportExportType::WalletFromBackup => wallet_from_backup(&sender, path).await,
        } {
            if let Err(e) = sender.send(Progress::Error(e)) {
                tracing::error!("Import/Export fail to send msg: {}", e);
            }
        }
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
                    tokio::sync::mpsc::error::TryRecvError::Empty => false,
                    tokio::sync::mpsc::error::TryRecvError::Disconnected => true,
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
    sender: &UnboundedSender<Progress>,
    daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    path: PathBuf,
) -> Result<(), Error> {
    let daemon = daemon.ok_or(Error::DaemonMissing)?;
    let mut file = open_file_write(&path).await?;

    let header = "Date,Label,Value,Fee,Txid,Block\n".to_string();
    file.write_all(header.as_bytes())?;

    // look 2 hour forward
    // https://github.com/bitcoin/bitcoin/blob/62bd61de110b057cbfd6e31e4d0b727d93119c72/src/chain.h#L29
    let mut end = ((Utc::now() + Duration::hours(2)).timestamp()) as u32;
    let total_txs = daemon
        .list_confirmed_txs(0, end, u32::MAX as u64)
        .await?
        .transactions
        .len();

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
        let history_txs = daemon.list_history_txs(0, end, limit).await?;
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
                return Err(Error::TxTimeMissing);
            };
            let last = if let Some(t) = history_txs.last().expect("checked").time {
                t
            } else {
                return Err(Error::TxTimeMissing);
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
        file.write_all(line.as_bytes())?;
    }
    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Ended);
    Ok(())
}

pub async fn export_descriptor(
    sender: &UnboundedSender<Progress>,
    path: PathBuf,
    descriptor: LianaDescriptor,
) -> Result<(), Error> {
    let mut file = open_file_write(&path).await?;

    let descr_string = descriptor.to_string();
    file.write_all(descr_string.as_bytes())?;
    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Ended);

    Ok(())
}

pub async fn export_string(
    sender: &UnboundedSender<Progress>,
    path: PathBuf,
    str: String,
) -> Result<(), Error> {
    let mut file = open_file_write(&path).await?;
    file.write_all(str.as_bytes())?;
    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Ended);
    Ok(())
}

pub async fn import_psbt(
    daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    sender: &UnboundedSender<Progress>,
    path: PathBuf,
    txid: Option<Txid>,
) -> Result<(), Error> {
    let mut file = File::open(&path)?;
    let daemon = daemon.ok_or(Error::DaemonMissing)?;

    let descr = daemon.get_info().await?.descriptors.main;

    let mut psbt_str = String::new();
    file.read_to_string(&mut psbt_str)?;
    psbt_str = psbt_str.trim().to_string();

    let psbt = Psbt::from_str(&psbt_str).map_err(|_| Error::ParsePsbt)?;
    send_progress!(sender, Progress(50.0));
    descr
        .partial_spend_info(&psbt)
        .map_err(|_| Error::InsanePsbt)?;

    if let Some(txid) = &txid {
        if psbt.unsigned_tx.compute_txid() != *txid {
            return Err(Error::TxidNotMatch);
        }
    }

    let e = daemon.update_spend_tx(&psbt).await;
    if let (None, Err(error)) = (txid, &e) {
        if let DaemonError::Unexpected(e) = error {
            if e.contains("Unknown outpoint") {
                return Err(Error::OutpointNotOwned);
            } else {
                return Err(Error::Daemon(error.to_string()));
            }
        }
    } else {
        e?;
    }
    send_progress!(sender, Psbt(psbt));

    send_progress!(sender, Progress(100.0));
    Ok(())
}

pub async fn import_descriptor(
    sender: &UnboundedSender<Progress>,
    path: PathBuf,
) -> Result<(), Error> {
    let mut file = File::open(path)?;

    let mut descr_str = String::new();
    file.read_to_string(&mut descr_str)?;
    let descr_str = descr_str.trim();

    let descriptor = LianaDescriptor::from_str(descr_str).map_err(|_| Error::ParseDescriptor)?;

    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Descriptor(descriptor));
    Ok(())
}

pub async fn import_xpub(
    sender: &UnboundedSender<Progress>,
    path: PathBuf,
    network: Network,
) -> Result<(), Error> {
    let mut file = File::open(path)?;

    let mut xpub_str = String::new();
    file.read_to_string(&mut xpub_str)?;
    let xpub_str = xpub_str.trim().to_string();

    let (descriptor_pubkey, key) =
        if let Some(DescriptorPublicKey::XPub(key)) = parse_raw_xpub(&xpub_str) {
            (DescriptorPublicKey::XPub(key.clone()), key)
        } else if let Some(DescriptorPublicKey::XPub(key)) = parse_coldcard_xpub(&xpub_str) {
            (DescriptorPublicKey::XPub(key.clone()), key)
        } else {
            return Err(Error::ParseXpub);
        };
    let xpub_str = descriptor_pubkey.to_string();

    let valid = if network == Network::Bitcoin {
        key.xkey.network == Network::Bitcoin.into()
    } else {
        key.xkey.network == Network::Testnet.into()
    };
    if valid {
        send_progress!(sender, Progress(100.0));
        send_progress!(sender, Xpub(xpub_str));
    } else {
        return Err(Error::XpubNetwork);
    }

    Ok(())
}

pub fn parse_raw_xpub(raw_xpub: &str) -> Option<DescriptorPublicKey> {
    DescriptorPublicKey::from_str(raw_xpub).ok()
}

pub fn parse_coldcard_xpub(coldcard_xpub: &str) -> Option<DescriptorPublicKey> {
    if let serde_json::Value::Object(map) = serde_json::from_str(coldcard_xpub).ok()? {
        let fg = map.get("xfp")?.to_string().to_lowercase();
        let fg = fg.replace("\"", "");
        if let serde_json::Value::Object(bip48) = map.get("bip48_2")? {
            let deriv = bip48.get("deriv")?.to_string();
            let deriv = deriv.replace("\"", "");
            let deriv = deriv.replace("m", "");
            let xpub = bip48.get("xpub")?.to_string();
            let xpub = xpub.replace("\"", "");
            let raw_xpub = format!("[{fg}{deriv}]{xpub}");
            return parse_raw_xpub(&raw_xpub);
        }
    }
    None
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
    network_dir: &NetworkDirectory,
    wallet: Arc<Wallet>,
    sender: &UnboundedSender<Progress>,
    path: PathBuf,
    daemon: Option<Arc<dyn Daemon + Sync + Send>>,
) -> Result<(), Error> {
    let daemon = daemon.ok_or(Error::DaemonMissing)?;

    // TODO: drop after support for restore to liana-connect
    if matches!(daemon.backend(), DaemonBackend::RemoteBackend) {
        return Err(Error::BackupImport(
            "Restore to a Liana-connect backend is not yet supported!".into(),
        ));
    }

    // Load backup from file
    let mut file = File::open(&path)?;

    let mut backup_str = String::new();
    file.read_to_string(&mut backup_str)?;
    backup_str = backup_str.trim().to_string();

    let backup: Result<Backup, _> = serde_json::from_str(&backup_str);
    let backup = match backup {
        Ok(psbt) => psbt,
        Err(e) => {
            return Err(Error::BackupImport(format!("{:?}", e)));
        }
    };

    // get backend info
    let info = match daemon.get_info().await {
        Ok(info) => info,
        Err(e) => {
            return Err(Error::Daemon(format!("{e:?}")));
        }
    };

    // check if networks matches
    let network = info.network;
    if backup.network != network {
        return Err(Error::BackupImport(
            "The network of the backup don't match the wallet network!".into(),
        ));
    }

    // check if descriptors matches
    let descriptor = info.descriptors.main;
    let account = match backup.accounts.len() {
        0 => {
            return Err(Error::BackupImport(
                "There is no account in the backup!".into(),
            ));
        }
        1 => backup.accounts.first().expect("already checked"),
        _ => {
            return Err(Error::BackupImport(
                "Liana is actually not supporting import of backup with several accounts!".into(),
            ));
        }
    };

    let backup_descriptor = match LianaDescriptor::from_str(&account.descriptor) {
        Ok(d) => d,
        Err(_) => {
            return Err(Error::BackupImport(
                "The backup descriptor is not a valid Liana descriptor!".into(),
            ));
        }
    };

    if backup_descriptor != descriptor {
        return Err(Error::BackupImport(
            "The backup descriptor do not match this wallet!".into(),
        ));
    }

    // TODO: check if timestamp matches?

    // check if labels can be imported w/o conflict
    let mut write_labels = true;
    let backup_labels = if let Some(labels) = account.labels.clone() {
        let db_labels = match daemon.get_labels_bip329(0, u32::MAX).await {
            Ok(l) => l,
            Err(_) => {
                return Err(Error::BackupImport("Failed to dump DB labels".into()));
            }
        };

        let labels_map = db_labels.clone().into_map();
        let backup_labels_map = labels.clone().into_map();

        // if there is a conflict, we ask user to ACK before overwrite
        let (ack_sender, mut ack_receiver) = channel(1);
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
            write_labels = match ack_receiver.recv().await {
                Some(b) => b,
                None => {
                    return Err(Error::BackupImport("Failed to receive labels ACK".into()));
                }
            }
        }

        labels.into_vec()
    } else {
        Vec::new()
    };

    // check if key aliases can be imported w/o conflict
    let mut write_aliases = true;
    let settings = if !account.keys.is_empty() {
        let wallet_settings =
            match WalletSettings::from_file(network_dir, |w| w.wallet_id() == wallet.id()) {
                Ok(Some(s)) => s,
                _ => {
                    return Err(Error::BackupImport("Failed to get App Settings".into()));
                }
            };

        let settings_aliases: HashMap<_, _> = wallet_settings
            .keys
            .clone()
            .into_iter()
            .map(|s| (s.master_fingerprint, s))
            .collect();

        let (ack_sender, mut ack_receiver) = channel(1);
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
            write_aliases = match ack_receiver.recv().await {
                Some(a) => a,
                None => {
                    return Err(Error::BackupImport("Failed to receive aliases ACK".into()));
                }
            };
        }

        Some(settings_aliases)
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
        return Err(Error::BackupImport(
            "Failed to update derivation indexes".into(),
        ));
    }

    // parse PSBTs
    let mut psbts = Vec::new();
    for psbt_str in &account.psbts {
        match Psbt::from_str(psbt_str) {
            Ok(p) => {
                psbts.push(p);
            }
            Err(_) => {
                return Err(Error::BackupImport("Failed to parse PSBT".into()));
            }
        }
    }

    // import PSBTs
    for psbt in psbts {
        if daemon.update_spend_tx(&psbt).await.is_err() {
            return Err(Error::BackupImport("Failed to store PSBT".into()));
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
            return Err(Error::BackupImport("Failed to import labels".into()));
        }
    }

    // update aliases if no conflict or user ACK
    if let (true, Some(mut settings_aliases)) = (write_aliases, settings) {
        for (k, v) in &account.keys {
            if let Some(ks) = KeySetting::from_backup(
                v.alias.clone().unwrap_or("".into()),
                *k,
                v.role,
                v.key_type,
                v.proprietary.clone(),
            ) {
                settings_aliases.insert(*k, ks);
            }
        }

        if let Err(e) = update_settings_file(network_dir, |mut settings| {
            if let Some(wallet) = settings
                .wallets
                .iter_mut()
                .find(|w| w.wallet_id() == wallet.id())
            {
                wallet.keys = settings_aliases.clone().into_values().collect();
            }
            settings
        })
        .await
        {
            return Err(Error::BackupImport(format!(
                "Failed to import keys aliases: {}",
                e
            )));
        } else {
            // Update wallet state
            send_progress!(sender, UpdateAliases(settings_aliases));
        }
    }

    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Ended);
    Ok(())
}

#[derive(Debug)]
pub enum RestoreBackupError {
    Daemon(DaemonError),
    Network,
    InvalidDescriptor,
    WrongDescriptor,
    NoAccount,
    SeveralAccounts,
    LianaConnectNotSupported,
    GetLabels,
    LabelsNotEmpty,
    InvalidPsbt,
}

impl Display for RestoreBackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestoreBackupError::Daemon(e) => write!(f, "Daemon error during restore process: {e}"),
            RestoreBackupError::Network => write!(f, "Backup & wallet network don't matches"),
            RestoreBackupError::InvalidDescriptor => write!(f, "The backup descriptor is invalid"),
            RestoreBackupError::WrongDescriptor => {
                write!(f, "Backup & wallet descriptor don't matches")
            }
            RestoreBackupError::NoAccount => write!(f, "There is no account in the backup"),
            RestoreBackupError::SeveralAccounts => {
                write!(f, "There is several accounts in the backup")
            }
            RestoreBackupError::LianaConnectNotSupported => {
                write!(f, "Restore a backup to Liana-connect is not yet supported")
            }
            RestoreBackupError::GetLabels => write!(f, "Fails to get labels during backup restore"),
            RestoreBackupError::LabelsNotEmpty => write!(
                f,
                "Cannot load labels: there is already labels into the database"
            ),
            RestoreBackupError::InvalidPsbt => write!(f, "Psbt is invalid"),
        }
    }
}

impl From<DaemonError> for RestoreBackupError {
    fn from(value: DaemonError) -> Self {
        Self::Daemon(value)
    }
}

/// Create a wallet from a backup
///    - load backup from file
///    - extract descriptor
///    - extract network
///    - extract aliases
pub async fn wallet_from_backup(
    sender: &UnboundedSender<Progress>,
    path: PathBuf,
) -> Result<(), Error> {
    // Load backup from file
    let mut file = File::open(path)?;

    let mut backup_str = String::new();
    file.read_to_string(&mut backup_str)?;
    backup_str = backup_str.trim().to_string();

    let backup: Result<Backup, _> = serde_json::from_str(&backup_str);
    let backup = match backup {
        Ok(psbt) => psbt,
        Err(e) => {
            return Err(Error::BackupImport(format!("{:?}", e)));
        }
    };

    let network = backup.network;

    let account = match backup.accounts.len() {
        0 => {
            return Err(Error::BackupImport(
                "There is no account in the backup!".into(),
            ));
        }
        1 => backup.accounts.first().expect("already checked"),
        _ => {
            return Err(Error::BackupImport(
                "Liana is actually not supporting import of backup with several accounts!".into(),
            ));
        }
    };

    let descriptor = match LianaDescriptor::from_str(&account.descriptor) {
        Ok(d) => d,
        Err(_) => {
            return Err(Error::BackupImport(
                "The backup descriptor is not a valid Liana descriptor!".into(),
            ));
        }
    };

    let mut aliases: HashMap<Fingerprint, settings::KeySetting> = HashMap::new();
    for (k, v) in &account.keys {
        if let Some(ks) = KeySetting::from_backup(
            v.alias.clone().unwrap_or("".into()),
            *k,
            v.role,
            v.key_type,
            v.proprietary.clone(),
        ) {
            aliases.insert(*k, ks);
        }
    }

    send_progress!(
        sender,
        WalletFromBackup((descriptor, network, aliases, backup))
    );
    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Ended);
    Ok(())
}

#[allow(unused)]
/// Import backup data if wallet created from a backup
///    - check if networks matches
///    - check if descriptors matches
///    - check if labels are empty
///    - update receive and change indexes
///    - parse psbt from backup
///    - import PSBTs
///    - import labels
pub async fn import_backup_at_launch(
    cache: Cache,
    wallet: Arc<Wallet>,
    config: Config,
    daemon: Arc<dyn Daemon + Sync + Send>,
    datadir: LianaDirectory,
    internal_bitcoind: Option<Bitcoind>,
    backup: Backup,
) -> Result<
    (
        Cache,
        Arc<Wallet>,
        Config,
        Arc<dyn Daemon + Sync + Send>,
        LianaDirectory,
        Option<Bitcoind>,
    ),
    RestoreBackupError,
> {
    // TODO: drop after support for restore to liana-connect
    if matches!(daemon.backend(), DaemonBackend::RemoteBackend) {
        return Err(RestoreBackupError::LianaConnectNotSupported);
    }

    // get backend info
    let info = daemon.get_info().await?;

    // check if networks matches
    let network = info.network;
    if backup.network != network {
        return Err(RestoreBackupError::Network);
    }

    // check if descriptors matches
    let descriptor = info.descriptors.main;
    let account = match backup.accounts.len() {
        0 => return Err(RestoreBackupError::NoAccount),
        1 => backup.accounts.first().expect("already checked"),
        _ => return Err(RestoreBackupError::SeveralAccounts),
    };

    let backup_descriptor = LianaDescriptor::from_str(&account.descriptor)
        .map_err(|_| RestoreBackupError::InvalidDescriptor)?;

    if backup_descriptor != descriptor {
        return Err(RestoreBackupError::WrongDescriptor);
    }

    // check there is no labels in DB
    if account.labels.is_some()
        && !daemon
            .get_labels_bip329(0, u32::MAX)
            .await
            .map_err(|_| RestoreBackupError::GetLabels)?
            .to_vec()
            .is_empty()
    {
        return Err(RestoreBackupError::LabelsNotEmpty);
    }

    // parse PSBTs
    let mut psbts = Vec::new();
    for psbt_str in &account.psbts {
        psbts.push(Psbt::from_str(psbt_str).map_err(|_| RestoreBackupError::InvalidPsbt)?);
    }

    // update receive & change index
    let db_receive = info.receive_index;
    let i = account.receive_index.unwrap_or(0);
    let receive = if db_receive < i { Some(i) } else { None };

    let db_change = info.change_index;
    let i = account.change_index.unwrap_or(0);
    let change = if db_change < i { Some(i) } else { None };

    daemon.update_deriv_indexes(receive, change).await?;

    // import labels
    if let Some(labels) = account.labels.clone().map(|l| l.into_vec()) {
        let labels: HashMap<LabelItem, Option<String>> = labels
            .into_iter()
            .filter_map(|l| {
                if let Some((item, label)) = LabelItem::from_bip329(&l, network) {
                    Some((item, Some(label)))
                } else {
                    None
                }
            })
            .collect();
        daemon.update_labels(&labels).await?;
    }

    // import PSBTs
    for psbt in psbts {
        if let Err(e) = daemon.update_spend_tx(&psbt).await {
            tracing::error!("Failed to restore PSBT: {e}")
        }
    }

    Ok((cache, wallet, config, daemon, datadir, internal_bitcoind))
}

pub async fn export_labels(
    sender: &UnboundedSender<Progress>,
    daemon: Option<Arc<dyn Daemon + Sync + Send>>,
    path: PathBuf,
) -> Result<(), Error> {
    let daemon = daemon.ok_or(Error::DaemonMissing)?;
    let mut labels = Labels::new(Vec::new());
    let mut offset = 0u32;
    loop {
        let mut fetched = daemon
            .get_labels_bip329(offset, DUMP_LABELS_LIMIT)
            .await?
            .into_vec();
        let fetch_len = fetched.len() as u32;
        labels.append(&mut fetched);
        if fetch_len < DUMP_LABELS_LIMIT {
            break;
        } else {
            offset += DUMP_LABELS_LIMIT;
        }
    }
    let json = labels.export()?;
    let mut file = open_file_write(&path).await?;

    file.write_all(json.as_bytes())?;
    send_progress!(sender, Progress(100.0));
    send_progress!(sender, Ended);
    Ok(())
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

pub async fn app_backup(
    datadir: LianaDirectory,
    network: Network,
    config: Arc<Config>,
    wallet: Arc<Wallet>,
    daemon: Arc<dyn Daemon + Sync + Send>,
    sender: &UnboundedSender<Progress>,
) -> Result<String, backup::Error> {
    let backup = Backup::from_app(datadir, network, config, wallet, daemon, sender).await?;
    serde_json::to_string_pretty(&backup).map_err(|_| backup::Error::Json)
}

pub async fn app_backup_export(
    datadir: LianaDirectory,
    network: Network,
    config: Arc<Config>,
    wallet: Arc<Wallet>,
    daemon: Arc<dyn Daemon + Sync + Send>,
    path: PathBuf,
    sender: &UnboundedSender<Progress>,
) -> Result<(), Error> {
    let backup = app_backup(datadir.clone(), network, config, wallet, daemon, sender)
        .await
        .map_err(Error::Backup)?;
    export_string(sender, path, backup).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_coldcard_xpub() {
        let raw = r#"
            {
              "chain": "XTN",
              "xfp": "C658B283",
              "account": 3,
              "xpub": "tpubD6NzVbkrYhZ4XHQ1pLJ7pdpEGWCVbSUEaUakxnrtENzaZaDp4vL6gBgGH7n983ZPgsVe5G2JEAM2oYZkEPCNrfo9XLq8nHFhp9GzFjGc1uQ",
              "bip44": {
                "name": "p2pkh",
                "xfp": "F623F3D0",
                "deriv": "m/44h/1h/3h",
                "xpub": "tpubDCrmGPwVjNJsUDsh7pxsWgTZ1sjFZtnPPhpgCxM3yXg6RXjfDQ73g6mX6H2Hn69j5S5MJnhEr7mSvTzaz7qcrYzyZK7Aw836Qwkj1brgDh8",
                "desc": "pkh([c658b283/44h/1h/3h]tpubDCrmGPwVjNJsUDsh7pxsWgTZ1sjFZtnPPhpgCxM3yXg6RXjfDQ73g6mX6H2Hn69j5S5MJnhEr7mSvTzaz7qcrYzyZK7Aw836Qwkj1brgDh8/<0;1>/*)#2w5s7qf5",
                "first": "miu3fgGrAZqtPb6iWwUD1pi7MRqWnkVTeG"
              },
              "bip49": {
                "name": "p2sh-p2wpkh",
                "xfp": "1226F685",
                "deriv": "m/49h/1h/3h",
                "xpub": "tpubDDMPh7VRRQ7waHhLDskFU63ZY4Pdue8vPLhim4Nf34nX8KpFZ4yPt5wDtwuQQ79jn7AvpGBCVreVdhPvJCqCSi5zznCwZ61YYnLdGBmn3As",
                "desc": "sh(wpkh([c658b283/49h/1h/3h]tpubDDMPh7VRRQ7waHhLDskFU63ZY4Pdue8vPLhim4Nf34nX8KpFZ4yPt5wDtwuQQ79jn7AvpGBCVreVdhPvJCqCSi5zznCwZ61YYnLdGBmn3As/<0;1>/*))#3c4vfpj8",
                "_pub": "upub5EUyFsezG5Y3kbw8GcQHduRgh2re6PfN6NYm4KdrZ8tzDjhrisdcsTHybHwrstEjmHHbxFseamkoHf4ckFjvAZauLANN7ptr9eZHLRHAtJz",
                "first": "2MueqD2UoZZ566mLqTVCVT5Dm7YMbVBwPeq"
              },
              "bip84": {
                "name": "p2wpkh",
                "xfp": "B74B1EF5",
                "deriv": "m/84h/1h/3h",
                "xpub": "tpubDDKQtgKtTeTVebMfJ6RJ6vL7UMnDjhUfK7scrYiWGMWy8htipN9dCkuHqx9PmJoAUoydwsc9TEoj3A1C1FbPqzxKth8qfn7axA5qHc5YbJz",
                "desc": "wpkh([c658b283/84h/1h/3h]tpubDDKQtgKtTeTVebMfJ6RJ6vL7UMnDjhUfK7scrYiWGMWy8htipN9dCkuHqx9PmJoAUoydwsc9TEoj3A1C1FbPqzxKth8qfn7axA5qHc5YbJz/<0;1>/*)#0ac7rpv0",
                "_pub": "vpub5ZHFm7ANT1R5gCnaBBrxUpojoJPfs4zbwGEswCsbAS1KHDbZEpyQpBvBZW9SEzY5sdD7qLu9zpGaaQHTAzv8N68q6QzgpRpNpkN8kStaFVA",
                "first": "tb1qt8l4mel8c8epzcrqchmrsdsv6e8n0chkynuxzz"
              },
              "bip86": {
                "name": "p2tr",
                "xfp": "99B6CEE8",
                "deriv": "m/86h/1h/3h",
                "xpub": "tpubDDNzAa2tRWaaiDVf6qnzMYjELyz68DrBzGW6PtsZkWz3tU4QZLUhB9TSxxT4KF4sXncg856etJ1rDM2XHibm21uCxQtLjMd4aR9EXydtEpY",
                "desc": "tr([c658b283/86h/1h/3h]tpubDDNzAa2tRWaaiDVf6qnzMYjELyz68DrBzGW6PtsZkWz3tU4QZLUhB9TSxxT4KF4sXncg856etJ1rDM2XHibm21uCxQtLjMd4aR9EXydtEpY/<0;1>/*)#ggndxtk6",
                "first": "tb1pcawjnx5krtffagyzvcmqz40z3hds3nycc2vtjj9ngy8hskk5zwzsfh2a3w"
              },
              "bip48_1": {
                "name": "p2sh-p2wsh",
                "xfp": "141AB091",
                "deriv": "m/48h/1h/3h/1h",
                "xpub": "tpubDFmeRMxr4X7dtY9C9H6gAnVBfzBfjrmJ961ST2STHfQQwrHWMtEU1Zgr2PUfQQL4q9uywHxDJcffmRRpL58RJeSuaVs5CYzrcBrMoobyVRH",
                "desc": "sh(wsh(sortedmulti(M,[c658b283/48h/1h/3h/1h]tpubDFmeRMxr4X7dtY9C9H6gAnVBfzBfjrmJ961ST2STHfQQwrHWMtEU1Zgr2PUfQQL4q9uywHxDJcffmRRpL58RJeSuaVs5CYzrcBrMoobyVRH/0/*,...)))",
                "_pub": "Upub5ToK7MrrUA67VRYN8gDhAgD7Ykgw8xyLAPW9fYyCBWMHfSk2J6Gy63uXXSUbScdy3o6dwsenGkAUYYiH5MC6Az4UkM8uAhMA6nLtTzhps22"
              },
              "bip48_2": {
                "name": "p2wsh",
                "xfp": "88AD98C4",
                "deriv": "m/48h/1h/3h/2h",
                "xpub": "tpubDFmeRMxr4X7dxNKxxBKWXu1rskHEQYB8vY5PYPmiB74EjyrE814HHpQzh2XEFpm3z5uJpk7Cjt2hmhcMYmBbot6CmRHn3CKK2K6vzLPBMbH",
                "desc": "wsh(sortedmulti(M,[c658b283/48h/1h/3h/2h]tpubDFmeRMxr4X7dxNKxxBKWXu1rskHEQYB8vY5PYPmiB74EjyrE814HHpQzh2XEFpm3z5uJpk7Cjt2hmhcMYmBbot6CmRHn3CKK2K6vzLPBMbH/0/*,...))",
                "_pub": "Vpub5ndaR2XmcqdbQYvFmwE9jsqHvUvwkGNfrx6KYKCLSxNzWg7yJsGLzNHpDHUkHwiscNCmaoQLAft4S7WP1jfHUTPNocG2bFV6ndf736mPM9R"
              },
              "bip48_3": {
                "name": "p2tr",
                "xfp": "C3F84B2C",
                "deriv": "m/48h/1h/3h/3h",
                "xpub": "tpubDFmeRMxr4X7e1LErDJWLDRjrHGfirhnFxk3aZoFe8tjMHUyPjh1mATfLfyC6VKUfuS4uBEhMuow7kQWfNqA7U2uHz7fyT9S6V49MLQmyzjm",
                "desc": "tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,sortedmulti_a(M,[c658b283/48h/1h/3h/3h]tpubDFmeRMxr4X7e1LErDJWLDRjrHGfirhnFxk3aZoFe8tjMHUyPjh1mATfLfyC6VKUfuS4uBEhMuow7kQWfNqA7U2uHz7fyT9S6V49MLQmyzjm/0/*,...))"
              }
            }
        "#;
        let expected = "[c658b283/48'/1'/3'/2']tpubDFmeRMxr4X7dxNKxxBKWXu1rskHEQYB8vY5PYPmiB74EjyrE814HHpQzh2XEFpm3z5uJpk7Cjt2hmhcMYmBbot6CmRHn3CKK2K6vzLPBMbH".to_string();
        assert_eq!(expected, parse_coldcard_xpub(raw).unwrap().to_string());
    }
}
