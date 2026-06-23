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
//! ## Canonical wire contract (`ecies-secp256k1-hkdf-aes256gcm-v1`)
//!
//! The desktop **defines** this contract; `coincube-api` (blob store) and
//! `keychain-app` (ECDH-decrypt) must match it byte-for-byte. The
//! known-answer tests at the bottom of this file are the cross-repo vectors.
//!
//! ```text
//! recipient_child_pub = derive_pub(recipient_xpub, derivation)   // I5: a
//!                                                                // dedicated
//!                                                                // non-hardened
//!                                                                // child, never
//!                                                                // the signing key
//! (eph_sk, eph_pk)    = fresh ephemeral secp256k1 keypair (per envelope)
//! S  = SHA256( compressed( eph_sk · recipient_child_pub ) )      // secp256k1
//!                                                                // ecdh::SharedSecret
//! K  = HKDF-SHA256( salt = "", ikm = S,
//!                   info = "coincube-inheritance-ecies-v1", L = 32 )
//! AAD = "ecies-secp256k1-hkdf-aes256gcm-v1" || 0x00 ||
//!        kind_byte || derivation_utf8 || eph_pk_compressed(33)
//! ct||tag = AES-256-GCM-Seal( key = K, nonce = random 12B, aad = AAD, pt )
//! ```
//!
//! The heir reconstructs the *same* `S` as
//! `SHA256(compressed(recovery_child_sk · eph_pk))` (ECDH symmetry), so
//! Keychain returns `K = HKDF-SHA256(S, …)` and the desktop opens with it.
//! Every public envelope field is bound into the GCM AAD, so a tampered
//! `derivation`, `artifact_kind`, or `ephemeral_pubkey` fails the tag.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use coincube_core::miniscript::bitcoin::bip32::{DerivationPath, Xpub};
use coincube_core::miniscript::bitcoin::secp256k1::{
    ecdh::SharedSecret, PublicKey, Secp256k1, SecretKey,
};
use rand::RngCore;
use zeroize::Zeroizing;

use super::error::EciesError;

/// Wire identifier for this scheme. A new value here is a wire-format break
/// requiring matching changes in `coincube-api` and `keychain-app`.
pub const SCHEME: &str = "ecies-secp256k1-hkdf-aes256gcm-v1";

/// HKDF `info` string binding the derived key to this purpose. Keychain uses
/// the identical bytes when it derives `K`.
pub const HKDF_INFO: &[u8] = b"coincube-inheritance-ecies-v1";

/// The dedicated, non-hardened encryption child derived from the keyholder's
/// registered xpub (invariant I5 — never the signing key, never chain 0/1
/// which the wallet descriptor spends from). Stored in each envelope's
/// `derivation` field so Keychain replays the exact path against its xpriv.
pub const ENCRYPTION_CHILD_DERIVATION: &str = "9/0";

const KEY_LEN: usize = 32; // AES-256
const NONCE_LEN: usize = 12; // GCM standard
const TAG_LEN: usize = 16;
/// Compressed secp256k1 point.
const PUBKEY_LEN: usize = 33;

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

    /// One-byte AAD discriminant. Distinct from `as_wire` so a future kind
    /// can't silently collide on a shared prefix.
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
/// dumping the raw bytes to any `{:?}` site and never let the plaintext-
/// adjacent fields read as innocuous. Public fields (scheme, derivation,
/// kind, ephemeral pubkey) are non-secret and shown.
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
    /// The non-hardened child path used (relative to the keyholder xpub).
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

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// The Additional Authenticated Data bound into the GCM tag. Reconstructed
/// identically by the heir from public envelope fields, so any mutation of
/// `scheme` / `kind` / `derivation` / `ephemeral_pubkey` breaks the tag.
fn aad_bytes(kind: ArtifactKind, derivation: &str, ephemeral_pubkey: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(SCHEME.len() + 2 + derivation.len() + ephemeral_pubkey.len());
    aad.extend_from_slice(SCHEME.as_bytes());
    aad.push(0x00); // domain separator between scheme and the rest
    aad.push(kind.aad_byte());
    aad.extend_from_slice(derivation.as_bytes());
    aad.extend_from_slice(ephemeral_pubkey);
    aad
}

/// HKDF-SHA256 over `ring` (already in-tree; avoids the hmac/sha2 digest
/// version split). Empty salt → RFC-5869 all-zero salt, matching a Go
/// `hkdf.New(sha256, ikm, nil, info)` on the Keychain side.
fn hkdf_sha256_key(ikm: &[u8]) -> Zeroizing<[u8; KEY_LEN]> {
    let salt = ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, &[]);
    let prk = salt.extract(ikm);
    // `HKDF_SHA256` as its own `KeyType` yields a 32-byte (SHA-256-len) OKM.
    let okm = prk
        .expand(&[HKDF_INFO], ring::hkdf::HKDF_SHA256)
        .expect("HKDF expand of one SHA-256 block never exceeds the length cap");
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    okm.fill(key.as_mut())
        .expect("OKM length matches the requested key length");
    key
}

/// Derives the per-envelope symmetric key from a completed ECDH. This is the
/// exact computation Keychain performs (ECDH on the recovery child private
/// key, then HKDF) and returns to the desktop as `K`; kept here so the
/// desktop can (a) seal owner-side and (b) round-trip in tests without a
/// Keychain. **Never** call this with the owner's seed material at heir
/// recovery time — `K` then comes from Keychain, not from a local key. It is
/// `pub(crate)` only so the sibling escrow tests can exercise the full path.
pub(crate) fn shared_key_from_ecdh(
    point: &PublicKey,
    scalar: &SecretKey,
) -> Zeroizing<[u8; KEY_LEN]> {
    // `SharedSecret::new` = SHA256(compressed(scalar · point)) — the `S` of
    // the wire contract. ECDH symmetry makes owner and heir agree on it.
    let s = SharedSecret::new(point, scalar);
    hkdf_sha256_key(&s.secret_bytes())
}

/// **Owner side.** Seals `plaintext` to a keyholder's `recipient_xpub` under
/// the dedicated encryption child at `derivation`. Generates a fresh
/// ephemeral keypair and nonce per call. Public-key encryption only — no
/// secret of the recipient's is needed, so this runs on the owner's desktop
/// with no Keychain involvement.
pub fn seal_to_xpub(
    recipient_xpub: &Xpub,
    derivation: &DerivationPath,
    kind: ArtifactKind,
    plaintext: &[u8],
) -> Result<Envelope, EciesError> {
    let secp = Secp256k1::new();
    // Derive the dedicated, non-hardened encryption child pubkey (I5). An
    // xpub can only walk non-hardened steps; `derive_pub` errors otherwise.
    let recipient_child_pub = recipient_xpub
        .derive_pub(&secp, derivation)
        .map_err(EciesError::Derivation)?
        .public_key;

    let eph_sk = random_secret_key();
    let eph_pk = PublicKey::from_secret_key(&secp, &eph_sk);
    let ephemeral_pubkey = eph_pk.serialize().to_vec(); // 33-byte compressed

    let key = shared_key_from_ecdh(&recipient_child_pub, &eph_sk);

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);

    let derivation_str = derivation.to_string();
    let aad = aad_bytes(kind, &derivation_str, &ephemeral_pubkey);

    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(EciesError::Cipher)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| EciesError::Seal)?;

    Ok(Envelope {
        artifact_kind: kind,
        scheme: SCHEME.to_string(),
        ephemeral_pubkey,
        ciphertext,
        nonce: nonce_bytes.to_vec(),
        derivation: derivation_str,
    })
}

/// **Heir side.** Opens an envelope with the symmetric key `K` returned by
/// Keychain (which did the ECDH + HKDF on the recovery private key). The
/// plaintext lands in a `Zeroizing<Vec<u8>>` wiped on drop. A wrong key,
/// tampered ciphertext, or tampered AAD all surface as
/// [`EciesError::BadKeyOrCorrupt`] — indistinguishable by design.
pub fn open_with_shared_key(
    shared_key: &[u8; KEY_LEN],
    env: &Envelope,
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

    let aad = aad_bytes(env.artifact_kind, &env.derivation, &env.ephemeral_pubkey);
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
    use coincube_core::miniscript::bitcoin::bip32::Xpriv;
    use coincube_core::miniscript::bitcoin::Network;
    use std::str::FromStr;

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
        // Pretend the registered key is an account-level xpub; the exact
        // origin path is irrelevant to the ECIES contract (the `derivation`
        // is relative to whatever xpub the owner holds).
        let account_path = DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();
        let account_xpriv = master.derive_priv(&secp, &account_path).unwrap();
        let account_xpub = Xpub::from_priv(&secp, &account_xpriv);
        TestKeyholder {
            account_xpub,
            account_xpriv,
        }
    }

    /// Mirrors what Keychain does end-to-end: derive the recovery child
    /// private key at `derivation`, ECDH against the envelope's ephemeral
    /// pubkey, HKDF → `K`. Test-only: at recovery `K` comes from Keychain.
    fn keychain_derive_shared_key(kh: &TestKeyholder, env: &Envelope) -> Zeroizing<[u8; KEY_LEN]> {
        let secp = Secp256k1::new();
        let path = DerivationPath::from_str(&env.derivation).unwrap();
        let child_sk = kh
            .account_xpriv
            .derive_priv(&secp, &path)
            .unwrap()
            .private_key;
        let eph_pk = PublicKey::from_slice(&env.ephemeral_pubkey).unwrap();
        shared_key_from_ecdh(&eph_pk, &child_sk)
    }

    fn derivation() -> DerivationPath {
        DerivationPath::from_str(ENCRYPTION_CHILD_DERIVATION).unwrap()
    }

    #[test]
    fn encryption_child_derivation_is_non_hardened_and_parses() {
        // Must parse and be reachable from an xpub: every step non-hardened
        // (no `'`/`h` marker). The roundtrip tests further prove `derive_pub`
        // accepts it (which only succeeds for non-hardened paths).
        let rendered = derivation().to_string();
        assert!(
            !rendered.contains('\'') && !rendered.contains('h'),
            "encryption child path must be non-hardened, got {}",
            rendered
        );
    }

    #[test]
    fn seal_then_keychain_ecdh_then_open_roundtrips_descriptor() {
        let kh = keyholder_from_seed(b"keyholder-seed-descriptor-roundtrip-vector-0");
        let plaintext = b"wsh(or_d(multi(2,xpubA,xpubB),and_v(...)))#abcdefgh";

        let env = seal_to_xpub(
            &kh.account_xpub,
            &derivation(),
            ArtifactKind::Descriptor,
            plaintext,
        )
        .unwrap();

        assert_eq!(env.scheme, SCHEME);
        assert_eq!(env.artifact_kind, ArtifactKind::Descriptor);
        assert_eq!(env.derivation, ENCRYPTION_CHILD_DERIVATION);
        assert_eq!(env.ephemeral_pubkey.len(), PUBKEY_LEN);
        assert_eq!(env.nonce.len(), NONCE_LEN);

        // Heir path: Keychain derives K, desktop opens.
        let k = keychain_derive_shared_key(&kh, &env);
        let pt = open_with_shared_key(&k, &env).unwrap();
        assert_eq!(pt.as_slice(), plaintext);
    }

    #[test]
    fn seal_then_open_roundtrips_seed() {
        let kh = keyholder_from_seed(b"keyholder-seed-seed-roundtrip-vector-000000");
        // A representative SeedBlob-sized JSON payload.
        let plaintext = br#"{"version":1,"cube":{"uuid":"u"},"mnemonic":{"phrase":"abandon abandon ... about","language":"en"}}"#;

        let env =
            seal_to_xpub(&kh.account_xpub, &derivation(), ArtifactKind::Seed, plaintext).unwrap();
        let k = keychain_derive_shared_key(&kh, &env);
        let pt = open_with_shared_key(&k, &env).unwrap();
        assert_eq!(pt.as_slice(), plaintext.as_slice());
    }

    #[test]
    fn wrong_keyholder_key_fails_closed() {
        let owner_target = keyholder_from_seed(b"the-real-heir-keyholder-seed-vector-0000000");
        let attacker = keyholder_from_seed(b"a-different-keyholder-who-should-not-decrypt");

        let env = seal_to_xpub(
            &owner_target.account_xpub,
            &derivation(),
            ArtifactKind::Descriptor,
            b"secret descriptor",
        )
        .unwrap();

        // The attacker's Keychain derives a different shared key.
        let wrong_k = keychain_derive_shared_key(&attacker, &env);
        let err = open_with_shared_key(&wrong_k, &env).unwrap_err();
        assert!(matches!(err, EciesError::BadKeyOrCorrupt));
    }

    #[test]
    fn tampered_ciphertext_fails_tag() {
        let kh = keyholder_from_seed(b"tamper-ct-keyholder-seed-vector-00000000000");
        let mut env =
            seal_to_xpub(&kh.account_xpub, &derivation(), ArtifactKind::Descriptor, b"hello")
                .unwrap();
        env.ciphertext[0] ^= 0x01;
        let k = keychain_derive_shared_key(&kh, &env);
        assert!(matches!(
            open_with_shared_key(&k, &env).unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn tampered_derivation_in_aad_fails_tag() {
        // The derivation rides in the AAD, not the ciphertext. Swapping it
        // (e.g. a server trying to redirect the heir to a different child)
        // must break the tag even though the key `K` would still be derived
        // from the original child by an honest Keychain.
        let kh = keyholder_from_seed(b"tamper-aad-keyholder-seed-vector-0000000000");
        let env =
            seal_to_xpub(&kh.account_xpub, &derivation(), ArtifactKind::Descriptor, b"hello")
                .unwrap();
        let k = keychain_derive_shared_key(&kh, &env);

        let mut tampered = env.clone();
        tampered.derivation = "9/1".to_string();
        assert!(matches!(
            open_with_shared_key(&k, &tampered).unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn tampered_artifact_kind_in_aad_fails_tag() {
        let kh = keyholder_from_seed(b"tamper-kind-keyholder-seed-vector-000000000");
        let env = seal_to_xpub(&kh.account_xpub, &derivation(), ArtifactKind::Seed, b"hello")
            .unwrap();
        let k = keychain_derive_shared_key(&kh, &env);

        let mut tampered = env.clone();
        tampered.artifact_kind = ArtifactKind::Descriptor;
        assert!(matches!(
            open_with_shared_key(&k, &tampered).unwrap_err(),
            EciesError::BadKeyOrCorrupt
        ));
    }

    #[test]
    fn unsupported_scheme_rejected() {
        let kh = keyholder_from_seed(b"scheme-keyholder-seed-vector-00000000000000");
        let mut env =
            seal_to_xpub(&kh.account_xpub, &derivation(), ArtifactKind::Descriptor, b"x").unwrap();
        env.scheme = "ecies-v2-some-future-thing".to_string();
        let k = keychain_derive_shared_key(&kh, &env);
        assert!(matches!(
            open_with_shared_key(&k, &env).unwrap_err(),
            EciesError::UnsupportedScheme(_)
        ));
    }

    #[test]
    fn distinct_ephemeral_keys_across_calls() {
        // Two seals to the same recipient must use different ephemeral keys
        // (and thus different ciphertext) — otherwise the RNG isn't feeding
        // the ephemeral keypair.
        let kh = keyholder_from_seed(b"distinct-eph-keyholder-seed-vector-00000000");
        let a =
            seal_to_xpub(&kh.account_xpub, &derivation(), ArtifactKind::Descriptor, b"same").unwrap();
        let b =
            seal_to_xpub(&kh.account_xpub, &derivation(), ArtifactKind::Descriptor, b"same").unwrap();
        assert_ne!(a.ephemeral_pubkey, b.ephemeral_pubkey);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    // ---- Cross-repo known-answer vectors --------------------------------
    //
    // These pin the HKDF and the full ECDH→HKDF→AES-GCM path against fixed
    // inputs so `coincube-api` (storage) and `keychain-app` (ECDH-decrypt)
    // can verify byte-compatibility. If a KAT changes, the wire contract
    // changed — every escrowed envelope in the wild would need re-encryption.

    #[test]
    fn kat_hkdf_pins_derived_key() {
        // Fixed 32-byte ECDH shared secret `S` → fixed `K`. Independent of
        // any EC math, so a Go implementer can check HKDF alone.
        let s = [0x42u8; 32];
        let k = hkdf_sha256_key(&s);
        // HKDF-SHA256(salt="", ikm=0x42*32, info="coincube-inheritance-ecies-v1", L=32)
        assert_eq!(
            hex(&k[..]),
            "58c2505e26eb737e87d50f0175b806048024671511f3679e3a21ae1c909973cb"
        );
    }

    #[test]
    fn kat_full_envelope_fixed_keys() {
        // Fixed recipient + fixed ephemeral key → the heir recovers the
        // exact plaintext. We can't pin the ciphertext bytes (the public
        // `seal_to_xpub` draws a random ephemeral key + nonce), so this KAT
        // drives the deterministic internals: derive the child pub, ECDH
        // with a fixed ephemeral secret, and assert the round-trip key.
        let kh = keyholder_from_seed(&[0x01u8; 32]);
        let secp = Secp256k1::new();
        let recipient_child_pub = kh
            .account_xpub
            .derive_pub(&secp, &derivation())
            .unwrap()
            .public_key;
        let eph_sk = SecretKey::from_slice(&[0x07u8; 32]).unwrap();

        // Owner computes K from (eph_sk, recipient_child_pub)...
        let k_owner = shared_key_from_ecdh(&recipient_child_pub, &eph_sk);
        // ...heir computes K from (recovery_child_sk, eph_pk); must match.
        let eph_pk = PublicKey::from_secret_key(&secp, &eph_sk);
        let child_sk = kh
            .account_xpriv
            .derive_priv(&secp, &derivation())
            .unwrap()
            .private_key;
        let k_heir = shared_key_from_ecdh(&eph_pk, &child_sk);

        assert_eq!(&k_owner[..], &k_heir[..], "ECDH symmetry broken");
        // K = HKDF(SHA256(compressed(eph·child)),…) for seed=0x01*32,
        // derivation "9/0", eph_sk=0x07*32.
        assert_eq!(
            hex(&k_owner[..]),
            "9c713238b700cd88322be148353948e04022e9a163162fac958a44ece8e2c8c7"
        );
    }
}
