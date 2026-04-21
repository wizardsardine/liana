//! Spark wallet panel state machines.
//!
//! One module per Menu::Spark entry — Overview, Send, Receive,
//! Transactions, Settings. Each holds an `Option<Arc<SparkBackend>>`
//! (None when the cube has no Spark signer or the bridge subprocess
//! failed to spawn) and renders an "unavailable" stub in that case.

pub mod overview;
pub mod receive;
pub mod send;
pub mod settings;
pub mod transactions;

pub use overview::SparkOverview;
pub use receive::{SparkReceive, SparkReceiveMethod, SparkReceivePhase};
pub use send::{SparkSend, SparkSendPhase};
pub use settings::SparkSettings;
pub use transactions::SparkTransactions;

use std::sync::Arc;

use coincube_spark_protocol::PaymentSummary;
use iced::Task;

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
        async move { backend.list_payments(Some(20)).await },
        move |result| match result {
            Ok(list) => on_loaded(list.payments),
            Err(e) => on_failed(e.to_string()),
        },
    )
}
