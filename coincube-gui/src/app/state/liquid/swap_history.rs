//! Local persistence for completed cross-asset swaps.
//!
//! A SideSwap swap is stored by the SDK as an ordinary Liquid payment with
//! no marker distinguishing it from a plain send/receive. To surface swaps
//! as such — in "Last Swaps" on the Swap screen and as a "Swap" label in
//! Transactions / Overview — we record each completed swap locally, keyed
//! by the send leg's tx id, in a small JSON file in the cube's network dir.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::send::SendAsset;

/// Cap on retained records — plenty for history, bounded on disk.
const MAX_RECORDS: usize = 200;

/// Serializable asset tag (avoids a serde dep on `SendAsset`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwapAsset {
    Lbtc,
    Usdt,
}

impl From<SendAsset> for SwapAsset {
    fn from(a: SendAsset) -> Self {
        match a {
            SendAsset::Lbtc => SwapAsset::Lbtc,
            SendAsset::Usdt => SwapAsset::Usdt,
        }
    }
}

impl SwapAsset {
    pub fn to_send_asset(self) -> SendAsset {
        match self {
            SwapAsset::Lbtc => SendAsset::Lbtc,
            SwapAsset::Usdt => SendAsset::Usdt,
        }
    }
}

/// One completed swap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapRecord {
    /// Tx id of the send leg (from the `send_payment` response), if known.
    pub tx_id: Option<String>,
    pub from_asset: SwapAsset,
    pub to_asset: SwapAsset,
    /// `from`-asset base units paid (exchange + fees).
    pub from_base: u64,
    /// `to`-asset base units received.
    pub to_base: u64,
    /// Payment timestamp (seconds), for sorting/display.
    pub timestamp: u32,
}

/// Append-only swap log backed by a JSON file. Records are newest-first.
#[derive(Debug, Default, Clone)]
pub struct SwapHistory {
    path: PathBuf,
    records: Vec<SwapRecord>,
}

impl SwapHistory {
    /// Load the history from `path` (missing/corrupt → empty).
    pub fn load(path: PathBuf) -> Self {
        let records = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<SwapRecord>>(&bytes).ok())
            .unwrap_or_default();
        Self { path, records }
    }

    pub fn records(&self) -> &[SwapRecord] {
        &self.records
    }

    /// The set of known swap send-leg tx ids, for labelling payment rows.
    pub fn tx_ids(&self) -> HashSet<String> {
        self.records
            .iter()
            .filter_map(|r| r.tx_id.clone())
            .collect()
    }

    /// Prepend a record and persist (best-effort — a write failure is logged
    /// but not fatal; the swap itself already settled).
    pub fn record(&mut self, record: SwapRecord) {
        self.records.insert(0, record);
        self.records.truncate(MAX_RECORDS);
        if let Err(e) = self.persist() {
            log::warn!(target: "breez_swap", "failed to persist swap history: {e}");
        }
    }

    fn persist(&self) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(&self.records)?;
        std::fs::write(&self.path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(tx: &str, ts: u32) -> SwapRecord {
        SwapRecord {
            tx_id: Some(tx.to_string()),
            from_asset: SwapAsset::Lbtc,
            to_asset: SwapAsset::Usdt,
            from_base: 100_000,
            to_base: 6_000_000_000,
            timestamp: ts,
        }
    }

    #[test]
    fn records_newest_first_persist_and_reload() {
        let path =
            std::env::temp_dir().join(format!("coincube-swaps-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut h = SwapHistory::load(path.clone());
        assert!(h.records().is_empty());
        h.record(rec("aaa", 100));
        h.record(rec("bbb", 200));

        // Newest first.
        assert_eq!(h.records()[0].tx_id.as_deref(), Some("bbb"));
        assert_eq!(h.records()[1].tx_id.as_deref(), Some("aaa"));
        assert_eq!(h.tx_ids().len(), 2);

        // Persisted and reloadable.
        let reloaded = SwapHistory::load(path.clone());
        assert_eq!(reloaded.records().len(), 2);
        assert_eq!(reloaded.records()[0].tx_id.as_deref(), Some("bbb"));

        let _ = std::fs::remove_file(&path);
    }
}
