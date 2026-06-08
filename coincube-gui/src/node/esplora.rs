use std::fmt;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ConfigField {
    Address,
}

impl fmt::Display for ConfigField {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConfigField::Address => write!(f, "Esplora URL"),
        }
    }
}

/// Returns true if `value` looks like a plausible Esplora HTTP(S) URL.
pub fn is_esplora_address_valid(value: &str) -> bool {
    normalize_esplora_address(value).is_some()
}

/// Normalize user-entered Esplora URLs to the server base URL.
///
/// Users often paste a probed endpoint such as
/// `http://localhost:8082/api/v1/esplora/bitcoin/signet/blocks/tip/hash`.
/// The daemon expects the Esplora base URL and appends endpoint paths itself,
/// so storing that full endpoint later turns sync requests into nonsense.
pub fn normalize_esplora_address(value: &str) -> Option<String> {
    let mut value = value.trim().trim_end_matches('/').to_string();
    let rest = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))?;
    let host = rest.split('/').next().unwrap_or_default();
    if host.is_empty() {
        return None;
    }

    for marker in [
        "/blocks/",
        "/block/",
        "/tx/",
        "/address/",
        "/scripthash/",
        "/mempool",
        "/fee-estimates",
    ] {
        if let Some(pos) = value.find(marker) {
            value.truncate(pos);
            value = value.trim_end_matches('/').to_string();
            break;
        }
    }

    Some(value)
}
