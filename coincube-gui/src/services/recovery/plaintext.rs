//! Plaintext blob types for the Cube Recovery Kit.
//!
//! Two blobs are carried independently inside the envelope:
//!   * `SeedBlob`  — encryption of the master mnemonic + minimal cube
//!     metadata needed to restore the cube shell.
//!   * `DescriptorBlob` — encryption of the wallet descriptor and its
//!     signer xpubs (no private keys).
//!
//! Both are serde-JSON before encryption and back to serde-JSON after
//! decryption; the envelope carries the ciphertext bytes as opaque
//! payload.
//!
//! Aligned with `PLAN-cube-recovery-kit-desktop.md` §2.1.

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Latest schema version this client writes for `SeedBlob` / `DescriptorBlob`.
/// Bump on any breaking shape change; the reader should refuse unknown
/// versions. Carried inside each blob so mixing kits across client versions
/// is safe.
pub const BLOB_VERSION: u8 = 1;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SeedBlob {
    pub version: u8,
    pub cube: SeedBlobCube,
    pub mnemonic: SeedBlobMnemonic,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SeedBlobCube {
    pub uuid: String,
    pub name: String,
    /// One of "bitcoin" | "testnet" | "signet" | "regtest". String rather
    /// than an enum so a future network addition doesn't break older kits.
    pub network: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightning_address: Option<String>,
}

/// Wraps the mnemonic phrase in its own struct with `ZeroizeOnDrop` so
/// that even if `SeedBlob` leaks through state cloning (e.g. Iced message
/// snapshots), the phrase material is wiped when the clone drops.
///
/// `Debug` is implemented manually below with the `phrase` field
/// redacted. **Do not `#[derive(Debug)]`** — the derived impl would
/// print the mnemonic in plaintext to any `{:?}` format, tracing
/// subscriber, or debugger watch-window, defeating the whole point of
/// the zeroize wrapping. The manual impl below is the only `Debug`
/// for this type; keep it that way.
#[derive(Serialize, Deserialize, Clone, Zeroize, ZeroizeOnDrop)]
pub struct SeedBlobMnemonic {
    pub phrase: String,
    /// BIP39 wordlist language, e.g. "en". Persisted so restore doesn't
    /// have to guess.
    pub language: String,
}

impl std::fmt::Debug for SeedBlobMnemonic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeedBlobMnemonic")
            .field("phrase", &"<redacted>")
            .field("language", &self.language)
            .finish()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DescriptorBlob {
    pub version: u8,
    pub cube: DescriptorBlobCube,
    pub vault: DescriptorBlobVault,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DescriptorBlobCube {
    pub uuid: String,
    pub network: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DescriptorBlobVault {
    pub name: String,
    pub descriptor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_descriptor: Option<String>,
    pub signers: Vec<DescriptorBlobSigner>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DescriptorBlobSigner {
    pub name: String,
    /// Lowercase hex, no `0x` prefix, 8 chars — master key fingerprint.
    pub fingerprint: String,
    pub xpub: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canary string unlikely to appear anywhere else in the
    /// `Debug` output (field names, "<redacted>" marker, etc.).
    /// If this ever surfaces in a `{:?}` dump, the redacting
    /// `Debug` impl has regressed.
    const CANARY_PHRASE: &str = "sentinel-mnemonic-canary-word-sequence-XYZZY";

    #[test]
    fn seed_blob_mnemonic_debug_redacts_phrase() {
        let m = SeedBlobMnemonic {
            phrase: CANARY_PHRASE.to_string(),
            language: "en".to_string(),
        };
        let rendered = format!("{:?}", m);
        assert!(
            !rendered.contains(CANARY_PHRASE),
            "mnemonic phrase must not appear in Debug output, got: {}",
            rendered,
        );
        assert!(
            rendered.contains("<redacted>"),
            "expected '<redacted>' marker in Debug output, got: {}",
            rendered,
        );
        // Language is fine to leak — it's "en", not secret.
        assert!(rendered.contains("\"en\""));
    }

    #[test]
    fn seed_blob_debug_does_not_leak_mnemonic_through_parent() {
        // The `SeedBlob` parent still derives `Debug`; verify the
        // redacting impl on the child propagates cleanly so printing
        // the whole blob doesn't accidentally re-expose the phrase.
        let blob = SeedBlob {
            version: BLOB_VERSION,
            cube: SeedBlobCube {
                uuid: "u".into(),
                name: "n".into(),
                network: "bitcoin".into(),
                created_at: "2026-04-23T00:00:00Z".into(),
                lightning_address: None,
            },
            mnemonic: SeedBlobMnemonic {
                phrase: CANARY_PHRASE.to_string(),
                language: "en".to_string(),
            },
        };
        let rendered = format!("{:?}", blob);
        assert!(
            !rendered.contains(CANARY_PHRASE),
            "SeedBlob Debug leaked the phrase: {}",
            rendered,
        );
    }

    #[test]
    fn seed_blob_roundtrip_json() {
        let blob = SeedBlob {
            version: BLOB_VERSION,
            cube: SeedBlobCube {
                uuid: "cube-uuid".into(),
                name: "My Cube".into(),
                network: "bitcoin".into(),
                created_at: "2026-04-22T00:00:00Z".into(),
                lightning_address: Some("alice@coincube.io".into()),
            },
            mnemonic: SeedBlobMnemonic {
                phrase: "abandon abandon abandon abandon abandon abandon abandon abandon \
                         abandon abandon abandon about"
                    .into(),
                language: "en".into(),
            },
        };
        let json = serde_json::to_string(&blob).unwrap();
        let back: SeedBlob = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, BLOB_VERSION);
        assert_eq!(back.cube.uuid, "cube-uuid");
        assert_eq!(back.mnemonic.language, "en");
        assert_eq!(
            back.cube.lightning_address.as_deref(),
            Some("alice@coincube.io")
        );
    }

    #[test]
    fn descriptor_blob_roundtrip_json() {
        let blob = DescriptorBlob {
            version: BLOB_VERSION,
            cube: DescriptorBlobCube {
                uuid: "cube-uuid".into(),
                network: "bitcoin".into(),
            },
            vault: DescriptorBlobVault {
                name: "Vault A".into(),
                descriptor: "wsh(...)".into(),
                change_descriptor: Some("wsh(...change)".into()),
                signers: vec![DescriptorBlobSigner {
                    name: "Device 1".into(),
                    fingerprint: "deadbeef".into(),
                    xpub: "xpub...".into(),
                }],
            },
        };
        let json = serde_json::to_string(&blob).unwrap();
        let back: DescriptorBlob = serde_json::from_str(&json).unwrap();
        assert_eq!(back.vault.name, "Vault A");
        assert_eq!(back.vault.signers.len(), 1);
        assert_eq!(back.vault.signers[0].fingerprint, "deadbeef");
    }

    #[test]
    fn optional_lightning_address_missing_deserialises() {
        // A kit written before we added the field (or by a client that
        // didn't have one) must still load.
        let json = r#"{
            "version": 1,
            "cube": {
                "uuid": "x",
                "name": "n",
                "network": "bitcoin",
                "created_at": "2026-01-01T00:00:00Z"
            },
            "mnemonic": { "phrase": "p", "language": "en" }
        }"#;
        let blob: SeedBlob = serde_json::from_str(json).unwrap();
        assert!(blob.cube.lightning_address.is_none());
    }
}
