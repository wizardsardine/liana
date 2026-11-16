//! Backfill of Keychain key provenance for Vaults created before it was
//! recorded.
//!
//! A Vault built in the descriptor editor records which of its keys came from
//! the COINCUBE Keychain ([`KeySetting::keychain_key_id`]). Older Vaults do not,
//! so the Pair panel cannot tell a Keychain key from any other. This asks
//! Connect once for the Cube's registered Keychain keys, matches them to the
//! Vault's descriptor keys by master fingerprint (as the signing flow's
//! `build_keychain_index` does), and records the answer.
//!
//! Read-only against Connect: it lists the Cube's keys and never writes to the
//! server. In particular it does not reconcile vault membership, which the
//! signing flow does before classification.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

use crate::app::settings::{self, KeySetting, Settings};
use crate::app::wallet::Wallet;
use crate::dir::CoincubeDirectory;
use crate::services::coincube::{CoincubeClient, CubeKeyRaw};

/// A Vault key found among the Cube's Keychain keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundKeychainKey {
    pub key_id: u64,
    /// The key's name in Connect, used only when the Vault has none for it.
    pub name: String,
}

/// The Vault's descriptor keys that are registered Keychain keys of the Cube.
///
/// A Cube key whose fingerprint doesn't parse, or that isn't in this Vault, is
/// ignored. If one fingerprint is registered more than once, the lowest key id
/// wins, so the answer never depends on the order the server lists them in.
pub fn match_keychain_keys(
    descriptor_keys: &HashSet<Fingerprint>,
    cube_keys: &[CubeKeyRaw],
) -> HashMap<Fingerprint, FoundKeychainKey> {
    let mut found: HashMap<Fingerprint, FoundKeychainKey> = HashMap::new();
    for key in cube_keys {
        let Ok(fingerprint) = key.fingerprint.parse::<Fingerprint>() else {
            continue;
        };
        if !descriptor_keys.contains(&fingerprint) {
            continue;
        }
        let candidate = FoundKeychainKey {
            key_id: key.id,
            name: key.name.clone(),
        };
        found
            .entry(fingerprint)
            .and_modify(|existing| {
                if candidate.key_id < existing.key_id {
                    *existing = candidate.clone();
                }
            })
            .or_insert(candidate);
    }
    found
}

/// Record `found` on the Vault's entry in `settings` and mark its provenance
/// complete. Keys already named keep their name; a Keychain key the file has
/// no entry for is added under its Connect name. Returns `false`, leaving
/// `settings` untouched, when the Vault has no entry here.
pub fn record_keychain_keys(
    settings: &mut Settings,
    wallet: &Wallet,
    found: &HashMap<Fingerprint, FoundKeychainKey>,
) -> bool {
    let wallet_id = wallet.id();
    let Some(entry) = settings
        .wallets
        .iter_mut()
        .find(|w| w.wallet_id() == wallet_id)
    else {
        return false;
    };
    for key in entry.keys.iter_mut() {
        key.keychain_key_id = found.get(&key.master_fingerprint).map(|f| f.key_id);
    }
    let mut missing: Vec<_> = found
        .iter()
        .filter(|(fp, _)| !entry.keys.iter().any(|k| k.master_fingerprint == **fp))
        .collect();
    missing.sort_by_key(|(fp, _)| **fp);
    for (fp, key) in missing {
        entry.keys.push(KeySetting {
            name: key.name.clone(),
            master_fingerprint: *fp,
            provider_key: None,
            is_border_wallet: false,
            grid_seed_source: None,
            replay_protected: None,
            keychain_key_id: Some(key.key_id),
        });
    }
    entry.keychain_keys_recorded = true;
    true
}

/// `wallet` with `found` applied, as [`record_keychain_keys`] leaves the file.
pub fn apply_to_wallet(wallet: &Wallet, found: &HashMap<Fingerprint, FoundKeychainKey>) -> Wallet {
    let mut wallet = wallet.clone();
    for (fp, key) in found {
        wallet
            .keys_aliases
            .entry(*fp)
            .or_insert_with(|| key.name.clone());
    }
    let ids = found.iter().map(|(fp, key)| (*fp, key.key_id)).collect();
    wallet.with_keychain_keys(ids, true)
}

/// Ask Connect for the Cube's Keychain keys and record which of the Vault's
/// keys they are.
///
/// `persist` is `false` when there is no Vault daemon; the answer then holds
/// for this session only. A failure anywhere records nothing, so the Vault stays
/// unrecorded and is asked about again next time rather than being marked as
/// having no Keychain keys.
pub async fn lookup(
    datadir: CoincubeDirectory,
    wallet: Arc<Wallet>,
    tokens: Arc<tokio::sync::RwLock<crate::services::connect::client::auth::AccessTokenResponse>>,
    cube_uuid: String,
    persist: bool,
) -> Result<HashMap<Fingerprint, FoundKeychainKey>, String> {
    let mut client = CoincubeClient::new();
    client.set_token(&tokens.read().await.access_token);
    let cube_keys = client
        .get_cube_keys(&cube_uuid)
        .await
        .map_err(|e| format!("Failed to fetch cube keys: {}", e))?;
    let found = match_keychain_keys(&wallet.descriptor_keys(), &cube_keys);
    if persist {
        let network_dir = datadir.network_directory(wallet.chain);
        let found_for_file = found.clone();
        let wallet_for_file = wallet.clone();
        settings::update_settings_file(&network_dir, move |mut settings| {
            // Always `Some`: `None` would delete the settings file.
            record_keychain_keys(&mut settings, &wallet_for_file, &found_for_file);
            Some(settings)
        })
        .await
        .map_err(|e| format!("Failed to save Mobile keys: {}", e))?;
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::descriptors::CoincubeDescriptor;
    use std::str::FromStr;

    const DESC: &str = "wsh(or_d(multi(2,[f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<0;1>/*,[2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<0;1>/*),and_v(v:thresh(1,pkh([f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<2;3>/*),a:pkh([2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<2;3>/*)),older(65535))))#9s8ekrce";
    const PHONE: &str = "f714c228";
    const LEDGER: &str = "2522f23c";

    fn fp(hex: &str) -> Fingerprint {
        Fingerprint::from_str(hex).unwrap()
    }

    fn cube_key(id: u64, fingerprint: &str, name: &str) -> CubeKeyRaw {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": name,
            "fingerprint": fingerprint,
            "derivationPath": "m/48'/1'/0'/2'",
            "network": "testnet",
            "status": "active",
        }))
        .unwrap()
    }

    fn wallet() -> Wallet {
        Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap()).with_key_aliases(HashMap::from([
            (fp(PHONE), "Renamed phone".to_string()),
            (fp(LEDGER), "Ledger".to_string()),
        ]))
    }

    fn settings_for(wallet: &Wallet, keys: Vec<KeySetting>) -> Settings {
        Settings {
            wallets: vec![settings::WalletSettings {
                name: wallet.name.clone(),
                alias: None,
                descriptor_checksum: wallet.descriptor_checksum.clone(),
                pinned_at: wallet.pinned_at,
                keys,
                hardware_wallets: vec![],
                remote_backend_auth: None,
                start_internal_bitcoind: None,
                pending_rescan: None,
                keychain_keys_recorded: false,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn only_descriptor_keys_registered_on_the_cube_match() {
        let descriptor_keys = wallet().descriptor_keys();
        let found = match_keychain_keys(
            &descriptor_keys,
            &[
                cube_key(42, PHONE, "My iPhone"),
                // Registered on the Cube, but not a signer of this Vault.
                cube_key(43, "deadbeef", "Other phone"),
                cube_key(44, "not-hex", "Broken"),
            ],
        );
        assert_eq!(
            found,
            HashMap::from([(
                fp(PHONE),
                FoundKeychainKey {
                    key_id: 42,
                    name: "My iPhone".to_string()
                }
            )])
        );
        assert!(match_keychain_keys(&descriptor_keys, &[]).is_empty());
    }

    #[test]
    fn a_fingerprint_registered_twice_resolves_to_the_lowest_id() {
        let descriptor_keys = wallet().descriptor_keys();
        for order in [[50, 42], [42, 50]] {
            let found = match_keychain_keys(
                &descriptor_keys,
                &order.map(|id| cube_key(id, PHONE, "phone")),
            );
            assert_eq!(found[&fp(PHONE)].key_id, 42);
        }
    }

    #[test]
    fn recording_keeps_names_marks_the_vault_and_adds_missing_keys() {
        let wallet = wallet();
        let named = |hex: &str, name: &str| KeySetting {
            name: name.to_string(),
            master_fingerprint: fp(hex),
            provider_key: None,
            is_border_wallet: false,
            grid_seed_source: None,
            replay_protected: Some(true),
            keychain_key_id: None,
        };
        let found = HashMap::from([(
            fp(PHONE),
            FoundKeychainKey {
                key_id: 42,
                name: "My iPhone".to_string(),
            },
        )]);

        // Both keys named in the file: the user's name wins over Connect's.
        let mut s = settings_for(
            &wallet,
            vec![named(PHONE, "Renamed phone"), named(LEDGER, "Ledger")],
        );
        assert!(record_keychain_keys(&mut s, &wallet, &found));
        let entry = &s.wallets[0];
        assert!(entry.keychain_keys_recorded);
        let phone = entry
            .keys
            .iter()
            .find(|k| k.master_fingerprint == fp(PHONE))
            .unwrap();
        assert_eq!(phone.name, "Renamed phone");
        assert_eq!(phone.keychain_key_id, Some(42));
        assert_eq!(phone.replay_protected, Some(true), "other fields untouched");
        let ledger = entry
            .keys
            .iter()
            .find(|k| k.master_fingerprint == fp(LEDGER))
            .unwrap();
        assert_eq!(ledger.keychain_key_id, None);

        // No entry for the Keychain key: added under its Connect name.
        let mut s = settings_for(&wallet, vec![named(LEDGER, "Ledger")]);
        assert!(record_keychain_keys(&mut s, &wallet, &found));
        let phone = s.wallets[0]
            .keys
            .iter()
            .find(|k| k.master_fingerprint == fp(PHONE))
            .unwrap();
        assert_eq!(phone.name, "My iPhone");
        assert_eq!(phone.keychain_key_id, Some(42));

        // No Keychain keys at all is still an answer, and recorded as one.
        let mut s = settings_for(&wallet, vec![named(LEDGER, "Ledger")]);
        assert!(record_keychain_keys(&mut s, &wallet, &HashMap::new()));
        assert!(s.wallets[0].keychain_keys_recorded);
        assert!(s.wallets[0]
            .keys
            .iter()
            .all(|k| k.keychain_key_id.is_none()));
    }

    #[test]
    fn recording_leaves_other_vaults_and_a_missing_entry_alone() {
        let wallet = wallet();
        let mut s = settings_for(&wallet, vec![]);
        s.wallets[0].descriptor_checksum = "someothervault".to_string();
        let before = serde_json::to_string(&s).unwrap();
        assert!(!record_keychain_keys(&mut s, &wallet, &HashMap::new()));
        assert_eq!(serde_json::to_string(&s).unwrap(), before);
    }

    #[test]
    fn the_wallet_gains_the_record_without_losing_its_names() {
        let found = HashMap::from([(
            fp(PHONE),
            FoundKeychainKey {
                key_id: 42,
                name: "My iPhone".to_string(),
            },
        )]);
        let updated = apply_to_wallet(&wallet(), &found);
        assert!(updated.keychain_keys_recorded);
        assert_eq!(updated.keychain_key_ids, HashMap::from([(fp(PHONE), 42)]));
        assert_eq!(updated.keys_aliases[&fp(PHONE)], "Renamed phone");

        let unnamed = Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap());
        let updated = apply_to_wallet(&unnamed, &found);
        assert_eq!(updated.keys_aliases[&fp(PHONE)], "My iPhone");
    }
}
