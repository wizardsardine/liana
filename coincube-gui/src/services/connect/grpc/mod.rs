pub mod device;
pub mod interceptor;
pub mod session;
pub mod stream;

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

/// Create a TLS-enabled tonic channel to the gRPC endpoint.
pub async fn create_channel(
    grpc_url: &str,
) -> Result<Channel, Box<dyn std::error::Error + Send + Sync>> {
    let tls = ClientTlsConfig::new();
    let channel = Channel::from_shared(grpc_url.to_string())?
        .tls_config(tls)?
        .connect()
        .await?;
    Ok(channel)
}
