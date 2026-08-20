//! The password a **passkey** Cube encrypts its seed files under.
//!
//! # The gap this closes
//!
//! Every mnemonic the Vault installer writes goes through
//! `Signer::store_encrypted`, which takes a password and has no plaintext
//! branch (**I5**). The installer gets that password from
//! [`crate::installer::Context::seed_password`], which knew two sources: the
//! PIN chosen on `RestorePinSetupStep`, and the session PIN of the Cube the
//! installer was launched from. Both are PIN-shaped, and a passkey Cube has no
//! PIN at all — [`crate::app::session::pin_for`] answers `None` for it by
//! design. So "Set up a Vault" inside a passkey Cube reached the seed write
//! with nothing to encrypt under and failed loudly at the last step of the
//! installer.
//!
//! Failing loudly was the right call for the case it was written for (a PIN
//! Cube whose session had gone), but a passkey Cube is not a missing PIN — it
//! is a Cube whose root secret is in hand and simply is not a PIN.
//!
//! # What we encrypt under instead
//!
//! A key derived from the Cube's own master seed, at a reserved **hardened**
//! path:
//!
//! ```text
//! m / 7001' / coin_type' / 0'      coin_type = 0' mainnet, 1' otherwise
//! ```
//!
//! rendered as the 32-byte private key in lowercase hex — 256 bits of entropy
//! where a PIN has ~13, fed to the same Argon2id + AES-256-GCM file format.
//!
//! - **Nothing new to back up.** The master seed re-derives it, and a passkey
//!   Cube's master seed comes from the WebAuthn PRF assertion at unlock. So
//!   any machine that can open the Cube can open these files, and one that
//!   cannot open the Cube cannot open them either — the seed file inherits
//!   exactly the Cube's own protection, no more and no less.
//! - **No extra ceremony.** The signer is already in
//!   [`crate::app::session`] (the unlock put it there); deriving asks the
//!   authenticator for nothing, so setting up a Vault does not sprout a second
//!   Touch ID prompt.
//! - **Key separation (I9/I3) holds.** `7001'` is a dedicated hardened purpose,
//!   not a BIP-43 purpose any wallet uses (44/45/48/49/84/86/87), sitting next
//!   to the `7000'` reserved by the Connect-blinding encryption key
//!   ([`crate::services::connect::crypto::ENC_PURPOSE`]). Hardened means no
//!   signing path can reach it and it can reach no signing path, and nothing
//!   under it is ever exported as an xpub or written into a descriptor.
//!
//! # Why not a second PRF domain
//!
//! The obvious alternative — a new PRF salt in the domain registry — would put
//! a fresh authenticator assertion between the user and every seed read, and
//! would make the seed file openable *only* by the credential rather than by
//! the seed. That is strictly worse for **I11**: a Recovery-Kit restore hands
//! back the master seed, not the credential, so a PRF-domain password would
//! leave restored seed files unopenable on the restored machine.
//!
//! # Scope
//!
//! This is the password for *seed files a passkey Cube writes* — today the
//! Vault hot signer, and a recovered signer on the restore-a-Vault-into-this-
//! Cube flow. It is **not** the Cube's master seed file: a passkey Cube has
//! none, and this module does not give it one.

use coincube_core::miniscript::bitcoin::bip32::{ChildNumber, DerivationPath};
use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;
use coincube_core::miniscript::bitcoin::Network;
use coincube_core::signer::MasterSigner;
use zeroize::Zeroizing;

/// Reserved **hardened** BIP-32 purpose for a passkey Cube's seed-file
/// password. See the module docs for why it cannot collide with a signing path.
///
/// Registered alongside `7000'`
/// ([`crate::services::connect::crypto::ENC_PURPOSE`], Connect blinding). Both
/// live in the same reserved block on purpose, so a future third consumer looks
/// here before picking a number.
pub const SEED_FILE_PURPOSE: u32 = 7001;

/// The full derivation path of the seed-file password key on `network`:
/// `m/7001'/coin_type'/0'`, with the BIP-44 coin type (`0'` mainnet, `1'`
/// everywhere else — testnet, signet and regtest all share `1'`, as elsewhere
/// in the wallet).
pub fn seed_file_password_path(network: Network) -> DerivationPath {
    let coin_type = if network == Network::Bitcoin { 0 } else { 1 };
    // All three indices are well under 2^31, so hardening them cannot fail.
    let path: Vec<ChildNumber> = [SEED_FILE_PURPOSE, coin_type, 0]
        .iter()
        .map(|i| {
            ChildNumber::from_hardened_idx(*i).expect("index is below the hardened-derivation cap")
        })
        .collect();
    DerivationPath::from(path)
}

/// Derive the seed-file password for a passkey Cube from its unlocked master
/// signer.
///
/// `signer` must be the Cube's **master seed** signer — the one behind
/// [`crate::app::settings::CubeSettings::master_signer_fingerprint`] — not a
/// Vault hot signer. A different seed derives a different password, and a file
/// written under it would never open again. Callers that can check the
/// fingerprint should ([`crate::installer::Installer::new`] does).
///
/// Returned [`Zeroizing`] so the hex string is wiped when the last holder drops
/// it; the intermediate private key is dropped inside this function.
pub fn derive(signer: &MasterSigner, network: Network) -> Zeroizing<String> {
    let secp = Secp256k1::signing_only();
    let xpriv = signer.xpriv_at(&seed_file_password_path(network), &secp);
    let secret = Zeroizing::new(xpriv.private_key.secret_bytes());
    Zeroizing::new(hex::encode::<[u8; 32]>(*secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signer() -> MasterSigner {
        MasterSigner::generate(Network::Bitcoin).unwrap()
    }

    #[test]
    fn the_path_is_three_hardened_indices_at_the_reserved_purpose() {
        let path = seed_file_password_path(Network::Bitcoin);
        assert_eq!(path.to_string(), "7001'/0'/0'");
        assert_eq!(
            seed_file_password_path(Network::Signet).to_string(),
            "7001'/1'/0'"
        );
        assert!(path.into_iter().all(|c| c.is_hardened()));
    }

    #[test]
    fn the_same_seed_derives_the_same_password() {
        // The whole contract: a Vault installed today has to be readable by
        // every later unlock, and the only thing carried between the two is the
        // seed itself.
        let s = signer();
        let again = s.try_clone().unwrap();
        assert_eq!(
            derive(&s, Network::Bitcoin).as_str(),
            derive(&again, Network::Bitcoin).as_str()
        );
    }

    #[test]
    fn a_different_seed_derives_a_different_password() {
        assert_ne!(
            derive(&signer(), Network::Bitcoin).as_str(),
            derive(&signer(), Network::Bitcoin).as_str()
        );
    }

    #[test]
    fn networks_are_separated() {
        let s = signer();
        assert_ne!(
            derive(&s, Network::Bitcoin).as_str(),
            derive(&s, Network::Signet).as_str()
        );
    }

    #[test]
    fn it_is_thirty_two_bytes_of_lowercase_hex() {
        let pw = derive(&signer(), Network::Bitcoin);
        assert_eq!(pw.len(), 64);
        assert!(pw
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    }

    #[test]
    fn it_is_not_the_connect_encryption_key() {
        // Key separation (I3): the two reserved purposes must not collide, and
        // a change to either constant that made them equal has to fail here
        // rather than in the field.
        use crate::services::connect::crypto::ENC_PURPOSE;
        assert_ne!(SEED_FILE_PURPOSE, ENC_PURPOSE);

        let s = signer();
        let seed_pw = derive(&s, Network::Bitcoin);
        let enc = crate::services::connect::crypto::CubeEncryptionKey::derive(&s, Network::Bitcoin);
        assert_ne!(seed_pw.as_str(), enc.public_key_hex());
    }
}
