//! Client-side, per-keyholder ECIES for inheritance heir-escrow.
//!
//! At Vault-monitoring opt-in the **owner's desktop** seals the recovery
//! material (the wallet descriptor always; the master seed too for the
//! Full-Cube tier) to *each designated heir keyholder's* secp256k1 xpub.
//! COINCUBE stores opaque ciphertext it cannot read and only gates its
//! release (decision record `2026-06-22-inheritance-ecies-heir-escrow.md`,
//! invariants I1/I5/I8). At recovery the heir's **Keychain** performs the
//! ECDH with the matching recovery private child key and returns the derived
//! symmetric key `K`; the desktop completes the AES-256-GCM open here.
//!
//! ## Canonical wire contract — `SPEC-ecies-v1.md` (§1, §3, §4, §7)
//!
//! This module MUST match `SPEC-ecies-v1.md` byte-for-byte; `coincube-api`
//! (blob store) and `keychain-app` (ECDH-decrypt) match the same spec. The
//! `kat_spec_v1_*` tests at the bottom replay the §7 known-answer vector.
//!
//! ```text
//! P  = CKDpub(recipient_account_xpub, 7000)        // dedicated enc child (I5)
//! (e, E) = fresh ephemeral secp256k1 keypair (per envelope)
//! ikm = compressed_SEC1( e · P )                   // 33B — the RAW point, NOT
//!                                                  // SHA256'd (no hashed ECDH)
//! K  = HKDF-SHA256( salt = 0x00*32, ikm,
//!                   info = "coincube-inheritance-ecies-v1\x00" ‖ E ‖ P, L=32 )
//! AAD = "coincube-inheritance-ecies-v1\x00" ‖ kind_byte(1) ‖
//!        cube_id(u64 BE) ‖ keyholder_key_id(u64 BE)
//! ct‖tag = AES-256-GCM-Seal( key = K, nonce = random 12B, aad = AAD, pt )
//! ```
//!
//! The heir reconstructs the same `ikm` as `compressed(d · E)` (ECDH symmetry,
//! `d · E == e · P`); Keychain returns `K = HKDF(ikm, …)` and the desktop opens
//! with it. `cube_id` + `keyholder_key_id` are bound into the AAD (not the
//! envelope body), so the server cannot silently re-target an envelope.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use coincube_core::miniscript::bitcoin::bip32::{ChildNumber, Xpub};
use coincube_core::miniscript::bitcoin::secp256k1::{
    ecdh::shared_secret_point, PublicKey, Secp256k1, SecretKey,
};
use rand::RngCore;
use zeroize::Zeroizing;

use super::error::EciesError;

/// Wire identifier for this scheme (SPEC-ecies-v1 §scheme id). A new value
/// here is a wire-format break requiring matching changes in `coincube-api`
/// and `keychain-app`.
pub const SCHEME: &str = "ecies-secp256k1-hkdf-sha256-aes256gcm-v1";

/// The label prefixing both the HKDF `info` and the AEAD `aad` (SPEC §1). The
/// trailing `\x00` is part of the label — it separates the label from the
/// binary that follows.
const ECIES_LABEL: &[u8] = b"coincube-inheritance-ecies-v1\x00";

/// HKDF salt: 32 explicit zero bytes (SPEC §1 — not empty/None).
const HKDF_SALT: [u8; 32] = [0u8; 32];

/// §4b key-wrap scheme id (the keychain→desktop wrap of `K`). The wire blob is a
/// fixed binary layout (see [`unwrap_shared_key`]); this id is the SPEC
/// identifier, not an on-wire field.
pub const WRAP_SCHEME: &str = "ecies-secp256k1-hkdf-sha256-aes256gcm-wrap-v1";

/// Domain label for the §4b key wrap's HKDF `info` and AEAD `aad` — distinct
/// from [`ECIES_LABEL`] so a wrapped-key blob and an envelope can never be
/// opened as one another. The trailing `\x00` separates label from binary.
const WRAP_LABEL: &[u8] = b"coincube-inheritance-wrap-v1\x00";

/// Version byte prefixing a `wrapped_shared_key` blob (§4b layout).
const WRAP_VERSION: u8 = 0x01;

/// Reserved non-hardened child index for inheritance encryption (invariant
/// I5). A sibling of the wallet's `0`/`1` branch nodes but a different index,
/// so it can never collide with a signing key or address. The owner derives
/// `P = CKDpub(account_xpub, 7000)`; the envelope's `derivation` carries the
/// full path from the seed root so Keychain derives the matching `d`.
pub const ENCRYPTION_CHILD_INDEX: u32 = 7000;

const KEY_LEN: usize = 32; // AES-256
const NONCE_LEN: usize = 12; // GCM standard
const TAG_LEN: usize = 16;
/// Compressed secp256k1 point.
const PUBKEY_LEN: usize = 33;
/// Fixed length of a `wrapped_shared_key` (§4b): `version(1) ‖ E_w(33) ‖
/// nonce(12) ‖ ct‖tag(48)` — the wrapped plaintext is always the 32-byte `K`.
const WRAPPED_LEN: usize = 1 + PUBKEY_LEN + NONCE_LEN + KEY_LEN + TAG_LEN;

/// Which recovery artifact an envelope carries. The byte form is bound into
/// the AAD; the wire string matches the API's `artifactKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// The wallet output descriptor (Vault-only and Full-Cube tiers).
    Descriptor,
    /// The master seed / mnemonic (Full-Cube tier only).
    Seed,
}

impl ArtifactKind {
    /// Lowercase wire token, matching `coincube-api`'s `artifactKind`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Descriptor => "descriptor",
            Self::Seed => "seed",
        }
    }

    /// One-byte AAD discriminant (SPEC §1: descriptor=0x01, seed=0x02).
    fn aad_byte(self) -> u8 {
        match self {
            Self::Descriptor => 0x01,
            Self::Seed => 0x02,
        }
    }

    /// Parses the API wire token. Unknown kinds are rejected (fail-closed)
    /// rather than mapped to a default.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "descriptor" => Some(Self::Descriptor),
            "seed" => Some(Self::Seed),
            _ => None,
        }
    }
}

/// One sealed envelope: everything `coincube-api` stores opaquely and returns
/// to the gated heir. Field names mirror the API schema (PR A).
///
/// `Debug` is manual: the `ciphertext` is encrypted, but we still avoid
/// dumping the raw bytes to any `{:?}` site.
#[derive(Clone)]
pub struct Envelope {
    pub artifact_kind: ArtifactKind,
    pub scheme: String,
    /// Compressed (33-byte) ephemeral public key.
    pub ephemeral_pubkey: Vec<u8>,
    /// `ciphertext || GCM tag`.
    pub ciphertext: Vec<u8>,
    /// 12-byte GCM nonce.
    pub nonce: Vec<u8>,
    /// Full BIP32 path from the seed root to the dedicated encryption child
    /// (e.g. `m/48h/1h/0h/2h/7000`) — Keychain derives `d` from it. Metadata
    /// only: it is NOT part of the AAD (the AAD binds cube + keyholder id).
    pub derivation: String,
}

impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Envelope")
            .field("artifact_kind", &self.artifact_kind)
            .field("scheme", &self.scheme)
            .field("derivation", &self.derivation)
            .field("ephemeral_pubkey", &hex(&self.ephemeral_pubkey))
            .field("ciphertext_len", &self.ciphertext.len())
            .field("nonce_len", &self.nonce.len())
            .finish()
    }
}

/// A fresh, in-memory secp256k1 transport keypair for the heir desktop's
/// per-recovery wrap target. In the async decrypt relay the heir's Keychain
/// ECIES-**wraps** `K` to this public key, so the server only ever relays an
/// opaque blob; the desktop unwraps it locally with the private scalar. The
/// secret is zeroized on drop and **never persisted**. [`unwrap_shared_key`]
/// consumes it to recover `K` from the Keychain's `wrapped_shared_key` (§4b).
pub struct TransportKeypair {
    secret: Zeroizing<[u8; 32]>,
    /// Compressed SEC1 (33-byte) public key — the `desktop_transport_pubkey`.
    public: [u8; PUBKEY_LEN],
}

impl TransportKeypair {
    /// The compressed-SEC1 public key to send as `desktop_transport_pubkey`.
    pub fn public_key(&self) -> [u8; PUBKEY_LEN] {
        self.public
    }

    /// The raw private scalar (32 bytes), for the wrap-open. Borrowed from the
    /// zeroizing buffer so it isn't copied out un-wiped.
    pub fn secret_bytes(&self) -> &[u8; 32] {
        &self.secret
    }
}

impl std::fmt::Debug for TransportKeypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportKeypair")
            .field("public", &hex(&self.public))
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Generates a fresh transport keypair (see [`TransportKeypair`]).
pub fn transport_keypair() -> TransportKeypair {
    let secp = Secp256k1::new();
    let sk = random_secret_key();
    let pk = PublicKey::from_secret_key(&secp, &sk);
    let mut secret = Zeroizing::new([0u8; 32]);
    secret.copy_from_slice(&sk.secret_bytes());
    TransportKeypair {
        secret,
        public: pk.serialize(),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// The Additional Authenticated Data bound into the GCM tag (SPEC §1):
/// `label ‖ kind_byte ‖ cube_id(u64 BE) ‖ keyholder_key_id(u64 BE)`. The heir
/// rebuilds it from the envelope kind + the recovery context, so a server that
/// re-targets an envelope to a different cube/keyholder breaks the tag.
fn aad_bytes(kind: ArtifactKind, cube_id: u64, keyholder_key_id: u64) -> Vec<u8> {
    let mut aad = Vec::with_capacity(ECIES_LABEL.len() + 1 + 8 + 8);
    aad.extend_from_slice(ECIES_LABEL);
    aad.push(kind.aad_byte());
    aad.extend_from_slice(&cube_id.to_be_bytes());
    aad.extend_from_slice(&keyholder_key_id.to_be_bytes());
    aad
}

/// The ECDH input keying material (SPEC §1): the **compressed SEC1 encoding of
/// `scalar · point`** — the raw curve point, deliberately NOT the SHA256-hashed
/// `ecdh::SharedSecret`. `shared_secret_point` returns the uncompressed `x‖y`;
/// we compress it (prefix from the parity of `y`).
fn ecdh_ikm(point: &PublicKey, scalar: &SecretKey) -> Zeroizing<[u8; PUBKEY_LEN]> {
    let xy = shared_secret_point(point, scalar); // [u8; 64] = x(32) ‖ y(32), BE
    let mut ikm = Zeroizing::new([0u8; PUBKEY_LEN]);
    ikm[0] = 0x02 | (xy[63] & 0x01); // compressed prefix from y's least bit
    ikm[1..].copy_from_slice(&xy[0..32]); // x
    ikm
}

/// HKDF-SHA256 per SPEC §1 / §4b: `salt = 0x00*32`, `info = label ‖ E ‖ P`,
/// 32-byte output. `label` is [`ECIES_LABEL`] for envelopes and [`WRAP_LABEL`]
/// for the §4b key wrap; `eph_pub` is `E`, `recipient_pub` is `P` (compressed).
fn hkdf_key(
    label: &[u8],
    ikm: &[u8],
    eph_pub: &[u8],
    recipient_pub: &[u8],
) -> Zeroizing<[u8; KEY_LEN]> {
    let salt = ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, &HKDF_SALT);
    let prk = salt.extract(ikm);
    // Bound to a local: `Okm` borrows the info slice until `fill`, so it must
    // outlive the `expand` call (the parts aren't all `'static`).
    let info: [&[u8]; 3] = [label, eph_pub, recipient_pub];
    let okm = prk
        .expand(&info, ring::hkdf::HKDF_SHA256)
        .expect("HKDF expand of one SHA-256 block never exceeds the length cap");
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    okm.fill(key.as_mut())
        .expect("OKM length matches the requested key length");
    key
}

/// The **Keychain half** of the open (also used in tests to stand in for
/// Keychain): given the recovery child private key `d` and the envelope's
/// ephemeral pubkey `E`, returns `K = HKDF(ikm = d·E, info = label‖E‖P)` where
/// `P = d·G`. Never exposes `d`, the seed, or the raw ECDH point. At real
/// recovery `K` comes from Keychain, not from a local key; this is here so the
/// desktop can seal owner-side and round-trip in tests without a Keychain. It
/// is test-only (`#[cfg(test)]`) so the sibling escrow/heir tests can exercise
/// the full path without a Keychain; production never derives `K` locally.
#[cfg(test)]
pub(crate) fn keychain_shared_key(
    child_sk: &SecretKey,
    eph_pub: &PublicKey,
) -> Zeroizing<[u8; KEY_LEN]> {
    let secp = Secp256k1::new();
    let recipient_pub = PublicKey::from_secret_key(&secp, child_sk);
    let ikm = ecdh_ikm(eph_pub, child_sk);
    hkdf_key(
        ECIES_LABEL,
        ikm.as_ref(),
        &eph_pub.serialize(),
        &recipient_pub.serialize(),
    )
}

/// **Owner side.** Seals `plaintext` to a keyholder's `recipient_xpub` (their
/// registered account xpub) under the dedicated encryption child (index
/// [`ENCRYPTION_CHILD_INDEX`], invariant I5). `full_derivation` is the full
/// path from the seed root stored in the envelope so Keychain can derive `d`.
/// `cube_id` + `keyholder_key_id` are bound into the AAD. Generates a fresh
/// ephemeral keypair and nonce per call; public-key encryption only (no
/// recipient secret), so it runs on the owner's desktop without Keychain.
pub fn seal_to_xpub(
    recipient_xpub: &Xpub,
    full_derivation: &str,
    kind: ArtifactKind,
    cube_id: u64,
    keyholder_key_id: u64,
    plaintext: &[u8],
) -> Result<Envelope, EciesError> {
    let secp = Secp256k1::new();
    // Derive the dedicated, non-hardened encryption child pubkey from the
    // account xpub. An xpub can only walk non-hardened steps; `derive_pub`
    // errors otherwise.
    let child = ChildNumber::from_normal_idx(ENCRYPTION_CHILD_INDEX)
        .map_err(|_| EciesError::MalformedEnvelope("encryption child index"))?;
    let recipient_child_pub = recipient_xpub
        .derive_pub(&secp, &[child])
        .map_err(EciesError::Derivation)?
        .public_key;

    let eph_sk = random_secret_key();
    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);

    seal_with_ephemeral(
        &recipient_child_pub,
        &eph_sk,
        &nonce,
        kind,
        cube_id,
        keyholder_key_id,
        full_derivation,
        plaintext,
    )
}

/// The deterministic seal core (fixed ephemeral key + nonce). [`seal_to_xpub`]
/// supplies random ones; the KAT test supplies the §7 fixtures.
#[allow(clippy::too_many_arguments)]
fn seal_with_ephemeral(
    recipient_child_pub: &PublicKey,
    eph_sk: &SecretKey,
    nonce: &[u8; NONCE_LEN],
    kind: ArtifactKind,
    cube_id: u64,
    keyholder_key_id: u64,
    full_derivation: &str,
    plaintext: &[u8],
) -> Result<Envelope, EciesError> {
    let secp = Secp256k1::new();
    let eph_pk = PublicKey::from_secret_key(&secp, eph_sk);
    let ikm = ecdh_ikm(recipient_child_pub, eph_sk);
    let key = hkdf_key(
        ECIES_LABEL,
        ikm.as_ref(),
        &eph_pk.serialize(),
        &recipient_child_pub.serialize(),
    );

    let aad = aad_bytes(kind, cube_id, keyholder_key_id);
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(EciesError::Cipher)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| EciesError::Seal)?;

    Ok(Envelope {
        artifact_kind: kind,
        scheme: SCHEME.to_string(),
        ephemeral_pubkey: eph_pk.serialize().to_vec(),
        ciphertext,
        nonce: nonce.to_vec(),
        derivation: full_derivation.to_string(),
    })
}

/// **Heir side.** Opens an envelope with the symmetric key `K` returned by
/// Keychain (which did the ECDH + HKDF on the recovery private key). The AAD is
/// rebuilt from the envelope kind + the recovery context's `cube_id` /
/// `keyholder_key_id` (SPEC §1). The plaintext lands in a `Zeroizing<Vec<u8>>`
/// wiped on drop. A wrong key, tampered ciphertext, or tampered AAD all surface
/// as [`EciesError::BadKeyOrCorrupt`] — indistinguishable by design.
pub fn open_with_shared_key(
    shared_key: &[u8; KEY_LEN],
    env: &Envelope,
    cube_id: u64,
    keyholder_key_id: u64,
) -> Result<Zeroizing<Vec<u8>>, EciesError> {
    if env.scheme != SCHEME {
        return Err(EciesError::UnsupportedScheme(env.scheme.clone()));
    }
    if env.nonce.len() != NONCE_LEN {
        return Err(EciesError::MalformedEnvelope("nonce length"));
    }
    if env.ephemeral_pubkey.len() != PUBKEY_LEN {
        return Err(EciesError::MalformedEnvelope("ephemeral pubkey length"));
    }
    if env.ciphertext.len() < TAG_LEN {
        return Err(EciesError::MalformedEnvelope("ciphertext shorter than tag"));
    }

    let aad = aad_bytes(env.artifact_kind, cube_id, keyholder_key_id);
    let cipher = Aes256Gcm::new_from_slice(shared_key).map_err(EciesError::Cipher)?;
    let pt = cipher
        .decrypt(
            Nonce::from_slice(&env.nonce),
            Payload {
                msg: &env.ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| EciesError::BadKeyOrCorrupt)?;
    Ok(Zeroizing::new(pt))
}

/// **Heir desktop, §4b.** Unwraps the `wrapped_shared_key` the Keychain returned
/// (an ECIES wrap of `K` to our per-recovery transport key) into the 32-byte
/// envelope key `K`. `request_id` is the `CreateDecryptRequest` id, bound into
/// the wrap AAD. `wrapped` is the fixed [`WRAPPED_LEN`]-byte layout
/// `version ‖ E_w(33) ‖ nonce(12) ‖ ct‖tag(48)`. A malformed blob, the wrong
/// transport key, or a tampered AAD all fail closed as
/// [`EciesError::BadKeyOrCorrupt`] / [`EciesError::MalformedEnvelope`].
pub fn unwrap_shared_key(
    transport: &TransportKeypair,
    request_id: &str,
    wrapped: &[u8],
) -> Result<Zeroizing<[u8; KEY_LEN]>, EciesError> {
    if wrapped.len() != WRAPPED_LEN {
        return Err(EciesError::MalformedEnvelope("wrapped key length"));
    }
    if wrapped[0] != WRAP_VERSION {
        return Err(EciesError::MalformedEnvelope("wrapped key version"));
    }
    let eph_pub_bytes = &wrapped[1..1 + PUBKEY_LEN];
    let nonce = &wrapped[1 + PUBKEY_LEN..1 + PUBKEY_LEN + NONCE_LEN];
    let ct = &wrapped[1 + PUBKEY_LEN + NONCE_LEN..];

    let eph_pub = PublicKey::from_slice(eph_pub_bytes)
        .map_err(|_| EciesError::MalformedEnvelope("wrapped key ephemeral pubkey"))?;
    let t = SecretKey::from_slice(transport.secret_bytes())
        .map_err(|_| EciesError::MalformedEnvelope("transport secret"))?;

    // ikm = t · E_w (== e_w · T); HKDF over the WRAP domain, info = E_w ‖ T.
    let ikm = ecdh_ikm(&eph_pub, &t);
    let wrap_key = hkdf_key(
        WRAP_LABEL,
        ikm.as_ref(),
        eph_pub_bytes,
        &transport.public_key(),
    );

    let mut aad = Vec::with_capacity(WRAP_LABEL.len() + request_id.len());
    aad.extend_from_slice(WRAP_LABEL);
    aad.extend_from_slice(request_id.as_bytes());

    let cipher = Aes256Gcm::new_from_slice(wrap_key.as_ref()).map_err(EciesError::Cipher)?;
    let pt = Zeroizing::new(
        cipher
            .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad: &aad })
            .map_err(|_| EciesError::BadKeyOrCorrupt)?,
    );
    if pt.len() != KEY_LEN {
        return Err(EciesError::MalformedEnvelope("unwrapped key length"));
    }
    let mut k = Zeroizing::new([0u8; KEY_LEN]);
    k.copy_from_slice(&pt);
    Ok(k)
}

/// Generates a uniformly random valid secp256k1 secret key. The reject loop
/// only ever retries on the negligible chance of a zero / out-of-range
/// scalar; in practice it succeeds on the first draw.
fn random_secret_key() -> SecretKey {
    let mut rng = rand::thread_rng();
    loop {
        let mut bytes = Zeroizing::new([0u8; 32]);
        rng.fill_bytes(bytes.as_mut());
        if let Ok(sk) = SecretKey::from_slice(bytes.as_ref()) {
            return sk;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::bip32::{DerivationPath, Xpriv};
    use coincube_core::miniscript::bitcoin::Network;
    use std::str::FromStr;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// A deterministic test keyholder: master xpriv → account xpub (what the
    /// owner sees as `models.Key.XPub`) plus the matching account xpriv (what
    /// Keychain holds to derive the recovery child).
    struct TestKeyholder {
        account_xpub: Xpub,
        account_xpriv: Xpriv,
    }

    fn keyholder_from_seed(seed: &[u8]) -> TestKeyholder {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, seed).unwrap();
        let account_path = DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();
        let account_xpriv = master.derive_priv(&secp, &account_path).unwrap();
        let account_xpub = Xpub::from_priv(&secp, &account_xpriv);
        TestKeyholder {
            account_xpub,
            account_xpriv,
        }
    }

    /// Stands in for Keychain: derive the recovery child (`/7000`) private key
    /// from the account xpriv, then ECDH+HKDF against the envelope's ephemeral
    /// pubkey → `K`.
    fn keychain_k(kh: &TestKeyholder, env: &Envelope) -> Zeroizing<[u8; KEY_LEN]> {
        let secp = Secp256k1::new();
        let child = ChildNumber::from_normal_idx(ENCRYPTION_CHILD_INDEX).unwrap();
        let child_sk = kh
            .account_xpriv
            .derive_priv(&secp, &[child])
            .unwrap()
            .private_key;
        let eph_pub = PublicKey::from_slice(&env.ephemeral_pubkey).unwrap();
        keychain_shared_key(&child_sk, &eph_pub)
    }

    // Test cube / keyholder ids bound into the AAD (arbitrary but fixed).
    const CUBE: u64 = 1;
    const KEY: u64 = 10;
    const FULL_DERIV: &str = "m/48'/0'/0'/2'/7000";

    #[test]
    fn seal_then_keychain_ecdh_then_open_roundtrips_descriptor() {
        let kh = keyholder_from_seed(b"keyholder-seed-descriptor-roundtrip-vector-0");
        let plaintext = b"wsh(or_d(multi(2,xpubA,xpubB),and_v(...)))#abcdefgh";

        let env = seal_to_xpub(
            &kh.account_xpub,
            FULL_DERIV,
            ArtifactKind::Descriptor,
            CUBE,
            KEY,
            plaintext,
        )
        .unwrap();

        assert_eq!(env.scheme, SCHEME);
        assert_eq!(env.artifact_kind, ArtifactKind::Descriptor);
        assert_eq!(env.derivation, FULL_DERIV);
        assert_eq!(env.ephemeral_pubkey.len(), PUBKEY_LEN);
        assert_eq!(env.nonce.len(), NONCE_LEN);

        let k = keychain_k(&kh, &env);
        let pt = open_with_shared_key(&k, &env, CUBE, KEY).unwrap();
        assert_eq!(pt.as_slice(), plaintext);
    }

    #[test]
    fn seal_then_open_roundtrips_seed() {
        let kh = keyholder_from_seed(b"keyholder-seed-seed-roundtrip-vector-000000");
        let plaintext = br#"{"version":1,"cube":{"uuid":"u"},"mnemonic":{"phrase":"abandon abandon ... about","language":"en"}}"#;

        let env = seal_to_xpub(
            &kh.account_xpub,
            FULL_DERIV,
            ArtifactKind::Seed,
            CUBE,
            KEY,
            plaintext,
        )
        .unwrap();
        let k = keychain_k(&kh, &env);
        let pt = open_with_shared_key(&k, &env, CUBE, KEY).unwrap();
        assert_eq!(pt.as_slice(), plaintext.as_slice());
    }

    #[test]
    fn wrong_keyholder_key_fails_closed() {
        let target = keyholder_from_seed(b"the-real-heir-keyholder-seed-vector-0000000");
        let attacker = keyholder_from_seed(b"a-different-keyholder-who-should-not-decrypt");

        let env = seal_to_xpub(
            &target.account_xpub,
            FULL_DERIV,
            ArtifactKind::Descriptor,
            CUBE,
            KEY,
            b"secret descriptor",
        )
        .unwrap();

        let wrong_k = keychain_k(&attacker, &env);
        let err = open_with_shared_key(&wrong_k, &env, CUBE, KEY).unwrap_err();
        assert!(matches!(err, EciesError::BadKeyOrCorrupt));
    }

    #[test]
    fn tampered_ciphertext_fails_tag() {
        let kh = keyholder_from_seed(b"tamper-ct-keyholder-seed-vector-00000000000");
        let mut env = seal_to_xpub(
            &kh.account_xpub,
            FULL_DERIV,
            ArtifactKind::Descriptor,
            CUBE,
            KEY,
            b"hello",
        )
        .unwrap();
        env.ciphertext[0] ^= 0x01;
        let k = keychain_k(&kh, &env);
        assert!(matches!(
            open_with_shared_key(&k, &env, CUBE, KEY).unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn tampered_cube_id_in_aad_fails_tag() {
        // cube_id rides in the AAD; opening against a different cube must break
        // the tag (a server re-pointing the envelope at another cube).
        let kh = keyholder_from_seed(b"tamper-cube-keyholder-seed-vector-000000000");
        let env = seal_to_xpub(
            &kh.account_xpub,
            FULL_DERIV,
            ArtifactKind::Descriptor,
            CUBE,
            KEY,
            b"hello",
        )
        .unwrap();
        let k = keychain_k(&kh, &env);
        assert!(matches!(
            open_with_shared_key(&k, &env, CUBE + 1, KEY).unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn tampered_keyholder_id_in_aad_fails_tag() {
        let kh = keyholder_from_seed(b"tamper-khid-keyholder-seed-vector-000000000");
        let env = seal_to_xpub(
            &kh.account_xpub,
            FULL_DERIV,
            ArtifactKind::Descriptor,
            CUBE,
            KEY,
            b"hello",
        )
        .unwrap();
        let k = keychain_k(&kh, &env);
        assert!(matches!(
            open_with_shared_key(&k, &env, CUBE, KEY + 1).unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn tampered_artifact_kind_in_aad_fails_tag() {
        let kh = keyholder_from_seed(b"tamper-kind-keyholder-seed-vector-000000000");
        let env = seal_to_xpub(
            &kh.account_xpub,
            FULL_DERIV,
            ArtifactKind::Seed,
            CUBE,
            KEY,
            b"hello",
        )
        .unwrap();
        let k = keychain_k(&kh, &env);

        let mut tampered = env.clone();
        tampered.artifact_kind = ArtifactKind::Descriptor;
        assert!(matches!(
            open_with_shared_key(&k, &tampered, CUBE, KEY).unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn unsupported_scheme_rejected() {
        let kh = keyholder_from_seed(b"scheme-keyholder-seed-vector-00000000000000");
        let mut env = seal_to_xpub(
            &kh.account_xpub,
            FULL_DERIV,
            ArtifactKind::Descriptor,
            CUBE,
            KEY,
            b"x",
        )
        .unwrap();
        env.scheme = "ecies-v2-some-future-thing".to_string();
        let k = keychain_k(&kh, &env);
        assert!(matches!(
            open_with_shared_key(&k, &env, CUBE, KEY).unwrap_err(),
            EciesError::UnsupportedScheme(_)
        ));
    }

    #[test]
    fn transport_keypair_is_fresh_and_compressed() {
        let a = transport_keypair();
        let b = transport_keypair();
        // Compressed SEC1 pubkey, valid prefix, and parses as a point.
        assert_eq!(a.public_key().len(), PUBKEY_LEN);
        assert!(matches!(a.public_key()[0], 0x02 | 0x03));
        assert!(PublicKey::from_slice(&a.public_key()).is_ok());
        // Fresh each call (private and public differ).
        assert_ne!(a.public_key(), b.public_key());
        assert_ne!(a.secret_bytes(), b.secret_bytes());
    }

    // ─── SPEC-ecies-v1 §7 known-answer vector ───────────────────────────────
    //
    // Every implementation (owner seal, Keychain ECDH, desktop open) MUST
    // reproduce these exact bytes. Generated by `ecies_kat_reference.py`.

    const KAT_ACCOUNT_XPUB: &str = "xpub6EuX7TBEwhFgifQY24vFeMRqeWHGyGCupztDxk7G2ECAqGQ22Fik8E811p8GrM2LfajQzLidXy4qECxhdcxChkjiKhnq2fiVMVjdfSoZQwg";
    const KAT_P: &str = "02e25a19d7ceb9635790c029af449589048ebc99e370d0e54f6176ff5aae4cb857";
    const KAT_D: &str = "db1b87125557540f11359013f0bd4040de82edabe99a134328e9dacf1795bca3";
    const KAT_E: &str = "034f355bdcb7cc0af728ef3cceb9615d90684bb5b2ca5f859ab0f0b704075871aa";
    const KAT_NONCE: &str = "0000000000000000deadbeef";
    const KAT_AES_KEY: &str = "4ae180fd201c8401b63c475bf22ee59ef1c624f623affc54eb5e51625ff99ca8";
    const KAT_CT: &str = "2e283e30ebac64ec0741b8f0281b3ae458196e5563bf95ac308414d1a457e261c15b99ed0606c4ccd7d44645c52ad3874cf6030efacb5891b7df4c98d426e7cda4ee173ecb5334bd8bae6dea9f2428a5d8920f5b4c8779db83baf40e8ad890ca7465a4964f6ed2d4fc";
    const KAT_CUBE_ID: u64 = 42;
    const KAT_KEYHOLDER_KEY_ID: u64 = 7;
    const KAT_PLAINTEXT: &[u8] = b"wsh(or_d(pk([00000000/48h/1h/0h/2h]xpub.../0/*),and_v(v:pkh(...),older(65535))))#cccccccc";
    const KAT_ENC_PATH: &str = "m/48h/1h/0h/2h/7000";

    #[test]
    fn kat_spec_v1_recipient_pubkey_from_account_xpub() {
        // P = CKDpub(account_xpub, 7000) — owner derives it xpub-only.
        let secp = Secp256k1::new();
        let xpub = Xpub::from_str(KAT_ACCOUNT_XPUB).unwrap();
        let child = ChildNumber::from_normal_idx(ENCRYPTION_CHILD_INDEX).unwrap();
        let p = xpub.derive_pub(&secp, &[child]).unwrap().public_key;
        assert_eq!(hex(&p.serialize()), KAT_P);
    }

    #[test]
    fn kat_spec_v1_seal_matches_vector() {
        let p = PublicKey::from_slice(&unhex(KAT_P)).unwrap();
        // Fixed ephemeral scalar e = 0x11 * 32 (test only, per §7).
        let eph_sk = SecretKey::from_slice(&[0x11u8; 32]).unwrap();
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&unhex(KAT_NONCE));
        let env = seal_with_ephemeral(
            &p,
            &eph_sk,
            &nonce,
            ArtifactKind::Descriptor,
            KAT_CUBE_ID,
            KAT_KEYHOLDER_KEY_ID,
            KAT_ENC_PATH,
            KAT_PLAINTEXT,
        )
        .unwrap();
        assert_eq!(hex(&env.ephemeral_pubkey), KAT_E);
        assert_eq!(hex(&env.ciphertext), KAT_CT);
    }

    #[test]
    fn kat_spec_v1_keychain_key_and_open() {
        // Keychain half: d + E → K must equal the vector aes_key.
        let d = SecretKey::from_slice(&unhex(KAT_D)).unwrap();
        let eph_pub = PublicKey::from_slice(&unhex(KAT_E)).unwrap();
        let k = keychain_shared_key(&d, &eph_pub);
        assert_eq!(hex(k.as_ref()), KAT_AES_KEY);

        // Desktop half: open the vector ciphertext with the vector key.
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(&unhex(KAT_AES_KEY));
        let env = Envelope {
            artifact_kind: ArtifactKind::Descriptor,
            scheme: SCHEME.to_string(),
            ephemeral_pubkey: unhex(KAT_E),
            ciphertext: unhex(KAT_CT),
            nonce: unhex(KAT_NONCE),
            derivation: KAT_ENC_PATH.to_string(),
        };
        let pt = open_with_shared_key(&key, &env, KAT_CUBE_ID, KAT_KEYHOLDER_KEY_ID).unwrap();
        assert_eq!(pt.as_slice(), KAT_PLAINTEXT);
    }

    // ─── SPEC-ecies-v1 §7.2 key-wrap known-answer vector (§4b) ───────────────
    //
    // Wraps the §7.1 `aes_key` (the two vectors chain). Locked cross-impl values
    // from keychain-app + the reference generator.

    const WRAP_P_DESKTOP: &str =
        "02466d7fcae563e5cb09a0d1870bb580344804617879a14949cf22285f1bae3f27";
    const WRAP_E_WRAP: &str = "023c72addb4fdf09af94f0c94d7fe92a386a7e70cf8a1d85916386bb2535c7b1b1";
    const WRAP_KEY: &str = "8e6d42b3d27bd0b024ffd39ab3337c3083c287badb4a61ff8af827b6af12a09b";
    const WRAP_NONCE: &str = "0000000000000000feedface";
    const WRAP_REQUEST_ID: &str = "rq-test-0001";

    /// Construct a [`TransportKeypair`] from a fixed secret (test-only; the
    /// child test module can touch the parent's private fields).
    fn transport_from(sk: &SecretKey) -> TransportKeypair {
        let secp = Secp256k1::new();
        let pk = PublicKey::from_secret_key(&secp, sk);
        let mut secret = Zeroizing::new([0u8; 32]);
        secret.copy_from_slice(&sk.secret_bytes());
        TransportKeypair {
            secret,
            public: pk.serialize(),
        }
    }

    /// Mirrors the Keychain §4b wrap: ECIES `K` to the transport pubkey under a
    /// fixed ephemeral + nonce, returning the 94-byte `wrapped_shared_key`.
    fn wrap_shared_key(
        transport_pub: &PublicKey,
        eph_sk: &SecretKey,
        nonce: &[u8; NONCE_LEN],
        request_id: &str,
        k: &[u8; KEY_LEN],
    ) -> Vec<u8> {
        let secp = Secp256k1::new();
        let eph_pub = PublicKey::from_secret_key(&secp, eph_sk);
        let ikm = ecdh_ikm(transport_pub, eph_sk); // e_w · T
        let wrap_key = hkdf_key(
            WRAP_LABEL,
            ikm.as_ref(),
            &eph_pub.serialize(),
            &transport_pub.serialize(),
        );
        let mut aad = WRAP_LABEL.to_vec();
        aad.extend_from_slice(request_id.as_bytes());
        let cipher = Aes256Gcm::new_from_slice(wrap_key.as_ref()).unwrap();
        let ct = cipher
            .encrypt(Nonce::from_slice(nonce), Payload { msg: k, aad: &aad })
            .unwrap();
        let mut out = Vec::with_capacity(WRAPPED_LEN);
        out.push(WRAP_VERSION);
        out.extend_from_slice(&eph_pub.serialize());
        out.extend_from_slice(nonce);
        out.extend_from_slice(&ct);
        out
    }

    #[test]
    fn kat_spec_v1_wrap_matches_locked_values_and_unwraps() {
        // Fixed inputs: t = 0x22*32, e_w = 0x33*32, K = §7.1 aes_key.
        let t = SecretKey::from_slice(&[0x22u8; 32]).unwrap();
        let ew = SecretKey::from_slice(&[0x33u8; 32]).unwrap();
        let transport = transport_from(&t);

        // (1) transport pubkey T == locked P_desktop.
        assert_eq!(hex(&transport.public_key()), WRAP_P_DESKTOP);

        // (2) wrap ephemeral pubkey E_w == locked E_wrap.
        let secp = Secp256k1::new();
        let ew_pub = PublicKey::from_secret_key(&secp, &ew);
        assert_eq!(hex(&ew_pub.serialize()), WRAP_E_WRAP);

        // (3) wrap_key from (t, E_wrap) over the WRAP label == locked wrap_key.
        //     This is the byte-exact ECDH + HKDF + domain-label check.
        let e_wrap_point = PublicKey::from_slice(&unhex(WRAP_E_WRAP)).unwrap();
        let ikm = ecdh_ikm(&e_wrap_point, &t); // t · E_wrap == e_w · T
        let wkey = hkdf_key(
            WRAP_LABEL,
            ikm.as_ref(),
            &unhex(WRAP_E_WRAP),
            &unhex(WRAP_P_DESKTOP),
        );
        assert_eq!(hex(wkey.as_ref()), WRAP_KEY);

        // (4) Full §4b round-trip: wrap §7.1's K to T, then unwrap → exactly K.
        //     (AES-GCM is deterministic, so this `wrapped` equals the locked
        //     wrapped_shared_key byte-for-byte given the matching wrap_key.)
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&unhex(WRAP_NONCE));
        let mut k = [0u8; KEY_LEN];
        k.copy_from_slice(&unhex(KAT_AES_KEY));
        let t_pub = PublicKey::from_slice(&unhex(WRAP_P_DESKTOP)).unwrap();
        let wrapped = wrap_shared_key(&t_pub, &ew, &nonce, WRAP_REQUEST_ID, &k);

        assert_eq!(wrapped.len(), WRAPPED_LEN);
        assert_eq!(wrapped[0], WRAP_VERSION);
        assert_eq!(hex(&wrapped[1..1 + PUBKEY_LEN]), WRAP_E_WRAP);

        let unwrapped = unwrap_shared_key(&transport, WRAP_REQUEST_ID, &wrapped).unwrap();
        assert_eq!(hex(unwrapped.as_ref()), KAT_AES_KEY);
    }

    #[test]
    fn unwrap_rejects_tampered_request_id() {
        let t = SecretKey::from_slice(&[0x22u8; 32]).unwrap();
        let ew = SecretKey::from_slice(&[0x33u8; 32]).unwrap();
        let transport = transport_from(&t);
        let t_pub = PublicKey::from_slice(&unhex(WRAP_P_DESKTOP)).unwrap();
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&unhex(WRAP_NONCE));
        let mut k = [0u8; KEY_LEN];
        k.copy_from_slice(&unhex(KAT_AES_KEY));
        let wrapped = wrap_shared_key(&t_pub, &ew, &nonce, WRAP_REQUEST_ID, &k);

        // A different request_id (server replaying another request's wrap)
        // breaks the AAD → fail closed.
        assert!(matches!(
            unwrap_shared_key(&transport, "rq-test-0002", &wrapped),
            Err(EciesError::BadKeyOrCorrupt)
        ));
    }

    #[test]
    fn unwrap_rejects_malformed_blob() {
        let t = SecretKey::from_slice(&[0x22u8; 32]).unwrap();
        let transport = transport_from(&t);
        // Wrong length.
        assert!(matches!(
            unwrap_shared_key(&transport, WRAP_REQUEST_ID, &[0x01u8; 10]),
            Err(EciesError::MalformedEnvelope("wrapped key length"))
        ));
        // Right length, wrong version byte.
        assert!(matches!(
            unwrap_shared_key(&transport, WRAP_REQUEST_ID, &[0x02u8; WRAPPED_LEN]),
            Err(EciesError::MalformedEnvelope("wrapped key version"))
        ));
    }
}
