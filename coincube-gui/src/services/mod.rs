pub mod connect;
pub mod feeestimation;
pub mod fiat;

pub mod http;
pub mod keys;

pub mod coincube;
pub mod lnurl;
pub mod mavapay;
pub mod meld;
pub mod sideshift;

/// Resolves the Coincube API base URL with this precedence:
/// 1. Runtime `std::env::var("COINCUBE_API_URL")` — picked up from the shell or
///    the `.env` loaded in `main()`; change and restart without rebuilding.
/// 2. Compile-time `option_env!("COINCUBE_API_URL")` — values baked in by
///    `build.rs` from `.env` at build time. Release builds started from a
///    directory that has no `.env` still work via this path.
/// 3. Hardcoded `https://dev-api.coincube.io` as a debug fallback. Release
///    builds require the env var at build time via `env!`, so they never
///    reach step 3.
pub fn coincube_api_base_url() -> String {
    if let Ok(v) = std::env::var("COINCUBE_API_URL") {
        if !v.is_empty() {
            return v;
        }
    }
    if let Some(v) = option_env!("COINCUBE_API_URL") {
        return v.to_string();
    }
    #[cfg(debug_assertions)]
    {
        "https://dev-api.coincube.io".to_string()
    }
    #[cfg(not(debug_assertions))]
    {
        // Release builds must have COINCUBE_API_URL set at build time.
        env!("COINCUBE_API_URL").to_string()
    }
}
