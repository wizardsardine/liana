//! When a Vault may move from COINCUBE | Connect to the local node.
//!
//! Two things can make a switch to the local node lose what the user sees:
//!
//! * **A scan in flight.** Stopping the Connect backend discards a running
//!   address scan (`coincubed`'s Esplora full scan is all-or-nothing), and the
//!   restarted backend starts over. An automatic switch is therefore deferred
//!   until the scan has finished, and a manual one asks first.
//! * **Missing local wallet history.** Pruning does not erase transactions
//!   already recorded in Core's wallet. A newly created watch-only wallet,
//!   however, cannot discover transactions in blocks the node has deleted.
//!   Check the wallet's records before deciding whether it needs those blocks.
//!
//! Decisions are pure; the wallet probe reads the records needed to make them.

use std::convert::TryFrom;
use std::fmt;

/// What the local node's `getblockchaininfo` says about pruning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodePruning {
    /// The node keeps every block.
    Unpruned,
    /// The node keeps blocks only from `prune_height` on
    /// (`getblockchaininfo.pruneheight`: the lowest height still stored).
    Pruned { prune_height: u64 },
}

impl NodePruning {
    /// Read `pruned` / `pruneheight` from a `getblockchaininfo` result.
    ///
    /// `None` when the answer does not say: a missing `pruned`, or a pruned
    /// node that does not report its height. Not knowing is not the same as
    /// unpruned, so the caller must not switch on it.
    pub fn from_blockchain_info(result: &serde_json::Value) -> Option<Self> {
        match result["pruned"].as_bool()? {
            false => Some(Self::Unpruned),
            true => result["pruneheight"]
                .as_u64()
                .map(|prune_height| Self::Pruned { prune_height }),
        }
    }
}

/// How far back this Vault's history reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultHistory {
    /// The lowest block height of any coin the Vault has ever had, spent coins
    /// included.
    From(u32),
    /// Core's watch-only wallet already records all known funding and spending
    /// transactions. It needs no historical block rescan to recover them.
    TrackedLocally,
    /// No coin with a block height. `rescan_owed` is whether the Vault still
    /// carries a recorded rescan obligation (`WalletSettings::pending_rescan`):
    /// a restored Vault whose history has not been found yet, so how far back
    /// it reaches is unknown.
    NoConfirmedCoins { rescan_owed: bool },
}

impl VaultHistory {
    /// Build from the block heights of **all** the Vault's coins: unconfirmed,
    /// confirmed, spending and spent. Spent coins have to be included — a
    /// Vault whose history is entirely spent still needs those blocks for its
    /// transaction list, and would otherwise look like one with no history.
    pub fn from_coin_heights(
        heights: impl IntoIterator<Item = Option<i32>>,
        rescan_owed: bool,
    ) -> Self {
        heights
            .into_iter()
            .flatten()
            .filter_map(|h| u32::try_from(h).ok())
            .min()
            .map(Self::From)
            .unwrap_or(Self::NoConfirmedCoins { rescan_owed })
    }
}

/// Blocks a pruned node must keep *below* a Vault's earliest coin.
///
/// bitcoind rescans from a block *timestamp*, less a 2 h window
/// (`TIMESTAMP_WINDOW`), and block timestamps may themselves be up to 2 h off,
/// so a rescan for a coin at height M can start a few dozen blocks before M.
/// A pruned node also keeps advancing its prune height as the chain grows
/// (`prune=550` is a size target). One day of blocks covers both with room.
pub const PRUNE_MARGIN_BLOCKS: u64 = 144;

/// Why the local wallet cannot recover missing history from retained blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrunedHistory {
    /// The Vault has coins from `earliest`, below the node's prune height or
    /// within [`PRUNE_MARGIN_BLOCKS`] above it.
    CoinsBelowPrune { prune_height: u64, earliest: u32 },
    /// The Vault's history has not been found yet (a restore still owing its
    /// rescan), so it may lie anywhere below the prune height.
    HistoryUnknown { prune_height: u64 },
}

impl fmt::Display for PrunedHistory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CoinsBelowPrune {
                prune_height,
                earliest,
            } if u64::from(*earliest) < *prune_height => write!(
                f,
                "Your local node keeps blocks only from height {prune_height}, but this Vault \
                 has history from block {earliest}. COINCUBE Desktop could not confirm all of it in the \
                 local wallet. A recovery scan would need blocks the node no longer keeps. \
                 Keep using COINCUBE | Connect for this Vault."
            ),
            Self::CoinsBelowPrune {
                prune_height,
                earliest,
            } => write!(
                f,
                "Your local node keeps blocks only from height {prune_height}, and this Vault \
                 has history from block {earliest}. COINCUBE Desktop could not confirm all of it in the \
                 local wallet, and its history is too close to that height to rescan reliably. Keep using \
                 COINCUBE | Connect for this Vault."
            ),
            Self::HistoryUnknown { prune_height } => write!(
                f,
                "Your local node keeps blocks only from height {prune_height}, and this \
                 restored Vault's history hasn't been found yet, so it may be older than \
                 that. Keep using COINCUBE | Connect while COINCUBE Desktop recovers this Vault's history."
            ),
        }
    }
}

/// Whether the local wallet already tracks this Vault or can rescan its history.
///
/// `Ok(())` for an unpruned node whatever the history, and for a pruned one
/// whose wallet already tracks the history, or one that keeps at least
/// [`PRUNE_MARGIN_BLOCKS`] blocks below the Vault's earliest
/// coin (`prune_height + PRUNE_MARGIN_BLOCKS <= earliest`). A pruned node short
/// of that, or a pruned node and a Vault whose history is unknown, cannot.
pub fn pruned_node_serves(
    pruning: NodePruning,
    history: VaultHistory,
) -> Result<(), PrunedHistory> {
    let NodePruning::Pruned { prune_height } = pruning else {
        return Ok(());
    };
    match history {
        VaultHistory::From(earliest)
            if prune_height.saturating_add(PRUNE_MARGIN_BLOCKS) > u64::from(earliest) =>
        {
            Err(PrunedHistory::CoinsBelowPrune {
                prune_height,
                earliest,
            })
        }
        VaultHistory::From(_) | VaultHistory::TrackedLocally => Ok(()),
        VaultHistory::NoConfirmedCoins { rescan_owed: true } => {
            Err(PrunedHistory::HistoryUnknown { prune_height })
        }
        VaultHistory::NoConfirmedCoins { rescan_owed: false } => Ok(()),
    }
}

/// The scan a switch would discard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunningScan {
    /// The wallet is catching up with the chain — for a Connect Vault this
    /// includes its startup address scan.
    WalletSync,
    /// A rescan the user (or a restore) started is still running.
    Rescan,
}

/// Whether an address-scanning backend (Esplora/Electrum) is still on the
/// session's first scan: no poll has completed since startup
/// (`last_poll <= last_poll_at_startup`, `None` counting as "never").
/// Independent of the wallet's block height, which is 0 until that first
/// scan has written a tip.
pub fn first_address_scan_pending(
    scans_addresses: bool,
    last_poll: Option<u32>,
    last_poll_at_startup: Option<u32>,
) -> bool {
    scans_addresses && last_poll <= last_poll_at_startup
}

/// The scan in flight, if any. A rescan is reported first since it is the
/// longer of the two to redo.
pub fn running_scan(wallet_is_syncing: bool, rescan_pending: bool) -> Option<RunningScan> {
    if rescan_pending {
        Some(RunningScan::Rescan)
    } else if wallet_is_syncing {
        Some(RunningScan::WalletSync)
    } else {
        None
    }
}

/// Why an automatic switch to the local node is not happening yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchHold {
    /// Deferred until the scan finishes; retried on every node probe.
    Scanning(RunningScan),
    /// The node's pruning, or the Vault's history on a pruned node, has not
    /// been read yet. Retried on every node probe.
    Unknown,
    /// The node is pruned past this Vault's history. Not retried until either
    /// side changes.
    Pruned(PrunedHistory),
}

/// Whether an automatic switch to an already-synced local node may go ahead
/// now, and if not, why.
///
/// `history` is only needed for a pruned node; `None` means it could not be
/// read on this probe.
pub fn auto_switch_hold(
    scan: Option<RunningScan>,
    pruning: Option<NodePruning>,
    history: Option<VaultHistory>,
) -> Option<SwitchHold> {
    if let Some(scan) = scan {
        return Some(SwitchHold::Scanning(scan));
    }
    match pruning {
        None => Some(SwitchHold::Unknown),
        Some(NodePruning::Unpruned) => None,
        Some(pruned @ NodePruning::Pruned { .. }) => match history {
            None => Some(SwitchHold::Unknown),
            Some(history) => pruned_node_serves(pruned, history)
                .err()
                .map(SwitchHold::Pruned),
        },
    }
}

/// The same authentication used for the node sync probe and its wallet probe.
pub(crate) async fn rpc_credentials(
    cfg: &coincubed::config::BitcoindConfig,
) -> Result<(String, String), String> {
    use coincubed::config::BitcoindRpcAuth;
    match &cfg.rpc_auth {
        BitcoindRpcAuth::CookieFile(path) => {
            let cookie = tokio::fs::read_to_string(path)
                .await
                .map_err(|e| format!("Cannot read bitcoind cookie: {e}"))?;
            let (user, pass) = cookie
                .trim()
                .split_once(':')
                .ok_or_else(|| "Invalid cookie file format".to_string())?;
            Ok((user.to_string(), pass.to_string()))
        }
        BitcoindRpcAuth::UserPass(user, pass) => Ok((user.clone(), pass.clone())),
    }
}

/// Ask Core's wallet, rather than its block store, about known history.
/// Loads an existing watch-only wallet as daemon startup would; never creates
/// one or imports descriptors. RPC failures remain unknown, never absence.
pub(super) async fn local_wallet_tracks(
    cfg: &coincubed::config::BitcoindConfig,
    wallet_path: &str,
    chain: crate::chain::ChainId,
    txids: &[coincube_core::miniscript::bitcoin::Txid],
) -> Result<bool, String> {
    // Fork wallet RPCs require exact-chain admission through BitcoinD. This
    // Bitcoin-only probe must not bypass that boundary.
    if chain.is_blake2b() {
        return Err("Fork wallet history requires exact-chain admission".into());
    }
    if txids.is_empty() {
        return Err("No transactions to check".into());
    }
    let (user, pass) = rpc_credentials(cfg).await?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let node_url = format!("http://{}/", cfg.addr);
    let wallets: serde_json::Value = client
        .post(&node_url)
        .basic_auth(&user, Some(&pass))
        .json(&serde_json::json!({"jsonrpc":"2.0", "id":0, "method":"listwallets", "params":[]}))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let loaded = wallets["result"]
        .as_array()
        .ok_or("Cannot read loaded wallets")?;
    if !loaded.iter().any(|name| name.as_str() == Some(wallet_path)) {
        // Managed local wallets use the same absolute path as daemon startup.
        // No wallet file means switching would create an empty one at "now".
        if !std::path::Path::new(wallet_path).exists() {
            return Ok(false);
        }
        let loaded: serde_json::Value = client.post(&node_url)
            .basic_auth(&user, Some(&pass))
            .json(&serde_json::json!({"jsonrpc":"2.0", "id":0, "method":"loadwallet", "params":[wallet_path]}))
            .send().await.map_err(|e| e.to_string())?
            .json().await.map_err(|e| e.to_string())?;
        if loaded["result"]["name"].as_str() != Some(wallet_path) || !loaded["error"].is_null() {
            return Err("Cannot load existing local wallet to check its history".into());
        }
    }
    let mut wallet_url = reqwest::Url::parse(&node_url).map_err(|e| e.to_string())?;
    wallet_url
        .path_segments_mut()
        .map_err(|_| "Invalid node URL")?
        .clear()
        .push("wallet")
        .push(wallet_path);
    // Bound each batch, and match IDs rather than relying on response order.
    for chunk in txids.chunks(100) {
        let requests: Vec<_> = chunk.iter().enumerate().map(|(id, txid)|
            serde_json::json!({"jsonrpc":"2.0", "id":id, "method":"gettransaction", "params":[txid.to_string()]})).collect();
        let response: serde_json::Value = client
            .post(wallet_url.clone())
            .basic_auth(&user, Some(&pass))
            .json(&requests)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        match wallet_records(&response, chunk) {
            Some(true) => {}
            Some(false) => return Ok(false),
            None => return Err("Cannot read local wallet transactions".into()),
        }
    }
    Ok(true)
}

fn wallet_records(
    response: &serde_json::Value,
    txids: &[coincube_core::miniscript::bitcoin::Txid],
) -> Option<bool> {
    let entries = response.as_array()?;
    if entries.len() != txids.len() {
        return None;
    }
    let mut missing = false;
    for (id, txid) in txids.iter().enumerate() {
        let mut matching = entries
            .iter()
            .filter(|entry| entry["id"].as_u64() == Some(id as u64));
        let entry = matching.next()?;
        if matching.next().is_some() {
            return None;
        }
        if !entry["error"].is_null() {
            // Only Core's "transaction not in this wallet" is actual absence.
            if entry["error"]["code"].as_i64() != Some(-5) {
                return None;
            }
            missing = true;
        } else if entry["result"]["txid"].as_str() != Some(txid.to_string().as_str()) {
            return None;
        }
    }
    Some(!missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRUNE: u64 = 969_938;
    const PRUNED: NodePruning = NodePruning::Pruned {
        prune_height: PRUNE,
    };

    #[test]
    fn recorded_wallet_history_survives_block_pruning() {
        assert_eq!(
            pruned_node_serves(PRUNED, VaultHistory::TrackedLocally),
            Ok(())
        );
        assert_eq!(
            auto_switch_hold(None, Some(PRUNED), Some(VaultHistory::TrackedLocally)),
            None
        );
        assert_eq!(
            auto_switch_hold(
                Some(RunningScan::Rescan),
                Some(PRUNED),
                Some(VaultHistory::TrackedLocally)
            ),
            Some(SwitchHold::Scanning(RunningScan::Rescan))
        );
    }

    fn txids() -> Vec<coincube_core::miniscript::bitcoin::Txid> {
        use coincube_core::miniscript::bitcoin::{hashes::Hash, Txid};
        vec![
            Txid::from_byte_array([1; 32]),
            Txid::from_byte_array([2; 32]),
        ]
    }

    fn recorded() -> serde_json::Value {
        let ids = txids();
        // Core may return batch responses in any order.
        serde_json::json!([
            {"id":1, "result":{"txid":ids[1].to_string()}},
            {"id":0, "result":{"txid":ids[0].to_string()}}
        ])
    }

    #[test]
    fn wallet_records_requires_every_funding_and_spending_transaction() {
        let ids = txids();
        assert_eq!(wallet_records(&recorded(), &ids), Some(true));
        let mut response = recorded();
        response[0] = serde_json::json!({"id":1,"error":{"code":-5,"message":"Invalid or non-wallet transaction id"}});
        assert_eq!(wallet_records(&response, &ids), Some(false));
        for code in [-18, -19, -28, -32603] {
            response[0]["error"]["code"] = serde_json::json!(code);
            assert_eq!(wallet_records(&response, &ids), None);
        }
        assert_eq!(
            wallet_records(&serde_json::json!([recorded()[0]]), &ids),
            None
        );
        response = recorded();
        response[0]["id"] = serde_json::json!(0);
        assert_eq!(wallet_records(&response, &ids), None);
        response = recorded();
        response[0]["result"]["txid"] = serde_json::json!(ids[0].to_string());
        assert_eq!(wallet_records(&response, &ids), None);
    }

    #[tokio::test]
    async fn local_wallet_probe_reads_existing_records_instead_of_old_blocks() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        let wallets = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/")
                    .json_body_partial(r#"{"method":"listwallets"}"#);
                then.status(200)
                    .json_body(serde_json::json!({"result":["existing-vault"]}));
            })
            .await;
        let records=server.mock_async(|when,then| {
            when.method(POST).path("/wallet/existing-vault").json_body(serde_json::json!([
                {"jsonrpc":"2.0","id":0,"method":"gettransaction","params":[txids()[0].to_string()]},
                {"jsonrpc":"2.0","id":1,"method":"gettransaction","params":[txids()[1].to_string()]}
            ]));
            then.status(200).json_body(recorded());
        }).await;
        let cfg = coincubed::config::BitcoindConfig {
            addr: *server.address(),
            rpc_auth: coincubed::config::BitcoindRpcAuth::UserPass(
                "fixture".into(),
                "fixture".into(),
            ),
        };
        assert!(local_wallet_tracks(
            &cfg,
            "existing-vault",
            crate::chain::ChainId::Bitcoin,
            &txids()
        )
        .await
        .unwrap());
        wallets.assert_async().await;
        records.assert_async().await;
    }

    #[tokio::test]
    async fn local_wallet_probe_refuses_fork_rpc_before_any_request() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        let rpc = server
            .mock_async(|when, then| {
                when.method(POST);
                then.status(500);
            })
            .await;
        let cfg = coincubed::config::BitcoindConfig {
            addr: *server.address(),
            rpc_auth: coincubed::config::BitcoindRpcAuth::UserPass(
                "fixture".into(),
                "fixture".into(),
            ),
        };
        assert!(local_wallet_tracks(
            &cfg,
            "fork-wallet",
            crate::chain::ChainId::BitcoinBlake2b,
            &txids()
        )
        .await
        .is_err());
        rpc.assert_hits_async(0).await;
    }

    #[tokio::test]
    async fn local_wallet_probe_loads_existing_wallet_without_creating_one() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        let path =
            std::env::temp_dir().join(format!("local-wallet-probe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        let wallet = path.to_str().unwrap();
        server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/")
                    .json_body_partial(r#"{"method":"listwallets"}"#);
                then.status(200).json_body(serde_json::json!({"result":[]}));
            })
            .await;
        let load = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/")
                    .json_body_partial(r#"{"method":"loadwallet"}"#);
                then.status(200)
                    .json_body(serde_json::json!({"result":{"name":wallet}}));
            })
            .await;
        let records=server.mock_async(|when,then| {
            when.method(POST).json_body(serde_json::json!([
                {"jsonrpc":"2.0","id":0,"method":"gettransaction","params":[txids()[0].to_string()]},
                {"jsonrpc":"2.0","id":1,"method":"gettransaction","params":[txids()[1].to_string()]}
            ]));
            then.status(200).json_body(recorded());
        }).await;
        let cfg = coincubed::config::BitcoindConfig {
            addr: *server.address(),
            rpc_auth: coincubed::config::BitcoindRpcAuth::UserPass(
                "fixture".into(),
                "fixture".into(),
            ),
        };
        assert!(
            local_wallet_tracks(&cfg, wallet, crate::chain::ChainId::Bitcoin, &txids())
                .await
                .unwrap()
        );
        load.assert_async().await;
        records.assert_async().await;
        std::fs::remove_dir(&path).unwrap();
        // Missing local wallet is actual unavailable history, with no create/import RPC.
        assert!(
            !local_wallet_tracks(&cfg, wallet, crate::chain::ChainId::Bitcoin, &txids())
                .await
                .unwrap()
        );
        load.assert_hits_async(1).await;
        records.assert_hits_async(1).await;
    }

    #[test]
    fn unpruned_node_serves_any_history() {
        for history in [
            VaultHistory::From(1),
            VaultHistory::NoConfirmedCoins { rescan_owed: true },
            VaultHistory::NoConfirmedCoins { rescan_owed: false },
        ] {
            assert_eq!(pruned_node_serves(NodePruning::Unpruned, history), Ok(()));
        }
    }

    #[test]
    fn pruned_node_serves_coins_a_margin_above_its_prune_height() {
        let margin = PRUNE_MARGIN_BLOCKS as u32;
        // Exactly the margin: the rescan look-back still lands on kept blocks.
        assert_eq!(
            pruned_node_serves(PRUNED, VaultHistory::From(PRUNE as u32 + margin)),
            Ok(())
        );
        assert_eq!(
            pruned_node_serves(PRUNED, VaultHistory::From(PRUNE as u32 + 500)),
            Ok(())
        );
    }

    #[test]
    fn pruned_node_refuses_coins_inside_the_margin() {
        let margin = PRUNE_MARGIN_BLOCKS as u32;
        for earliest in [PRUNE as u32, PRUNE as u32 + 1, PRUNE as u32 + margin - 1] {
            assert_eq!(
                pruned_node_serves(PRUNED, VaultHistory::From(earliest)),
                Err(PrunedHistory::CoinsBelowPrune {
                    prune_height: PRUNE,
                    earliest,
                }),
                "{}",
                earliest
            );
        }
        // The copy does not claim the coin is below the prune height.
        let copy = PrunedHistory::CoinsBelowPrune {
            prune_height: PRUNE,
            earliest: PRUNE as u32 + 10,
        }
        .to_string();
        assert!(copy.contains("too close"), "{}", copy);
        assert!(!copy.contains("cannot rescan"), "{}", copy);
    }

    #[test]
    fn first_address_scan_is_pending_until_a_poll_this_session() {
        // Never polled, startup unknown: the first-ever scan.
        assert!(first_address_scan_pending(true, None, None));
        assert!(first_address_scan_pending(true, Some(100), Some(100)));
        assert!(first_address_scan_pending(true, None, Some(100)));
        assert!(!first_address_scan_pending(true, Some(200), Some(100)));
        assert!(!first_address_scan_pending(true, Some(200), None));
        // A local bitcoind does no address scan.
        assert!(!first_address_scan_pending(false, None, None));
    }

    #[test]
    fn pruned_node_refuses_coins_below_its_prune_height() {
        // The 2026-10-06 incident: coins at 958601 and 960326, prune height 969938.
        let history = VaultHistory::from_coin_heights([Some(960_326), Some(958_601)], false);
        assert_eq!(history, VaultHistory::From(958_601));
        assert_eq!(
            pruned_node_serves(PRUNED, history),
            Err(PrunedHistory::CoinsBelowPrune {
                prune_height: PRUNE,
                earliest: 958_601,
            })
        );
        // One block short is still short.
        assert!(pruned_node_serves(PRUNED, VaultHistory::From(PRUNE as u32 - 1)).is_err());
    }

    #[test]
    fn spent_only_history_still_counts() {
        // Every coin spent: the heights still come from the daemon's full
        // coin list, so the history is known and dated.
        let history = VaultHistory::from_coin_heights([Some(958_601), None], true);
        assert_eq!(history, VaultHistory::From(958_601));
        assert!(matches!(
            pruned_node_serves(PRUNED, history),
            Err(PrunedHistory::CoinsBelowPrune { .. })
        ));
        // ... and spent coins above the prune height are fine.
        let history = VaultHistory::from_coin_heights([Some(971_000)], false);
        assert_eq!(pruned_node_serves(PRUNED, history), Ok(()));
    }

    #[test]
    fn no_coins_with_a_pending_rescan_is_unknown_history() {
        // Unconfirmed coins have no height and date nothing.
        let history = VaultHistory::from_coin_heights([None], true);
        assert_eq!(
            history,
            VaultHistory::NoConfirmedCoins { rescan_owed: true }
        );
        assert_eq!(
            pruned_node_serves(PRUNED, history),
            Err(PrunedHistory::HistoryUnknown {
                prune_height: PRUNE
            })
        );
    }

    #[test]
    fn no_coins_and_no_obligation_is_a_new_vault() {
        let history = VaultHistory::from_coin_heights(std::iter::empty(), false);
        assert_eq!(
            history,
            VaultHistory::NoConfirmedCoins { rescan_owed: false }
        );
        assert_eq!(pruned_node_serves(PRUNED, history), Ok(()));
    }

    #[test]
    fn pruning_is_read_from_blockchain_info() {
        let read = |v: serde_json::Value| NodePruning::from_blockchain_info(&v);
        assert_eq!(
            read(serde_json::json!({"pruned": false})),
            Some(NodePruning::Unpruned)
        );
        assert_eq!(
            read(serde_json::json!({"pruned": true, "pruneheight": PRUNE})),
            Some(PRUNED)
        );
        // Not saying is not "unpruned".
        assert_eq!(read(serde_json::json!({"pruned": true})), None);
        assert_eq!(read(serde_json::json!({})), None);
    }

    #[test]
    fn running_scan_reports_a_rescan_first() {
        assert_eq!(running_scan(false, false), None);
        assert_eq!(running_scan(true, false), Some(RunningScan::WalletSync));
        assert_eq!(running_scan(false, true), Some(RunningScan::Rescan));
        assert_eq!(running_scan(true, true), Some(RunningScan::Rescan));
    }

    #[test]
    fn auto_switch_waits_for_a_running_scan() {
        // Even on an unpruned node: switching would discard the scan.
        assert_eq!(
            auto_switch_hold(
                Some(RunningScan::WalletSync),
                Some(NodePruning::Unpruned),
                None
            ),
            Some(SwitchHold::Scanning(RunningScan::WalletSync))
        );
        assert_eq!(
            auto_switch_hold(Some(RunningScan::Rescan), Some(PRUNED), None),
            Some(SwitchHold::Scanning(RunningScan::Rescan))
        );
        // Once the scan is over, an unpruned node is switched to.
        assert_eq!(
            auto_switch_hold(None, Some(NodePruning::Unpruned), None),
            None
        );
    }

    #[test]
    fn auto_switch_needs_pruning_and_history_known() {
        assert_eq!(
            auto_switch_hold(None, None, None),
            Some(SwitchHold::Unknown)
        );
        assert_eq!(
            auto_switch_hold(None, Some(PRUNED), None),
            Some(SwitchHold::Unknown)
        );
    }

    #[test]
    fn auto_switch_refuses_a_node_pruned_past_the_vault() {
        assert_eq!(
            auto_switch_hold(None, Some(PRUNED), Some(VaultHistory::From(958_601))),
            Some(SwitchHold::Pruned(PrunedHistory::CoinsBelowPrune {
                prune_height: PRUNE,
                earliest: 958_601,
            }))
        );
        assert_eq!(
            auto_switch_hold(None, Some(PRUNED), Some(VaultHistory::From(971_000))),
            None
        );
    }

    #[test]
    fn refusal_copy_only_offers_supported_actions() {
        for reason in [
            PrunedHistory::CoinsBelowPrune {
                prune_height: PRUNE,
                earliest: 958_601,
            },
            PrunedHistory::CoinsBelowPrune {
                prune_height: PRUNE,
                earliest: PRUNE as u32 + 1,
            },
            PrunedHistory::HistoryUnknown {
                prune_height: PRUNE,
            },
        ] {
            let copy = reason.to_string();
            assert!(copy.contains("COINCUBE | Connect"));
            assert!(!copy.contains("re-sync"));
            assert!(!copy.contains("hide"));
        }
    }

    #[test]
    fn refusal_copy_names_both_heights() {
        let copy = PrunedHistory::CoinsBelowPrune {
            prune_height: PRUNE,
            earliest: 958_601,
        }
        .to_string();
        assert!(copy.contains("969938"), "{}", copy);
        assert!(copy.contains("958601"), "{}", copy);
    }
}
