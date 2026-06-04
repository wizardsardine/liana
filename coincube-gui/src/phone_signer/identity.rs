//! Long-lived desktop identity for the local-signer TLS layer.
//!
//! We mint **one** ECDSA P-256 keypair + self-signed cert per desktop
//! on first use and persist them under [`CoincubeDirectory`]. The cert
//! DER is what we share with phones: it goes into every pairing QR so
//! phones can pin it, and rustls presents the same cert to the
//! connecting phone on every steady-state handshake.
//!
//! ### Key-alg history
//!
//! v1 used Ed25519 (OID `1.3.101.112`). The cert is valid TLS 1.3 in
//! modern stacks but is rejected at handshake time by Dart's
//! BoringSSL `X509_STORE` (the trust store the Keychain phone app
//! installs the cert into) — handshake fails with
//! "application verification failure" and the phone never sees the
//! `PairingComplete` callback. See
//! `plans/PLAN-local-signer-pairing-hang-fix-desktop.md`.
//!
//! v2 swaps to ECDSA P-256 (`PKCS_ECDSA_P256_SHA256`), which is TLS
//! 1.3's default ECDHE curve and is accepted by every BoringSSL
//! snapshot in use. The on-disk filename is bumped to
//! [`IDENTITY_FILENAME`] (`…_v2.json`) so installs that minted an
//! Ed25519 identity under v1 get a fresh re-mint on first launch.

use std::fs;
use std::io;
use std::path::PathBuf;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};

use crate::dir::CoincubeDirectory;

/// Versioned filename. v1 was `phone-signer-identity.json` and held an
/// Ed25519 keypair Dart's BoringSSL refused to verify; v2 holds an
/// ECDSA P-256 keypair. [`load_or_create`] deletes the v1 file on
/// first launch after the upgrade so the user never accidentally
/// re-uses the broken cert.
const IDENTITY_FILENAME: &str = "phone-signer-identity_v2.json";

/// Legacy v1 filename. Removed by [`load_or_create`] on the first
/// post-upgrade launch.
const LEGACY_IDENTITY_FILENAME: &str = "phone-signer-identity.json";

fn identity_path(dir: &CoincubeDirectory) -> PathBuf {
    dir.path().join(IDENTITY_FILENAME)
}

fn legacy_identity_path(dir: &CoincubeDirectory) -> PathBuf {
    dir.path().join(LEGACY_IDENTITY_FILENAME)
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
///
/// On the first launch after the Ed25519→P-256 swap, deletes the
/// pre-existing v1 file so we never accidentally fall back to a cert
/// Dart's BoringSSL refuses to verify. The user has to re-pair any
/// phones that pinned the v1 cert; that's expected and called out in
/// the upgrade plan.
pub fn load_or_create(dir: &CoincubeDirectory) -> io::Result<DesktopIdentity> {
    match fs::read(identity_path(dir)) {
        Ok(bytes) => {
            let stored: StoredIdentity = serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            identity_from_stored(&stored)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // Drop the v1 Ed25519 file (best-effort) before minting a
            // fresh P-256 identity. We don't propagate the removal
            // error: if it fails the file just sits there harmlessly,
            // and we'd rather complete the mint than fail the launch.
            let legacy = legacy_identity_path(dir);
            if legacy.exists() {
                if let Err(e) = fs::remove_file(&legacy) {
                    tracing::warn!(
                        "failed to remove legacy v1 identity file {:?}: {}",
                        legacy,
                        e,
                    );
                } else {
                    tracing::info!(
                        "removed legacy v1 identity {:?}; minting fresh P-256 identity",
                        legacy,
                    );
                }
            }
            let (identity, stored) = mint_new()?;
            write_atomic(dir, &stored)?;
            Ok(identity)
        }
        Err(e) => Err(e),
    }
}

fn mint_new() -> io::Result<(DesktopIdentity, StoredIdentity)> {
    // ECDSA P-256: rcgen built-in, TLS 1.3's default ECDHE curve,
    // accepted by Dart's BoringSSL trust store. See module docs for
    // why Ed25519 (the v1 choice) was rejected.
    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .map_err(|e| io::Error::other(format!("p256 keygen: {}", e)))?;

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

    Ok(DesktopIdentity { cert_der, key_der })
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

    // The persisted JSON contains the desktop's PEM-encoded ECDSA
    // P-256 private key, so the on-disk file must be owner-only. On
    // Unix we explicitly create the tmp with 0o600 via OpenOptionsExt
    // instead of `fs::write`, which would otherwise create the file
    // with the process umask (typically 0o644 → world-readable). On
    // non-Unix targets (Windows) `OpenOptionsExt::mode` is
    // unavailable; we fall back to `fs::write` and rely on the
    // user's profile directory ACL.
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&tmp, &bytes)?;
    }

    fs::rename(&tmp, &path)?;

    // Belt-and-braces: re-apply 0o600 to the final path. The rename
    // already preserves the tmp's mode, but this also tightens any
    // pre-existing identity file written before this fix shipped
    // (e.g. an existing 0o644 file from a prior release on the same
    // installation).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
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

        // Persisted identity is stable across calls: cert DER + key
        // DER both roundtrip byte-for-byte.
        assert_eq!(first.cert_der.as_ref(), second.cert_der.as_ref());
        assert_eq!(
            first.clone_key().secret_der(),
            second.clone_key().secret_der()
        );
    }

    #[test]
    fn fresh_identity_has_non_empty_cert() {
        let dir = fresh_dir();
        let id = load_or_create(&dir).expect("mint");
        assert!(!id.cert_der.as_ref().is_empty());
    }

    /// Upgrade regression: a v1 Ed25519 identity file
    /// (`phone-signer-identity.json`) sitting in the datadir from a
    /// pre-P-256 release must be removed on the first `load_or_create`
    /// after the upgrade, and a fresh v2 file must take its place.
    /// Otherwise the desktop would keep reading the v1 file (no, it
    /// wouldn't — different filename), but worse, a curious user
    /// inspecting their datadir would see two identity files and not
    /// know which one is live.
    #[test]
    fn load_or_create_removes_legacy_v1_file_on_first_launch() {
        let dir = fresh_dir();
        // Plant a v1 file with non-JSON garbage — the contents don't
        // matter, only the presence at `LEGACY_IDENTITY_FILENAME`.
        let legacy = dir.path().join(LEGACY_IDENTITY_FILENAME);
        std::fs::write(&legacy, b"legacy ed25519 identity").expect("plant legacy");
        assert!(legacy.exists(), "legacy file should be present pre-mint");

        let _ = load_or_create(&dir).expect("mint v2");

        assert!(
            !legacy.exists(),
            "legacy v1 file must be removed on first v2 launch",
        );
        assert!(
            dir.path().join(IDENTITY_FILENAME).exists(),
            "v2 identity must be created",
        );
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

    /// The persisted identity contains the desktop's ECDSA P-256
    /// private key, so the on-disk file must be owner-only. Default
    /// umask would otherwise leave it world-readable (typically 0o644
    /// on Linux/macOS dev installs).
    #[cfg(unix)]
    #[test]
    fn write_atomic_creates_owner_only_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = fresh_dir();
        load_or_create(&dir).expect("mint");
        let path = dir.path().join(IDENTITY_FILENAME);
        let meta = std::fs::metadata(&path).expect("metadata");
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "identity file must be owner-only, got 0o{:o}",
            mode,
        );
    }
}
