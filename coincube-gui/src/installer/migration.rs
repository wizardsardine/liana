//! One-shot migration of an existing per-vault `daemon.toml` from the
//! old Connect-only Esplora layout to the new public-primary +
//! Connect-fallback layout.
//!
//! Before this round, [`super::connect_url`] was the daemon's only
//! Esplora endpoint. New installs now use [`super::public_esplora_url`]
//! as `addr` and the Connect URL as `fallback_addr`, so wallet sync
//! traffic distributes across users' IPs instead of consolidating on
//! coincube-api's IP. Existing vaults already on disk wouldn't pick
//! that up without re-running the installer; this module rewrites
//! their `daemon.toml` in place at GUI startup.
//!
//! Idempotent: a daemon.toml that already has `fallback_addr` set, or
//! whose `addr` doesn't exactly match the Connect URL for its
//! network, is left untouched. So users who chose a custom Esplora
//! during install (or who already migrated) aren't disturbed.

use std::path::Path;

use coincube_core::miniscript::bitcoin::Network;

/// Migrate the daemon config at `path`, if applicable. Returns
/// `Ok(true)` when the file was rewritten, `Ok(false)` when no
/// migration was needed (missing file, already migrated, custom
/// Esplora, non-Esplora backend).
///
/// I/O failures while reading or writing surface as `Err`; a parse
/// failure on the existing TOML is treated as "leave it alone" and
/// returns `Ok(false)` after a warning log, because we'd rather skip
/// migration than corrupt a user's config on a schema we don't
/// understand.
pub(crate) fn migrate_esplora_config(path: &Path) -> std::io::Result<bool> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };

    // toml 0.9 dropped `FromStr` for `Value`; top-level documents
    // parse as `Table` now.
    let mut value: toml::Table = match toml::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "esplora-config migration: skipping {} (parse failed: {})",
                path.display(),
                e,
            );
            return Ok(false);
        }
    };

    // Identify the network from the bitcoin_config block — we need
    // it to compute the expected Connect URL for the comparison
    // below. Reading it from the same file (rather than asking the
    // caller) keeps the migration self-contained and removes a way
    // for caller/file disagreement to silently misclassify the addr.
    let network = match value
        .get("bitcoin_config")
        .and_then(|v| v.get("network"))
        .and_then(|v| v.as_str())
        .and_then(parse_network)
    {
        Some(n) => n,
        None => return Ok(false),
    };

    // `bitcoin_backend` on `coincubed::config::Config` is
    // `#[serde(flatten)]`, so an `Esplora(EsploraConfig)` variant
    // serialises with `[esplora_config]` at the document root rather
    // than under a `[bitcoin_backend.esplora_config]` table. (This
    // bit me on the first cut of this migration — it parsed but
    // matched nothing in the wild because the path didn't exist.)
    let Some(esplora) = value
        .get_mut("esplora_config")
        .and_then(|v| v.as_table_mut())
    else {
        // Not an Esplora backend (Bitcoind / Electrum / missing) —
        // nothing to migrate.
        return Ok(false);
    };

    let connect = super::connect_url(network);
    let mempool = super::public_esplora_url(network);

    // Three distinct cases we accept:
    //   Case A — pre-fallback (addr=Connect, no fallback):
    //     promote Connect to fallback, set mempool.space as primary,
    //     and (where blockstream is available) insert it as
    //     fallback so Connect ends up at secondary_fallback.
    //   Case B — previously migrated two-tier (addr=mempool,
    //     fallback=Connect, no secondary_fallback): insert
    //     blockstream as fallback and shift Connect to
    //     secondary_fallback. This is the path users on the
    //     intermediate two-tier shape take when upgrading.
    //   Anything else — user-customised addr, three-tier already in
    //     place, non-Esplora backend — gets left alone.
    let public_fallback = super::public_esplora_fallback_url(network);
    let addr = esplora
        .get("addr")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let has_fallback_addr = esplora.contains_key("fallback_addr");
    let fallback_addr_str = esplora
        .get("fallback_addr")
        .and_then(|v| v.as_str())
        .map(String::from);
    let has_secondary_fallback_addr = esplora.contains_key("secondary_fallback_addr");

    if addr == connect && !has_fallback_addr {
        // Case A.
        let primary_token = esplora
            .get("token")
            .and_then(|v| v.as_str())
            .map(String::from);
        esplora.insert("addr".to_string(), toml::Value::String(mempool));
        // mempool.space is anonymous — drop the inherited token from
        // the primary slot; it re-appears below on whichever slot the
        // Connect URL ends up in.
        esplora.remove("token");
        match public_fallback {
            Some(blockstream) => {
                esplora.insert(
                    "fallback_addr".to_string(),
                    toml::Value::String(blockstream),
                );
                esplora.insert(
                    "secondary_fallback_addr".to_string(),
                    toml::Value::String(connect),
                );
                if let Some(token) = primary_token {
                    esplora.insert(
                        "secondary_fallback_token".to_string(),
                        toml::Value::String(token),
                    );
                }
            }
            None => {
                esplora.insert("fallback_addr".to_string(), toml::Value::String(connect));
                if let Some(token) = primary_token {
                    esplora.insert(
                        "fallback_token".to_string(),
                        toml::Value::String(token),
                    );
                }
            }
        }
    } else if addr == mempool
        && fallback_addr_str.as_deref() == Some(connect.as_str())
        && !has_secondary_fallback_addr
    {
        // Case B — needs `public_fallback` to actually be available;
        // otherwise we'd shift Connect down to secondary_fallback
        // and leave nothing in the middle slot, which is no better
        // than what we started with.
        let Some(blockstream) = public_fallback else {
            return Ok(false);
        };
        let fallback_token = esplora
            .get("fallback_token")
            .and_then(|v| v.as_str())
            .map(String::from);
        esplora.insert(
            "fallback_addr".to_string(),
            toml::Value::String(blockstream),
        );
        // blockstream.info is anonymous — drop any inherited
        // fallback_token; it re-appears under secondary_fallback_token
        // below.
        esplora.remove("fallback_token");
        esplora.insert(
            "secondary_fallback_addr".to_string(),
            toml::Value::String(connect),
        );
        if let Some(token) = fallback_token {
            esplora.insert(
                "secondary_fallback_token".to_string(),
                toml::Value::String(token),
            );
        }
    } else {
        // Customised, already three-tier, or some other shape — skip.
        return Ok(false);
    }

    let serialized = toml::to_string_pretty(&value)
        .map_err(|e| std::io::Error::other(format!("serialize daemon.toml: {}", e)))?;

    // Atomic rewrite: write to a sibling tmp and rename. A
    // crash mid-write would otherwise leave a half-written file
    // that the daemon would refuse to start with.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, serialized)?;
    std::fs::rename(&tmp, path)?;

    tracing::info!(
        "esplora-config migration: rewrote provider chain in {}",
        path.display(),
    );
    Ok(true)
}

/// Map the `network` field from the daemon TOML to a [`Network`].
/// Mirrors the strings serde uses for `bitcoin::Network` so we don't
/// have to thread a serde-typed Config through the migration.
fn parse_network(s: &str) -> Option<Network> {
    match s {
        "bitcoin" => Some(Network::Bitcoin),
        "testnet" => Some(Network::Testnet),
        "testnet4" => Some(Network::Testnet4),
        "signet" => Some(Network::Signet),
        "regtest" => Some(Network::Regtest),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_path() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "coincube-migration-test-{}.toml",
            uuid::Uuid::new_v4()
        ));
        p
    }

    /// Build a daemon.toml string that points `addr` at the Connect
    /// URL for the given network, optionally with a JWT token. This
    /// is the shape every existing pre-migration vault has on disk.
    ///
    /// Critically, `[esplora_config]` lives at the document root
    /// (not under `[bitcoin_backend]`) because the `bitcoin_backend`
    /// field on the real `Config` struct is `#[serde(flatten)]`.
    fn legacy_connect_toml(network: Network, token: Option<&str>) -> String {
        let network_str = match network {
            Network::Bitcoin => "bitcoin",
            Network::Testnet => "testnet",
            Network::Testnet4 => "testnet4",
            Network::Signet => "signet",
            Network::Regtest => "regtest",
        };
        let token_line = match token {
            Some(t) => format!("token = \"{}\"\n", t),
            None => String::new(),
        };
        format!(
            r#"
data_directory = "/tmp/whatever"
log_level = "debug"

[bitcoin_config]
network = "{network}"
poll_interval_secs = 30

[esplora_config]
addr = "{addr}"
{token_line}
"#,
            network = network_str,
            addr = super::super::connect_url(network),
            token_line = token_line,
        )
    }

    #[test]
    fn missing_file_is_a_noop() {
        let p = fresh_path();
        assert!(!p.exists());
        let migrated = migrate_esplora_config(&p).expect("ok");
        assert!(!migrated);
        assert!(!p.exists(), "migration must not create the file");
    }

    #[test]
    fn migrates_connect_addr_with_token_to_three_tier() {
        let p = fresh_path();
        std::fs::write(
            &p,
            legacy_connect_toml(Network::Bitcoin, Some("jwt-token-here")),
        )
        .expect("seed");

        let migrated = migrate_esplora_config(&p).expect("ok");
        assert!(migrated, "legacy-Connect addr must trigger migration");

        let after = std::fs::read_to_string(&p).expect("read back");
        let parsed: toml::Table = toml::from_str(&after).expect("re-parse");
        let esplora = parsed
            .get("esplora_config")
            .and_then(|v| v.as_table())
            .expect("esplora_config table present");

        assert_eq!(
            esplora.get("addr").and_then(|v| v.as_str()),
            Some(super::super::public_esplora_url(Network::Bitcoin).as_str()),
            "primary must now be mempool.space",
        );
        assert!(
            !esplora.contains_key("token"),
            "primary token must be cleared (mempool.space is anonymous)",
        );
        assert_eq!(
            esplora.get("fallback_addr").and_then(|v| v.as_str()),
            super::super::public_esplora_fallback_url(Network::Bitcoin).as_deref(),
            "blockstream.info must be inserted as the middle fallback on mainnet",
        );
        assert!(
            !esplora.contains_key("fallback_token"),
            "blockstream.info is anonymous so no fallback_token should be written",
        );
        assert_eq!(
            esplora.get("secondary_fallback_addr").and_then(|v| v.as_str()),
            Some(super::super::connect_url(Network::Bitcoin).as_str()),
            "Connect URL must move to secondary_fallback_addr",
        );
        assert_eq!(
            esplora.get("secondary_fallback_token").and_then(|v| v.as_str()),
            Some("jwt-token-here"),
            "JWT must move to secondary_fallback_token",
        );

        std::fs::remove_file(&p).ok();
    }

    /// Case B — user is already on the intermediate two-tier shape
    /// (`addr=mempool, fallback=Connect`). The migration must insert
    /// blockstream.info as the new middle tier and shift Connect to
    /// `secondary_fallback`, preserving its JWT.
    #[test]
    fn migrates_two_tier_to_three_tier_inserting_blockstream() {
        let p = fresh_path();
        let two_tier = format!(
            r#"
data_directory = "/tmp/whatever"
log_level = "debug"

[bitcoin_config]
network = "bitcoin"
poll_interval_secs = 30

[esplora_config]
addr = "{primary}"
fallback_addr = "{connect}"
fallback_token = "jwt-token-here"
"#,
            primary = super::super::public_esplora_url(Network::Bitcoin),
            connect = super::super::connect_url(Network::Bitcoin),
        );
        std::fs::write(&p, two_tier).expect("seed");

        let migrated = migrate_esplora_config(&p).expect("ok");
        assert!(migrated, "two-tier shape must trigger insertion of blockstream");

        let after = std::fs::read_to_string(&p).expect("read back");
        let parsed: toml::Table = toml::from_str(&after).expect("re-parse");
        let esplora = parsed
            .get("esplora_config")
            .and_then(|v| v.as_table())
            .expect("esplora_config table present");

        assert_eq!(
            esplora.get("addr").and_then(|v| v.as_str()),
            Some(super::super::public_esplora_url(Network::Bitcoin).as_str()),
            "primary stays at mempool.space",
        );
        assert_eq!(
            esplora.get("fallback_addr").and_then(|v| v.as_str()),
            super::super::public_esplora_fallback_url(Network::Bitcoin).as_deref(),
            "fallback must now be blockstream.info",
        );
        assert!(
            !esplora.contains_key("fallback_token"),
            "blockstream.info is anonymous — fallback_token must be cleared",
        );
        assert_eq!(
            esplora.get("secondary_fallback_addr").and_then(|v| v.as_str()),
            Some(super::super::connect_url(Network::Bitcoin).as_str()),
            "Connect URL must shift down to secondary_fallback_addr",
        );
        assert_eq!(
            esplora.get("secondary_fallback_token").and_then(|v| v.as_str()),
            Some("jwt-token-here"),
            "JWT must move from fallback_token to secondary_fallback_token",
        );

        std::fs::remove_file(&p).ok();
    }

    /// A second pass over an already-three-tier config must be a
    /// byte-identical no-op. This catches a class of bugs where
    /// the migration would re-apply Case B to its own output.
    #[test]
    fn three_tier_config_is_idempotent_noop() {
        let p = fresh_path();
        std::fs::write(
            &p,
            legacy_connect_toml(Network::Bitcoin, Some("jwt-token-here")),
        )
        .expect("seed");
        // Case-A → three tier.
        assert!(migrate_esplora_config(&p).expect("ok"));
        let after_first = std::fs::read_to_string(&p).expect("read");

        // Second pass must no-op.
        assert!(!migrate_esplora_config(&p).expect("ok"));
        let after_second = std::fs::read_to_string(&p).expect("read");
        assert_eq!(after_first, after_second);

        std::fs::remove_file(&p).ok();
    }

    /// On a network where blockstream has no public Esplora
    /// (signet / testnet4 / regtest), Case A still applies but the
    /// result is only two-tier (mempool → Connect), with nothing
    /// in `secondary_fallback`. This matches the previous behaviour
    /// exactly for those networks — we don't synthesise a middle
    /// tier we don't actually have an endpoint for.
    #[test]
    fn case_a_falls_back_to_two_tier_when_blockstream_unavailable() {
        let p = fresh_path();
        std::fs::write(&p, legacy_connect_toml(Network::Signet, Some("sig-jwt"))).expect("seed");
        let migrated = migrate_esplora_config(&p).expect("ok");
        assert!(migrated);

        let after = std::fs::read_to_string(&p).expect("read back");
        let parsed: toml::Table = toml::from_str(&after).expect("re-parse");
        let esplora = parsed
            .get("esplora_config")
            .and_then(|v| v.as_table())
            .expect("esplora_config table");

        assert_eq!(
            esplora.get("addr").and_then(|v| v.as_str()),
            Some(super::super::public_esplora_url(Network::Signet).as_str()),
        );
        assert_eq!(
            esplora.get("fallback_addr").and_then(|v| v.as_str()),
            Some(super::super::connect_url(Network::Signet).as_str()),
        );
        assert_eq!(
            esplora.get("fallback_token").and_then(|v| v.as_str()),
            Some("sig-jwt"),
        );
        assert!(!esplora.contains_key("secondary_fallback_addr"));
        assert!(!esplora.contains_key("secondary_fallback_token"));

        std::fs::remove_file(&p).ok();
    }

    /// An input config that had no primary `token` (which would be
    /// unusual but is supported by the schema) must not synthesise
    /// either a `fallback_token` or a `secondary_fallback_token` — we
    /// only ever propagate tokens that actually existed.
    #[test]
    fn migration_without_token_leaves_token_slots_absent() {
        let p = fresh_path();
        std::fs::write(&p, legacy_connect_toml(Network::Bitcoin, None)).expect("seed");

        let migrated = migrate_esplora_config(&p).expect("ok");
        assert!(migrated);

        let after = std::fs::read_to_string(&p).expect("read back");
        let parsed: toml::Table = toml::from_str(&after).expect("re-parse");
        let esplora = parsed
            .get("esplora_config")
            .and_then(|v| v.as_table())
            .expect("esplora_config table");
        assert!(!esplora.contains_key("fallback_token"));
        assert!(!esplora.contains_key("secondary_fallback_token"));

        std::fs::remove_file(&p).ok();
    }

    // The older `already_migrated_file_is_idempotent_noop` test was
    // replaced by `three_tier_config_is_idempotent_noop` below — both
    // exercise the same "second pass is a byte-identical no-op"
    // property, but the new test runs against the current three-tier
    // output, which is what we actually persist now.

    #[test]
    fn user_customized_addr_is_left_alone() {
        // User chose mempool.space (or any non-Connect URL) during
        // install. The migration must not touch their `addr`, even
        // though `fallback_addr` is absent.
        let p = fresh_path();
        let custom = r#"
data_directory = "/tmp/whatever"
log_level = "debug"

[bitcoin_config]
network = "bitcoin"
poll_interval_secs = 30

[esplora_config]
addr = "https://blockstream.info/api"
"#;
        std::fs::write(&p, custom).expect("seed");
        let before = std::fs::read_to_string(&p).expect("read");

        let migrated = migrate_esplora_config(&p).expect("ok");
        assert!(!migrated, "custom addr must be left alone");

        let after = std::fs::read_to_string(&p).expect("read");
        assert_eq!(before, after, "file must be byte-identical");

        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn non_esplora_backend_is_left_alone() {
        // Bitcoind backend has no esplora_config table — must skip.
        let p = fresh_path();
        let bitcoind = r#"
data_directory = "/tmp/whatever"
log_level = "debug"

[bitcoin_config]
network = "bitcoin"
poll_interval_secs = 30

[bitcoind_config]
network = "bitcoin"
addr = "127.0.0.1:8332"
"#;
        std::fs::write(&p, bitcoind).expect("seed");
        let before = std::fs::read_to_string(&p).expect("read");

        let migrated = migrate_esplora_config(&p).expect("ok");
        assert!(!migrated);

        let after = std::fs::read_to_string(&p).expect("read");
        assert_eq!(before, after);

        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn parse_failure_is_a_noop_not_an_error() {
        // A daemon.toml the migration can't parse (perhaps a future
        // schema we don't recognise) must be left untouched, not
        // raised as an error that blocks GUI startup.
        let p = fresh_path();
        std::fs::write(&p, "this is = not [valid TOML").expect("seed");
        let before = std::fs::read_to_string(&p).expect("read");

        let migrated = migrate_esplora_config(&p).expect("must not raise");
        assert!(!migrated);

        let after = std::fs::read_to_string(&p).expect("read");
        assert_eq!(before, after);

        std::fs::remove_file(&p).ok();
    }
}
