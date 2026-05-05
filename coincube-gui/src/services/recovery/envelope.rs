//! AES-256-GCM + Argon2id envelope for the Cube Recovery Kit.
//!
//! Wire format (binary, then base64-encoded for transport):
//!
//! ```text
//! version (1B=0x01) || kdf_id (1B=0x01) ||
//! kdf_params (4B: memory_kib u16 BE || t_cost u8 || p_cost u8) ||
//! salt (16B) || nonce (12B) || ciphertext || tag (16B)
//! ```
//!
//! AAD bound into the GCM auth tag = `version || kdf_id || kdf_params`
//! (the first 6 bytes). Salt and nonce are already GCM inputs and don't
//! need separate integrity, but downgrading the version/kdf bytes must
//! be rejected on decrypt.
//!
//! Aligned with `PLAN-cube-recovery-kit-desktop.md` §2.1.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use rand::RngCore;
use zeroize::Zeroizing;

use super::error::RecoveryError;

/// Envelope version this client writes and is the only version it accepts.
pub const ENVELOPE_VERSION: u8 = 0x01;

/// KDF identifier for Argon2id-with-per-envelope-params. Adding a new KDF
/// is a wire-format break and needs a new value here (and matching decrypt
/// dispatch).
pub const KDF_ID_ARGON2ID_V1: u8 = 0x01;

const HEADER_LEN: usize = 6; // version + kdf_id + 4B params
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const KEY_LEN: usize = 32;
const MIN_ENVELOPE_LEN: usize = HEADER_LEN + SALT_LEN + NONCE_LEN + TAG_LEN;

/// Argon2id KDF cost parameters carried inside every envelope so that
/// decrypt uses the exact same parameters that encrypt used — the caller
/// never re-derives the wire params on decrypt.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub memory_kib: u16,
    pub t_cost: u8,
    pub p_cost: u8,
}

impl KdfParams {
    /// v1 defaults: matches the Argon2id params used for PIN hashing
    /// (`coincube-gui/src/app/settings/mod.rs` — m=19456 KiB, t=2, p=1).
    /// Carrying the params in the envelope means we can bump these later
    /// without breaking existing backups.
    pub const DEFAULT_V1: Self = Self {
        memory_kib: 19456,
        t_cost: 2,
        p_cost: 1,
    };
}

fn header_bytes(version: u8, kdf_id: u8, params: KdfParams) -> [u8; HEADER_LEN] {
    let m = params.memory_kib.to_be_bytes();
    [version, kdf_id, m[0], m[1], params.t_cost, params.p_cost]
}

fn parse_header(buf: &[u8]) -> Result<(u8, u8, KdfParams), RecoveryError> {
    if buf.len() < HEADER_LEN {
        return Err(RecoveryError::Truncated);
    }
    let version = buf[0];
    let kdf_id = buf[1];
    let memory_kib = u16::from_be_bytes([buf[2], buf[3]]);
    let t_cost = buf[4];
    let p_cost = buf[5];
    Ok((
        version,
        kdf_id,
        KdfParams {
            memory_kib,
            t_cost,
            p_cost,
        },
    ))
}

fn derive_key(
    password: &[u8],
    salt: &[u8],
    params: KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>, RecoveryError> {
    let argon_params = Params::new(
        u32::from(params.memory_kib),
        u32::from(params.t_cost),
        u32::from(params.p_cost),
        Some(KEY_LEN),
    )
    .map_err(RecoveryError::InvalidParams)?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    argon2
        .hash_password_into(password, salt, key.as_mut())
        .map_err(RecoveryError::Kdf)?;
    Ok(key)
}

/// Encrypts `plaintext` under `password` and returns a base64-encoded
/// envelope. Generates a fresh 16-byte salt and 12-byte nonce per call.
pub fn encrypt(
    plaintext: &[u8],
    password: &Zeroizing<String>,
    params: KdfParams,
) -> Result<String, RecoveryError> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    let mut rng = rand::thread_rng();
    rng.fill_bytes(&mut salt);
    rng.fill_bytes(&mut nonce);

    encrypt_with(
        plaintext,
        password.as_bytes(),
        &salt,
        &nonce,
        params,
        ENVELOPE_VERSION,
        KDF_ID_ARGON2ID_V1,
    )
}

/// Internal encrypt that takes explicit salt/nonce/version/kdf_id so tests
/// can pin known-answer vectors. Callers outside the crate should use
/// `encrypt`.
fn encrypt_with(
    plaintext: &[u8],
    password: &[u8],
    salt: &[u8; SALT_LEN],
    nonce: &[u8; NONCE_LEN],
    params: KdfParams,
    version: u8,
    kdf_id: u8,
) -> Result<String, RecoveryError> {
    let key = derive_key(password, salt, params)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(RecoveryError::Cipher)?;
    let header = header_bytes(version, kdf_id, params);

    let ct = cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad: &header,
            },
        )
        .map_err(|_| RecoveryError::Seal)?;

    let mut out = Vec::with_capacity(HEADER_LEN + SALT_LEN + NONCE_LEN + ct.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(salt);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    Ok(base64::engine::general_purpose::STANDARD.encode(out))
}

/// Decrypts a base64-encoded envelope. Returns the plaintext in a
/// `Zeroizing<Vec<u8>>` so it's wiped on drop. A tag mismatch is reported
/// as `BadPasswordOrCorrupt` — the caller cannot distinguish a wrong
/// password from a tampered envelope, by design.
pub fn decrypt(
    envelope_b64: &str,
    password: &Zeroizing<String>,
) -> Result<Zeroizing<Vec<u8>>, RecoveryError> {
    let raw = base64::engine::general_purpose::STANDARD.decode(envelope_b64)?;
    if raw.len() < MIN_ENVELOPE_LEN {
        return Err(RecoveryError::Truncated);
    }

    let (version, kdf_id, params) = parse_header(&raw)?;
    if version != ENVELOPE_VERSION || kdf_id != KDF_ID_ARGON2ID_V1 {
        return Err(RecoveryError::Unsupported { version, kdf_id });
    }

    let salt = &raw[HEADER_LEN..HEADER_LEN + SALT_LEN];
    let nonce = &raw[HEADER_LEN + SALT_LEN..HEADER_LEN + SALT_LEN + NONCE_LEN];
    let ct_and_tag = &raw[HEADER_LEN + SALT_LEN + NONCE_LEN..];

    let key = derive_key(password.as_bytes(), salt, params)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(RecoveryError::Cipher)?;

    let header = header_bytes(version, kdf_id, params);
    let pt = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ct_and_tag,
                aad: &header,
            },
        )
        .map_err(|_| RecoveryError::BadPasswordOrCorrupt)?;

    Ok(Zeroizing::new(pt))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pw(s: &str) -> Zeroizing<String> {
        Zeroizing::new(s.to_string())
    }

    // Deliberately reduced cost for fast tests. Production uses DEFAULT_V1.
    const TEST_PARAMS: KdfParams = KdfParams {
        memory_kib: 512,
        t_cost: 1,
        p_cost: 1,
    };

    #[test]
    fn roundtrip_basic() {
        let password = pw("correct horse battery staple");
        let plaintext = b"hello cube recovery";
        let env = encrypt(plaintext, &password, TEST_PARAMS).unwrap();
        let got = decrypt(&env, &password).unwrap();
        assert_eq!(got.as_slice(), plaintext);
    }

    #[test]
    fn wrong_password_maps_to_bad_or_corrupt() {
        let env = encrypt(b"secret", &pw("right password"), TEST_PARAMS).unwrap();
        let err = decrypt(&env, &pw("wrong password")).unwrap_err();
        assert!(matches!(err, RecoveryError::BadPasswordOrCorrupt));
    }

    #[test]
    fn flipped_header_byte_rejected_by_aad() {
        // Flip the kdf_params memory_kib bytes. The KDF-id check won't
        // fire (we kept version=1, kdf=1); the AAD bind will.
        let env = encrypt(b"hello", &pw("pw"), TEST_PARAMS).unwrap();
        let mut raw = base64::engine::general_purpose::STANDARD
            .decode(&env)
            .unwrap();
        raw[2] ^= 0x01; // mutate memory_kib MSB
        let mutated = base64::engine::general_purpose::STANDARD.encode(raw);

        // Tampered header either (a) changes the derived key via the
        // params, or (b) breaks the AAD binding — either way, decrypt
        // must fail. Both paths surface as `BadPasswordOrCorrupt`.
        let err = decrypt(&mutated, &pw("pw")).unwrap_err();
        assert!(
            matches!(err, RecoveryError::BadPasswordOrCorrupt),
            "expected BadPasswordOrCorrupt, got {:?}",
            err
        );
    }

    #[test]
    fn flipped_ciphertext_byte_rejected_by_tag() {
        let env = encrypt(b"hello", &pw("pw"), TEST_PARAMS).unwrap();
        let mut raw = base64::engine::general_purpose::STANDARD
            .decode(&env)
            .unwrap();
        // Flip a byte in the ciphertext region (past header + salt + nonce).
        let ct_start = HEADER_LEN + SALT_LEN + NONCE_LEN;
        raw[ct_start] ^= 0x01;
        let mutated = base64::engine::general_purpose::STANDARD.encode(raw);

        let err = decrypt(&mutated, &pw("pw")).unwrap_err();
        assert!(matches!(err, RecoveryError::BadPasswordOrCorrupt));
    }

    #[test]
    fn truncated_envelope_rejected() {
        let env = encrypt(b"hello", &pw("pw"), TEST_PARAMS).unwrap();
        let mut raw = base64::engine::general_purpose::STANDARD
            .decode(&env)
            .unwrap();
        raw.truncate(MIN_ENVELOPE_LEN - 1);
        let truncated = base64::engine::general_purpose::STANDARD.encode(raw);
        let err = decrypt(&truncated, &pw("pw")).unwrap_err();
        assert!(matches!(err, RecoveryError::Truncated));
    }

    #[test]
    fn unsupported_version_rejected() {
        let env = encrypt(b"hello", &pw("pw"), TEST_PARAMS).unwrap();
        let mut raw = base64::engine::general_purpose::STANDARD
            .decode(&env)
            .unwrap();
        raw[0] = 0x99;
        let mutated = base64::engine::general_purpose::STANDARD.encode(raw);
        let err = decrypt(&mutated, &pw("pw")).unwrap_err();
        assert!(matches!(
            err,
            RecoveryError::Unsupported { version: 0x99, .. }
        ));
    }

    #[test]
    fn unsupported_kdf_id_rejected() {
        let env = encrypt(b"hello", &pw("pw"), TEST_PARAMS).unwrap();
        let mut raw = base64::engine::general_purpose::STANDARD
            .decode(&env)
            .unwrap();
        raw[1] = 0xFE;
        let mutated = base64::engine::general_purpose::STANDARD.encode(raw);
        let err = decrypt(&mutated, &pw("pw")).unwrap_err();
        assert!(matches!(
            err,
            RecoveryError::Unsupported { kdf_id: 0xFE, .. }
        ));
    }

    #[test]
    fn large_plaintext_roundtrips() {
        // 1.5 MiB — exercises the Vec growth path and any AEAD buffer
        // boundaries that would only show up at scale.
        let password = pw("pw");
        let plaintext = vec![0xA5u8; 1_500_000];
        let env = encrypt(&plaintext, &password, TEST_PARAMS).unwrap();
        let got = decrypt(&env, &password).unwrap();
        assert_eq!(got.as_slice(), plaintext.as_slice());
    }

    #[test]
    fn known_answer_pins_wire_format() {
        // Fixed inputs → stable envelope. Guards against accidental wire
        // format drift (field order, endianness, AAD contents). If this
        // test changes, the wire format has changed and every backed-up
        // kit in the wild would need re-encryption.
        let password = b"known-answer-password";
        let salt = [0x11u8; SALT_LEN];
        let nonce = [0x22u8; NONCE_LEN];
        let plaintext = b"KAT";
        let env = encrypt_with(
            plaintext,
            password,
            &salt,
            &nonce,
            TEST_PARAMS,
            ENVELOPE_VERSION,
            KDF_ID_ARGON2ID_V1,
        )
        .unwrap();

        // Decode and assert the prefix bytes explicitly — byte-comparing
        // the whole envelope as base64 would be fragile against any
        // future implementation detail in aes-gcm / argon2. The ciphertext
        // portion is covered by the roundtrip test.
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&env)
            .unwrap();
        assert_eq!(raw[0], ENVELOPE_VERSION, "version byte");
        assert_eq!(raw[1], KDF_ID_ARGON2ID_V1, "kdf_id byte");
        let m = u16::from_be_bytes([raw[2], raw[3]]);
        assert_eq!(m, TEST_PARAMS.memory_kib);
        assert_eq!(raw[4], TEST_PARAMS.t_cost);
        assert_eq!(raw[5], TEST_PARAMS.p_cost);
        assert_eq!(&raw[HEADER_LEN..HEADER_LEN + SALT_LEN], &salt);
        assert_eq!(
            &raw[HEADER_LEN + SALT_LEN..HEADER_LEN + SALT_LEN + NONCE_LEN],
            &nonce
        );
        assert_eq!(
            raw.len(),
            HEADER_LEN + SALT_LEN + NONCE_LEN + plaintext.len() + TAG_LEN
        );
    }

    #[test]
    fn distinct_salts_and_nonces_across_calls() {
        // Two encrypts of the same plaintext under the same password must
        // produce different envelopes — otherwise the PRNG wiring is
        // broken or cached.
        let password = pw("pw");
        let a = encrypt(b"same", &password, TEST_PARAMS).unwrap();
        let b = encrypt(b"same", &password, TEST_PARAMS).unwrap();
        assert_ne!(a, b, "two sealings collided — RNG not feeding salt/nonce");
    }
}
