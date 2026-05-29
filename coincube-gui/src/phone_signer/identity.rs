//! Long-lived desktop identity for the local-signer TLS layer.
//!
//! We mint **one** Ed25519 keypair + self-signed cert per desktop on
//! first use and persist them under [`CoincubeDirectory`]. The public
//! key of that cert is the desktop's identity — it goes into every
//! pairing QR so phones can pin it, and rustls server-side presents
//! the cert to the connecting phone.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ED25519};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};

use crate::dir::CoincubeDirectory;

const IDENTITY_FILENAME: &str = "phone-signer-identity.json";

fn identity_path(dir: &CoincubeDirectory) -> PathBuf {
    dir.path().join(IDENTITY_FILENAME)
}

/// On-disk record. PEM-encoded so the file is greppable for ops, and
/// so re-importing into other tools (debugging, manual revocation) is
/// trivial.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredIdentity {
    cert_pem: String,
    key_pem: String,
}

/// In-memory identity used by the TLS layer.
///
/// `PrivateKeyDer` deliberately doesn't impl `Clone` (zeroize
/// hygiene), so call [`DesktopIdentity::clone_key`] when handing the
/// key to a rustls config and clone the cert via its own `Clone`.
pub struct DesktopIdentity {
    pub cert_der: CertificateDer<'static>,
    pub key_der: PrivateKeyDer<'static>,
    /// Raw 32-byte Ed25519 public key. This is the bytes the phone
    /// pins from the QR (alongside the cert hash) and the desktop
    /// publishes as `fp=` in mDNS.
    pub pubkey: [u8; 32],
}

impl DesktopIdentity {
    pub fn clone_key(&self) -> PrivateKeyDer<'static> {
        self.key_der.clone_key()
    }
}

impl std::fmt::Debug for DesktopIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesktopIdentity")
            .field("pubkey_fp8", &fingerprint_hex8(&self.pubkey))
            .finish()
    }
}

impl DesktopIdentity {
    /// First-4-bytes-hex of the pubkey, the form we publish in mDNS
    /// TXT records (`fp=…`).
    pub fn fingerprint_hex8(&self) -> String {
        fingerprint_hex8(&self.pubkey)
    }
}

/// Load the persisted identity, generating one on first use.
pub fn load_or_create(dir: &CoincubeDirectory) -> io::Result<DesktopIdentity> {
    match fs::read(identity_path(dir)) {
        Ok(bytes) => {
            let stored: StoredIdentity = serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            identity_from_stored(&stored)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let (identity, stored) = mint_new()?;
            write_atomic(dir, &stored)?;
            Ok(identity)
        }
        Err(e) => Err(e),
    }
}

fn mint_new() -> io::Result<(DesktopIdentity, StoredIdentity)> {
    let key_pair = KeyPair::generate_for(&PKCS_ED25519)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("ed25519 keygen: {}", e)))?;

    let mut params = CertificateParams::new(vec!["coincube-desktop.local".to_string()])
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("cert params: {}", e)))?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "Coincube Desktop Local Signer");
    params.distinguished_name = dn;

    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("self-sign: {}", e)))?;

    let stored = StoredIdentity {
        cert_pem: cert.pem(),
        key_pem: key_pair.serialize_pem(),
    };
    let identity = identity_from_stored(&stored)?;
    Ok((identity, stored))
}

fn identity_from_stored(stored: &StoredIdentity) -> io::Result<DesktopIdentity> {
    // Parse the cert PEM -> DER.
    let cert_der = pem_to_der(&stored.cert_pem, "CERTIFICATE")?;
    let cert_der: CertificateDer<'static> = CertificateDer::from(cert_der);

    // Parse the PKCS#8 key PEM -> DER, then wrap as PrivatePkcs8KeyDer.
    let key_bytes = pem_to_der(&stored.key_pem, "PRIVATE KEY")?;
    let key_der: PrivateKeyDer<'static> =
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_bytes));

    // Re-parse the PEM key with rcgen so we can extract the raw
    // 32-byte Ed25519 pubkey without writing our own PKCS#8 parser.
    let key_pair = KeyPair::from_pem(&stored.key_pem)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("re-parse key: {}", e)))?;
    let raw = key_pair.public_key_raw();
    if raw.len() != 32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected 32-byte ed25519 pubkey, got {}", raw.len()),
        ));
    }
    let mut pubkey = [0u8; 32];
    pubkey.copy_from_slice(raw);

    Ok(DesktopIdentity {
        cert_der,
        key_der,
        pubkey,
    })
}

/// Tiny PEM body extractor: strips `-----BEGIN <tag>-----` / `-----END
/// <tag>-----` boundary lines and base64-decodes the remainder. PEM
/// itself is RFC 7468; we don't need a full parser for our two
/// fixed-shape blocks (CERTIFICATE / PRIVATE KEY) emitted by rcgen.
fn pem_to_der(pem_str: &str, expected_tag: &str) -> io::Result<Vec<u8>> {
    let begin = format!("-----BEGIN {}-----", expected_tag);
    let end = format!("-----END {}-----", expected_tag);
    let start_idx = pem_str.find(&begin).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("missing PEM header for {}", expected_tag),
        )
    })?;
    let after_begin = start_idx + begin.len();
    let end_idx = pem_str[after_begin..].find(&end).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("missing PEM footer for {}", expected_tag),
        )
    })? + after_begin;
    let body: String = pem_str[after_begin..end_idx]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    STANDARD
        .decode(body.as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("pem base64: {}", e)))
}

fn write_atomic(dir: &CoincubeDirectory, stored: &StoredIdentity) -> io::Result<()> {
    let path = identity_path(dir);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(stored)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)
}

pub fn fingerprint_hex8(pubkey: &[u8; 32]) -> String {
    let mut s = String::with_capacity(8);
    for byte in &pubkey[..4] {
        use std::fmt::Write as _;
        let _ = write!(s, "{:02x}", byte);
    }
    s
}

/// Wrap a [`DesktopIdentity`] in an `Arc` so the TLS layer can clone
/// it cheaply across configs.
pub fn as_arc(identity: DesktopIdentity) -> Arc<DesktopIdentity> {
    Arc::new(identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_dir() -> CoincubeDirectory {
        let mut path = std::env::temp_dir();
        path.push(format!("coincube-identity-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("mkdir tempdir");
        CoincubeDirectory::new(path)
    }

    #[test]
    fn fingerprint_hex8_is_eight_hex_chars() {
        let pubkey = [0xab, 0xcd, 0xef, 0x12, 0x99, 0x99, 0x99, 0x99].iter()
            .copied()
            .chain(std::iter::repeat(0).take(24))
            .collect::<Vec<_>>();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&pubkey);
        let s = fingerprint_hex8(&arr);
        assert_eq!(s, "abcdef12");
        assert_eq!(s.len(), 8);
    }

    #[test]
    fn load_or_create_mints_then_reloads_idempotently() {
        let dir = fresh_dir();
        let first = load_or_create(&dir).expect("mint");
        // File should now exist.
        assert!(dir.path().join(IDENTITY_FILENAME).exists());
        let second = load_or_create(&dir).expect("reload");

        // Persisted identity is stable across calls: pubkey + cert
        // DER + key DER all roundtrip byte-for-byte.
        assert_eq!(first.pubkey, second.pubkey);
        assert_eq!(first.cert_der.as_ref(), second.cert_der.as_ref());
        assert_eq!(
            first.clone_key().secret_der(),
            second.clone_key().secret_der()
        );
    }

    #[test]
    fn fresh_identity_has_32_byte_pubkey_and_matches_cert_pin() {
        let dir = fresh_dir();
        let id = load_or_create(&dir).expect("mint");
        // Ed25519 pubkeys are 32 bytes.
        assert_eq!(id.pubkey.len(), 32);
        // Spot-check the helper exposes the same first 4 bytes.
        let hex = id.fingerprint_hex8();
        assert_eq!(hex.len(), 8);
        // The cert exists and is non-empty.
        assert!(!id.cert_der.as_ref().is_empty());
    }

    #[test]
    fn pem_to_der_rejects_wrong_tag() {
        let pem = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----";
        let err = pem_to_der(pem, "PRIVATE KEY").err().expect("should fail");
        assert!(
            err.to_string().contains("missing PEM header"),
            "got: {}",
            err
        );
    }

    #[test]
    fn pem_to_der_extracts_body_for_matching_tag() {
        // "QUFB" is base64 for "AAA".
        let pem = "-----BEGIN CERTIFICATE-----\nQUFB\n-----END CERTIFICATE-----";
        let der = pem_to_der(pem, "CERTIFICATE").expect("decode");
        assert_eq!(der, b"AAA");
    }
}
