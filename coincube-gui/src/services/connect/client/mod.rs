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
}

/// COINCUBE-owned Connect service config (`GET /api/v1/connect/service-config`).
/// Source of the Connect signing gRPC endpoint for **all** networks — replaces
/// the per-network lianalite `grpc_url`. The endpoint is network-agnostic (one
/// gRPC deployment serves every network), so we don't pass a `network` query.
#[derive(Debug, Clone, Deserialize)]
struct CoincubeServiceConfig {
    #[serde(rename = "grpcUrl")]
    grpc_url: Option<String>,
    #[serde(default)]
    tls: bool,
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
    // NOTE: the lianalite `/v1/desktop` call remains only for the auth +
    // remote-backend fields, which are tracked for removal separately
    // (see plans/liana-api-removal-rust.md). The signing `grpc_url` no
    // longer comes from lianalite.
    let res: ServiceConfigResource =
        reqwest::get(format!("{}/v1/desktop", default_backend_api_url))
            .await?
            .json()
            .await?;
    // gRPC endpoint discovery, COINCUBE-only, in precedence order:
    //   1. runtime env override (staging/local),
    //   2. COINCUBE service-config endpoint (env-specific, all networks),
    //   3. compile-time baked default (resilience if the endpoint is down).
    let grpc_url = match runtime_grpc_url_override() {
        Some(u) => Some(u),
        None => match coincube_grpc_url().await {
            Some(u) => Some(u),
            None => buildtime_grpc_url_default(),
        },
    };
    Ok(ServiceConfig {
        auth_api_url: res.auth_api_url,
        auth_api_public_key: res.auth_api_public_key,
        backend_api_url: runtime_backend_api_url_override()
            .unwrap_or_else(|| default_backend_api_url.to_string()),
        grpc_url,
    })
}

/// Fetch the Connect signing gRPC URL from coincube-api. Best-effort: any
/// network/parse failure yields `None` so the caller can fall back, and we
/// never fall back to lianalite. The returned URL is normalized to a
/// tonic-ready form (`https://…` when TLS, `http://…` otherwise) so
/// `create_channel` enables TLS correctly.
async fn coincube_grpc_url() -> Option<String> {
    let base = crate::services::coincube_api_base_url();
    let url = format!("{base}/api/v1/connect/service-config");
    let cfg: CoincubeServiceConfig = reqwest::get(url).await.ok()?.json().await.ok()?;
    let raw = cfg.grpc_url?;
    normalize_grpc_url(&raw, cfg.tls)
}

/// Compile-time baked gRPC URL (`COINCUBE_CONNECT_GRPC_URL` at build time),
/// used only when the service-config endpoint is unreachable. `None` when not
/// baked in.
fn buildtime_grpc_url_default() -> Option<String> {
    // Normalize like the service-config path so a baked bare `host:port`
    // still enables TLS in `create_channel` (which keys on the `https://`
    // prefix). A scheme-qualified value is passed through unchanged;
    // default to TLS otherwise — plaintext dev endpoints must spell out
    // `http://`.
    option_env!("COINCUBE_CONNECT_GRPC_URL").and_then(|v| normalize_grpc_url(v, true))
}

/// Normalize a gRPC endpoint to a scheme-qualified URL tonic accepts. Returns
/// `None` for an empty value. An already-qualified `http(s)://` value is kept
/// as-is; otherwise the scheme is chosen from `tls`.
fn normalize_grpc_url(raw: &str, tls: bool) -> Option<String> {
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() {
        return None;
    }
    if raw.starts_with("http://") || raw.starts_with("https://") {
        Some(raw.to_string())
    } else if tls {
        Some(format!("https://{raw}"))
    } else {
        Some(format!("http://{raw}"))
    }
}

fn runtime_grpc_url_override() -> Option<String> {
    // Normalize like the service-config path so a bare `host:port` from the
    // env override still enables TLS in `create_channel` (which keys on the
    // `https://` prefix). Scheme-qualified values pass through unchanged;
    // default to TLS otherwise — plaintext/local endpoints must spell out
    // `http://`.
    std::env::var(CONNECT_GRPC_URL_ENV)
        .ok()
        .or_else(|| std::env::var(LEGACY_GRPC_URL_ENV).ok())
        .and_then(|v| normalize_grpc_url(&v, true))
}

fn runtime_backend_api_url_override() -> Option<String> {
    std::env::var("COINCUBE_API_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::normalize_grpc_url;

    #[test]
    fn normalize_adds_https_when_tls() {
        assert_eq!(
            normalize_grpc_url("grpc.coincube.io:443", true).as_deref(),
            Some("https://grpc.coincube.io:443")
        );
    }

    #[test]
    fn normalize_adds_http_when_not_tls() {
        assert_eq!(
            normalize_grpc_url("localhost:50051", false).as_deref(),
            Some("http://localhost:50051")
        );
    }

    #[test]
    fn normalize_keeps_existing_scheme_and_trims_slash() {
        assert_eq!(
            normalize_grpc_url("https://grpc.coincube.io:443/", false).as_deref(),
            Some("https://grpc.coincube.io:443")
        );
        assert_eq!(
            normalize_grpc_url("http://host:1/", true).as_deref(),
            Some("http://host:1")
        );
    }

    #[test]
    fn normalize_rejects_empty() {
        assert_eq!(normalize_grpc_url("   ", true), None);
        assert_eq!(normalize_grpc_url("", false), None);
    }
}
