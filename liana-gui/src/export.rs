use std::{
    collections::HashMap,
    fs::{self, File},
    io::Write,
    path::PathBuf,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc, Mutex,
    },
    time::{self},
};

use chrono::{DateTime, Duration, Utc};
use liana::miniscript::bitcoin::{Amount, Txid};
use tokio::{
    task::{JoinError, JoinHandle},
    time::sleep,
};

use iced::futures::{SinkExt, Stream};

use crate::{
    app::view,
    daemon::{
        model::{HistoryTransaction, Labelled},
        Daemon, DaemonBackend, DaemonError,
    },
    lianalite::client::backend::api::DEFAULT_LIMIT,
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

macro_rules! send_progress {
    ($sender:ident, $progress:ident) => {
        if let Err(e) = $sender.send(ExportProgress::$progress) {
            tracing::error!("ExportState::start() fail to send msg: {}", e);
        }
    };
    ($sender:ident, $progress:ident($val:expr)) => {
        if let Err(e) = $sender.send(ExportProgress::$progress($val)) {
            tracing::error!("ExportState::start() fail to send msg: {}", e);
        }
    };
}

#[derive(Debug, Clone)]
pub enum ExportMessage {
    Open,
    ExportProgress(ExportProgress),
    TimedOut,
    UserStop,
    Path(Option<PathBuf>),
    Close,
}

impl From<ExportMessage> for view::Message {
    fn from(value: ExportMessage) -> Self {
        Self::Export(value)
    }
}

#[derive(Debug, PartialEq)]
pub enum ExportState {
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
pub enum Status {
    Init,
    Running,
    Stopped,
}

#[derive(Debug, Clone)]
pub enum ExportProgress {
    Started(Arc<Mutex<JoinHandle<()>>>),
    Progress(f32),
    Ended,
    Finished,
    Error(Error),
    None,
}

pub struct State {
    pub receiver: Receiver<ExportProgress>,
    pub sender: Option<Sender<ExportProgress>>,
    pub handle: Option<Arc<Mutex<JoinHandle<()>>>>,
    pub daemon: Arc<dyn Daemon + Sync + Send>,
    pub path: Box<PathBuf>,
}

impl State {
    pub fn new(daemon: Arc<dyn Daemon + Sync + Send>, path: Box<PathBuf>) -> Self {
        let (sender, receiver) = channel();
        State {
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
            });
            let handle = Arc::new(Mutex::new(handle));

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
    daemon: Arc<dyn Daemon + Sync + Send>,
    path: PathBuf,
) -> impl Stream<Item = ExportProgress> {
    iced::stream::channel(100, move |mut output| async move {
        let mut state = State::new(daemon, Box::new(path));
        loop {
            match state.state() {
                Status::Init => {
                    state.start().await;
                }
                Status::Stopped => {
                    break;
                }
                Status::Running => {
                    sleep(time::Duration::from_millis(100)).await;
                    continue;
                }
            }
            let msg = state.receiver.try_recv();
            let disconnected = match msg {
                Ok(m) => {
                    let _ = output.send(m).await;
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
                    let _ = output.send(ExportProgress::Error(Error::HandleLost)).await;
                    continue;
                }
            };
            let msg = {
                let h = handle.lock().expect("should not fail");
                if h.is_finished() {
                    Some(ExportProgress::Finished)
                } else if disconnected {
                    Some(ExportProgress::Error(Error::ChannelLost))
                } else {
                    None
                }
            };
            if let Some(msg) = msg {
                let _ = output.send(msg).await;
                continue;
            }
            // => release handle lock
            state.handle = Some(handle);

            sleep(time::Duration::from_millis(100)).await;
            let _ = output.send(ExportProgress::None).await;
        }
    })
}

pub async fn get_path() -> Option<PathBuf> {
    let date = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S");
    let file_name = format!("liana-txs-{date}.csv");
    rfd::AsyncFileDialog::new()
        .set_title("Choose a location to export...")
        .set_file_name(file_name)
        .save_file()
        .await
        .map(|fh| fh.path().to_path_buf())
}
