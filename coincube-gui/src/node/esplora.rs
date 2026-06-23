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
    let value = value.trim().trim_end_matches('/');
    let scheme_len = if value.starts_with("http://") {
        "http://".len()
    } else if value.starts_with("https://") {
        "https://".len()
    } else {
        return None;
    };
    let rest = &value[scheme_len..];
    let host = rest.split('/').next().unwrap_or_default();
    if host.is_empty() {
        return None;
    }

    // Byte offset where the path begins (right after `scheme://host[:port]`).
    // Endpoint markers must only be matched within the *path* — searching the
    // whole URL means a marker substring that also occurs in the host wrongly
    // truncates it. Concretely, the `/mempool` marker matched the `//mempool`
    // in `https://mempool.space/...` (the second slash of `://`), collapsing the
    // URL down to `https:` and breaking every mempool.space/emzy endpoint.
    let host_end = rest.find('/').map(|p| scheme_len + p);
    let path_start = match host_end {
        Some(p) => p,
        // No path at all (e.g. `https://mempool.space`) — nothing to strip.
        None => return Some(value.to_string()),
    };

    let mut value = value.to_string();
    for marker in [
        "/blocks/",
        "/block/",
        "/tx/",
        "/address/",
        "/scripthash/",
        "/mempool",
        "/fee-estimates",
    ] {
        if let Some(rel) = value[path_start..].find(marker) {
            value.truncate(path_start + rel);
            value = value.trim_end_matches('/').to_string();
            break;
        }
    }

    Some(value)
}

#[cfg(test)]
mod tests {
    use super::normalize_esplora_address as norm;

    /// Regression: a marker substring that also appears in the *host* must not
    /// truncate the URL. The `/mempool` marker used to match the `//mempool`
    /// in `https://mempool.space/...`, collapsing the whole URL to `https:`
    /// and breaking every mempool.space / mempool.emzy.de endpoint.
    #[test]
    fn host_containing_marker_substring_is_preserved() {
        assert_eq!(
            norm("https://mempool.space/testnet4/api").as_deref(),
            Some("https://mempool.space/testnet4/api"),
        );
        assert_eq!(
            norm("https://mempool.emzy.de/testnet4/api").as_deref(),
            Some("https://mempool.emzy.de/testnet4/api"),
        );
    }

    /// A genuine probed endpoint path is still trimmed back to the base URL.
    #[test]
    fn endpoint_path_is_trimmed_to_base() {
        assert_eq!(
            norm("https://mempool.space/testnet4/api/blocks/tip/hash").as_deref(),
            Some("https://mempool.space/testnet4/api"),
        );
        assert_eq!(
            norm("http://localhost:8082/api/v1/esplora/bitcoin/signet/blocks/tip/hash").as_deref(),
            Some("http://localhost:8082/api/v1/esplora/bitcoin/signet"),
        );
        // The `/mempool` endpoint marker still works when it's actually in the path.
        assert_eq!(
            norm("https://blockstream.info/api/mempool/recent").as_deref(),
            Some("https://blockstream.info/api"),
        );
    }

    #[test]
    fn trailing_slash_and_no_path_are_handled() {
        assert_eq!(
            norm("https://mempool.space/testnet4/api/").as_deref(),
            Some("https://mempool.space/testnet4/api"),
        );
        assert_eq!(
            norm("https://mempool.space").as_deref(),
            Some("https://mempool.space"),
        );
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert_eq!(norm("ftp://example.com/api"), None);
        assert_eq!(norm("mempool.space/api"), None);
    }
}
