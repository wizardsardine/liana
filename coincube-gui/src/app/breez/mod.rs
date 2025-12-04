// breez/mod.rs
pub use crate::app::breez::client::{BreezClient, BreezPaymentRequest, BreezReceiveRequest};
pub use crate::app::breez::config::BreezConfig;

#[derive(thiserror::Error, Debug)]
pub enum BreezError {
    #[error("Breez API key missing (set BREEZ_API_KEY)")]
    MissingApiKey,
    #[error("failed to connect Breez SDK: {0}")]
    Connection(String),
    #[error("SDK request failed: {0}")]
    Sdk(String),
}