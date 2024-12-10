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
use liana::miniscript::bitcoin::Amount;
use tokio::{
    task::{JoinError, JoinHandle},
    time::sleep,
};

use crate::daemon::{
    model::{Labelled, TransactionKind},
    Daemon, DaemonError,
};

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

pub struct ExportState {
    pub receiver: Receiver<ExportProgress>,
    pub sender: Option<Sender<ExportProgress>>,
    pub handle: Option<Arc<Mutex<JoinHandle<()>>>>,
    pub daemon: Arc<dyn Daemon + Sync + Send>,
    pub path: Box<PathBuf>,
}

impl ExportState {
    pub fn new(daemon: Arc<dyn Daemon + Sync + Send>, path: Box<PathBuf>) -> Self {
        let (sender, receiver) = channel();
        ExportState {
            receiver,
            sender: Some(sender),
            handle: None,
            daemon,
            path,
        }
    }

    pub async fn start(&mut self) {
        if let (true, Some(sender)) = (self.handle.is_none(), self.sender.take()) {
            let daemon = self.daemon.clone();
            let path = self.path.clone();

            let cloned_sender = sender.clone();
            let handle = tokio::spawn(async move {
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
                let header = "Date,Label,Value,Fee,Txid,Block\n".to_string();
                if let Err(e) = file.write_all(header.as_bytes()) {
                    send_error!(sender, e.into());
                    return;
                }

                let info = daemon.get_info().await;

                let start = match info {
                    Ok(info) => info.timestamp,
                    Err(e) => {
                        send_error!(sender, e.into());
                        return;
                    }
                };
                // look 2 hour forward
                let end = ((Utc::now() + Duration::hours(2)).timestamp()) as u32;

                let history = daemon.list_history_txs(start, end, u32::MAX as u64).await;
                let txs = match history {
                    Ok(h) => h,
                    Err(e) => {
                        send_error!(sender, e.into());
                        return;
                    }
                };

                for mut tx in txs {
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


                    let txid = tx.txid.clone().to_string();
                    let txid_label = tx.labels().get(&txid).cloned();
                    let addr = if let TransactionKind::IncomingSinglePayment(outpoint) = tx.kind {
                        tx.coins.get(&outpoint).map(|c| c.address.to_string())
                    } else {
                        None
                    };
                    let mut label = if let Some(txid) = txid_label {
                        txid
                    } else if let Some(addr) = addr {
                        addr
                    } else if tx.is_send_to_self() {
                        "self send".to_string()
                    } else {
                        "".to_string()
                    };
                    if !label.is_empty() {
                        label = format!("\"{}\"", label);
                    }
                    let fee = tx.fee_amount.unwrap_or(Amount::ZERO).to_btc();
                    let value = tx.incoming_amount.to_btc() - tx.outgoing_amount.to_btc() - fee;
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

                if let Err(e) = sender.send(ExportProgress::Ended(path)) {
                    tracing::error!("ExportState::start() fail to send msg: {}", e);
                }
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

pub async fn get_path() -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Choose a location to export...")
        .set_file_name("liana.csv")
        .save_file()
        .await
        .map(|fh| fh.path().to_path_buf())
}
