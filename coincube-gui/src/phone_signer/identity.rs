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

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
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
    /// Raw 32-byte Ed25519 public key from the cert's SPKI. Retained
    /// for debug/log purposes only; the wire contract identifies the
    /// desktop by [`Self::cert_fp`] (SHA-256 of the cert DER), not by
    /// this raw pubkey.
    pub pubkey: [u8; 32],
}

impl DesktopIdentity {
    pub fn clone_key(&self) -> PrivateKeyDer<'static> {
        self.key_der.clone_key()
    }

    /// SHA-256 of the cert DER, lowercase hex (64 chars). Embedded
    /// in the pairing QR's `certFp` field so the phone can pin the
    /// desktop's TLS cert; also matched against the cert presented
    /// during steady-state handshakes.
    pub fn cert_fp(&self) -> String {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(self.cert_der.as_ref());
        let mut s = String::with_capacity(64);
        for byte in digest.as_slice() {
            use std::fmt::Write as _;
            let _ = write!(s, "{:02x}", byte);
        }
        s
    }

    /// First 8 hex chars of [`Self::cert_fp`] — the same shape the
    /// phone publishes in its mDNS TXT `fp=` record.
    pub fn cert_fp8(&self) -> String {
        self.cert_fp().chars().take(8).collect()
    }

    /// Base64url-encoded (no padding) cert DER bytes. Embedded in
    /// the pairing QR's `cert` field so the phone can add the
    /// desktop's cert to its TLS trust store before the desktop
    /// dials. Companion to [`Self::cert_fp`] — both source the same
    /// `cert_der`, so they're guaranteed to agree.
    pub fn cert_der_b64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.cert_der.as_ref())
    }
}

impl std::fmt::Debug for DesktopIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesktopIdentity")
            .field("cert_fp8", &self.cert_fp8())
            .finish()
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
        .map_err(|e| io::Error::other(format!("ed25519 keygen: {}", e)))?;

    let mut params = CertificateParams::new(vec!["coincube-desktop.local".to_string()])
        .map_err(|e| io::Error::other(format!("cert params: {}", e)))?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "Coincube Desktop Local Signer");
    params.distinguished_name = dn;

    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| io::Error::other(format!("self-sign: {}", e)))?;

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
    let key_der: PrivateKeyDer<'static> = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_bytes));

    // Re-parse the PEM key with rcgen so we can extract the raw
    // 32-byte Ed25519 pubkey without writing our own PKCS#8 parser.
    let key_pair = KeyPair::from_pem(&stored.key_pem)
        .map_err(|e| io::Error::other(format!("re-parse key: {}", e)))?;
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
    let bytes = serde_json::to_vec_pretty(stored).map_err(io::Error::other)?;
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)
}

/// First 8 hex chars of a paired-phone's 32-byte cert pin (which is
/// `SHA-256(cert DER)` truncated to its first 4 bytes). The same
/// shape the phone publishes in its mDNS `fp=` TXT record. Used
/// across `hw.rs`, `mdns.rs`, and the settings panel as a short
/// human-readable identifier.
pub fn pin_hex8(pin: &[u8; 32]) -> String {
    let mut s = String::with_capacity(8);
    for byte in &pin[..4] {
        use std::fmt::Write as _;
        let _ = write!(s, "{:02x}", byte);
    }
    s
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
    fn pin_hex8_is_eight_hex_chars() {
        let mut arr = [0u8; 32];
        arr[..4].copy_from_slice(&[0xab, 0xcd, 0xef, 0x12]);
        let s = pin_hex8(&arr);
        assert_eq!(s, "abcdef12");
        assert_eq!(s.len(), 8);
    }

    #[test]
    fn cert_fp_is_64_lowercase_hex_chars() {
        let dir = fresh_dir();
        let id = load_or_create(&dir).expect("mint");
        let fp = id.cert_fp();
        assert_eq!(fp.len(), 64);
        assert!(fp
            .chars()
            .all(|c| c.is_ascii_hexdigit() && (!c.is_alphabetic() || c.is_lowercase())));
    }

    #[test]
    fn cert_fp8_matches_first_eight_of_cert_fp() {
        let dir = fresh_dir();
        let id = load_or_create(&dir).expect("mint");
        let fp = id.cert_fp();
        assert_eq!(id.cert_fp8(), &fp[..8]);
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
    fn fresh_identity_has_32_byte_pubkey_and_non_empty_cert() {
        let dir = fresh_dir();
        let id = load_or_create(&dir).expect("mint");
        assert_eq!(id.pubkey.len(), 32);
        assert!(!id.cert_der.as_ref().is_empty());
    }

    #[test]
    fn pem_to_der_rejects_wrong_tag() {
        let pem = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----";
        let err = pem_to_der(pem, "PRIVATE KEY").expect_err("should fail");
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
