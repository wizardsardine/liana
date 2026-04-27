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

/// `Debug` is implemented manually — the derived version would
/// recursively print `cube` and `mnemonic`, exposing PII (cube
/// name, UUID, Lightning address) and the mnemonic phrase through
/// any `{:?}` site. Keep the manual impl in sync with the struct
/// fields.
#[derive(Serialize, Deserialize, Clone)]
pub struct SeedBlob {
    pub version: u8,
    pub cube: SeedBlobCube,
    pub mnemonic: SeedBlobMnemonic,
}

impl std::fmt::Debug for SeedBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Preserve `version` — it's non-sensitive and useful when
        // diagnosing a version-mismatch rejection. Delegate the
        // other two fields to their own redacting Debug impls.
        f.debug_struct("SeedBlob")
            .field("version", &self.version)
            .field("cube", &self.cube)
            .field("mnemonic", &self.mnemonic)
            .finish()
    }
}

/// Cube-scoped metadata on `SeedBlob`. `Debug` is manual to avoid
/// leaking the UUID (cube identifier), user-chosen `name`, and
/// `lightning_address` (directly identifying) via any `{:?}` site.
/// Non-sensitive fields (`network`, `created_at`) stay visible
/// because they're useful diagnostic context.
#[derive(Serialize, Deserialize, Clone)]
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

impl std::fmt::Debug for SeedBlobCube {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeedBlobCube")
            .field("uuid", &"<redacted>")
            .field("name", &"<redacted>")
            .field("network", &self.network)
            .field("created_at", &self.created_at)
            .field(
                "lightning_address",
                &self.lightning_address.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
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

/// Top-level descriptor blob. Manual `Debug` so the recursive
/// formatters never print the raw descriptor string or xpubs. The
/// `version` field is preserved — useful for diagnosing a
/// version-mismatch rejection.
#[derive(Serialize, Deserialize, Clone)]
pub struct DescriptorBlob {
    pub version: u8,
    pub cube: DescriptorBlobCube,
    pub vault: DescriptorBlobVault,
}

impl std::fmt::Debug for DescriptorBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DescriptorBlob")
            .field("version", &self.version)
            .field("cube", &self.cube)
            .field("vault", &self.vault)
            .finish()
    }
}

/// Cube-scoped metadata on `DescriptorBlob`. Manual `Debug` redacts
/// the UUID; `network` stays visible (non-sensitive).
#[derive(Serialize, Deserialize, Clone)]
pub struct DescriptorBlobCube {
    pub uuid: String,
    pub network: String,
}

impl std::fmt::Debug for DescriptorBlobCube {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DescriptorBlobCube")
            .field("uuid", &"<redacted>")
            .field("network", &self.network)
            .finish()
    }
}

/// Wallet contents. Manual `Debug` redacts the descriptor (which
/// contains xpubs inline), the change descriptor, and the vault
/// `name`. Exposes the signer count so a `{:?}` dump still conveys
/// "this is a 2-of-3 with two signers" at a glance for diagnostics.
#[derive(Serialize, Deserialize, Clone)]
pub struct DescriptorBlobVault {
    pub name: String,
    pub descriptor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_descriptor: Option<String>,
    pub signers: Vec<DescriptorBlobSigner>,
}

impl std::fmt::Debug for DescriptorBlobVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DescriptorBlobVault")
            .field("name", &"<redacted>")
            .field("descriptor", &"<redacted>")
            .field(
                "change_descriptor",
                &self.change_descriptor.as_ref().map(|_| "<redacted>"),
            )
            .field("signers", &self.signers) // delegates to redacting impl
            .finish()
    }
}

/// Per-signer metadata. Manual `Debug` redacts the user-chosen `name`
/// and the `xpub` (extended public key — watch-only spend access to
/// every historical and future address). The `fingerprint` stays
/// visible: it's a 4-byte hash of the xpub, commonly shown in UIs
/// (hardware wallet displays, descriptor listings) and useful for
/// identifying which signer a log line refers to.
#[derive(Serialize, Deserialize, Clone)]
pub struct DescriptorBlobSigner {
    pub name: String,
    /// Lowercase hex, no `0x` prefix, 8 chars — master key fingerprint.
    pub fingerprint: String,
    pub xpub: String,
}

impl std::fmt::Debug for DescriptorBlobSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DescriptorBlobSigner")
            .field("name", &"<redacted>")
            .field("fingerprint", &self.fingerprint)
            .field("xpub", &"<redacted>")
            .finish()
    }
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

    // Regression tests for the PII / wallet-privacy redaction in
    // `SeedBlobCube` / `DescriptorBlob*`. Each uses a canary value
    // that shouldn't appear in Debug output; the redacting impls
    // preserve non-sensitive metadata (network, version, signer
    // fingerprint) for diagnostic value.

    #[test]
    fn seed_blob_cube_debug_redacts_uuid_name_lightning() {
        let cube = SeedBlobCube {
            uuid: "uuid-canary-XYZZY".into(),
            name: "name-canary-XYZZY".into(),
            network: "mainnet".into(),
            created_at: "2026-04-23T00:00:00Z".into(),
            lightning_address: Some("lightning-canary-XYZZY@example".into()),
        };
        let r = format!("{:?}", cube);
        assert!(!r.contains("uuid-canary-XYZZY"), "uuid leaked: {}", r);
        assert!(!r.contains("name-canary-XYZZY"), "name leaked: {}", r);
        assert!(
            !r.contains("lightning-canary-XYZZY"),
            "lightning address leaked: {}",
            r
        );
        // Non-sensitive fields are preserved for diagnostics.
        assert!(
            r.contains("\"mainnet\""),
            "network should be visible: {}",
            r
        );
        assert!(
            r.contains("2026-04-23T00:00:00Z"),
            "created_at should be visible: {}",
            r
        );
    }

    #[test]
    fn seed_blob_cube_debug_none_lightning_renders_as_none() {
        let cube = SeedBlobCube {
            uuid: "u".into(),
            name: "n".into(),
            network: "mainnet".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            lightning_address: None,
        };
        let r = format!("{:?}", cube);
        // Absent address → `None`, not `Some(<redacted>)`.
        assert!(r.contains("None"), "got: {}", r);
    }

    #[test]
    fn descriptor_blob_cube_debug_redacts_uuid() {
        let c = DescriptorBlobCube {
            uuid: "uuid-canary-XYZZY".into(),
            network: "mainnet".into(),
        };
        let r = format!("{:?}", c);
        assert!(!r.contains("uuid-canary-XYZZY"), "uuid leaked: {}", r);
        assert!(r.contains("\"mainnet\""));
    }

    #[test]
    fn descriptor_blob_vault_debug_redacts_name_and_descriptors() {
        let v = DescriptorBlobVault {
            name: "vault-name-canary-XYZZY".into(),
            descriptor: "wsh(descriptor-canary-XYZZY)".into(),
            change_descriptor: Some("wsh(change-canary-XYZZY)".into()),
            signers: vec![],
        };
        let r = format!("{:?}", v);
        assert!(!r.contains("vault-name-canary-XYZZY"), "name leaked: {}", r);
        assert!(
            !r.contains("descriptor-canary-XYZZY"),
            "descriptor leaked: {}",
            r
        );
        assert!(
            !r.contains("change-canary-XYZZY"),
            "change_descriptor leaked: {}",
            r
        );
    }

    #[test]
    fn descriptor_blob_signer_debug_redacts_name_and_xpub_but_keeps_fingerprint() {
        let s = DescriptorBlobSigner {
            name: "signer-name-canary-XYZZY".into(),
            fingerprint: "deadbeef".into(),
            xpub: "xpub-canary-XYZZY".into(),
        };
        let r = format!("{:?}", s);
        assert!(
            !r.contains("signer-name-canary-XYZZY"),
            "name leaked: {}",
            r
        );
        assert!(!r.contains("xpub-canary-XYZZY"), "xpub leaked: {}", r);
        // Fingerprint is the low-entropy 4-byte identifier; exposing
        // it aligns with hardware-wallet UI conventions and gives
        // `{:?}` dumps enough context to distinguish signers.
        assert!(
            r.contains("deadbeef"),
            "fingerprint should be visible: {}",
            r
        );
    }

    #[test]
    fn descriptor_blob_debug_propagates_redaction_through_children() {
        // Composite Debug on the top-level blob must delegate to
        // the redacting child impls, not leak through a derived
        // impl that went stale.
        let blob = DescriptorBlob {
            version: BLOB_VERSION,
            cube: DescriptorBlobCube {
                uuid: "uuid-canary-XYZZY".into(),
                network: "mainnet".into(),
            },
            vault: DescriptorBlobVault {
                name: "vault-canary-XYZZY".into(),
                descriptor: "descriptor-canary-XYZZY".into(),
                change_descriptor: None,
                signers: vec![DescriptorBlobSigner {
                    name: "signer-canary-XYZZY".into(),
                    fingerprint: "deadbeef".into(),
                    xpub: "xpub-canary-XYZZY".into(),
                }],
            },
        };
        let r = format!("{:?}", blob);
        for canary in [
            "uuid-canary-XYZZY",
            "vault-canary-XYZZY",
            "descriptor-canary-XYZZY",
            "signer-canary-XYZZY",
            "xpub-canary-XYZZY",
        ] {
            assert!(!r.contains(canary), "{} leaked: {}", canary, r);
        }
        // Version and fingerprint are preserved diagnostics.
        assert!(r.contains("version: 1"), "version missing: {}", r);
        assert!(r.contains("deadbeef"), "fingerprint missing: {}", r);
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
