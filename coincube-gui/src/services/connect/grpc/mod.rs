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
    let tls = ClientTlsConfig::new();
    Ok(Channel::from_shared(grpc_url.to_string())?
        .tls_config(tls)?
        .connect()
        .await?)
}
