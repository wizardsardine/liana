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

    // Bump short poll intervals to a value that fits inside public
    // Esplora free tiers. Done independently of (and before) the
    // provider-chain rewrite below so it also covers configs that
    // are already on the right chain shape. Tracked so the final
    // "did we change anything?" return covers both edits.
    let bumped_poll_interval = bump_short_poll_interval(&mut value);

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
        // Not an Esplora backend (Bitcoind / Electrum / missing).
        // If we already bumped the poll interval above we still need
        // to persist that edit, so flush instead of an unconditional
        // no-op return.
        if bumped_poll_interval {
            return write_back(path, &value);
        }
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
        // Customised, already three-tier, or some other shape — the
        // provider chain itself doesn't need rewriting. Still flush
        // if we bumped the poll interval up above.
        if bumped_poll_interval {
            return write_back(path, &value);
        }
        return Ok(false);
    }

    write_back(path, &value)
}

/// Bump `bitcoin_config.poll_interval_secs` to
/// [`MIN_POLL_INTERVAL_SECS`] when (a) the config has an
/// `esplora_config` block (i.e. this is an Esplora-backed daemon —
/// bitcoind/Electrum backends don't have public-provider rate
/// limits to dodge) and (b) the current value is below the floor.
/// Returns `true` if the value was changed.
///
/// Old Coincube builds defaulted to 10s or 30s, which on an
/// Esplora backend with ~80 SPKs translates to ~9,600 HTTP
/// requests/hour — far over public providers' free-tier hourly
/// caps (~700/hr on Blockstream). The bump aligns existing
/// installs with the new default and the cooldown machinery's
/// expectations.
fn bump_short_poll_interval(value: &mut toml::Table) -> bool {
    /// Below this we know the user is on an old default, not a
    /// deliberate "I want fast polls" choice. Anything at or
    /// above is left untouched so power users who set a custom
    /// value keep it.
    const POLL_INTERVAL_FLOOR_SECS: i64 = 300;
    /// Value we bump up to — same as `default_poll_interval()` in
    /// `coincubed::config`.
    const MIN_POLL_INTERVAL_SECS: i64 = 600;

    // Bitcoind/Electrum backends don't pay per-request to a public
    // API — they talk to a local node. Their old poll cadence is
    // fine; leave them alone.
    if !value.contains_key("esplora_config") {
        return false;
    }

    let Some(bitcoin_config) = value
        .get_mut("bitcoin_config")
        .and_then(|v| v.as_table_mut())
    else {
        return false;
    };
    let current = match bitcoin_config
        .get("poll_interval_secs")
        .and_then(|v| v.as_integer())
    {
        Some(n) => n,
        None => return false,
    };
    if current >= POLL_INTERVAL_FLOOR_SECS {
        return false;
    }
    bitcoin_config.insert(
        "poll_interval_secs".to_string(),
        toml::Value::Integer(MIN_POLL_INTERVAL_SECS),
    );
    tracing::info!(
        "esplora-config migration: bumped poll_interval_secs from {} to {} \
         (old value below the free-tier-safe floor of {})",
        current,
        MIN_POLL_INTERVAL_SECS,
        POLL_INTERVAL_FLOOR_SECS,
    );
    true
}

/// Atomically write the (possibly edited) TOML back to disk.
/// Returns `Ok(true)` so the caller can use the result directly.
/// Pulled into a helper so the multiple early-return paths above
/// don't need to copy the serialize + tmp + rename incantation.
fn write_back(path: &Path, value: &toml::Table) -> std::io::Result<bool> {
    let serialized = toml::to_string_pretty(value)
        .map_err(|e| std::io::Error::other(format!("serialize daemon.toml: {}", e)))?;
    // Atomic rewrite via tmp + rename. A crash mid-write would
    // otherwise leave a half-written file the daemon would refuse
    // to start with.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, serialized)?;
    std::fs::rename(&tmp, path)?;
    tracing::info!(
        "esplora-config migration: persisted edits to {}",
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

    /// Reading `poll_interval_secs` out of a parsed TOML table.
    fn poll_interval(parsed: &toml::Table) -> Option<i64> {
        parsed
            .get("bitcoin_config")
            .and_then(|v| v.get("poll_interval_secs"))
            .and_then(|v| v.as_integer())
    }

    /// On an Esplora-backed config with a sub-floor `poll_interval_secs`,
    /// the migration must bump it to 600 (the new default) and persist
    /// the edit, even if everything else about the file is already
    /// in the current three-tier shape.
    #[test]
    fn poll_interval_is_bumped_on_esplora_config() {
        let p = fresh_path();
        let three_tier = format!(
            r#"
data_directory = "/tmp/whatever"
log_level = "debug"

[bitcoin_config]
network = "bitcoin"
poll_interval_secs = 30

[esplora_config]
addr = "{primary}"
fallback_addr = "{blockstream}"
secondary_fallback_addr = "{connect}"
secondary_fallback_token = "jwt-here"
"#,
            primary = super::super::public_esplora_url(Network::Bitcoin),
            blockstream =
                super::super::public_esplora_fallback_url(Network::Bitcoin).unwrap(),
            connect = super::super::connect_url(Network::Bitcoin),
        );
        std::fs::write(&p, three_tier).expect("seed");

        // Already on the right chain shape, so the chain-rewrite
        // branches are no-ops; the poll-interval bump is the only
        // edit.
        assert!(migrate_esplora_config(&p).expect("ok"));

        let after = std::fs::read_to_string(&p).expect("read");
        let parsed: toml::Table = toml::from_str(&after).expect("re-parse");
        assert_eq!(
            poll_interval(&parsed),
            Some(600),
            "Esplora config with poll_interval_secs < 300 must be bumped to 600",
        );

        // Second pass must be a no-op now that we're at 600.
        assert!(!migrate_esplora_config(&p).expect("ok"));

        std::fs::remove_file(&p).ok();
    }

    /// A user who explicitly set a poll interval at or above the
    /// floor (300s = 5 min) is expressing a preference; the
    /// migration must leave them alone.
    #[test]
    fn poll_interval_at_or_above_floor_is_left_alone() {
        let p = fresh_path();
        let custom = format!(
            r#"
data_directory = "/tmp/whatever"
log_level = "debug"

[bitcoin_config]
network = "bitcoin"
poll_interval_secs = 300

[esplora_config]
addr = "{primary}"
fallback_addr = "{blockstream}"
secondary_fallback_addr = "{connect}"
"#,
            primary = super::super::public_esplora_url(Network::Bitcoin),
            blockstream =
                super::super::public_esplora_fallback_url(Network::Bitcoin).unwrap(),
            connect = super::super::connect_url(Network::Bitcoin),
        );
        std::fs::write(&p, custom).expect("seed");
        let before = std::fs::read_to_string(&p).expect("read");

        assert!(!migrate_esplora_config(&p).expect("ok"));

        let after = std::fs::read_to_string(&p).expect("read");
        assert_eq!(before, after, "300s poll must not be bumped");

        std::fs::remove_file(&p).ok();
    }

    /// Bitcoind / Electrum backends talk to a local node and don't
    /// pay per-request to a public API. Their old poll cadence is
    /// fine; the migration must not touch them.
    #[test]
    fn poll_interval_is_not_bumped_on_non_esplora_backend() {
        let p = fresh_path();
        let bitcoind = r#"
data_directory = "/tmp/whatever"
log_level = "debug"

[bitcoin_config]
network = "bitcoin"
poll_interval_secs = 10

[bitcoind_config]
network = "bitcoin"
addr = "127.0.0.1:8332"
"#;
        std::fs::write(&p, bitcoind).expect("seed");
        let before = std::fs::read_to_string(&p).expect("read");

        assert!(!migrate_esplora_config(&p).expect("ok"));

        let after = std::fs::read_to_string(&p).expect("read");
        assert_eq!(before, after, "bitcoind backend must not have its poll bumped");

        std::fs::remove_file(&p).ok();
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
        // User chose a non-Connect URL (e.g. their own blockstream
        // mirror or a self-hosted Esplora) during install. The
        // migration must not touch their `addr`, even though
        // `fallback_addr` is absent.
        //
        // Poll interval is set above the bump floor so this test
        // covers *only* the customised-addr path. The bump
        // behaviour is exercised separately.
        let p = fresh_path();
        let custom = r#"
data_directory = "/tmp/whatever"
log_level = "debug"

[bitcoin_config]
network = "bitcoin"
poll_interval_secs = 600

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
