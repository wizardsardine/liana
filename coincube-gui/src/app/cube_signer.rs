//! The Cube's master signer, for the features that derive from the master seed.
//!
//! Every non-wallet feature that hangs off the Cube's master seed — the
//! P2P/Mostro identity, the Connect-blinding [`CubeEncryptionKey`] and the
//! Vault builder's copy of that key — used to borrow the signer from the
//! since-removed Liquid client. Liquid removal (desktop PR 2; plan
//! `company-brain/plans/liquid-removal/`) moved it here so those features
//! never depended on Liquid again. Key derivation is unchanged:
//! `coincube-core` is not touched (master invariant I1), and the golden
//! vectors in [`golden`] pin every derived value (I2).
//!
//! A [`CubeSigner`] is resolved once at unlock from the same
//! [`SeedSource::resolve`] call the wallet loaders make, so a PIN Cube keeps
//! the session-cache fast path and a passkey Cube uses its in-memory seed. A
//! seed-less (watch-only) Cube has none.

use std::path::Path;
use std::sync::{Arc, Mutex};

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, Network};
use coincube_core::signer::{MasterSigner, SignerError};

use crate::app::seed_source::SeedSource;
use crate::services::connect::crypto::CubeEncryptionKey;

/// The unlocked master signer of the open Cube.
///
/// Cheap to clone: clones share one signer.
#[derive(Clone)]
pub struct CubeSigner(Arc<Mutex<MasterSigner>>);

/// Why the Cube's master signer could not be resolved at unlock.
///
/// An *absent* signer is not an error: [`CubeSigner::resolve`] returns
/// `Ok(None)` for it.
#[derive(Debug, Clone)]
pub enum CubeSignerError {
    /// No signer for this fingerprint where one was required (a restore that
    /// just wrote the seed).
    NotFound(Fingerprint),
    /// The signer exists but could not be opened: wrong PIN, decryption or IO
    /// failure, missing device secret.
    Signer(String),
}

impl std::fmt::Display for CubeSignerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(fp) => write!(f, "no master signer found for {}", fp),
            Self::Signer(msg) => write!(f, "Signer error: {}", msg),
        }
    }
}

impl std::error::Error for CubeSignerError {}

impl CubeSigner {
    pub fn new(signer: MasterSigner) -> Self {
        Self(Arc::new(Mutex::new(signer)))
    }

    /// Resolves the Cube's master signer at unlock.
    ///
    /// Same call and same error mapping the former Liquid loader used: a genuinely
    /// absent signer (no seed file for the fingerprint, or no signer with it)
    /// is `Ok(None)`, so a seed-less Cube still opens without P2P or a Connect
    /// key; every other failure — wrong PIN, decryption, IO, missing device
    /// secret — is an error and is never masked as a signer-less Cube.
    pub fn resolve(
        datadir: &Path,
        network: Network,
        seed: &SeedSource<'_>,
        cube_id: &str,
    ) -> Result<Option<Self>, CubeSignerError> {
        match seed.resolve(datadir, network, cube_id) {
            Ok(signer) => Ok(Some(Self::new(signer))),
            Err(SignerError::MnemonicStorage(io_err))
                if io_err.kind() == std::io::ErrorKind::NotFound =>
            {
                log::info!(
                    "No master signer for cube on {network} (fingerprint {}); \
                     opening without one",
                    seed.fingerprint()
                );
                Ok(None)
            }
            Err(SignerError::SignerNotFound(fingerprint)) => {
                log::info!(
                    "No master signer for cube on {network} (fingerprint {fingerprint}); \
                     opening without one"
                );
                Ok(None)
            }
            Err(e) => Err(CubeSignerError::Signer(e.to_string())),
        }
    }

    /// The signer the unlock parked in the session, for when resolving at
    /// unlock failed.
    ///
    /// Both the PIN and the passkey unlock store their decrypted signer in the
    /// session before the wallets load, so a load that failed afterwards must
    /// not cost the Cube its signer: P2P, Connect blinding and the Vault
    /// builder would lose it. `None` when the session holds none for this Cube
    /// and fingerprint — a seed-less Cube, or a session closed since.
    pub fn after_failed_unlock_load(cube: &crate::app::settings::CubeSettings) -> Option<Self> {
        let signer = cube
            .master_signer_fingerprint
            .and_then(|fp| crate::app::session::unlocked_signer(&cube.id, fp));
        if signer.is_none() {
            tracing::warn!(
                "No unlocked master signer for Cube {} in the session; it opens without one",
                cube.id
            );
        }
        signer.map(Self::new)
    }

    /// The mnemonic the P2P/Mostro panel derives its Nostr identity from.
    /// `None` if it is empty, which disables the panel.
    pub fn p2p_mnemonic(&self) -> Option<zeroize::Zeroizing<String>> {
        let mnemonic = self.0.lock().expect("signer lock").mnemonic_str();
        (!mnemonic.is_empty()).then_some(mnemonic)
    }

    /// The Cube's Connect-blinding encryption key (`SPEC-cube-xpub-envelope-v1`
    /// §3) — the key whose public half was registered with Connect.
    pub fn encryption_key(&self, network: Network) -> Option<Arc<CubeEncryptionKey>> {
        let guard = self.0.lock().ok()?;
        Some(Arc::new(CubeEncryptionKey::derive(&guard, network)))
    }
}

impl std::fmt::Debug for CubeSigner {
    /// Manual: `MasterSigner` is a master seed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CubeSigner").finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) mod golden {
    //! Golden vectors for master invariant I2: the same Cube yields
    //! byte-identical P2P mnemonic/identity, Connect `CubeEncryptionKey` and
    //! Vault-builder encryption key before and after the signer leaves
    //! `BreezClient`.
    //!
    //! Two fixture Cubes, each on mainnet and testnet:
    //!
    //! - a **PIN Cube** whose master is the BIP-39 test mnemonic, written to an
    //!   encrypted seed file under [`PIN`]. Its testnet encryption key is the
    //!   cross-repo known-answer key `KAT_P` in
    //!   `services::connect::crypto::cube_enc_key`, so the existing Connect
    //!   xpub-envelope fixture opens with it.
    //! - a **passkey Cube** whose master is derived from a fixed WebAuthn PRF
    //!   output with `MasterSigner::from_prf_output`, as passkey unlock does.
    //!   It has no seed file.
    //!
    //! The constants were captured once from the pre-change code (the signer
    //! read through `BreezClient::liquid_signer()` after `load_breez_client`,
    //! commit "pin golden vectors for the Cube master signer's consumers"),
    //! and are hard-coded: the same assertions passed there and pass here on
    //! [`super::CubeSigner`], so a change to any derivation fails here.

    use coincube_core::miniscript::bitcoin::{secp256k1::Secp256k1, Network};
    use coincube_core::signer::MasterSigner;

    /// The throwaway BIP-39 test mnemonic. Never real funds.
    pub(crate) const PIN_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon \
         abandon abandon abandon abandon abandon about";
    pub(crate) const PIN: &str = "4826";
    /// A fixed WebAuthn PRF output standing in for a passkey assertion.
    pub(crate) const PRF_OUTPUT: [u8; 32] = [0x42; 32];
    /// The mnemonic [`PRF_OUTPUT`] derives (`MasterSigner::from_prf_output`).
    pub(crate) const PASSKEY_MNEMONIC: &str =
        "simple prison cake final zoo defense arena tourist example ethics immense grace";

    /// Connect `CubeEncryptionKey` public keys (compressed SEC1, hex).
    pub(crate) const PIN_ENC_KEY_MAINNET: &str =
        "033d142e71b54203c513199de2f369e772c03c6ab427df6a93164db4af4f541868";
    /// Equal to `cube_enc_key::tests::KAT_P`, the cross-repo known answer.
    pub(crate) const PIN_ENC_KEY_TESTNET: &str =
        "0217af5fa4d4b084db9cde6a882ebc99a8e18532371fa1cf267ab66878d103d33b";
    pub(crate) const PASSKEY_ENC_KEY_MAINNET: &str =
        "0294ff35f490e700f03c05240187bddef1bdd1437e3d3a62217e7e7cbb97d72af3";
    pub(crate) const PASSKEY_ENC_KEY_TESTNET: &str =
        "022de7ae821320b7623575ecc09f24edc220ee35abf016fc8ebc0384ff405a337f";

    /// The Mostro/P2P Nostr identity the panel derives from the mnemonic:
    /// trade index 0 on the mostrix path (`view::p2p::mostro`).
    pub(crate) const PIN_P2P_IDENTITY: &str =
        "faa27ea81c85e00798598b46d1f36c1700221a1242b563861fa536dc2314f1df";
    pub(crate) const PASSKEY_P2P_IDENTITY: &str =
        "872a24fbc3d4d7a71e7206a5a817c4addcc9990c17220212687b927648ee9d58";

    pub(crate) const NETWORKS: [Network; 2] = [Network::Bitcoin, Network::Testnet];

    pub(crate) fn pin_signer(network: Network) -> MasterSigner {
        MasterSigner::from_str(network, PIN_MNEMONIC).unwrap()
    }

    pub(crate) fn passkey_signer(network: Network) -> MasterSigner {
        MasterSigner::from_prf_output(network, &PRF_OUTPUT).unwrap()
    }

    pub(crate) fn pin_enc_key(network: Network) -> &'static str {
        match network {
            Network::Bitcoin => PIN_ENC_KEY_MAINNET,
            _ => PIN_ENC_KEY_TESTNET,
        }
    }

    pub(crate) fn passkey_enc_key(network: Network) -> &'static str {
        match network {
            Network::Bitcoin => PASSKEY_ENC_KEY_MAINNET,
            _ => PASSKEY_ENC_KEY_TESTNET,
        }
    }

    /// The P2P identity public key the Mostro code derives from `mnemonic`
    /// (same arguments as `view::p2p::mostro::derive_trade_keys(_, 0)`).
    pub(crate) fn p2p_identity(mnemonic: &str) -> String {
        use nostr_sdk::prelude::*;
        Keys::from_mnemonic_advanced(mnemonic, None, Some(38383), Some(0), Some(0))
            .unwrap()
            .public_key()
            .to_hex()
    }

    /// A fresh datadir under the system temp dir, removed on drop.
    pub(crate) struct Scratch(pub(crate) std::path::PathBuf);

    impl Scratch {
        pub(crate) fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "coincube-cube-signer-{tag}-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Writes the PIN fixture's seed file (v2, no device secret) and returns
    /// its fingerprint.
    pub(crate) fn write_pin_seed(
        datadir: &std::path::Path,
        network: Network,
        cube_id: &str,
    ) -> coincube_core::miniscript::bitcoin::bip32::Fingerprint {
        let secp = Secp256k1::signing_only();
        let signer = pin_signer(network);
        signer
            .store_encrypted(datadir, network, &secp, None, PIN, cube_id, None)
            .unwrap();
        signer.fingerprint(&secp)
    }

    #[test]
    fn fixtures_are_what_they_claim() {
        use crate::services::connect::crypto::CubeEncryptionKey;
        let key = |s: MasterSigner, n| CubeEncryptionKey::derive(&s, n).public_key_hex();
        let passkey_mnemonic = passkey_signer(Network::Bitcoin).mnemonic_str();
        let actual = vec![
            passkey_mnemonic.to_string(),
            p2p_identity(PIN_MNEMONIC),
            p2p_identity(&passkey_mnemonic),
            key(pin_signer(Network::Bitcoin), Network::Bitcoin),
            key(pin_signer(Network::Testnet), Network::Testnet),
            key(passkey_signer(Network::Bitcoin), Network::Bitcoin),
            key(passkey_signer(Network::Testnet), Network::Testnet),
        ];
        let expected = vec![
            PASSKEY_MNEMONIC,
            PIN_P2P_IDENTITY,
            PASSKEY_P2P_IDENTITY,
            PIN_ENC_KEY_MAINNET,
            PIN_ENC_KEY_TESTNET,
            PASSKEY_ENC_KEY_MAINNET,
            PASSKEY_ENC_KEY_TESTNET,
        ];
        assert_eq!(actual, expected);
    }

    /// The unlock resolution (PIN: encrypted seed file; passkey: in-memory
    /// seed) produces the golden P2P mnemonic/identity and Connect encryption
    /// key, through the exact expressions the App uses.
    #[test]
    fn unlock_paths_yield_the_golden_values() {
        use super::CubeSigner;
        use crate::app::seed_source::SeedSource;
        use std::sync::Arc;

        let _guard = crate::app::session::test_guard();
        crate::app::session::close();
        for network in NETWORKS {
            let scratch = Scratch::new("golden");
            let cube_id = "cube-golden-pin";
            let fp = write_pin_seed(&scratch.0, network, cube_id);
            let pin_signer = CubeSigner::resolve(
                &scratch.0,
                network,
                &SeedSource::encrypted_file(fp, PIN),
                cube_id,
            )
            .unwrap();
            let passkey_signer = CubeSigner::resolve(
                &scratch.0,
                network,
                &SeedSource::in_memory(Arc::new(passkey_signer(network))),
                "cube-golden-passkey",
            )
            .unwrap();

            for (signer, mnemonic, identity, enc_key) in [
                (
                    &pin_signer,
                    PIN_MNEMONIC,
                    PIN_P2P_IDENTITY,
                    pin_enc_key(network),
                ),
                (
                    &passkey_signer,
                    PASSKEY_MNEMONIC,
                    PASSKEY_P2P_IDENTITY,
                    passkey_enc_key(network),
                ),
            ] {
                // The P2P panel's expression (`Panels::new`/`new_without_vault`).
                let p2p = signer.as_ref().and_then(CubeSigner::p2p_mnemonic).unwrap();
                assert_eq!(p2p.as_str(), mnemonic, "{network}");
                assert_eq!(p2p_identity(&p2p), identity, "{network}");
                // `App::new_inner` / `App::new_without_wallet`.
                let key = crate::app::derive_cube_encryption_key(signer.as_ref().unwrap(), network)
                    .unwrap();
                assert_eq!(key.public_key_hex(), enc_key, "{network}");
            }
        }
    }

    /// The existing Connect xpub-envelope known-answer fixture still opens with
    /// the key the PIN unlock path derives.
    #[test]
    fn the_connect_envelope_fixture_opens_after_unlock() {
        use super::CubeSigner;
        use crate::app::seed_source::SeedSource;
        use crate::services::connect::crypto::cube_enc_key::tests as kat;

        let _guard = crate::app::session::test_guard();
        crate::app::session::close();
        let network = Network::Testnet;
        let scratch = Scratch::new("envelope");
        let cube_id = "cube-golden-envelope";
        let fp = write_pin_seed(&scratch.0, network, cube_id);
        let signer = CubeSigner::resolve(
            &scratch.0,
            network,
            &SeedSource::encrypted_file(fp, PIN),
            cube_id,
        )
        .unwrap()
        .unwrap();
        let key = crate::app::derive_cube_encryption_key(&signer, network).unwrap();
        let xpub = key
            .open(&kat::kat_envelope(), kat::KAT_CUBE_ID, kat::KAT_KEY_ID)
            .expect("the existing envelope fixture opens");
        assert_eq!(std::str::from_utf8(&xpub).unwrap(), kat::KAT_XPUB);
    }
}

#[cfg(test)]
mod tests {
    use super::golden::{self, Scratch};
    use super::*;
    use crate::app::session;
    use crate::app::settings::CubeSettings;
    use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;

    #[test]
    fn an_absent_signer_resolves_to_none_on_every_network() {
        let _g = session::test_guard();
        session::close();
        let scratch = Scratch::new("absent");
        for network in [Network::Bitcoin, Network::Testnet, Network::Signet] {
            let signer = CubeSigner::resolve(
                &scratch.0,
                network,
                &SeedSource::encrypted_file(Fingerprint::default(), golden::PIN),
                "cube-seedless",
            )
            .expect("an absent signer is not an error");
            assert!(signer.is_none());
        }
    }

    #[test]
    fn a_wrong_pin_is_an_error_not_a_seedless_cube() {
        let _g = session::test_guard();
        session::close();
        let scratch = Scratch::new("wrong-pin");
        let network = Network::Bitcoin;
        let fp = golden::write_pin_seed(&scratch.0, network, "cube-wrong-pin");
        let result = CubeSigner::resolve(
            &scratch.0,
            network,
            &SeedSource::encrypted_file(fp, "0000"),
            "cube-wrong-pin",
        );
        assert!(matches!(result, Err(CubeSignerError::Signer(_))));
        // A seed file bound to another Cube is refused too.
        let result = CubeSigner::resolve(
            &scratch.0,
            network,
            &SeedSource::encrypted_file(fp, golden::PIN),
            "another-cube",
        );
        assert!(matches!(result, Err(CubeSignerError::Signer(_))));
    }

    #[test]
    fn the_session_fast_path_is_kept() {
        let _g = session::test_guard();
        session::close();
        let network = Network::Bitcoin;
        let signer = golden::pin_signer(network);
        let fp = signer.fingerprint(&Secp256k1::signing_only());
        session::store_unlocked_signer("cube-cached", fp, signer);
        // No seed file anywhere: only the session can answer.
        let resolved = CubeSigner::resolve(
            Path::new("/nonexistent/datadir"),
            network,
            &SeedSource::encrypted_file(fp, "not-the-pin"),
            "cube-cached",
        )
        .unwrap()
        .expect("the unlock's cached signer");
        assert_eq!(
            resolved.p2p_mnemonic().unwrap().as_str(),
            golden::PIN_MNEMONIC
        );
        session::close();
    }

    #[test]
    fn the_unlock_fallback_takes_the_session_signer_or_none() {
        let _g = session::test_guard();
        session::close();
        let network = Network::Bitcoin;
        let signer = golden::pin_signer(network);
        let fp = signer.fingerprint(&Secp256k1::signing_only());
        let cube = CubeSettings::new("Fallback".to_string(), network).with_master_signer(fp);
        assert!(CubeSigner::after_failed_unlock_load(&cube).is_none());
        session::store_unlocked_signer(&cube.id, fp, signer);
        assert!(CubeSigner::after_failed_unlock_load(&cube).is_some());
        // Another Cube's session signer is never borrowed.
        let other = CubeSettings::new("Other".to_string(), network).with_master_signer(fp);
        assert!(CubeSigner::after_failed_unlock_load(&other).is_none());
        session::close();
    }

    /// The App opens a seed-less Cube with P2P disabled and no Connect key,
    /// and a Cube with a signer with both, its key matching the golden vector.
    #[test]
    fn the_app_opens_with_and_without_a_cube_signer() {
        let _g = session::test_guard();
        session::close();
        let network = Network::Bitcoin;
        let scratch = Scratch::new("app-open");
        let open = |signer: Option<CubeSigner>| {
            let (app, _task) = crate::app::App::new_without_wallet(
                signer,
                None,
                crate::app::Config::new(false),
                crate::dir::CoincubeDirectory::new(scratch.0.clone()),
                network,
                CubeSettings::new("Golden".into(), crate::chain::ChainId::Bitcoin),
            )
            .unwrap();
            app
        };

        let seedless = open(None);
        assert!(
            !seedless.cache().has_p2p,
            "a seed-less Cube has P2P disabled"
        );
        assert!(seedless.cache().cube_encryption_key.is_none());
        drop(seedless);

        let with_signer = open(Some(CubeSigner::new(golden::pin_signer(network))));
        assert!(with_signer.cache().has_p2p);
        assert_eq!(
            with_signer
                .cache()
                .cube_encryption_key
                .as_ref()
                .unwrap()
                .public_key_hex(),
            golden::PIN_ENC_KEY_MAINNET
        );
    }

    /// The equivalence passkey unlock rests on (moved here from the removed
    /// Liquid loader's tests): a signer handed in as `SeedSource::InMemory`
    /// and the *same* seed read back out of its encrypted file resolve to the
    /// same wallet. Compares xpubs, not mnemonics — it is the xpub that
    /// decides whether the user's coins are there.
    #[test]
    fn in_memory_and_encrypted_file_resolve_the_same_xpubs() {
        use coincube_core::miniscript::bitcoin::bip32::DerivationPath;
        use std::str::FromStr;
        let _g = session::test_guard();
        session::close();
        let scratch = Scratch::new("equivalence");
        let network = Network::Testnet;
        let cube_id = "cube-equivalence";
        let secp = Secp256k1::signing_only();
        let signer = MasterSigner::generate(network).unwrap();
        let fingerprint = signer.fingerprint(&secp);
        signer
            .store_encrypted(&scratch.0, network, &secp, None, "1234", cube_id, None)
            .unwrap();
        let path = DerivationPath::from_str("m/84'/0'/0'").unwrap();
        let xpub_of = |source: SeedSource<'_>| {
            let resolved = CubeSigner::resolve(&scratch.0, network, &source, cube_id)
                .unwrap()
                .expect("the signer resolves");
            let guard = resolved.0.lock().unwrap();
            guard.xpub_at(&path, &secp)
        };
        let from_file = xpub_of(SeedSource::encrypted_file(fingerprint, "1234"));
        let from_memory = xpub_of(SeedSource::in_memory(Arc::new(signer.try_clone().unwrap())));
        assert_eq!(from_file, from_memory);
        assert_eq!(from_file, signer.xpub_at(&path, &secp));
    }

    /// Passkey unlock: the in-memory arm never touches the datadir, so a Cube
    /// with no seed file on disk still resolves its signer.
    #[test]
    fn the_in_memory_arm_resolves_with_no_seed_file_on_disk() {
        let scratch = Scratch::new("no-seed-file");
        let network = Network::Testnet;
        let signer = MasterSigner::generate(network).unwrap();
        let resolved = CubeSigner::resolve(
            &scratch.0,
            network,
            &SeedSource::in_memory(Arc::new(signer)),
            "cube-passkey",
        )
        .expect("a passkey Cube has no seed file and must still resolve");
        assert!(resolved.is_some());
        assert!(std::fs::read_dir(&scratch.0).unwrap().next().is_none());
    }

    /// Restore completion opens a PIN-only session before resolving the signer.
    /// Exercise that actual loader with a v3 seed and no cached master, rather
    /// than testing only the lower-level unlock helper.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "needs a code-signed binary (data-protection keychain returns -34018 unsigned)"
    )]
    fn restored_v3_pin_only_session_loads_without_cached_signer() {
        use crate::app::session;
        use crate::services::unlock::device_secret;
        use coincube_core::miniscript::bitcoin::bip32::DerivationPath;
        use coincube_core::signer::SignerError;
        use std::str::FromStr;

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
                root: {
                    let root = std::env::temp_dir()
                        .join(format!("coincube-restored-v3-{}", uuid::Uuid::new_v4()));
                    std::fs::create_dir_all(&root).unwrap();
                    root
                },
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
            // Both networks take the same restore-completion resolver.
            {
                let resolved =
                    CubeSigner::resolve(&fixture.root, network, &source, &fixture.cube_id)
                        .expect("restore completion must carry the restored v3 signer")
                        .expect("the restored signer");
                assert_eq!(
                    resolved.0.lock().unwrap().xpub_at(&path, &secp),
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

    /// Liquid removal, master invariant I3: a Cube whose datadir still holds
    /// the old Liquid SDK working directory and a seed file under the legacy
    /// `-liquid_` label opens — its signer resolves from that file — and both
    /// are left exactly as they were.
    #[test]
    fn a_cube_with_legacy_liquid_data_opens_and_keeps_it() {
        let _g = session::test_guard();
        session::close();
        let network = Network::Bitcoin;
        let scratch = Scratch::new("legacy-liquid");
        let cube_id = "cube-legacy-liquid";
        let fp = golden::write_pin_seed(&scratch.0, network, cube_id);

        // Relabel the seed file the way alpha builds named it.
        fn files(dir: &Path) -> Vec<std::path::PathBuf> {
            let mut out = Vec::new();
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    out.extend(files(&path));
                } else {
                    out.push(path);
                }
            }
            out
        }
        let master = files(&scratch.0)
            .into_iter()
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains(&fp.to_string()))
            })
            .expect("the seed file");
        // `mnemonic-<fp>-<label><checksum>-<timestamp>.txt`, as the Liquid
        // signer's seed file was named (`LEGACY_LIQUID_SEED_LABEL`).
        let legacy = master.with_file_name(format!("mnemonic-{}-liquid_cube-1700000000.txt", fp));
        std::fs::rename(&master, &legacy).unwrap();
        let legacy_bytes = std::fs::read(&legacy).unwrap();

        // The Liquid SDK's working directory, as an alpha build left it.
        let wallet_dir = scratch.0.join("breez/mainnet/legacy-signer");
        std::fs::create_dir_all(&wallet_dir).unwrap();
        std::fs::write(wallet_dir.join("storage.sql"), b"alpha liquid wallet").unwrap();

        let signer = CubeSigner::resolve(
            &scratch.0,
            network,
            &SeedSource::encrypted_file(fp, golden::PIN),
            cube_id,
        )
        .unwrap()
        .expect("the legacy-labelled seed file still resolves");
        let mut cube = CubeSettings::new("Legacy".into(), crate::chain::ChainId::Bitcoin)
            .with_master_signer(fp);
        cube.id = cube_id.to_string();
        let (app, _task) = crate::app::App::new_without_wallet(
            Some(signer),
            None,
            crate::app::Config::new(false),
            crate::dir::CoincubeDirectory::new(scratch.0.clone()),
            network,
            cube,
        )
        .expect("the Cube opens");
        assert!(app.cache().has_p2p);
        assert_eq!(
            app.cache()
                .cube_encryption_key
                .as_ref()
                .unwrap()
                .public_key_hex(),
            golden::PIN_ENC_KEY_MAINNET
        );
        drop(app);

        assert_eq!(std::fs::read(&legacy).unwrap(), legacy_bytes);
        assert_eq!(
            std::fs::read(wallet_dir.join("storage.sql")).unwrap(),
            b"alpha liquid wallet"
        );
        session::close();
    }

    #[test]
    fn debug_never_prints_the_seed() {
        let rendered = format!(
            "{:?}",
            CubeSigner::new(golden::pin_signer(Network::Bitcoin))
        );
        assert!(!rendered.contains("abandon"), "{}", rendered);
    }
}
