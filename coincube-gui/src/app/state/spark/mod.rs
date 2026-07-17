//! Spark wallet panel state machines.
//!
//! One module per Menu::Spark entry — Overview, Send, Receive,
//! Transactions, Settings. Each holds an `Option<Arc<SparkBackend>>`
//! (None when the cube has no Spark signer or the bridge subprocess
//! failed to spawn) and renders an "unavailable" stub in that case.

pub mod cross_chain;
pub mod esplora;
pub mod overview;
pub mod receive;
pub mod send;
pub mod settings;
pub mod sideshift_receive;
pub mod transactions;

pub use overview::SparkOverview;
pub use receive::{SparkReceive, SparkReceiveMethod, SparkReceivePhase};
pub use send::{SparkSend, SparkSendPhase};
pub use settings::SparkSettings;
pub use transactions::SparkTransactions;

use std::convert::TryInto;
use std::sync::Arc;

use coincube_spark_protocol::{PaymentSummary, StableBalanceSnapshot};
use iced::Task;

use crate::app::cache::Cache;
use crate::app::message::Message;
use crate::app::wallets::SparkBackend;

/// Fire a `list_payments` RPC on the Spark bridge and route the result
/// through caller-supplied message constructors. Shared by the Send
/// and Receive panels so the "Last transactions" feed stays identical
/// across wallet screens — callers differ only in which panel message
/// they wrap the response in.
pub(crate) fn fetch_payments_task(
    backend: Option<Arc<SparkBackend>>,
    on_loaded: impl FnOnce(Vec<PaymentSummary>) -> Message + Send + 'static,
    on_failed: impl FnOnce(String) -> Message + Send + 'static,
) -> Task<Message> {
    let Some(backend) = backend else {
        return Task::none();
    };
    Task::perform(
        async move { backend.list_payments(Some(20), None).await },
        move |result| match result {
            Ok(list) => on_loaded(list.payments),
            Err(e) => on_failed(e.to_string()),
        },
    )
}

/// Fetch the Spark wallet balance via `get_info`, for the two-card
/// "YOU SEND / YOU RECEIVE" balance line: the BTC balance plus the Stable
/// Balance snapshot, which the caller folds into a unified sats total with
/// [`unified_spark_balance_sats`]. Best-effort UI polish — an error resolves to
/// `None` so the caller keeps its last value rather than surfacing a failure.
pub(crate) fn fetch_balance_task(
    backend: Option<Arc<SparkBackend>>,
    on_result: impl FnOnce(Option<(u64, Option<StableBalanceSnapshot>)>) -> Message + Send + 'static,
) -> Task<Message> {
    let Some(backend) = backend else {
        return Task::none();
    };
    Task::perform(async move { backend.get_info().await }, move |result| {
        on_result(
            result
                .ok()
                .map(|info| (info.balance_sats, info.stable_balance)),
        )
    })
}

/// The unified Spark balance in sats: the BTC balance plus any Stable Balance
/// (USDB) holding, converted at the cache's BTC/USD reference price. Shared by
/// the Home card and the Send/Receive card subtitles so every screen shows the
/// same figure — Stable Balance auto-sweeps BTC into USDB, so a BTC-only
/// reading would understate a funded wallet.
pub(crate) fn unified_spark_balance_sats(
    btc_sats: u64,
    stable: Option<&StableBalanceSnapshot>,
    cache: &Cache,
) -> u64 {
    let reference_price = reference_btc_usd_price(cache);
    let usdb_as_sats = stable
        .map(|sb| {
            crate::app::breez_spark::assets::stable_token_as_sats(
                sb.balance,
                sb.decimals,
                reference_price,
            )
        })
        .unwrap_or(0);
    btc_sats.saturating_add(usdb_as_sats)
}

/// The BTC/USD price used to value USD-pegged tokens (USDB / USDT / USDC) in
/// sats. `btc_usd_price` is only set when the user's fiat preference is USD; for
/// other fiats fall back to the user-fiat converter's per-BTC price so a holding
/// still shows (with a small FX-spread approximation) rather than collapsing to
/// zero. `None` when no price is known. Mirrors `global_home`'s balance handler.
pub(crate) fn reference_btc_usd_price(cache: &Cache) -> Option<f64> {
    cache.btc_usd_price.or_else(|| {
        let converter: Option<crate::app::view::FiatAmountConverter> =
            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
        converter.map(|c| c.price_per_btc())
    })
}
