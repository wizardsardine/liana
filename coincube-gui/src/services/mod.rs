pub mod bitcoin_import;
pub mod branta;
pub mod claim_observation;
pub mod connect;
pub mod connect_esplora;
pub(crate) mod esplora_backoff;
pub mod feeestimation;
pub mod fiat;
pub mod foreign_scan;
#[allow(dead_code)] // Keys admission will consume this read-only device proof.
pub(crate) mod imported_device_identity;
// The signer API is staged behind the external inventory anchor/recovery gate.
// Only owner-verified import removal calls it in production at this head.
#[allow(dead_code)]
pub(crate) mod imported_inventory_anchor;
#[cfg(all(target_os = "macos", debug_assertions, feature = "native-anchor-probe"))]
pub mod imported_keys;
#[cfg(not(all(target_os = "macos", debug_assertions, feature = "native-anchor-probe")))]
pub(crate) mod imported_keys;
// Stored-key signing for imported wallets, offered by the Vault signer
// picker (`feature_flags::IMPORTED_KEY_SIGNING_ENABLED`).
pub(crate) mod imported_signer;
pub mod psbt_file;
pub mod public_account_source;
pub mod public_hardware_source;
// Test fixtures for the imported-wallet hardware binder (`split_hardware`).
#[cfg(test)]
pub(crate) mod public_wallet_pair;
pub mod split_fees;
#[cfg(test)]
pub(crate) mod split_source;
#[cfg(test)]
pub(crate) mod split_test_connect;
#[cfg(test)]
pub(crate) mod split_test_wallets;

pub mod http;

pub mod chain_status;
pub mod coincube;
pub mod duress;
pub mod entangled;
pub mod inheritance;
pub mod mavapay;
pub mod meld;
pub mod passkey;
pub mod recovery;
pub mod sideshift;
pub mod split_coins;
pub mod unlock;

/// Resolves the Coincube API base URL with this precedence:
/// 1. Runtime `std::env::var("COINCUBE_API_URL")` — picked up from the shell or
///    the `.env` loaded in `main()`; change and restart without rebuilding.
/// 2. Compile-time `option_env!("COINCUBE_API_URL")` — values baked in by
///    `build.rs` from `.env` at build time. Release builds started from a
///    directory that has no `.env` still work via this path.
/// 3. Hardcoded `https://api.coincube.io` (production) as a debug fallback.
///    COINCUBE has no dev API server; point COINCUBE_API_URL at a local API
///    (`http://127.0.0.1:8080`) for backend work. Release builds require the
///    env var at build time via `env!`, so they never reach step 3.
///
/// Any trailing slashes are trimmed so callers can safely use
/// `format!("{}/api/v1/...", base)` without producing double-slash paths.
pub fn coincube_api_base_url() -> String {
    let runtime = std::env::var("COINCUBE_API_URL").ok();
    resolve_api_base_url(
        runtime.as_deref(),
        option_env!("COINCUBE_API_URL"),
        &default_base_url(),
    )
}

/// `runtime` is `Some` when `COINCUBE_API_URL` is set, including to an empty
/// string. An empty runtime value is ignored so a blank shell override cannot
/// erase the baked or production base. A non-empty one wins, then trailing
/// slashes are removed so `format!("{base}/api/v1/...")` cannot double-slash.
fn resolve_api_base_url(runtime: Option<&str>, compiled: Option<&str>, fallback: &str) -> String {
    let raw = match runtime {
        Some(value) if !value.is_empty() => value,
        _ => compiled.unwrap_or(fallback),
    };
    let url = raw.trim_end_matches('/').to_string();
    // A release binary must not follow COINCUBE_API_URL onto cleartext HTTP.
    // Debug builds keep http:// for local API servers.
    #[cfg(not(debug_assertions))]
    if url.starts_with("http://") {
        panic!(
            "COINCUBE_API_URL must use https:// in release builds, got {}",
            url
        );
    }
    url
}

fn default_base_url() -> String {
    #[cfg(debug_assertions)]
    {
        "https://api.coincube.io".to_string()
    }
    #[cfg(not(debug_assertions))]
    {
        // Release builds must have COINCUBE_API_URL set at build time.
        env!("COINCUBE_API_URL").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_api_base_url;

    const PRODUCTION: &str = "https://api.coincube.io";
    const BAKED: &str = "https://baked.example";

    fn service_config(base: &str) -> String {
        format!("{base}/api/v1/connect/service-config")
    }

    #[test]
    fn runtime_override_wins_and_trailing_slashes_do_not_double_the_api_path() {
        let base = resolve_api_base_url(
            Some("https://local.example:8080///"),
            Some(BAKED),
            PRODUCTION,
        );
        assert_eq!(base, "https://local.example:8080");
        assert_eq!(
            service_config(&base),
            "https://local.example:8080/api/v1/connect/service-config"
        );
        // A path prefix is kept; only the trailing slash is removed.
        assert_eq!(
            resolve_api_base_url(Some("https://local.example/prefix/"), None, PRODUCTION),
            "https://local.example/prefix"
        );
    }

    #[test]
    fn an_empty_runtime_override_falls_through_instead_of_becoming_the_base() {
        let baked = format!("{BAKED}/");
        assert_eq!(
            resolve_api_base_url(Some(""), Some(baked.as_str()), PRODUCTION),
            BAKED
        );
        assert_eq!(resolve_api_base_url(Some(""), None, PRODUCTION), PRODUCTION);
        assert_eq!(resolve_api_base_url(None, Some(BAKED), PRODUCTION), BAKED);
        // A baked empty string is present, so it is not replaced by production.
        assert_eq!(resolve_api_base_url(None, Some(""), PRODUCTION), "");
        assert_eq!(
            resolve_api_base_url(Some(""), Some(""), "https://fallback.example/"),
            ""
        );
    }

    #[test]
    fn missing_runtime_and_baked_values_use_the_supplied_fallback() {
        let base = resolve_api_base_url(None, None, "https://fallback.example/");
        assert_eq!(base, "https://fallback.example");
        assert_eq!(
            service_config(&base),
            "https://fallback.example/api/v1/connect/service-config"
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_builds_fall_back_to_the_production_api() {
        assert_eq!(super::default_base_url(), PRODUCTION);
        assert_eq!(
            resolve_api_base_url(None, None, &super::default_base_url()),
            PRODUCTION
        );
    }
}
