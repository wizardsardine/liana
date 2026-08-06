//! Where a wallet loader gets its master seed from.
//!
//! Both wallet backends — Liquid ([`crate::app::breez_liquid::load_breez_client`])
//! and Spark ([`crate::app::breez_spark::load_spark_client`]) — need the Cube's
//! [`MasterSigner`]. Until now they each took `(fingerprint, password)` and
//! resolved it the same implicit way: check the session cache, else decrypt the
//! seed file. That pair of parameters encodes an assumption a passkey Cube
//! breaks — **there is no seed file**. Its seed is re-derived from a WebAuthn
//! PRF assertion at unlock and exists only in memory.
//!
//! [`SeedSource`] makes the choice explicit instead of implicit:
//!
//! - [`SeedSource::EncryptedFile`] is today's behaviour, unchanged, down to the
//!   session-cache fast path that keeps an unlock from paying Argon2id at
//!   256 MiB three times (~831 ms each). Every PIN Cube takes this arm.
//! - [`SeedSource::InMemory`] is a signer the caller already holds — a passkey
//!   PRF derivation, or an unlock that already paid the Argon2id cost and would
//!   rather not pay it again.
//!
//! # Why the fingerprint is carried, not derived
//!
//! Callers need it for more than loading: the Spark loader namespaces its
//! SDK storage directory by fingerprint, and the Liquid loader reports
//! `SignerNotFound(fp)`. Recomputing it from the signer means a fresh
//! `Secp256k1` context per call, so [`SeedSource::in_memory`] computes it once
//! at construction and every later read is a copy.

use std::path::Path;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, secp256k1::Secp256k1, Network};
use coincube_core::signer::{MasterSigner, SignerError};

/// How a wallet loader should obtain the Cube's master signer.
#[derive(Clone)]
pub enum SeedSource<'a> {
    /// Decrypt the on-disk seed file. Today's behaviour, unchanged.
    EncryptedFile {
        fingerprint: Fingerprint,
        password: &'a str,
    },
    /// Use a signer the caller already holds — a passkey PRF derivation, or an
    /// unlock that already paid the Argon2id cost.
    InMemory {
        fingerprint: Fingerprint,
        signer: Arc<MasterSigner>,
    },
}

impl<'a> SeedSource<'a> {
    /// The on-disk arm: resolve by fingerprint, decrypting with `password`.
    pub fn encrypted_file(fingerprint: Fingerprint, password: &'a str) -> Self {
        Self::EncryptedFile {
            fingerprint,
            password,
        }
    }

    /// The in-memory arm. Computes the signer's fingerprint once here so no
    /// consumer has to stand up a secp context to ask for it.
    pub fn in_memory(signer: Arc<MasterSigner>) -> Self {
        let secp = Secp256k1::signing_only();
        let fingerprint = signer.fingerprint(&secp);
        Self::InMemory {
            fingerprint,
            signer,
        }
    }

    /// The master key this source resolves to. Same value on both arms — the
    /// one recorded in [`crate::app::settings::CubeSettings::master_signer_fingerprint`]
    /// — so storage paths derived from it are stable across the two.
    pub fn fingerprint(&self) -> Fingerprint {
        match self {
            Self::EncryptedFile { fingerprint, .. } | Self::InMemory { fingerprint, .. } => {
                *fingerprint
            }
        }
    }

    /// Produce an owned [`MasterSigner`].
    ///
    /// On the [`SeedSource::InMemory`] arm this is a `try_clone` — a BIP-39
    /// seed stretch plus a BIP-32 master derivation (~1 ms), not another
    /// Argon2id pass.
    ///
    /// On the [`SeedSource::EncryptedFile`] arm it prefers the signer the
    /// unlock already decrypted ([`crate::app::session::unlocked_signer`]) and
    /// falls back to reading the seed file. That ordering is load-bearing
    /// twice over: it keeps one unlock from paying ~831 ms three times, and it
    /// is the *only* way a v3 seed resolves at all — a v3 file needs the
    /// OS-keystore device secret, which `coincube-core` cannot reach.
    pub fn resolve(
        &self,
        datadir: &Path,
        network: Network,
        cube_id: &str,
    ) -> Result<MasterSigner, SignerError> {
        match self {
            Self::InMemory { signer, .. } => signer.try_clone(),
            Self::EncryptedFile {
                fingerprint,
                password,
            } => match crate::app::session::unlocked_signer(cube_id, *fingerprint) {
                Some(signer) => Ok(signer),
                None => MasterSigner::from_datadir_by_fingerprint(
                    datadir,
                    network,
                    *fingerprint,
                    Some(password),
                    cube_id,
                ),
            },
        }
    }
}

impl std::fmt::Debug for SeedSource<'_> {
    /// Manual: the derived impl would print the password in the clear at any
    /// `{:?}` site, and `MasterSigner` is a master seed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EncryptedFile { fingerprint, .. } => f
                .debug_struct("SeedSource::EncryptedFile")
                .field("fingerprint", fingerprint)
                .finish_non_exhaustive(),
            Self::InMemory { fingerprint, .. } => f
                .debug_struct("SeedSource::InMemory")
                .field("fingerprint", fingerprint)
                .finish_non_exhaustive(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signer() -> MasterSigner {
        MasterSigner::generate(Network::Bitcoin).unwrap()
    }

    #[test]
    fn in_memory_carries_the_signers_own_fingerprint() {
        let s = signer();
        let secp = Secp256k1::signing_only();
        let expected = s.fingerprint(&secp);
        let src = SeedSource::in_memory(Arc::new(s));
        assert_eq!(src.fingerprint(), expected);
    }

    #[test]
    fn in_memory_resolves_without_touching_disk() {
        // The whole point: a passkey Cube has no seed file, so a path that does
        // not exist must still resolve. `words()` compares the mnemonic, which
        // is fine in a test — production comparisons use xpubs.
        let s = signer();
        let words = s.words();
        let src = SeedSource::in_memory(Arc::new(s));
        let got = src
            .resolve(
                Path::new("/nonexistent/datadir"),
                Network::Bitcoin,
                "cube-a",
            )
            .expect("an in-memory signer resolves with no datadir at all");
        assert_eq!(got.words(), words);
    }

    #[test]
    fn debug_never_prints_the_password() {
        let src = SeedSource::encrypted_file(Fingerprint::default(), "1234");
        let rendered = format!("{src:?}");
        assert!(
            !rendered.contains("1234"),
            "SeedSource's Debug leaked the PIN: {}",
            rendered
        );
    }
}
