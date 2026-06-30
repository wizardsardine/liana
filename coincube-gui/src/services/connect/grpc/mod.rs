pub mod bootstrap;
pub mod device;
pub mod interceptor;
pub mod session;
pub mod stream;

use tonic::codegen::http::uri::InvalidUri;
use tonic::transport::{Channel, ClientTlsConfig};

pub mod connect_v1 {
    tonic::include_proto!("connect.v1");
}

pub use connect_v1::*;

/// Messages emitted by the realtime gRPC stream, routed through Iced subscriptions.
#[derive(Debug, Clone)]
pub enum ConnectStreamMessage {
    Connected,
    SessionEvent(connect_v1::SessionEvent),
    Disconnected(String),
    Error(String),
    /// Duress was activated elsewhere (Keychain Settings, email-link, Approach
    /// C, another desktop's PIN). Phase 7b: the receiving desktop locks into the
    /// cryptic screen but does **NOT** wipe — remote activation can be
    /// accidental, and wiping then would be too destructive.
    DuressActivated {
        unlock_at: Option<chrono::DateTime<chrono::Utc>>,
        source: String,
    },
    /// Duress was cleared server-side — exit the cryptic screen.
    DuressCleared,
    /// Duress was *disabled* (turned off) account-wide (Issue 2). Every device
    /// disarms its local enrollment — no UI lock, no wipe. `account_id`
    /// identifies the account so a device only disarms a matching local
    /// (Connect) enrollment, never an unrelated sovereign one.
    DuressDisabled {
        account_id: String,
    },
}

#[derive(Debug)]
pub enum CreateChannelError {
    InvalidUri(InvalidUri),
    Transport(tonic::transport::Error),
}

impl std::fmt::Display for CreateChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUri(e) => write!(f, "invalid gRPC URL: {}", e),
            Self::Transport(e) => write!(f, "gRPC transport error: {}", e),
        }
    }
}

impl std::error::Error for CreateChannelError {}

impl From<InvalidUri> for CreateChannelError {
    fn from(e: InvalidUri) -> Self {
        Self::InvalidUri(e)
    }
}

impl From<tonic::transport::Error> for CreateChannelError {
    fn from(e: tonic::transport::Error) -> Self {
        Self::Transport(e)
    }
}

/// Create a TLS-enabled tonic channel to the gRPC endpoint.
pub async fn create_channel(grpc_url: &str) -> Result<Channel, CreateChannelError> {
    let endpoint = Channel::from_shared(grpc_url.to_string())?;
    let endpoint = if grpc_url.starts_with("https://") {
        let tls = ClientTlsConfig::new().with_native_roots();
        endpoint.tls_config(tls)?
    } else {
        endpoint
    };
    Ok(endpoint.connect().await?)
}
