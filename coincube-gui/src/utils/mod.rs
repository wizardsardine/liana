use std::{
    str::FromStr,
    time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH},
};

use coincube_core::miniscript::bitcoin::{self, bip32::DerivationPath, Network};

pub mod serde;

#[cfg(test)]
pub mod sandbox;

#[cfg(test)]
pub mod mock;

/// Returns the current time as a [`Duration`] since the UNIX epoch.
pub fn now() -> Duration {
    now_fallible().expect("cannot fail")
}

/// Faliible version of [`now`].
pub fn now_fallible() -> Result<Duration, SystemTimeError> {
    SystemTime::now().duration_since(UNIX_EPOCH)
}

pub fn example_xpub(network: Network) -> String {
    format!("[aabbccdd/42'/0']{}pub6DAkq8LWw91WGgUGnkR5Sbzjev5JCsXaTVZQ9MwsPV4BkNFKygtJ8GHodfDVx1udR723nT7JASqGPpKvz7zQ25pUTW6zVEBdiWoaC4aUqik",
        if network == bitcoin::Network::Bitcoin { "x" } else { "t" }
    )
}

pub fn default_derivation_path(network: Network) -> DerivationPath {
    // Note that "m" is ignored when parsing string and could be removed:
    // https://github.com/rust-bitcoin/rust-bitcoin/pull/2677
    DerivationPath::from_str({
        if network == Network::Bitcoin {
            "m/48'/0'/0'/2'"
        } else {
            "m/48'/1'/0'/2'"
        }
    })
    .unwrap()
}

pub fn format_timestamp(timestamp: u64) -> Option<String> {
    use chrono::{DateTime, Local, Utc};

    let dt = DateTime::<Utc>::from_timestamp(timestamp as i64, 0)?;

    Some(
        dt.with_timezone(&Local)
            .format("%b. %d, %Y - %T")
            .to_string(),
    )
}

pub fn format_time_ago(timestamp: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let diff = now.saturating_sub(timestamp).max(0) as u64;

    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        let minutes = diff / 60;
        format!(
            "{} minute{} ago",
            minutes,
            if minutes == 1 { "" } else { "s" }
        )
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
    } else {
        let days = diff / 86400;
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    }
}

/// Middle-elide an address or txid for display/logging:
/// `bc1p7g…p7ff6v`. Keeps the first `prefix_len` and last `suffix_len`
/// characters joined by a `…`. Strings short enough that elision would not
/// actually shorten them (i.e. `len <= prefix_len + suffix_len + 3`, where
/// 3 accounts for the ellipsis + sentinel overhead) are returned unchanged.
///
/// This is the single shared implementation used by both UI rendering
/// (`view::liquid::transactions`) and privacy-preserving logging
/// (`app::breez::client`). Keeping one threshold here prevents a 15-char
/// string from being rendered differently in the two call sites.
pub fn truncate_middle(s: &str, prefix_len: usize, suffix_len: usize) -> String {
    if s.len() <= prefix_len + suffix_len + 3 {
        return s.to_string();
    }
    format!("{}…{}", &s[..prefix_len], &s[s.len() - suffix_len..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_middle_short_unchanged() {
        assert_eq!(truncate_middle("short", 6, 6), "short");
    }

    #[test]
    fn truncate_middle_elides_center() {
        let long = "bc1p7gznc2zpn7aq3vqd695eml450d2ls33vw65tvwd77x936jquadnsp7ff6v";
        assert_eq!(truncate_middle(long, 6, 6), "bc1p7g…p7ff6v");
    }

    #[test]
    fn truncate_middle_boundary_15_chars_unchanged() {
        // Regression guard: both prior impls disagreed on a 15-char input.
        // The shared impl must treat it consistently — unchanged, because
        // truncating to 6…6 = 13 chars is not a meaningful shortening.
        assert_eq!(truncate_middle("123456789012345", 6, 6), "123456789012345");
    }
}
