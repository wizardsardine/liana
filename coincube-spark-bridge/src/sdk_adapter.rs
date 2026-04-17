//! Thin wrapper around [`breez_sdk_spark::BreezSdk`] with a coincube-friendly
//! constructor and an `Arc`-friendly handle.
//!
//! Everything the bridge's server loop (or the smoke-test harness) needs
//! from the SDK goes through here so the SDK's type surface doesn't leak
//! into the JSON-RPC layer.

use std::sync::Arc;

use breez_sdk_spark::{
    connect, default_config, BreezSdk, ConnectRequest, Network as SparkNetwork, Seed,
    StableBalanceConfig, StableBalanceToken,
};

/// Mainnet USDB token identifier. Published by Breez in the Stable
/// Balance guide at
/// <https://sdk-doc-spark.breez.technology/guide/stable_balance.html>.
/// Using a hardcoded constant rather than an env var is deliberate:
/// the identifier is stable across deployments and leaking the
/// knob to ops would just mean a way to silently misconfigure a
/// production wallet.
const USDB_MAINNET_TOKEN_IDENTIFIER: &str =
    "btkn1xgrvjwey5ngcagvap2dzzvsy4uk8ua9x69k82dwvt5e7ef9drm9qztux87";

/// Integrator-defined label used to reference the USDB token in
/// [`breez_sdk_spark::UpdateUserSettingsRequest`]. This string is
/// plumbing, not user copy — the gui always renders the feature as
/// "Stable Balance" without leaking this label.
pub const STABLE_BALANCE_LABEL: &str = "USDB";

/// Cloneable SDK handle. The inner [`BreezSdk`] is `Send + Sync`, so the
/// bridge can freely share it across async tasks serving different
/// JSON-RPC requests concurrently.
#[derive(Clone)]
pub struct SdkHandle {
    pub sdk: Arc<BreezSdk>,
}

/// Build a mainnet Spark SDK config with the given API key.
///
/// Phase 6: always wires up [`StableBalanceConfig`] with the single
/// USDB token. `default_active_label` is `None` so Stable Balance
/// starts deactivated — the user opts in explicitly via the Spark
/// Settings toggle, which then calls `update_user_settings`.
/// Omitting `default_active_label` also means existing users keep
/// their previous state (persisted by the SDK locally) across
/// restarts.
pub fn mainnet_config(api_key: String) -> breez_sdk_spark::Config {
    let mut config = default_config(SparkNetwork::Mainnet);
    config.api_key = Some(api_key);
    config.stable_balance_config = Some(StableBalanceConfig {
        tokens: vec![StableBalanceToken {
            label: STABLE_BALANCE_LABEL.to_string(),
            token_identifier: USDB_MAINNET_TOKEN_IDENTIFIER.to_string(),
        }],
        default_active_label: None,
        threshold_sats: None,
        max_slippage_bps: None,
    });
    config
}

/// Connect to Spark mainnet with the given mnemonic.
///
/// `storage_dir` must be a writable directory — the SDK uses it for its
/// internal sqlite database, so picking the same dir twice from two
/// processes will collide.
pub async fn connect_mainnet(
    api_key: String,
    mnemonic: String,
    passphrase: Option<String>,
    storage_dir: String,
) -> anyhow::Result<SdkHandle> {
    let config = mainnet_config(api_key);
    let request = ConnectRequest {
        config,
        seed: Seed::Mnemonic {
            mnemonic,
            passphrase,
        },
        storage_dir,
    };
    let sdk = connect(request).await?;
    Ok(SdkHandle { sdk: Arc::new(sdk) })
}
