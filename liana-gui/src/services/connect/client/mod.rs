pub mod auth;
pub mod backend;
pub mod cache;

use liana::miniscript::bitcoin::{self, Network};

use serde::Deserialize;

const DEFAULT_CONNECT_SIGNET_URL: &str = "https://api.connect.signet.lianawallet.com";
const DEFAULT_CONNECT_MAINNET_URL: &str = "https://api.connect.lianawallet.com";

pub const BUSINESS_MAINNET_API_URL: &str = "https://api.business.lianawallet.com";
pub const BUSINESS_SIGNET_API_URL: &str = "https://api.signet.business.lianawallet.com";

#[derive(Debug, Clone, Deserialize)]
struct ServiceConfigResource {
    pub auth_api_url: String,
    pub auth_api_public_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceConfig {
    pub auth_api_url: String,
    pub auth_api_public_key: String,
    pub backend_api_url: String,
}

#[derive(Debug, Clone, Copy)]
pub enum BackendType {
    LianaConnect,
    LianaBusiness,
}

pub async fn get_service_config(
    network: bitcoin::Network,
    backend: BackendType,
) -> Result<ServiceConfig, reqwest::Error> {
    let backend_api_url = match (network, backend) {
        (Network::Bitcoin, BackendType::LianaConnect) => DEFAULT_CONNECT_MAINNET_URL.to_string(),
        (Network::Bitcoin, BackendType::LianaBusiness) => BUSINESS_MAINNET_API_URL.to_string(),
        (_, BackendType::LianaConnect) => std::env::var("LIANALITE_SIGNET_API_URL")
            .unwrap_or_else(|_| DEFAULT_CONNECT_SIGNET_URL.to_string()),
        (_, BackendType::LianaBusiness) => std::env::var("LIANA_BUSINESS_SIGNET_API_URL")
            .unwrap_or_else(|_| BUSINESS_SIGNET_API_URL.to_string()),
    };
    let client = reqwest::Client::new();
    let res: ServiceConfigResource = client
        .get(format!("{}/v1/desktop", backend_api_url))
        .header("User-Agent", format!("liana-gui/{}", crate::VERSION))
        .send()
        .await?
        .json()
        .await?;
    Ok(ServiceConfig {
        auth_api_url: res.auth_api_url,
        auth_api_public_key: res.auth_api_public_key,
        backend_api_url,
    })
}
