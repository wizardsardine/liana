use std::{collections::HashMap, sync::Arc};

use chrono::{Duration, Utc};
use liana::miniscript::bitcoin::Txid;

use crate::{export, services::connect::client::backend::api::DEFAULT_LIMIT};

use super::{model::HistoryTransaction, Daemon, DaemonBackend, DaemonError};

#[derive(Debug)]
pub enum ListTransactionError {
    Daemon(DaemonError),
    TxTimeMissing,
}

impl From<DaemonError> for ListTransactionError {
    fn from(value: DaemonError) -> Self {
        ListTransactionError::Daemon(value)
    }
}

impl From<ListTransactionError> for export::Error {
    fn from(value: ListTransactionError) -> Self {
        match value {
            ListTransactionError::Daemon(e) => export::Error::Daemon(e.to_string()),
            ListTransactionError::TxTimeMissing => export::Error::TxTimeMissing,
        }
    }
}

pub enum FetchProgress {
    Progress(f32),
    Ended,
}

impl From<FetchProgress> for export::Progress {
    fn from(value: FetchProgress) -> Self {
        match value {
            FetchProgress::Progress(p) => export::Progress::Progress(p),
            FetchProgress::Ended => export::Progress::Ended,
        }
    }
}

pub async fn list_confirmed_transactions<F>(
    daemon: Arc<dyn Daemon + Sync + Send>,
    notif: F,
    send_notif: bool,
    limit: Option<usize>,
) -> Result<Vec<HistoryTransaction>, ListTransactionError>
where
    F: Fn(FetchProgress) + Send,
{
    // look 2 hour forward
    // https://github.com/bitcoin/bitcoin/blob/62bd61de110b057cbfd6e31e4d0b727d93119c72/src/chain.h#L29
    let mut end = ((Utc::now() + Duration::hours(2)).timestamp()) as u32;

    let total_txs = if send_notif {
        let total_txs = daemon
            .list_confirmed_txs(0, end, u32::MAX as u64)
            .await?
            .transactions
            .len();
        if total_txs == 0 {
            notif(FetchProgress::Ended);
            return Ok(vec![]);
        } else {
            notif(FetchProgress::Progress(5.0));
        }
        Some(total_txs)
    } else {
        None
    };

    let max = match daemon.backend() {
        DaemonBackend::RemoteBackend => DEFAULT_LIMIT as u64,
        _ => u32::MAX as u64,
    };

    // store txs in a map to avoid duplicates
    let mut map = HashMap::<Txid, HistoryTransaction>::new();
    let mut fetch_limit = max;

    loop {
        let history_txs = daemon.list_history_txs(0, end, fetch_limit).await?;
        let dl = map.len() + history_txs.len();
        if let (Some(total_txs), true) = (total_txs, send_notif) {
            if dl > 0 {
                let progress = (dl as f32) / (total_txs as f32) * 80.0;
                notif(FetchProgress::Progress(progress));
            }
        }
        // all txs have been fetched
        if history_txs.is_empty() {
            break;
        }
        if history_txs.len() == fetch_limit as usize {
            let first = if let Some(t) = history_txs.first().expect("checked").time {
                t
            } else {
                return Err(ListTransactionError::TxTimeMissing);
            };
            let last = if let Some(t) = history_txs.last().expect("checked").time {
                t
            } else {
                return Err(ListTransactionError::TxTimeMissing);
            };
            // limit too low, all tx are in the same timestamp
            // we must increase limit and retry
            if first == last {
                fetch_limit += DEFAULT_LIMIT as u64;
                continue;
            } else {
                // add txs to map
                for tx in history_txs {
                    let txid = tx.txid;
                    map.insert(txid, tx);
                }
                fetch_limit = max;
                end = first.min(last);
                if let Some(limit) = limit {
                    if map.len() >= limit {
                        break;
                    }
                }
                continue;
            }
        } else
        // history_txs.len() < fetch_limit  => no more poll requested
        {
            // add txs to map
            for tx in history_txs {
                let txid = tx.txid;
                map.insert(txid, tx);
            }
            break;
        }
    }

    let txs: Vec<_> = map.into_values().collect();
    Ok(txs)
}
