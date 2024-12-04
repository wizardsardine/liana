use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc, Mutex,
    },
    time,
};

use chrono::{DateTime, Duration, Utc};
use liana::miniscript::bitcoin::{Amount, Denomination::Bitcoin};
use lianad::commands::LabelItem;
use tokio::{
    runtime::Runtime,
    task::{JoinError, JoinHandle},
    time::sleep,
};

use crate::daemon::{model::Labelled, Daemon, DaemonError};

macro_rules! send_error {
    ($sender:ident, $error:ident) => {
        if let Err(e) = $sender.send(ExportProgress::Error(Error::$error)) {
            tracing::error!("ExportState::start() fail to send msg: {}", e);
        }
    };
    ($sender:ident, $error:expr) => {
        if let Err(e) = $sender.send(ExportProgress::Error($error)) {
            tracing::error!("ExportState::start() fail to send msg: {}", e);
        }
    };
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

#[derive(Debug)]
pub enum State {
    Init,
    Running,
    Stopped,
}

#[derive(Debug, Clone)]
pub enum ExportProgress {
    Started(Arc<Mutex<JoinHandle<()>>>),
    Progress(f32),
    Ended(Box<PathBuf>),
    Finnished,
    Error(Error),
    None,
}

#[derive(Debug)]
pub enum ExportType {
    Transactions,
}

pub struct ExportState {
    pub receiver: Receiver<ExportProgress>,
    pub sender: Option<Sender<ExportProgress>>,
    pub handle: Option<Arc<Mutex<JoinHandle<()>>>>,
    pub daemon: Arc<dyn Daemon + Sync + Send>,
    pub export_type: ExportType,
    pub path: Box<PathBuf>,
}

impl ExportState {
    pub fn new(
        daemon: Arc<dyn Daemon + Sync + Send>,
        export_type: ExportType,
        path: Box<PathBuf>,
    ) -> Self {
        let (sender, receiver) = channel();
        ExportState {
            receiver,
            sender: Some(sender),
            handle: None,
            daemon,
            export_type,
            path,
        }
    }

    pub async fn start(&mut self) {
        if let (true, Some(sender)) = (self.handle.is_none(), self.sender.take()) {
            let daemon = self.daemon.clone();
            let path = self.path.clone();
            let function = self.function();

            let cloned_sender = sender.clone();
            let handle = tokio::spawn(async move {
                function(sender, daemon, path);
            });
            let handle = Arc::new(Mutex::new(handle));

            // we send the handle to the GUI so we can kill the thread on timeout
            // or user cancel action
            if let Err(e) = cloned_sender.send(ExportProgress::Started(handle.clone())) {
                tracing::error!("ExportState::start fail to send msg: {}", e);
            }
            self.handle = Some(handle);
        } else {
            tracing::error!("ExportState can start only once!");
        }
    }
    pub fn state(&self) -> State {
        match (&self.sender, &self.handle) {
            (Some(_), None) => State::Init,
            (None, Some(_)) => State::Running,
            (None, None) => State::Stopped,
            _ => unreachable!(),
        }
    }

    pub fn function(
        &self,
    ) -> impl Fn(Sender<ExportProgress>, Arc<dyn Daemon + Sync + Send>, Box<PathBuf>) {
        match self.export_type {
            ExportType::Transactions => export_transactions,
        }
    }
}

pub async fn export_subscription(mut state: ExportState) -> (ExportProgress, ExportState) {
    match state.state() {
        State::Init => {
            state.start().await;
        }
        State::Stopped => {
            sleep(time::Duration::from_millis(1000)).await;
            return (ExportProgress::None, state);
        }
        State::Running => { /* continue */ }
    }
    let msg = state.receiver.try_recv();
    let disconnected = match msg {
        Ok(m) => return (m, state),
        Err(e) => match e {
            std::sync::mpsc::TryRecvError::Empty => false,
            std::sync::mpsc::TryRecvError::Disconnected => true,
        },
    };

    let handle = match state.handle.take() {
        Some(h) => h,
        None => return (ExportProgress::Error(Error::HandleLost), state),
    };
    {
        let h = handle.lock().expect("should not fail");
        if h.is_finished() {
            return (ExportProgress::Finnished, state);
        } else if disconnected {
            return (ExportProgress::Error(Error::ChannelLost), state);
        }
    } // => release handle lock
    state.handle = Some(handle);

    sleep(time::Duration::from_millis(100)).await;
    (ExportProgress::None, state)
}

pub fn export_transactions(
    sender: Sender<ExportProgress>,
    daemon: Arc<dyn Daemon + Sync + Send>,
    path: Box<PathBuf>,
) {
    log::info!("export_transactions()");
    let dir = match path.parent() {
        Some(dir) => dir,
        None => {
            send_error!(sender, NoParentDir);
            return;
        }
    };
    if !dir.exists() {
        if let Err(e) = fs::create_dir_all(dir) {
            send_error!(sender, e.into());
            return;
        }
    }
    let mut file = match File::create(path.as_path()) {
        Ok(f) => f,
        Err(e) => {
            send_error!(sender, e.into());
            return;
        }
    };
    let header = "Date,Label,Value,Fee,Txid,Block".to_string();
    if let Err(e) = file.write_all(header.as_bytes()) {
        send_error!(sender, e.into());
        return;
    }
    log::info!("export_transactions() header written");

    let rt = Runtime::new().unwrap();
    let info = rt.block_on(daemon.get_info());

    let start = match info {
        Ok(info) => info.timestamp,
        Err(e) => {
            send_error!(sender, e.into());
            return;
        }
    };
    // look 2 hour forward
    let end = ((Utc::now() + Duration::hours(2)).timestamp()) as u32;

    let history = rt.block_on(daemon.list_history_txs(start, end, u64::MAX));
    let txs = match history {
        Ok(h) => h,
        Err(e) => {
            send_error!(sender, e.into());
            return;
        }
    };
    log::info!("export_transactions() history received");

    for tx in txs {
        let date_time = tx
            .time
            .map(|t| {
                let mut str = DateTime::from_timestamp(t as i64, 0)
                    .expect("bitcoin timestamp")
                    .to_rfc3339();
                str = str.replace("T", " ");
                str[0..(str.len() - 6)].to_string()
            })
            .unwrap_or("".to_string());

        let labels = tx.labelled();
        let txid = labels
            .iter()
            .filter(|l| matches!(l, LabelItem::Txid(_)))
            .collect::<Vec<_>>()
            .first()
            .map(|l| l.to_string());
        let addr = labels
            .iter()
            .filter(|l| matches!(l, LabelItem::Address(_)))
            .collect::<Vec<_>>()
            .first()
            .map(|l| l.to_string());
        let outpoint = labels
            .iter()
            .filter(|l| matches!(l, LabelItem::Txid(_)))
            .collect::<Vec<_>>()
            .first()
            .map(|l| l.to_string());
        let mut label = txid.unwrap_or(addr.unwrap_or(outpoint.unwrap_or("".to_string())));
        if !label.is_empty() {
            label = format!("\"{}\"", label);
        }
        let fee = tx.fee_amount.unwrap_or(Amount::ZERO);
        let value = (tx.incoming_amount - tx.outgoing_amount - fee).to_string_in(Bitcoin);
        let txid = tx.txid.to_string();
        let block = tx.height.map(|h| h.to_string()).unwrap_or("".to_string());

        let line = format!(
            "{},{},{},{},{},{}\n",
            date_time, label, value, fee, txid, block
        );
        if let Err(e) = file.write_all(line.as_bytes()) {
            send_error!(sender, e.into());
            return;
        }
    }
    log::info!("export_transactions() written");

    if let Err(e) = sender.send(ExportProgress::Ended(path)) {
        tracing::error!("ExportState::start() fail to send msg: {}", e);
    }
}
