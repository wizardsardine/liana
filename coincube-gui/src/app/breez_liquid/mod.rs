pub mod assets;
mod client;
mod config;
pub mod swap_status;

pub use client::BreezClient;
pub use config::BreezConfig;

// Re-export Breez SDK response types
pub use breez_sdk_liquid::prelude::{GetInfoResponse, ReceivePaymentResponse, SendPaymentResponse};

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, Network};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::app::seed_source::SeedSource;

#[derive(Debug, Clone)]
pub enum BreezError {
    NetworkNotSupported(Network),
    Connection(String),
    Sdk(String),
    SignerNotFound(Fingerprint),
    SignerError(String),
}

impl std::fmt::Display for BreezError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BreezError::NetworkNotSupported(n) => {
                write!(f, "Liquid wallet is not supported on {} network", n)
            }
            BreezError::Connection(msg) => write!(f, "failed to connect Breez SDK: {}", msg),
            BreezError::Sdk(msg) => write!(f, "SDK request failed: {}", msg),
            BreezError::SignerNotFound(fp) => {
                write!(f, "Liquid wallet signer not found for fingerprint: {}", fp)
            }
            BreezError::SignerError(msg) => write!(f, "Signer error: {}", msg),
        }
    }
}

impl std::error::Error for BreezError {}

/// The directories under the Breez working dir that hold a real, initialized
/// Liquid wallet — one per signer the SDK has connected on this machine.
///
/// Layout, created by the SDK itself (we only hand it `working_dir`):
/// `<datadir>/breez/<network>/<signer-id>/storage.sql`. The `storage.sql` file
/// is the load-bearing part: the SDK creates the enclosing directories eagerly,
/// so a bare directory proves nothing, whereas the DB only exists once a
/// wallet has actually been initialized.
///
/// This is the on-disk half of [`crate::app::features::LiquidGate`] — the
/// signal that says "this machine already has a Liquid wallet, so the sunset
/// gate must not hide it".
///
/// **Machine-scoped, not cube-scoped**, and deliberately so. The `<signer-id>`
/// segment is the SDK's own derivation, not our fingerprint, so we can't map a
/// subdir back to a cube without guessing at SDK internals. A machine with one
/// grandfathered Liquid cube will therefore also show Liquid on a second, newer
/// cube. That's the correct way to be wrong here: the failure mode is an extra
/// empty wallet in the nav, versus stranding real funds if we guessed the other
/// way and guessed wrong.
pub fn liquid_state_dirs(datadir: &Path, network: Network) -> Vec<std::path::PathBuf> {
    let network_dir = datadir.join("breez").join(match network {
        Network::Bitcoin => "mainnet",
        // Every other network maps to the SDK's Testnet (see `BreezConfig`).
        _ => "testnet",
    });
    let Ok(entries) = std::fs::read_dir(&network_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("storage.sql").is_file())
        .collect()
}

/// Whether a Liquid wallet has been initialized on this machine. The on-disk
/// input to the sunset gate — see [`liquid_state_dirs`].
///
/// Cheap (one `read_dir` + a `stat` per entry), synchronous, and needs no PIN,
/// no decryption and no SDK — so the gate can be resolved before the wallet is
/// open, and *offline*: the SDK's `storage.sql` caches balance and history
/// locally, which is what lets a funded wallet still surface when Connect (or
/// the whole network) is unreachable.
pub fn local_state_exists(datadir: &Path, network: Network) -> bool {
    !liquid_state_dirs(datadir, network).is_empty()
}

/// Load BreezClient for a Cube, taking its master seed from `seed`.
///
/// `seed` is an explicit [`SeedSource`] rather than a `(fingerprint, password)`
/// pair because a passkey Cube has no seed file at all — its master seed is
/// re-derived from a WebAuthn PRF assertion at unlock and only ever exists in
/// memory. [`SeedSource::EncryptedFile`] is the unchanged PIN-Cube path,
/// session-cache fast path included.
///
/// The master signer is always loaded — its mnemonic backs seed-derived
/// features (Spark, P2P/Mostro identity, Connect blinding and the Vault
/// builder's encryption key).
///
/// The Liquid SDK is **never connected** (Liquid removal, desktop PR 1; plan
/// `company-brain/plans/liquid-removal/`). Liquid has been hidden for everyone
/// since v175 (`LiquidGate::show()` is constant `false`), so connecting only
/// synced a wallet nobody could see. Every network now gets a disconnected
/// client that carries the signer. Nothing on disk is read or deleted: an
/// existing Liquid working directory is left exactly as it was.
pub async fn load_breez_client(
    datadir: &Path,
    network: Network,
    seed: SeedSource<'_>,
    cube_id: &str,
) -> Result<Arc<BreezClient>, BreezError> {
    let master_signer_fingerprint = seed.fingerprint();

    // `SeedSource::resolve` prefers the signer the unlock already decrypted.
    // Verifying the PIN is a full Argon2id pass over the seed file, so
    // re-reading it here would pay ~831 ms for a result we are already
    // holding. It falls back to disk whenever the session has nothing for this
    // Cube+fingerprint — every other entry point (restore, login, tests) takes
    // that path unchanged.
    let liquid_signer = match seed.resolve(datadir, network, cube_id) {
        Ok(signer) => Arc::new(Mutex::new(signer)),
        Err(e) => {
            let mapped = match e {
                coincube_core::signer::SignerError::MnemonicStorage(io_err)
                    if io_err.kind() == std::io::ErrorKind::NotFound =>
                {
                    BreezError::SignerNotFound(master_signer_fingerprint)
                }
                coincube_core::signer::SignerError::SignerNotFound(fingerprint) => {
                    BreezError::SignerNotFound(fingerprint)
                }
                _ => BreezError::SignerError(e.to_string()),
            };
            // A genuinely *absent* signer (seed-less / watch-only cube) is not
            // fatal now that Liquid never connects — degrade to a signer-less
            // disconnected client so the cube still loads. Before PR 1 this
            // only applied off mainnet; on mainnet the caller's fallback did
            // the same thing after logging a warning. Every other failure
            // (wrong PIN, decryption, IO errors) propagates so it surfaces as
            // a real error rather than being silently masked.
            if matches!(mapped, BreezError::SignerNotFound(_)) {
                log::info!(
                    "No master signer for cube on {network}: {mapped}; \
                     using a signer-less disconnected client"
                );
                return Ok(Arc::new(BreezClient::disconnected(network)));
            }
            return Err(mapped);
        }
    };

    Ok(Arc::new(BreezClient::disconnected_with_signer(
        network,
        liquid_signer,
    )))
}

#[cfg(test)]
mod seed_source_tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::bip32::DerivationPath;
    use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;
    use coincube_core::signer::MasterSigner;
    use std::str::FromStr;

    /// A fresh datadir under the system temp dir. `tempfile` isn't a dependency
    /// of this crate; the same hand-rolled pattern is used in
    /// `phone_signer::pairing_store`.
    fn scratch_datadir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "coincube-seed-source-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The equivalence PR A exists to prove: a signer handed in as
    /// `SeedSource::InMemory` and the *same* seed read back out of its
    /// encrypted file produce the same wallet.
    ///
    /// Compares **xpubs**, not mnemonics — the same rule the two-machine
    /// acceptance gate runs under. Two different encodings of one key would
    /// compare unequal as words and equal as xpubs, and it is the xpub that
    /// decides whether the user's coins are there.
    ///
    /// `load_breez_client` never connects the SDK, so this needs no Breez SDK,
    /// network or API key on any network.
    #[tokio::test]
    async fn in_memory_and_encrypted_file_derive_the_same_xpubs() {
        let datadir = scratch_datadir("equivalence");
        let network = Network::Testnet;
        let cube_id = "cube-equivalence";
        let pin = "1234";
        let secp = Secp256k1::signing_only();

        let signer = MasterSigner::generate(network).unwrap();
        let fingerprint = signer.fingerprint(&secp);
        // v2 file (no device secret), so the read below needs only the PIN.
        signer
            .store_encrypted(&datadir, network, &secp, None, pin, cube_id, None)
            .unwrap();

        let from_file = load_breez_client(
            &datadir,
            network,
            SeedSource::encrypted_file(fingerprint, pin),
            cube_id,
        )
        .await
        .expect("the encrypted-file arm loads");

        let from_memory = load_breez_client(
            &datadir,
            network,
            SeedSource::in_memory(Arc::new(signer.try_clone().unwrap())),
            cube_id,
        )
        .await
        .expect("the in-memory arm loads");

        // BIP-84 account 0 — an arbitrary but real derivation, so this compares
        // derived key material rather than just the master fingerprint.
        let path = DerivationPath::from_str("m/84'/0'/0'").unwrap();
        let xpub_of = |client: &Arc<BreezClient>| {
            let arc = client
                .liquid_signer()
                .expect("a disconnected client still carries its signer");
            let guard = arc.lock().unwrap();
            guard.xpub_at(&path, &secp)
        };

        assert_eq!(
            xpub_of(&from_file),
            xpub_of(&from_memory),
            "the two SeedSource arms must produce the same wallet"
        );
        assert_eq!(
            xpub_of(&from_file),
            signer.xpub_at(&path, &secp),
            "and both must match the seed they were built from"
        );

        let _ = std::fs::remove_dir_all(&datadir);
    }

    /// Restore completion opens a PIN-only session before loading this client.
    /// Exercise that actual loader with a v3 seed and no cached master, rather
    /// than testing only the lower-level unlock helper.
    // The guard deliberately spans the client load: the global session must
    // stay PIN-only until this async restore-completion path finishes.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "needs a code-signed binary (data-protection keychain returns -34018 unsigned)"
    )]
    async fn restored_v3_pin_only_session_loads_without_cached_signer() {
        use crate::app::session;
        use crate::services::unlock::device_secret;
        use coincube_core::signer::SignerError;

        struct Fixture {
            root: std::path::PathBuf,
            cube_id: String,
            other_id: String,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                session::close();
                device_secret::delete(&self.cube_id).expect("remove disposable device secret");
                device_secret::delete(&self.other_id).expect("remove other disposable secret");
                std::fs::remove_dir_all(&self.root).expect("remove disposable seed files");
            }
        }

        let _guard = session::test_guard();
        for network in [Network::Bitcoin, Network::Testnet4] {
            let fixture = Fixture {
                root: scratch_datadir("restored-v3"),
                cube_id: format!("seed-source-restore-{}", uuid::Uuid::new_v4()),
                other_id: format!("seed-source-other-{}", uuid::Uuid::new_v4()),
            };
            session::close();
            let pin = "9753";
            let secp = Secp256k1::signing_only();
            let master = MasterSigner::generate(network).unwrap();
            let fp = master.fingerprint(&secp);
            let secret = device_secret::get_or_create(&fixture.root, &fixture.cube_id).unwrap();
            master
                .store_encrypted(
                    &fixture.root,
                    network,
                    &secp,
                    None,
                    pin,
                    &fixture.cube_id,
                    Some(&secret),
                )
                .unwrap();
            // Core cannot obtain the device secret; routing here recreates the
            // pre-fix GUI failure, even though the Keychain item exists.
            assert!(matches!(
                MasterSigner::from_datadir_by_fingerprint(
                    &fixture.root,
                    network,
                    fp,
                    Some(pin),
                    &fixture.cube_id,
                ),
                Err(SignerError::DeviceSecretRequired)
            ));
            session::open(&fixture.cube_id, zeroize::Zeroizing::new(pin.into()));
            assert!(session::unlocked_signer(&fixture.cube_id, fp).is_none());
            let source = SeedSource::encrypted_file(fp, pin);
            let path = DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();
            assert_eq!(
                source
                    .resolve(&fixture.root, network, &fixture.cube_id)
                    .expect("restore must load the v3 root without a restart")
                    .xpub_at(&path, &secp),
                master.xpub_at(&path, &secp),
            );
            // Both networks take the same restore-completion loader; neither
            // connects the Liquid SDK or needs an API credential.
            {
                let client =
                    load_breez_client(&fixture.root, network, source.clone(), &fixture.cube_id)
                        .await
                        .expect("restore completion must carry the restored v3 signer");
                assert_eq!(
                    client
                        .liquid_signer()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .xpub_at(&path, &secp),
                    master.xpub_at(&path, &secp),
                );
            }
            assert!(session::unlocked_signer(&fixture.cube_id, fp).is_none());
            assert!(matches!(
                SeedSource::encrypted_file(fp, "0000").resolve(
                    &fixture.root,
                    network,
                    &fixture.cube_id
                ),
                Err(SignerError::InvalidPassword)
            ));
            device_secret::get_or_create(&fixture.root, &fixture.other_id).unwrap();
            assert!(matches!(
                source.resolve(&fixture.root, network, &fixture.other_id),
                Err(SignerError::InvalidPassword)
            ));
            assert!(matches!(
                SeedSource::encrypted_file(Fingerprint::default(), pin).resolve(
                    &fixture.root,
                    network,
                    &fixture.cube_id
                ),
                Err(SignerError::SignerNotFound(_))
            ));
            assert!(
                device_secret::load(&fixture.cube_id).unwrap() == secret,
                "loading must not replace the device secret",
            );
            device_secret::delete(&fixture.cube_id).unwrap();
            assert!(matches!(
                source.resolve(&fixture.root, network, &fixture.cube_id),
                Err(SignerError::DeviceSecretRequired)
            ));
            assert!(device_secret::load_optional(&fixture.cube_id)
                .unwrap()
                .is_none());
        }
    }

    /// The property that makes passkey unlock possible at all: the in-memory
    /// arm never touches the datadir, so a Cube with no seed file on disk still
    /// loads its wallets.
    #[tokio::test]
    async fn the_in_memory_arm_loads_with_no_seed_file_on_disk() {
        let datadir = scratch_datadir("no-seed-file");
        let network = Network::Testnet;
        let signer = MasterSigner::generate(network).unwrap();

        let client = load_breez_client(
            &datadir,
            network,
            SeedSource::in_memory(Arc::new(signer)),
            "cube-passkey",
        )
        .await
        .expect("a passkey Cube has no seed file and must still load");
        assert!(client.liquid_signer().is_some());

        let _ = std::fs::remove_dir_all(&datadir);
    }

    /// Liquid removal PR 1: mainnet no longer connects the Liquid SDK. An
    /// existing on-disk Liquid wallet (a grandfathered alpha Cube) still gets a
    /// disconnected client that carries the signer, and its working directory
    /// is left exactly as it was — nothing is read, rewritten or deleted.
    ///
    /// This test is hermetic only because nothing connects: before PR 1 it
    /// would have needed the Breez SDK, a network and an API key.
    #[tokio::test]
    async fn mainnet_never_connects_and_leaves_liquid_state_untouched() {
        let datadir = scratch_datadir("mainnet-no-connect");
        let network = Network::Bitcoin;
        let wallet_dir = datadir.join("breez/mainnet/grandfathered-signer");
        std::fs::create_dir_all(&wallet_dir).unwrap();
        std::fs::write(wallet_dir.join("storage.sql"), b"alpha wallet").unwrap();
        let before = liquid_state_dirs(&datadir, network);
        assert_eq!(before, vec![wallet_dir.clone()]);

        let secp = Secp256k1::signing_only();
        let signer = MasterSigner::generate(network).unwrap();
        let path = DerivationPath::from_str("m/84'/0'/0'").unwrap();
        let expected = signer.xpub_at(&path, &secp);

        let client = load_breez_client(
            &datadir,
            network,
            SeedSource::in_memory(Arc::new(signer)),
            "cube-grandfathered",
        )
        .await
        .expect("a mainnet Cube with Liquid state loads without the SDK");

        assert!(
            !client.is_connected(),
            "the Liquid SDK must not be connected on mainnet"
        );
        let arc = client
            .liquid_signer()
            .expect("the disconnected client still carries the signer");
        assert_eq!(arc.lock().unwrap().xpub_at(&path, &secp), expected);
        assert_eq!(
            liquid_state_dirs(&datadir, network),
            before,
            "the existing Liquid working directory must survive"
        );
        assert_eq!(
            std::fs::read(wallet_dir.join("storage.sql")).unwrap(),
            b"alpha wallet"
        );

        let _ = std::fs::remove_dir_all(&datadir);
    }

    /// A seed-less (watch-only) Cube degrades to a signer-less client on every
    /// network, mainnet included, instead of returning an error the caller
    /// has to paper over.
    #[tokio::test]
    async fn a_missing_signer_degrades_on_mainnet_too() {
        let datadir = scratch_datadir("mainnet-seedless");
        for network in [Network::Bitcoin, Network::Testnet] {
            let client = load_breez_client(
                &datadir,
                network,
                SeedSource::encrypted_file(Fingerprint::default(), "1234"),
                "cube-seedless",
            )
            .await
            .expect("an absent signer is not fatal");
            assert!(client.liquid_signer().is_none());
            assert!(!client.is_connected());
        }
        let _ = std::fs::remove_dir_all(&datadir);
    }
}

#[cfg(test)]
mod sunset_gate_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "coincube-liquid-gate-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_wallet(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("storage.sql"), b"wallet").unwrap();
    }

    fn sorted(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
        dirs.sort();
        dirs
    }

    #[test]
    fn only_a_storage_sql_file_marks_liquid_state_and_networks_stay_apart() {
        let scratch = Scratch::new("state");
        let root = scratch.path();
        assert!(liquid_state_dirs(root, Network::Bitcoin).is_empty());
        assert!(!local_state_exists(root, Network::Bitcoin));
        assert!(!local_state_exists(&root.join("missing"), Network::Testnet));

        let bare = root.join("breez/mainnet/bare");
        std::fs::create_dir_all(&bare).unwrap();
        let sql_dir = root.join("breez/mainnet/sql-is-a-directory");
        std::fs::create_dir_all(sql_dir.join("storage.sql")).unwrap();
        let nested = root.join("breez/mainnet/nested/extra");
        write_wallet(&nested);
        assert!(
            liquid_state_dirs(root, Network::Bitcoin).is_empty(),
            "a bare directory, a storage.sql directory, or a nested file is not a wallet"
        );

        let mainnet = root.join("breez/mainnet/signer-a");
        write_wallet(&mainnet);
        assert_eq!(
            sorted(liquid_state_dirs(root, Network::Bitcoin)),
            vec![mainnet]
        );
        assert!(local_state_exists(root, Network::Bitcoin));
        assert!(
            liquid_state_dirs(root, Network::Testnet).is_empty(),
            "a mainnet wallet must not satisfy the testnet gate"
        );
        assert!(liquid_state_dirs(root, Network::Regtest).is_empty());

        let testnet = root.join("breez/testnet/signer-b");
        write_wallet(&testnet);
        assert_eq!(
            sorted(liquid_state_dirs(root, Network::Testnet)),
            vec![testnet.clone()]
        );
        assert_eq!(
            sorted(liquid_state_dirs(root, Network::Signet)),
            vec![testnet],
            "every non-mainnet network shares the SDK testnet directory"
        );
        assert!(local_state_exists(root, Network::Bitcoin));
    }
}
