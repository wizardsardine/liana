//! Thin Esplora client used to surface live confirmation counts for
//! pending Spark on-chain deposits.
//!
//! The Breez Spark SDK only exposes `is_mature: bool` on each
//! `DepositInfo` — it never reports the *count* of confirmations a
//! deposit currently has. To render "1 / 3 confirmations" in the
//! Pending Deposits card we query a public Esplora REST endpoint
//! ourselves: one GET per pending txid plus one GET for the chain tip,
//! turning `(tip - block_height + 1)` into a confirmation count (`0`
//! when the tx is still in mempool).
//!
//! Mainnet → mempool.space's Esplora. Regtest has no public Esplora,
//! so the helper returns an empty map there and the view falls back to
//! the SDK's plain "Waiting for confirmation" wording.
//!
//! All errors are swallowed (logged at `warn`) and the affected
//! deposit simply omits its confirmation count from the result map —
//! the row degrades gracefully to the SDK-driven status text.
//!
//! The 3-confirmation maturity threshold comes from Breez's own docs
//! (`docs/breez-sdk/src/guide/onchain_claims.md`): "after **3 on-chain
//! confirmations** the deposit has sufficient confirmations".

use std::collections::HashMap;
use std::time::Duration;

use coincube_core::miniscript::bitcoin::Network;
use serde::Deserialize;

/// Number of on-chain confirmations the Spark SDK requires before
/// `is_mature` flips and the SDK auto-claims the deposit.
pub const DEPOSIT_MATURITY_CONFIRMATIONS: u32 = 3;

const ESPLORA_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Deserialize)]
struct TxStatus {
    confirmed: bool,
    block_height: Option<u32>,
}

fn esplora_base(network: Network) -> Option<&'static str> {
    match network {
        Network::Bitcoin => Some("https://mempool.space/api"),
        // Spark only supports Mainnet + Regtest in this Cube. Regtest
        // runs against a local SDK fixture chain with no public
        // Esplora — skip the fetch entirely there.
        _ => None,
    }
}

/// Whether [`fetch_confirmations`] has anywhere to query for `network`.
/// Callers short-circuit on `false` to avoid scheduling work that's
/// guaranteed to come back empty (and to suppress the 30s poll
/// subscription on regtest).
pub fn is_supported(network: Network) -> bool {
    esplora_base(network).is_some()
}

fn confirmation_count(status: &TxStatus, tip_height: u32) -> u32 {
    if !status.confirmed {
        return 0;
    }
    status
        .block_height
        .map(|h| tip_height.saturating_sub(h).saturating_add(1))
        .unwrap_or(1)
}

/// Fetch the current confirmation count for each `(txid, vout)` against
/// the given network's Esplora.
///
/// Returns a map keyed by `(txid, vout)`. Missing entries mean the
/// fetch failed for that deposit (network error, tx not found, …) —
/// the caller renders the SDK's fallback status text in that case.
pub async fn fetch_confirmations(
    network: Network,
    deposits: Vec<(String, u32)>,
) -> HashMap<(String, u32), u32> {
    let mut out = HashMap::new();
    if deposits.is_empty() {
        return out;
    }
    let Some(base) = esplora_base(network) else {
        return out;
    };

    let client = match reqwest::Client::builder().timeout(ESPLORA_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("esplora: failed to build reqwest client: {e}");
            return out;
        }
    };

    let tip_height: u32 = match client
        .get(format!("{base}/blocks/tip/height"))
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        Ok(resp) => match resp.text().await {
            Ok(body) => match body.trim().parse() {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!("esplora: tip height parse failed ({body:?}): {e}");
                    return out;
                }
            },
            Err(e) => {
                tracing::warn!("esplora: tip height body read failed: {e}");
                return out;
            }
        },
        Err(e) => {
            tracing::warn!("esplora: tip height fetch failed: {e}");
            return out;
        }
    };

    for (txid, vout) in deposits {
        let url = format!("{base}/tx/{txid}/status");
        match client
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            Ok(resp) => match resp.json::<TxStatus>().await {
                Ok(status) => {
                    let confs = confirmation_count(&status, tip_height);
                    out.insert((txid, vout), confs);
                }
                Err(e) => tracing::warn!("esplora: status decode failed for {txid}: {e}"),
            },
            Err(e) => tracing::warn!("esplora: status fetch failed for {txid}: {e}"),
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_matrix_only_enables_mainnet_esplora() {
        assert!(is_supported(Network::Bitcoin));
        assert!(!is_supported(Network::Regtest));
        assert!(!is_supported(Network::Testnet));
        assert!(!is_supported(Network::Signet));
    }

    #[test]
    fn confirmation_count_tracks_confirmed_mempool_and_missing_height_cases() {
        assert_eq!(
            confirmation_count(
                &TxStatus {
                    confirmed: true,
                    block_height: Some(98),
                },
                100,
            ),
            3
        );
        assert_eq!(
            confirmation_count(
                &TxStatus {
                    confirmed: false,
                    block_height: None,
                },
                100,
            ),
            0
        );
        assert_eq!(
            confirmation_count(
                &TxStatus {
                    confirmed: true,
                    block_height: None,
                },
                100,
            ),
            1
        );
    }

    #[test]
    fn confirmation_count_saturates_for_inconsistent_tip_data() {
        assert_eq!(
            confirmation_count(
                &TxStatus {
                    confirmed: true,
                    block_height: Some(101),
                },
                100,
            ),
            1
        );
    }

    #[tokio::test]
    async fn fetch_confirmations_short_circuits_without_network_for_empty_or_unsupported_inputs() {
        assert!(fetch_confirmations(Network::Bitcoin, Vec::new())
            .await
            .is_empty());
        assert!(
            fetch_confirmations(Network::Regtest, vec![("txid".to_string(), 0)])
                .await
                .is_empty()
        );
    }
}
