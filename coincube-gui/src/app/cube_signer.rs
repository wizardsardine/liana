//! The Cube's master signer, for the features that derive from the master seed.
//!
//! Liquid removal, desktop PR 2. This module starts as the golden-vector
//! fixtures (master invariant I2): values captured from the code paths that
//! read the signer out of the Liquid client, pinned so the move to a dedicated
//! signer type can be shown to change nothing.

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
    //! read through `BreezClient::liquid_signer()` after `load_breez_client`)
    //! and are hard-coded, so a later change to any derivation fails here.

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

    /// The unlock loaders (PIN: encrypted seed file; passkey: in-memory seed)
    /// produce the golden P2P mnemonic/identity and Connect encryption key.
    // The session guard deliberately spans the loads: no other test may touch
    // the global session while these resolve through it.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn unlock_paths_yield_the_golden_values() {
        use crate::app::breez_liquid::load_breez_client;
        use crate::app::seed_source::SeedSource;
        use std::sync::Arc;

        let _guard = crate::app::session::test_guard();
        crate::app::session::close();
        for network in NETWORKS {
            let scratch = Scratch::new("golden");
            let cube_id = "cube-golden-pin";
            let fp = write_pin_seed(&scratch.0, network, cube_id);
            let pin_client = load_breez_client(
                &scratch.0,
                network,
                SeedSource::encrypted_file(fp, PIN),
                cube_id,
            )
            .await
            .unwrap();
            let passkey_client = load_breez_client(
                &scratch.0,
                network,
                SeedSource::in_memory(Arc::new(passkey_signer(network))),
                "cube-golden-passkey",
            )
            .await
            .unwrap();

            for (client, mnemonic, identity, enc_key) in [
                (
                    &pin_client,
                    PIN_MNEMONIC,
                    PIN_P2P_IDENTITY,
                    pin_enc_key(network),
                ),
                (
                    &passkey_client,
                    PASSKEY_MNEMONIC,
                    PASSKEY_P2P_IDENTITY,
                    passkey_enc_key(network),
                ),
            ] {
                // The P2P panel's expression (`Panels::new`/`new_without_vault`).
                let p2p = client
                    .liquid_signer()
                    .map(|s| s.lock().expect("signer lock").mnemonic_str())
                    .unwrap();
                assert_eq!(p2p.as_str(), mnemonic, "{network}");
                assert_eq!(p2p_identity(&p2p), identity, "{network}");
                let key = crate::app::derive_cube_encryption_key(client, network).unwrap();
                assert_eq!(key.public_key_hex(), enc_key, "{network}");
            }
        }
    }

    /// The existing Connect xpub-envelope known-answer fixture still opens with
    /// the key the PIN unlock path derives.
    // The session guard deliberately spans the loads: no other test may touch
    // the global session while these resolve through it.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn the_connect_envelope_fixture_opens_after_unlock() {
        use crate::app::breez_liquid::load_breez_client;
        use crate::app::seed_source::SeedSource;
        use crate::services::connect::crypto::cube_enc_key::tests as kat;

        let _guard = crate::app::session::test_guard();
        crate::app::session::close();
        let network = Network::Testnet;
        let scratch = Scratch::new("envelope");
        let cube_id = "cube-golden-envelope";
        let fp = write_pin_seed(&scratch.0, network, cube_id);
        let client = load_breez_client(
            &scratch.0,
            network,
            SeedSource::encrypted_file(fp, PIN),
            cube_id,
        )
        .await
        .unwrap();
        let key = crate::app::derive_cube_encryption_key(&client, network).unwrap();
        let xpub = key
            .open(&kat::kat_envelope(), kat::KAT_CUBE_ID, kat::KAT_KEY_ID)
            .expect("the existing envelope fixture opens");
        assert_eq!(std::str::from_utf8(&xpub).unwrap(), kat::KAT_XPUB);
    }
}
