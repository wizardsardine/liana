//! Shared fixtures for vault test modules.

use std::sync::Arc;

use coincube_core::miniscript::bitcoin::Psbt;
use tokio::sync::RwLock;

use crate::services::connect::client::auth::AccessTokenResponse;

/// An empty (no inputs/outputs) unsigned PSBT, suitable as a placeholder in tests.
pub fn empty_psbt() -> Psbt {
    use coincube_core::miniscript::bitcoin::{absolute, transaction, Transaction};

    Psbt::from_unsigned_tx(Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
        input: vec![],
        output: vec![],
    })
    .expect("empty unsigned tx is psbt-compatible")
}

/// A shared, never-expiring set of Connect access tokens for tests.
pub fn tokens() -> Arc<RwLock<AccessTokenResponse>> {
    Arc::new(RwLock::new(AccessTokenResponse {
        access_token: "access".to_string(),
        refresh_token: "refresh".to_string(),
        expires_at: i64::MAX,
    }))
}
