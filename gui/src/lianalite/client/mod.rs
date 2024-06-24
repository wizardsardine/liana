pub mod auth;
pub mod backend;

use liana::miniscript::bitcoin;

use serde::Deserialize;

const LIANALITE_SIGNET_URL: &str = "https://signet.lianalite.com";
const LIANALITE_MAINNET_URL: &str = "https://lianalite.com";

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceConfig {
    pub auth_api_url: String,
    pub auth_api_public_key: String,
    pub backend_api_url: String,
}

pub async fn get_service_config(
    network: bitcoin::Network,
) -> Result<ServiceConfig, reqwest::Error> {
    reqwest::get(format!(
        "{}/api/env",
        if network == bitcoin::Network::Bitcoin {
            LIANALITE_MAINNET_URL
        } else {
            LIANALITE_SIGNET_URL
        }
    ))
    .await?
    .json()
    .await
}
