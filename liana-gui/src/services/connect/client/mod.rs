pub mod auth;
pub mod backend;
pub mod cache;

use liana::miniscript::bitcoin;

use serde::Deserialize;

const LIANALITE_SIGNET_URL: &str = "https://api.signet.lianalite.com";
const LIANALITE_MAINNET_URL: &str = "https://api.lianalite.com";

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

pub async fn get_service_config(
    network: bitcoin::Network,
) -> Result<ServiceConfig, reqwest::Error> {
    let backend_api_url = if network == bitcoin::Network::Bitcoin {
        LIANALITE_MAINNET_URL
    } else {
        LIANALITE_SIGNET_URL
    };
    let res: ServiceConfigResource = reqwest::get(format!("{}/v1/desktop", backend_api_url))
        .await?
        .json()
        .await?;
    Ok(ServiceConfig {
        auth_api_url: res.auth_api_url,
        auth_api_public_key: res.auth_api_public_key,
        backend_api_url: backend_api_url.to_string(),
    })
}
