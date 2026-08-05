//! The signing rail's end-to-end payload layer (`PLAN-connect-blinding.md`
//! PR D4, Track B).
//!
//! ## Why
//!
//! Connect's gRPC signing rail keeps the authoritative session state machine,
//! but it does not need to *read* what it coordinates. Before this, a PSBT
//! travelled the rail in plaintext (KEK-encrypted at rest, but parsed
//! server-side to build a `TxSummary`), which meant a DB snapshot exposed
//! amounts, addresses, and the whole transaction shape.
//!
//! Under `ECIES_V1` the desktop seals one envelope per resolved signer target,
//! sealed to that device's registered transport key, and the signer seals its
//! partial signature back to the desktop's. Connect relays opaque bytes. The
//! Keychain builds the approve screen from the PSBT it decrypts itself
//! (WYSIWYS, master I4) — no server or desktop claim about the transaction is
//! trusted for display.
//!
//! ## The key
//!
//! A per-device secp256k1 keypair, minted on first use and stored next to the
//! Connect cache in the network directory. Deliberately **not** derived from
//! the Cube seed and **not** the Rail 2 (LAN) P-256 TLS identity:
//!
//! - it is a transport key, scoped to one device rather than to an identity;
//! - re-installing mints a fresh one, which is exactly the wanted behaviour —
//!   sessions are short-lived, so old envelopes dying with the old install
//!   needs no rotation ceremony (master I3, key separation);
//! - it must be available without an unlocked Cube, because the rail comes up
//!   at login.
//!
//! ## Construction
//!
//! Identical primitives to every other envelope in the system
//! (`plans/SPEC-ecies-v1.md` §1: secp256k1, compressed SEC1, raw-point ECDH,
//! HKDF-SHA256 with a 32-zero-byte salt, AES-256-GCM) under a distinct domain
//! label, so a rail payload can never be opened as an inheritance envelope or a
//! cube-xpub envelope, or vice-versa:
//!
//! ```text
//! label = "coincube-connect-transport-v1\x00"
//! ikm   = compressed( e · P_device )
//! K     = HKDF-SHA256(ikm, salt = 0x00*32, info = label ‖ E ‖ P_device, L = 32)
//! AAD   = label ‖ utf8(request_id)
//! ```
//!
//! `request_id` is the **client-generated** session request id, which both
//! directions have: the desktop mints it for `CreateSigningSession`, and the
//! Keychain reads it back off `SigningSession.request_id`. Binding it means an
//! envelope from one session can't be replayed into another — the same role
//! `request_id` plays in the inheritance key-wrap (`SPEC-ecies-v1` §4b).

use std::io;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use coincube_core::miniscript::bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
use rand::RngCore;
use zeroize::Zeroizing;

use crate::dir::NetworkDirectory;
use crate::services::inheritance::ecies::{
    ecdh_ikm, hkdf_key, random_secret_key, NONCE_LEN, PUBKEY_LEN, TAG_LEN,
};
use crate::services::inheritance::EciesError;

/// Domain label for the signing rail, distinct from the inheritance and
/// cube-xpub labels. The trailing `\x00` separates the label from the binary
/// that follows.
const TRANSPORT_LABEL: &[u8] = b"coincube-connect-transport-v1\x00";

/// Sidecar holding this device's transport secret, next to `connect.json` in
/// the network directory. Contents are the 32-byte scalar as lowercase hex.
const TRANSPORT_KEY_FILENAME: &str = "connect_transport_key";

/// This device's transport keypair for the Connect signing rail.
///
/// The secret is zeroized on drop. It is persisted (the rail must come up at
/// login, before any Cube is unlocked), with owner-only permissions where the
/// platform supports them — the same posture as the access tokens already in
/// `connect.json` alongside it.
pub struct DeviceTransportKey {
    secret: Zeroizing<[u8; 32]>,
    public: [u8; PUBKEY_LEN],
}

impl std::fmt::Debug for DeviceTransportKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceTransportKey")
            .field("public", &hex::encode(self.public))
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl DeviceTransportKey {
    fn from_secret(sk: &SecretKey) -> Self {
        let secp = Secp256k1::new();
        let pk = PublicKey::from_secret_key(&secp, sk);
        let mut secret = Zeroizing::new([0u8; 32]);
        secret.copy_from_slice(&sk.secret_bytes());
        Self {
            secret,
            public: pk.serialize(),
        }
    }

    /// Loads this device's transport key, minting and persisting one on first
    /// use.
    ///
    /// A corrupt or truncated sidecar is replaced rather than fatal: the key is
    /// transport-only, so losing it costs nothing but a re-registration, and
    /// refusing to start the rail over an unreadable cache file would be a
    /// worse failure than minting a fresh one.
    pub fn load_or_create(network_dir: &NetworkDirectory) -> io::Result<Self> {
        let path = key_path(network_dir);
        if let Some(existing) = read_key(&path) {
            return Ok(existing);
        }
        let key = Self::from_secret(&random_secret_key());
        write_key(&path, &key.secret)?;
        tracing::info!(
            "Minted a Connect transport key for this device (pubkey {})",
            key.public_key_hex()
        );
        Ok(key)
    }

    /// The compressed-SEC1 public key registered with Connect and sealed to by
    /// the signers. Public material — safe to log.
    pub fn public_key(&self) -> [u8; PUBKEY_LEN] {
        self.public
    }

    /// The registered public key as lowercase hex.
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.public)
    }

    /// Opens a payload sealed to this device (a signature envelope coming back
    /// from a signer).
    ///
    /// `request_id` is the session's client-generated request id, rebuilt into
    /// the AAD — an envelope lifted from another session fails here rather than
    /// being merged into this one. Wrong key, tampered ciphertext, and a
    /// mismatched `request_id` are one indistinguishable error by design.
    pub fn open(
        &self,
        eph_pub: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
        request_id: &str,
    ) -> Result<Zeroizing<Vec<u8>>, EciesError> {
        if eph_pub.len() != PUBKEY_LEN {
            return Err(EciesError::MalformedEnvelope("ephemeral pubkey length"));
        }
        if nonce.len() != NONCE_LEN {
            return Err(EciesError::MalformedEnvelope("nonce length"));
        }
        if ciphertext.len() < TAG_LEN {
            return Err(EciesError::MalformedEnvelope("ciphertext shorter than tag"));
        }
        let e = PublicKey::from_slice(eph_pub)
            .map_err(|_| EciesError::MalformedEnvelope("ephemeral pubkey"))?;
        let d = SecretKey::from_slice(self.secret.as_ref())
            .map_err(|_| EciesError::MalformedEnvelope("transport secret"))?;

        let ikm = ecdh_ikm(&e, &d);
        let key = hkdf_key(TRANSPORT_LABEL, ikm.as_ref(), eph_pub, &self.public);
        let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(EciesError::Cipher)?;
        let pt = cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad_bytes(request_id),
                },
            )
            .map_err(|_| EciesError::BadKeyOrCorrupt)?;
        Ok(Zeroizing::new(pt))
    }
}

/// One sealed rail payload, in the shape the proto's `PayloadEnvelope` carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedPayload {
    /// Compressed SEC1 ephemeral public key (33 bytes).
    pub ephemeral_pubkey: Vec<u8>,
    /// AES-GCM nonce (12 bytes).
    pub nonce: Vec<u8>,
    /// Ciphertext with the 16-byte GCM tag appended.
    pub ciphertext: Vec<u8>,
}

/// Seals `plaintext` to a device's registered transport public key.
///
/// `recipient_pub` is the 33-byte compressed key from
/// `SignerTarget.transport_pubkey`; a target that registered none can't be
/// sealed to at all, which is what makes the caller fail closed instead of
/// downgrading the session (master I5).
///
/// A fresh ephemeral keypair and nonce per call, so every envelope gets a
/// unique AEAD key and nonce reuse is structurally impossible.
pub fn seal_to_device(
    recipient_pub: &[u8],
    request_id: &str,
    plaintext: &[u8],
) -> Result<SealedPayload, EciesError> {
    if recipient_pub.len() != PUBKEY_LEN {
        return Err(EciesError::MalformedEnvelope("transport pubkey length"));
    }
    let p = PublicKey::from_slice(recipient_pub)
        .map_err(|_| EciesError::MalformedEnvelope("transport pubkey"))?;
    let eph_sk = random_secret_key();
    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    seal_with_ephemeral(&p, &eph_sk, &nonce, request_id, plaintext)
}

/// The deterministic seal core — [`seal_to_device`] supplies random inputs, the
/// known-answer test supplies fixed ones.
fn seal_with_ephemeral(
    recipient_pub: &PublicKey,
    eph_sk: &SecretKey,
    nonce: &[u8; NONCE_LEN],
    request_id: &str,
    plaintext: &[u8],
) -> Result<SealedPayload, EciesError> {
    let secp = Secp256k1::new();
    let eph_pk = PublicKey::from_secret_key(&secp, eph_sk);
    let ikm = ecdh_ikm(recipient_pub, eph_sk);
    let key = hkdf_key(
        TRANSPORT_LABEL,
        ikm.as_ref(),
        &eph_pk.serialize(),
        &recipient_pub.serialize(),
    );
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(EciesError::Cipher)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad: &aad_bytes(request_id),
            },
        )
        .map_err(|_| EciesError::Seal)?;
    Ok(SealedPayload {
        ephemeral_pubkey: eph_pk.serialize().to_vec(),
        nonce: nonce.to_vec(),
        ciphertext,
    })
}

/// `label ‖ utf8(request_id)` — no length prefix, because the fixed-length
/// label precedes it (same encoding as `SPEC-ecies-v1` §4b).
fn aad_bytes(request_id: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(TRANSPORT_LABEL.len() + request_id.len());
    aad.extend_from_slice(TRANSPORT_LABEL);
    aad.extend_from_slice(request_id.as_bytes());
    aad
}

fn key_path(network_dir: &NetworkDirectory) -> PathBuf {
    network_dir.path().join(TRANSPORT_KEY_FILENAME)
}

/// Reads the sidecar, returning `None` for absent / unreadable / malformed
/// contents so the caller mints a replacement.
fn read_key(path: &Path) -> Option<DeviceTransportKey> {
    let raw = std::fs::read_to_string(path).ok()?;
    let bytes = hex::decode(raw.trim()).ok()?;
    let sk = SecretKey::from_slice(&bytes).ok()?;
    Some(DeviceTransportKey::from_secret(&sk))
}

/// Writes the scalar with owner-only permissions where the platform supports
/// them, matching how the master-seed mnemonics are stored.
fn write_key(path: &Path, secret: &[u8; 32]) -> io::Result<()> {
    let contents = Zeroizing::new(hex::encode(secret));
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(contents.as_bytes())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST_ID: &str = "req-0001";
    const PSBT: &[u8] = b"cHNidP8BAHECAAAAAfake-psbt-bytes-for-the-test";

    fn tmp_network_dir(tag: &str) -> NetworkDirectory {
        let base =
            std::env::temp_dir().join(format!("coincube-transport-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        NetworkDirectory::new(base)
    }

    #[test]
    fn seal_then_open_roundtrips() {
        let device = DeviceTransportKey::from_secret(&random_secret_key());
        let sealed = seal_to_device(&device.public_key(), REQUEST_ID, PSBT).unwrap();

        assert_eq!(sealed.ephemeral_pubkey.len(), PUBKEY_LEN);
        assert_eq!(sealed.nonce.len(), NONCE_LEN);
        assert_eq!(sealed.ciphertext.len(), PSBT.len() + TAG_LEN);

        let opened = device
            .open(
                &sealed.ephemeral_pubkey,
                &sealed.nonce,
                &sealed.ciphertext,
                REQUEST_ID,
            )
            .unwrap();
        assert_eq!(opened.as_slice(), PSBT);
    }

    #[test]
    fn another_device_cannot_open_it() {
        let target = DeviceTransportKey::from_secret(&random_secret_key());
        let other = DeviceTransportKey::from_secret(&random_secret_key());
        let sealed = seal_to_device(&target.public_key(), REQUEST_ID, PSBT).unwrap();

        assert!(matches!(
            other
                .open(
                    &sealed.ephemeral_pubkey,
                    &sealed.nonce,
                    &sealed.ciphertext,
                    REQUEST_ID
                )
                .unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn an_envelope_cannot_be_replayed_into_another_session() {
        // The reason request_id is in the AAD: a server relaying sessions must
        // not be able to move a signature from one session onto another.
        let device = DeviceTransportKey::from_secret(&random_secret_key());
        let sealed = seal_to_device(&device.public_key(), REQUEST_ID, PSBT).unwrap();
        assert!(matches!(
            device
                .open(
                    &sealed.ephemeral_pubkey,
                    &sealed.nonce,
                    &sealed.ciphertext,
                    "req-0002"
                )
                .unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn tampered_ciphertext_fails_closed() {
        let device = DeviceTransportKey::from_secret(&random_secret_key());
        let mut sealed = seal_to_device(&device.public_key(), REQUEST_ID, PSBT).unwrap();
        sealed.ciphertext[0] ^= 0x01;
        assert!(matches!(
            device
                .open(
                    &sealed.ephemeral_pubkey,
                    &sealed.nonce,
                    &sealed.ciphertext,
                    REQUEST_ID
                )
                .unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn malformed_inputs_are_rejected_by_name() {
        let device = DeviceTransportKey::from_secret(&random_secret_key());
        assert!(matches!(
            seal_to_device(&[0x02; 10], REQUEST_ID, PSBT).unwrap_err(),
            EciesError::MalformedEnvelope("transport pubkey length")
        ));
        // Right length, not a point on the curve: x = 0xFF…FF is above the
        // field prime, so no valid y exists.
        let mut off_curve = [0xFFu8; PUBKEY_LEN];
        off_curve[0] = 0x02;
        assert!(matches!(
            seal_to_device(&off_curve, REQUEST_ID, PSBT).unwrap_err(),
            EciesError::MalformedEnvelope("transport pubkey")
        ));
        assert!(matches!(
            device.open(&[0u8; 10], &[0u8; NONCE_LEN], &[0u8; 32], REQUEST_ID),
            Err(EciesError::MalformedEnvelope("ephemeral pubkey length"))
        ));
        assert!(matches!(
            device.open(&[0u8; PUBKEY_LEN], &[0u8; 4], &[0u8; 32], REQUEST_ID),
            Err(EciesError::MalformedEnvelope("nonce length"))
        ));
        assert!(matches!(
            device.open(&[0u8; PUBKEY_LEN], &[0u8; NONCE_LEN], &[0u8; 4], REQUEST_ID),
            Err(EciesError::MalformedEnvelope("ciphertext shorter than tag"))
        ));
    }

    #[test]
    fn a_rail_payload_cannot_be_opened_under_another_domain_label() {
        // Domain separation: the same recipient key, sealed under the
        // cube-xpub label, must not open here.
        let device = DeviceTransportKey::from_secret(&random_secret_key());
        let p = PublicKey::from_slice(&device.public_key()).unwrap();
        let eph = random_secret_key();
        let secp = Secp256k1::new();
        let eph_pub = PublicKey::from_secret_key(&secp, &eph);
        let ikm = ecdh_ikm(&p, &eph);
        let wrong = hkdf_key(
            b"coincube-connect-xpub-v1\x00",
            ikm.as_ref(),
            &eph_pub.serialize(),
            &p.serialize(),
        );
        let cipher = Aes256Gcm::new_from_slice(wrong.as_ref()).unwrap();
        let ct = cipher
            .encrypt(
                Nonce::from_slice(&[0x09u8; NONCE_LEN]),
                Payload {
                    msg: PSBT,
                    aad: &aad_bytes(REQUEST_ID),
                },
            )
            .unwrap();
        assert!(matches!(
            device.open(&eph_pub.serialize(), &[0x09u8; NONCE_LEN], &ct, REQUEST_ID),
            Err(EciesError::BadKeyOrCorrupt)
        ));
    }

    #[test]
    fn the_key_persists_across_loads() {
        let dir = tmp_network_dir("persist");
        let first = DeviceTransportKey::load_or_create(&dir).unwrap();
        let second = DeviceTransportKey::load_or_create(&dir).unwrap();
        assert_eq!(first.public_key(), second.public_key());

        // And a payload sealed against the first opens with the reloaded one —
        // the whole point of persisting it across launches.
        let sealed = seal_to_device(&first.public_key(), REQUEST_ID, PSBT).unwrap();
        assert_eq!(
            second
                .open(
                    &sealed.ephemeral_pubkey,
                    &sealed.nonce,
                    &sealed.ciphertext,
                    REQUEST_ID
                )
                .unwrap()
                .as_slice(),
            PSBT
        );
    }

    #[test]
    fn a_corrupt_sidecar_is_replaced_rather_than_fatal() {
        let dir = tmp_network_dir("corrupt");
        let first = DeviceTransportKey::load_or_create(&dir).unwrap();
        std::fs::write(key_path(&dir), "not hex at all").unwrap();

        let replaced = DeviceTransportKey::load_or_create(&dir).unwrap();
        assert_ne!(first.public_key(), replaced.public_key());
        // …and the replacement itself persists.
        assert_eq!(
            replaced.public_key(),
            DeviceTransportKey::load_or_create(&dir)
                .unwrap()
                .public_key()
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_sidecar_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp_network_dir("perms");
        let _ = DeviceTransportKey::load_or_create(&dir).unwrap();
        let mode = std::fs::metadata(key_path(&dir))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "transport key must not be group/world readable"
        );
    }

    #[test]
    fn debug_never_prints_the_secret() {
        let device = DeviceTransportKey::from_secret(&random_secret_key());
        let rendered = format!("{:?}", device);
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains(&hex::encode(*device.secret)));
    }
}
