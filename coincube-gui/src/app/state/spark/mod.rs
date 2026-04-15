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
pub use settings::{SparkSettings, SparkSettingsSnapshot};
pub use transactions::SparkTransactions;
