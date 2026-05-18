pub mod auth;
pub mod backend;
pub mod cache;

use coincube_core::miniscript::bitcoin;

use serde::Deserialize;

const LIANALITE_SIGNET_URL: &str = "https://api.signet.lianalite.com";
const LIANALITE_MAINNET_URL: &str = "https://api.lianalite.com";
const CONNECT_GRPC_URL_ENV: &str = "COINCUBE_CONNECT_GRPC_URL";
const LEGACY_GRPC_URL_ENV: &str = "COINCUBE_GRPC_URL";

#[derive(Debug, Clone, Deserialize)]
struct ServiceConfigResource {
    pub auth_api_url: String,
    pub auth_api_public_key: String,
    pub grpc_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceConfig {
    pub auth_api_url: String,
    pub auth_api_public_key: String,
    pub backend_api_url: String,
    pub grpc_url: Option<String>,
}

pub async fn get_service_config(
    network: bitcoin::Network,
) -> Result<ServiceConfig, reqwest::Error> {
    let default_backend_api_url = if network == bitcoin::Network::Bitcoin {
        LIANALITE_MAINNET_URL
    } else {
        LIANALITE_SIGNET_URL
    };
    let res: ServiceConfigResource =
        reqwest::get(format!("{}/v1/desktop", default_backend_api_url))
            .await?
            .json()
            .await?;
    let grpc_url = runtime_grpc_url_override().or(res.grpc_url);
    Ok(ServiceConfig {
        auth_api_url: res.auth_api_url,
        auth_api_public_key: res.auth_api_public_key,
        backend_api_url: runtime_backend_api_url_override()
            .unwrap_or_else(|| default_backend_api_url.to_string()),
        grpc_url,
    })
}

fn runtime_grpc_url_override() -> Option<String> {
    std::env::var(CONNECT_GRPC_URL_ENV)
        .ok()
        .or_else(|| std::env::var(LEGACY_GRPC_URL_ENV).ok())
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
}

fn runtime_backend_api_url_override() -> Option<String> {
    std::env::var("COINCUBE_API_URL")
        .ok()
        .or_else(|| option_env!("COINCUBE_API_URL").map(str::to_string))
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
}
