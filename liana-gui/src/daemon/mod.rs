pub mod client;
pub mod embedded;
pub mod model;

use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::fmt::Debug;
use std::io::ErrorKind;
use std::iter::FromIterator;

use async_trait::async_trait;

use liana::miniscript::bitcoin::{
    address,
    bip32::{ChildNumber, Fingerprint},
    psbt::Psbt,
    secp256k1, Address, Network, OutPoint, Txid,
};
use lianad::bip329::Labels;
use lianad::commands::UpdateDerivIndexesResult;
#[cfg(feature = "payjoin")]
use lianad::payjoin::types::PayjoinStatus;
use lianad::{
    commands::{CoinStatus, LabelItem, TransactionInfo},
    config::Config,
    StartupError,
};

use crate::{hw::HardwareWalletConfig, node};

#[derive(Debug)]
pub enum DaemonError {
    /// Something was wrong with the request.
    Rpc(i32, String),
    /// Something was wrong with the rpc socket communication.
    RpcSocket(Option<ErrorKind>, String),
    /// Something was wrong with the http communication.
    Http(Option<u16>, serde_json::Value),
    /// Something unexpected happened.
    Unexpected(String),
    /// No response.
    NoAnswer,
    /// Daemon stopped
    DaemonStopped,
    // Error at start up.
    Start(StartupError),
    // Error if the client is not supported.
    ClientNotSupported,
    /// Error when selecting coins for spend.
    CoinSelectionError,
    /// Not implemented feature
    NotImplemented,
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Rpc(code, e) => write!(f, "Daemon error rpc call: [{code:?}] {e}"),
            Self::NoAnswer => write!(f, "Daemon returned no answer"),
            Self::DaemonStopped => write!(f, "Daemon stopped"),
            Self::RpcSocket(kind, e) => write!(f, "Daemon transport error: [{kind:?}] {e}"),
            Self::Http(kind, e) => write!(f, "Http error: [{kind:?}] {e}"),
            Self::Unexpected(e) => write!(f, "Daemon unexpected error: {e}"),
            Self::Start(e) => write!(f, "Daemon did not start: {e}"),
            Self::ClientNotSupported => write!(f, "Daemon communication is not supported"),
            Self::CoinSelectionError => write!(f, "Coin selection error"),
            Self::NotImplemented => write!(f, "This feature is not implemented for this backend"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonBackend {
    EmbeddedLianad(Option<node::NodeType>),
    ExternalLianad,
    RemoteBackend,
}

/// Low/medium/high feerate presets (sat/vb) the remote backend provides
/// alongside the fiat price. Present only when the backend supplied both the low
/// and high bounds. `medium` is optional: older backends omit it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeerateEstimate {
    pub low: u64,
    pub medium: Option<u64>,
    pub high: u64,
}

impl FeerateEstimate {
    /// Build a sanitized estimate from the raw backend values. Each bound is
    /// floored to 1 sat/vbyte so a preset is always broadcastable, then the
    /// presets are forced into non-decreasing order (`low <= medium <= high`).
    /// A noisy estimate near the mempool floor can report medium below low or
    /// high below low; without this a preset would be cheaper than a lower one.
    pub fn new(low: i32, medium: Option<i32>, high: i32) -> Self {
        let low = low.max(1) as u64;
        let high = (high.max(1) as u64).max(low);
        let medium = medium.map(|m| (m.max(1) as u64).clamp(low, high));
        Self { low, medium, high }
    }
}

impl DaemonBackend {
    pub fn is_embedded(&self) -> bool {
        matches!(self, DaemonBackend::EmbeddedLianad(_))
    }

    pub fn is_lianad(&self) -> bool {
        matches!(
            self,
            DaemonBackend::EmbeddedLianad(_) | DaemonBackend::ExternalLianad
        )
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, DaemonBackend::RemoteBackend)
    }

    pub fn node_type(&self) -> Option<node::NodeType> {
        if let DaemonBackend::EmbeddedLianad(node_type) = self {
            *node_type
        } else {
            None
        }
    }
}

#[async_trait]
pub trait Daemon: Debug {
    fn backend(&self) -> DaemonBackend;
    fn config(&self) -> Option<&Config>;
    async fn is_alive(
        &self,
        datadir: &crate::dir::LianaDirectory,
        network: Network,
    ) -> Result<(), DaemonError>;
    async fn stop(&self) -> Result<(), DaemonError>;
    async fn get_info(&self) -> Result<model::GetInfoResult, DaemonError>;
    async fn get_new_address(&self) -> Result<model::GetAddressResult, DaemonError>;
    async fn list_revealed_addresses(
        &self,
        is_change: bool,
        exclude_used: bool,
        limit: usize,
        start_index: Option<ChildNumber>,
    ) -> Result<model::ListRevealedAddressesResult, DaemonError>;
    #[cfg(feature = "payjoin")]
    async fn receive_payjoin(&self) -> Result<model::GetAddressResult, DaemonError>;
    #[cfg(feature = "payjoin")]
    async fn get_payjoin_info(&self, txid: &Txid) -> Result<PayjoinStatus, DaemonError>;
    #[cfg(feature = "payjoin")]
    async fn get_active_payjoin_receiver_sessions(&self) -> Result<Vec<u32>, DaemonError>;
    #[cfg(feature = "payjoin")]
    async fn send_payjoin_proposal(&self, txid: &Txid) -> Result<(), DaemonError>;
    #[cfg(feature = "payjoin")]
    async fn broadcast_payjoin_fallback(&self, txid: &Txid) -> Result<(), DaemonError>;
    async fn update_deriv_indexes(
        &self,
        receive: Option<u32>,
        change: Option<u32>,
    ) -> Result<UpdateDerivIndexesResult, DaemonError>;
    async fn list_coins(
        &self,
        statuses: &[CoinStatus],
        outpoints: &[OutPoint],
    ) -> Result<model::ListCoinsResult, DaemonError>;
    async fn list_spend_txs(&self) -> Result<model::ListSpendResult, DaemonError>;
    async fn create_spend_tx(
        &self,
        coins_outpoints: &[OutPoint],
        destinations: &HashMap<Address<address::NetworkUnchecked>, u64>,
        feerate_vb: u64,
        change_address: Option<Address<address::NetworkUnchecked>>,
    ) -> Result<model::CreateSpendResult, DaemonError>;
    async fn rbf_psbt(
        &self,
        txid: &Txid,
        is_cancel: bool,
        feerate_vb: Option<u64>,
    ) -> Result<model::CreateSpendResult, DaemonError>;
    async fn update_spend_tx(&self, psbt: &Psbt) -> Result<(), DaemonError>;
    async fn delete_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError>;
    async fn broadcast_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError>;
    async fn start_rescan(&self, t: u32) -> Result<(), DaemonError>;
    async fn list_confirmed_txs(
        &self,
        _start: u32,
        _end: u32,
        _limit: u64,
    ) -> Result<model::ListTransactionsResult, DaemonError>;
    async fn create_recovery(
        &self,
        address: Address<address::NetworkUnchecked>,
        coins_outpoints: &[OutPoint],
        feerate_vb: u64,
        sequence: Option<u16>,
    ) -> Result<Psbt, DaemonError>;
    async fn list_txs(&self, txid: &[Txid]) -> Result<model::ListTransactionsResult, DaemonError>;
    async fn get_labels(
        &self,
        labels: &HashSet<LabelItem>,
    ) -> Result<HashMap<String, String>, DaemonError>;
    async fn update_labels(
        &self,
        labels: &HashMap<LabelItem, Option<String>>,
    ) -> Result<(), DaemonError>;
    async fn get_labels_bip329(&self, offset: u32, limit: u32) -> Result<Labels, DaemonError>;
    async fn send_wallet_invitation(&self, _email: &str) -> Result<(), DaemonError> {
        Ok(())
    }

    /// Returns fiat exchange rates from the backend, together with the optional
    /// high/low feerate presets. Keys are currency pair strings like "BTCEUR",
    /// values are the rates.
    async fn get_fiat_rates(
        &self,
    ) -> Result<(HashMap<String, f64>, Option<FeerateEstimate>), DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    /// Updates the user's wallet settings on the backend.
    async fn update_wallet_settings(
        &self,
        _fiat_currency: liana_connect::wallets::api::FiatCurrency,
    ) -> Result<(), DaemonError> {
        Err(DaemonError::NotImplemented)
    }

    // List spend transactions, optionally filtered to the specified `txids`.
    // Set `txids` to `None` for no filter (passing an empty slice returns no transactions).
    async fn list_spend_transactions(
        &self,
        txids: Option<&[Txid]>,
    ) -> Result<Vec<model::SpendTx>, DaemonError> {
        let info = self.get_info().await?;
        let mut spend_txs = Vec::new();
        let curve = secp256k1::Secp256k1::verification_only();
        // TODO: Use filters in `list_spend_txs` command.
        let mut txs = self.list_spend_txs().await?.spend_txs;
        if let Some(txids) = txids {
            txs.retain(|tx| txids.contains(&tx.psbt.unsigned_tx.compute_txid()));
        }
        let outpoints: Vec<_> = txs
            .iter()
            .flat_map(|tx| {
                tx.psbt
                    .unsigned_tx
                    .input
                    .iter()
                    .map(|txin| txin.previous_output)
                    .collect::<Vec<_>>()
            })
            .collect();
        let coins = self.list_coins(&[], &outpoints).await?.coins;
        for tx in txs {
            let coins = coins
                .iter()
                .filter(|coin| {
                    tx.psbt
                        .unsigned_tx
                        .input
                        .iter()
                        .any(|input| input.previous_output == coin.outpoint)
                })
                .cloned()
                .collect();

            #[cfg(feature = "payjoin")]
            let payjoin_status = self
                .get_payjoin_info(&tx.psbt.unsigned_tx.compute_txid())
                .await?;

            spend_txs.push(model::SpendTx::new(
                tx.updated_at,
                tx.psbt,
                coins,
                &info.descriptors.main,
                &curve,
                info.network,
                #[cfg(feature = "payjoin")]
                Some(payjoin_status),
                #[cfg(not(feature = "payjoin"))]
                None,
            ));
        }
        load_labels(self, &mut spend_txs).await?;
        spend_txs.sort_by(|a, b| {
            if a.status == b.status {
                // last updated first
                b.updated_at.cmp(&a.updated_at)
            } else {
                // follows status enum order
                a.status.cmp(&b.status)
            }
        });
        Ok(spend_txs)
    }

    async fn txs_to_historytxs(
        &self,
        txs: Vec<TransactionInfo>,
    ) -> Result<Vec<model::HistoryTransaction>, DaemonError> {
        let info = self.get_info().await?;
        let outpoints: Vec<_> = txs
            .iter()
            .flat_map(|tx| {
                (0..tx.tx.output.len())
                    .map(|vout| {
                        OutPoint::new(
                            tx.tx.compute_txid(),
                            vout.try_into()
                                .expect("number of transaction outputs must fit in u32"),
                        )
                    })
                    .chain(tx.tx.input.iter().map(|txin| txin.previous_output))
                    .collect::<Vec<_>>()
            })
            .collect::<HashSet<_>>() // remove duplicates
            .iter()
            .cloned()
            .collect();
        let coins = self.list_coins(&[], &outpoints).await?.coins;
        let mut txs = txs
            .into_iter()
            .map(|tx| {
                let mut tx_coins = Vec::new();
                let mut change_indexes = Vec::new();
                for coin in &coins {
                    if coin.outpoint.txid == tx.tx.compute_txid() {
                        change_indexes.push(coin.outpoint.vout as usize)
                    } else if tx
                        .tx
                        .input
                        .iter()
                        .any(|input| input.previous_output == coin.outpoint)
                    {
                        tx_coins.push(coin.clone());
                    }
                }
                model::HistoryTransaction::new(
                    tx.tx,
                    tx.height,
                    tx.time,
                    tx_coins,
                    change_indexes,
                    info.network,
                    #[cfg(feature = "payjoin")]
                    tx.payjoin_role,
                    #[cfg(not(feature = "payjoin"))]
                    None,
                )
            })
            .collect();
        load_labels(self, &mut txs).await?;
        Ok(txs)
    }

    async fn list_history_txs(
        &self,
        start: u32,
        end: u32,
        limit: u64,
    ) -> Result<Vec<model::HistoryTransaction>, DaemonError> {
        let txs = self
            .list_confirmed_txs(start, end, limit)
            .await?
            .transactions;
        self.txs_to_historytxs(txs).await
    }

    async fn get_history_txs(
        &self,
        txids: &[Txid],
    ) -> Result<Vec<model::HistoryTransaction>, DaemonError> {
        let txs = self.list_txs(txids).await?.transactions;
        self.txs_to_historytxs(txs).await
    }

    async fn list_pending_txs(&self) -> Result<Vec<model::HistoryTransaction>, DaemonError> {
        let info = self.get_info().await?;
        // We want coins that are inputs to and/or outputs of a pending tx,
        // which can only be unconfirmed and spending coins.
        let coins = self
            .list_coins(&[CoinStatus::Unconfirmed, CoinStatus::Spending], &[])
            .await?
            .coins;
        let mut txids: Vec<Txid> = Vec::new();
        for coin in &coins {
            if coin.block_height.is_none() && !txids.contains(&coin.outpoint.txid) {
                txids.push(coin.outpoint.txid);
            }

            if let Some(spend) = coin.spend_info {
                if !txids.contains(&spend.txid) {
                    txids.push(spend.txid);
                }
            }
        }

        if txids.is_empty() {
            return Ok(Vec::new());
        }

        let txs = self.list_txs(&txids).await?.transactions;
        let mut txs = txs
            .into_iter()
            .map(|tx| {
                let mut tx_coins = Vec::new();
                let mut change_indexes = Vec::new();
                for coin in &coins {
                    if coin.outpoint.txid == tx.tx.compute_txid() {
                        change_indexes.push(coin.outpoint.vout as usize)
                    } else if tx
                        .tx
                        .input
                        .iter()
                        .any(|input| input.previous_output == coin.outpoint)
                    {
                        tx_coins.push(coin.clone());
                    }
                }
                model::HistoryTransaction::new(
                    tx.tx,
                    tx.height,
                    tx.time,
                    tx_coins,
                    change_indexes,
                    info.network,
                    #[cfg(feature = "payjoin")]
                    tx.payjoin_role,
                    #[cfg(not(feature = "payjoin"))]
                    None,
                )
            })
            .collect();

        load_labels(self, &mut txs).await?;
        Ok(txs)
    }

    async fn list_pending_payments(&self) -> Result<Vec<model::Payment>, DaemonError> {
        let mut txs = self.list_pending_txs().await?;
        txs.sort_by(|a, b| b.time.cmp(&a.time));
        let events = txs.into_iter().fold(Vec::new(), |mut array, tx| {
            let mut events = model::payments_from_tx(tx);
            array.append(&mut events);
            array
        });

        Ok(events)
    }

    /// returns a sorted list of payments.
    async fn list_confirmed_payments(
        &self,
        start: u32,
        end: u32,
        limit: u64,
    ) -> Result<Vec<model::Payment>, DaemonError> {
        let mut txs = self.list_history_txs(start, end, limit).await?;
        txs.sort_by(|a, b| b.time.cmp(&a.time));
        let events = txs.into_iter().fold(Vec::new(), |mut array, tx| {
            let mut events = model::payments_from_tx(tx);
            array.append(&mut events);
            array
        });

        Ok(events)
    }

    /// Reimplemented by LianaLite backend
    async fn update_wallet_metadata(
        &self,
        _wallet_alias: Option<String>,
        _fingerprint_aliases: &HashMap<Fingerprint, String>,
        _hws: &[HardwareWalletConfig],
    ) -> Result<(), DaemonError> {
        Ok(())
    }
}

async fn load_labels<T: model::Labelled + model::LabelsLoader, D: Daemon + ?Sized>(
    daemon: &D,
    targets: &mut Vec<T>,
) -> Result<(), DaemonError> {
    if targets.is_empty() {
        return Ok(());
    }
    let mut items = HashSet::<LabelItem>::new();
    for target in &*targets {
        for item in target.labelled() {
            items.insert(item);
        }
    }
    let labels = HashMap::from_iter(
        daemon
            .get_labels(&items)
            .await?
            .into_iter()
            .map(|(k, v)| (k, Some(v))),
    );
    for target in targets {
        target.load_labels(&labels);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::FeerateEstimate;

    #[test]
    fn feerate_estimate_orders_presets() {
        // Reported case: a noisy backend estimate with medium below low.
        let est = FeerateEstimate::new(2, Some(1), 2);
        assert_eq!(est.low, 2);
        assert_eq!(est.medium, Some(2));
        assert_eq!(est.high, 2);

        // Well-ordered values pass through untouched.
        let est = FeerateEstimate::new(1, Some(5), 10);
        assert_eq!((est.low, est.medium, est.high), (1, Some(5), 10));

        // Medium above high is clamped down to high; high below low is raised.
        let est = FeerateEstimate::new(4, Some(9), 2);
        assert_eq!((est.low, est.medium, est.high), (4, Some(4), 4));

        // Zero/negative values are floored to 1.
        let est = FeerateEstimate::new(0, Some(-3), 0);
        assert_eq!((est.low, est.medium, est.high), (1, Some(1), 1));

        // Absent medium stays absent.
        assert_eq!(FeerateEstimate::new(1, None, 5).medium, None);
    }
}
