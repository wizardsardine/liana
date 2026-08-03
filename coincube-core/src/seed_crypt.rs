//! On-disk seed-file codec: Argon2id + AES-256-GCM with self-describing,
//! authenticated KDF parameters.
//!
//! Three wire formats are understood. v3 is written wherever the platform has
//! a usable OS keystore; v2 where it does not. v1 is decrypt-only.
//!
//! ```text
//! v1 (legacy, decrypt-only):
//!   "ENCRYPTED_V1" || salt[16] || nonce[12] || ciphertext||tag
//!   KDF params are NOT on the wire — the reader hardcodes m=262144, t=3, p=4.
//!   Nothing is authenticated except the ciphertext itself.
//!
//! v2 (PIN only):
//!   "ENCRYPTED_V2" || header[8] || salt[16] || nonce[12] || ciphertext||tag
//!   header = version u8 || kdf_id u8 || memory_kib u32 BE || t_cost u8 || p_cost u8
//!   AAD    = header || cube_id
//!   key    = Argon2id(pin, salt)
//!
//! v3 (PIN + device secret):
//!   "ENCRYPTED_V3" || header[8] || salt[16] || nonce[12] || ciphertext||tag
//!   ...identical layout; only the key derivation differs:
//!   key    = HKDF-SHA256(ikm  = Argon2id(pin, salt) || device_secret,
//!                        salt = salt,
//!                        info = "coincube/seed-v3" || cube_id)
//! ```
//!
//! v3 is what makes a copied datadir useless: the device secret lives in the OS
//! keystore, never in the datadir and never on the network.
//!
//! Two things changed in v2 and both matter:
//!
//! 1. **The KDF parameters travel with the file.** v1 pinned them in the
//!    reader, so raising the cost later would have stranded every existing
//!    file. v2 reads them off the wire.
//! 2. **The header is bound into the GCM tag as AAD.** v1's marker and
//!    (implicit) parameters were unauthenticated, so an attacker who could
//!    write to the datadir could rewrite the file to claim cheap parameters
//!    and nothing would detect it. Under v2 any edit to the header makes the
//!    tag fail.
//!
//! The `cube_id` half of the AAD additionally binds a seed file to the Cube it
//! belongs to — the mnemonics folder is per-*network*, not per-Cube, so
//! without it a file could be moved between two Cubes that share a datadir.
//!
//! This is the same pattern as the Recovery Kit envelope
//! (`coincube-gui/src/services/recovery/envelope.rs`) with one deliberate
//! deviation: `memory_kib` is a `u32`, not the envelope's `u16`. A `u16` caps
//! at 65535 KiB (~64 MiB) and **cannot represent the seed file's 262144**.
//! Do not "unify" the two structs without widening the envelope's field first.

use aes_gcm::{
    aead::{rand_core::RngCore, Aead, OsRng, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use argon2::{
    password_hash::{PasswordHasher, SaltString},
    Algorithm, Argon2, Params, Version,
};
use zeroize::Zeroizing;

use crate::signer::SignerError;

/// Length of the leading format marker, shared by every version.
pub const MARKER_LEN: usize = 12;
/// Legacy marker. Still decrypted, never written.
pub const ENCRYPTED_V1_MARKER: &[u8; MARKER_LEN] = b"ENCRYPTED_V1";
/// PIN-only marker. Still decrypted; written only when no device secret is
/// available for this Cube.
pub const ENCRYPTED_V2_MARKER: &[u8; MARKER_LEN] = b"ENCRYPTED_V2";
/// Two-key marker: PIN **and** an OS-keystore-held device secret.
pub const ENCRYPTED_V3_MARKER: &[u8; MARKER_LEN] = b"ENCRYPTED_V3";

/// Version byte inside a v2 header.
pub const SEED_FILE_VERSION_V2: u8 = 0x02;
/// Version byte inside a v3 header.
pub const SEED_FILE_VERSION_V3: u8 = 0x03;

/// HKDF info prefix for the v3 file key. Concatenated with the Cube id, so two
/// Cubes sharing a device secret still get unrelated file keys.
const V3_HKDF_INFO_PREFIX: &[u8] = b"coincube/seed-v3";
/// KDF identifier for "Argon2id with parameters carried in this header".
/// A different KDF is a wire break and needs a new value plus matching
/// dispatch in [`decrypt`].
pub const KDF_ID_ARGON2ID: u8 = 0x01;

/// version + kdf_id + memory_kib(4) + t_cost + p_cost
pub const HEADER_LEN: usize = 8;
pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const KEY_LEN: usize = 32;

const MIN_V1_LEN: usize = MARKER_LEN + SALT_LEN + NONCE_LEN + TAG_LEN;
const MIN_V2_LEN: usize = MARKER_LEN + HEADER_LEN + SALT_LEN + NONCE_LEN + TAG_LEN;

/// Sanity bounds on the *wire* parameters. A header is attacker-writable
/// input: without a ceiling, `memory_kib = u32::MAX` asks Argon2 for 4 TiB and
/// takes the process down. Without a floor, a rewritten header could ask for a
/// trivially cheap derivation — the AAD binding already makes such a rewrite
/// fail the tag, but rejecting it before spending any work is cheaper and
/// doesn't rely on that argument holding.
const MIN_MEMORY_KIB: u32 = 8;
const MAX_MEMORY_KIB: u32 = 1_048_576; // 1 GiB

/// Argon2id cost parameters, carried inside every v2 file so decrypt uses
/// exactly what encrypt used.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub t_cost: u8,
    pub p_cost: u8,
}

impl KdfParams {
    /// The seed file's production cost — 256 MiB, 3 passes, 4 lanes; ~831 ms
    /// on one commodity core.
    ///
    /// **This is a floor for the whole tree, not just this file.** Every
    /// verifier that gates access to a Cube must cost at least this much,
    /// or it becomes the cheap oracle an offline attacker attacks instead
    /// (invariant I1 of PLAN-cube-unlock-hardening). There is a regression
    /// test asserting no weaker Argon2 verifier exists.
    pub const SEED: Self = Self {
        memory_kib: 262_144,
        t_cost: 3,
        p_cost: 4,
    };

    /// Cheap parameters for tests only. Deriving at [`Self::SEED`] cost
    /// hundreds of times over serialises CI into uselessness.
    #[cfg(test)]
    pub const TEST: Self = Self {
        memory_kib: 64,
        t_cost: 1,
        p_cost: 1,
    };
}

/// The parameters this build writes. Test builds drop the cost; the format,
/// the AAD binding, and every code path are identical either way, because the
/// parameters live on the wire.
#[cfg(not(test))]
pub const WRITE_PARAMS: KdfParams = KdfParams::SEED;
#[cfg(test)]
pub const WRITE_PARAMS: KdfParams = KdfParams::TEST;

fn header_bytes(version: u8, kdf_id: u8, params: KdfParams) -> [u8; HEADER_LEN] {
    let m = params.memory_kib.to_be_bytes();
    [
        version,
        kdf_id,
        m[0],
        m[1],
        m[2],
        m[3],
        params.t_cost,
        params.p_cost,
    ]
}

fn parse_header(buf: &[u8]) -> Result<(u8, u8, KdfParams), SignerError> {
    if buf.len() < HEADER_LEN {
        return Err(SignerError::InvalidFileFormat);
    }
    Ok((
        buf[0],
        buf[1],
        KdfParams {
            memory_kib: u32::from_be_bytes([buf[2], buf[3], buf[4], buf[5]]),
            t_cost: buf[6],
            p_cost: buf[7],
        },
    ))
}

/// AAD for a v2 file: the header, then the owning Cube's id.
fn aad_bytes(header: &[u8; HEADER_LEN], cube_id: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(HEADER_LEN + cube_id.len());
    aad.extend_from_slice(header);
    aad.extend_from_slice(cube_id.as_bytes());
    aad
}

/// Argon2id → 32-byte AES key. Rejects out-of-range wire parameters before
/// allocating anything.
fn derive_key(
    password: &str,
    salt: &[u8],
    params: KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>, SignerError> {
    if !(MIN_MEMORY_KIB..=MAX_MEMORY_KIB).contains(&params.memory_kib)
        || params.t_cost == 0
        || params.p_cost == 0
    {
        return Err(SignerError::ArgonParamsError(format!(
            "seed file declares out-of-range Argon2 parameters (m={} KiB, t={}, p={})",
            params.memory_kib, params.t_cost, params.p_cost
        )));
    }

    let argon_params = Params::new(
        params.memory_kib,
        u32::from(params.t_cost),
        u32::from(params.p_cost),
        Some(KEY_LEN),
    )
    .map_err(|e| SignerError::ArgonParamsError(e.to_string()))?;

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    argon2
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|e| SignerError::PasswordHashError(e.to_string()))?;
    Ok(key)
}

/// Whether `data` is any recognised encrypted seed file.
pub fn is_encrypted(data: &[u8]) -> bool {
    format_version(data).is_some()
}

/// The on-wire format version of `data`: `1`, `2`, `3`, or `None` when it is
/// not an encrypted seed file at all. Used by the migration pass to decide what
/// needs rewriting without decrypting first.
pub fn format_version(data: &[u8]) -> Option<u8> {
    if data.starts_with(ENCRYPTED_V3_MARKER) {
        Some(3)
    } else if data.starts_with(ENCRYPTED_V2_MARKER) {
        Some(2)
    } else if data.starts_with(ENCRYPTED_V1_MARKER) {
        Some(1)
    } else {
        None
    }
}

/// The 32-byte key material a v3 file is sealed under, mixed with the PIN.
///
/// Generated per Cube at creation by a CSPRNG, stored in the OS keystore, and
/// **never transmitted** — that last part is what distinguishes it from the
/// server-issued wrapping key the duress design rejects (invariants I4/I6).
pub type DeviceSecret = Zeroizing<[u8; 32]>;

/// v3 file key: `HKDF-SHA256(ikm = Argon2id(pin, salt) ‖ device_secret,
/// salt = file_salt, info = "coincube/seed-v3" ‖ cube_id)`.
///
/// HKDF rather than feeding both halves through Argon2id: Argon2 is there to
/// harden the *low-entropy* half. Pushing 256 already-uniform bits through 256
/// MiB of memory-hard work buys nothing; HKDF is the right tool for combining.
fn derive_key_v3(
    password: &str,
    salt: &[u8],
    params: KdfParams,
    device_secret: &DeviceSecret,
    cube_id: &str,
) -> Result<Zeroizing<[u8; KEY_LEN]>, SignerError> {
    let pin_half = derive_key(password, salt, params)?;

    let mut ikm = Zeroizing::new(Vec::with_capacity(KEY_LEN * 2));
    ikm.extend_from_slice(pin_half.as_ref());
    ikm.extend_from_slice(device_secret.as_ref());

    let mut info = Vec::with_capacity(V3_HKDF_INFO_PREFIX.len() + cube_id.len());
    info.extend_from_slice(V3_HKDF_INFO_PREFIX);
    info.extend_from_slice(cube_id.as_bytes());

    let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(salt), &ikm);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    hk.expand(&info, key.as_mut())
        .map_err(|_| SignerError::KeyDerivationFailed)?;
    Ok(key)
}

/// Seal `plaintext` under `password`, bound to `cube_id`, at [`WRITE_PARAMS`].
///
/// Writes v3 when a device secret is supplied and v2 when it is not. Callers
/// should always pass one where the platform provides a keystore — a v2 file is
/// openable from a copied datadir, a v3 file is not.
pub fn encrypt(
    plaintext: &[u8],
    password: &str,
    cube_id: &str,
    device_secret: Option<&DeviceSecret>,
) -> Result<Vec<u8>, SignerError> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    encrypt_with(
        plaintext,
        password,
        cube_id,
        device_secret,
        &salt,
        &nonce,
        WRITE_PARAMS,
    )
}

/// PIN-only seal. Prefer [`encrypt`] with a device secret.
pub fn encrypt_v2(plaintext: &[u8], password: &str, cube_id: &str) -> Result<Vec<u8>, SignerError> {
    encrypt(plaintext, password, cube_id, None)
}

/// Explicit-salt/nonce/params variant so tests can pin known-answer vectors.
fn encrypt_with(
    plaintext: &[u8],
    password: &str,
    cube_id: &str,
    device_secret: Option<&DeviceSecret>,
    salt: &[u8; SALT_LEN],
    nonce: &[u8; NONCE_LEN],
    params: KdfParams,
) -> Result<Vec<u8>, SignerError> {
    let (version, marker, key) = match device_secret {
        Some(secret) => (
            SEED_FILE_VERSION_V3,
            ENCRYPTED_V3_MARKER,
            derive_key_v3(password, salt, params, secret, cube_id)?,
        ),
        None => (
            SEED_FILE_VERSION_V2,
            ENCRYPTED_V2_MARKER,
            derive_key(password, salt, params)?,
        ),
    };

    let cipher = Aes256Gcm::new_from_slice(key.as_ref())
        .map_err(|e| SignerError::CipherCreationError(e.to_string()))?;

    let header = header_bytes(version, KDF_ID_ARGON2ID, params);
    let aad = aad_bytes(&header, cube_id);

    let ct = cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|e| SignerError::EncryptionFailed(e.to_string()))?;

    let mut out = Vec::with_capacity(MIN_V2_LEN + plaintext.len());
    out.extend_from_slice(marker);
    out.extend_from_slice(&header);
    out.extend_from_slice(salt);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Open an encrypted seed file of either version.
///
/// `cube_id` is the id of the Cube the caller believes owns this file. It is
/// ignored for v1 (which has no AAD).
///
/// A wrong password, a tampered header, and a file belonging to a *different*
/// Cube are all reported as [`SignerError::InvalidPassword`] — the caller
/// cannot tell them apart, which is the point.
pub fn decrypt(
    data: &[u8],
    password: &str,
    cube_id: &str,
) -> Result<Zeroizing<Vec<u8>>, SignerError> {
    decrypt_with(data, password, cube_id, None)
}

/// Open a seed file, supplying the device secret needed by v3.
///
/// A v3 file without a device secret returns
/// [`SignerError::DeviceSecretRequired`] — deliberately **not**
/// `InvalidPassword`. A user whose keychain is locked or whose keyring entry is
/// gone must never be told their PIN is wrong (invariant I7).
pub fn decrypt_with(
    data: &[u8],
    password: &str,
    cube_id: &str,
    device_secret: Option<&DeviceSecret>,
) -> Result<Zeroizing<Vec<u8>>, SignerError> {
    match format_version(data) {
        Some(3) => {
            let secret = device_secret.ok_or(SignerError::DeviceSecretRequired)?;
            decrypt_v2_or_v3(data, password, cube_id, Some(secret))
        }
        // A v2 file opens with the PIN alone; a device secret, if the caller
        // has one, is simply not part of its key.
        Some(2) => decrypt_v2_or_v3(data, password, cube_id, None),
        Some(1) => decrypt_v1(data, password),
        _ => Err(SignerError::NotEncryptedFile),
    }
}

fn decrypt_v2_or_v3(
    data: &[u8],
    password: &str,
    cube_id: &str,
    device_secret: Option<&DeviceSecret>,
) -> Result<Zeroizing<Vec<u8>>, SignerError> {
    if data.len() < MIN_V2_LEN {
        return Err(SignerError::InvalidFileFormat);
    }
    let body = &data[MARKER_LEN..];
    let (version, kdf_id, params) = parse_header(body)?;
    let expected_version = match device_secret {
        Some(_) => SEED_FILE_VERSION_V3,
        None => SEED_FILE_VERSION_V2,
    };
    if version != expected_version || kdf_id != KDF_ID_ARGON2ID {
        return Err(SignerError::InvalidFileFormat);
    }

    let salt = &body[HEADER_LEN..HEADER_LEN + SALT_LEN];
    let nonce = &body[HEADER_LEN + SALT_LEN..HEADER_LEN + SALT_LEN + NONCE_LEN];
    let ct_and_tag = &body[HEADER_LEN + SALT_LEN + NONCE_LEN..];

    let key = match device_secret {
        Some(secret) => derive_key_v3(password, salt, params, secret, cube_id)?,
        None => derive_key(password, salt, params)?,
    };
    let cipher =
        Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| SignerError::InvalidPassword)?;
    let header = header_bytes(version, kdf_id, params);

    // Preferred binding: this file belongs to `cube_id`.
    let attempt = |aad: &[u8]| {
        cipher.decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ct_and_tag,
                aad,
            },
        )
    };

    if let Ok(pt) = attempt(&aad_bytes(&header, cube_id)) {
        return Ok(Zeroizing::new(pt));
    }

    // Fall back to the *unbound* AAD (empty cube_id) and nothing else.
    //
    // Some writers legitimately have no Cube id yet — the Vault installer runs
    // before `find_or_create_cube` mints one — and those files must stay
    // readable once the Cube exists. This fallback admits exactly those, and
    // is not a hole: a file sealed to Cube A still fails under Cube B, because
    // B tries only `B` and `""`, never `A`.
    if !cube_id.is_empty() {
        if let Ok(pt) = attempt(&aad_bytes(&header, "")) {
            return Ok(Zeroizing::new(pt));
        }
    }

    Err(SignerError::InvalidPassword)
}

/// Legacy path, byte-for-byte compatible with what shipped as `ENCRYPTED_V1`:
/// parameters hardcoded, no AAD, salt run through PHC base64 on the way in.
///
/// `encode_b64` + `hash_password` and `hash_password_into` produce the same key
/// for the same raw salt (the PHC wrapper decodes the salt and calls straight
/// through), so this could share [`derive_key`]. It deliberately doesn't: this
/// is the code path that opens every Cube in the wild, and it is worth having
/// it read as the original rather than as an argument about equivalence. The
/// `v1_written_by_legacy_code_still_decrypts` test pins the equivalence with
/// real bytes.
fn decrypt_v1(data: &[u8], password: &str) -> Result<Zeroizing<Vec<u8>>, SignerError> {
    if data.len() < MIN_V1_LEN {
        return Err(SignerError::InvalidFileFormat);
    }
    let body = &data[MARKER_LEN..];
    let salt_bytes = &body[..SALT_LEN];
    let nonce_bytes = &body[SALT_LEN..SALT_LEN + NONCE_LEN];
    let ciphertext = &body[SALT_LEN + NONCE_LEN..];

    let salt = SaltString::encode_b64(salt_bytes)
        .map_err(|e| SignerError::DecryptionFailed(e.to_string()))?;

    let params = Params::new(
        KdfParams::SEED.memory_kib,
        u32::from(KdfParams::SEED.t_cost),
        u32::from(KdfParams::SEED.p_cost),
        Some(KEY_LEN),
    )
    .map_err(|e| SignerError::DecryptionFailed(e.to_string()))?;

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| SignerError::InvalidPassword)?;
    let hash_output = password_hash.hash.ok_or(SignerError::InvalidPassword)?;

    let key_bytes = Zeroizing::new({
        let hash_bytes = hash_output.as_bytes();
        if hash_bytes.len() < KEY_LEN {
            return Err(SignerError::InvalidPassword);
        }
        hash_bytes[..KEY_LEN].to_vec()
    });

    let cipher =
        Aes256Gcm::new_from_slice(&key_bytes).map_err(|_| SignerError::InvalidPassword)?;
    let pt = cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|_| SignerError::InvalidPassword)?;

    Ok(Zeroizing::new(pt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::TryInto;

    /// Reproduces the pre-v2 `encrypt_mnemonic` exactly, so the v1 decrypt path
    /// is tested against bytes the old code would actually have written rather
    /// than against itself.
    fn legacy_v1_encrypt(plaintext: &str, password: &str, params: KdfParams) -> Vec<u8> {
        let mut salt_bytes = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt_bytes);
        let salt = SaltString::encode_b64(&salt_bytes).unwrap();

        let p = Params::new(
            params.memory_kib,
            u32::from(params.t_cost),
            u32::from(params.p_cost),
            Some(32),
        )
        .unwrap();
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, p);
        let password_hash = argon2.hash_password(password.as_bytes(), &salt).unwrap();
        let hash_output = password_hash.hash.unwrap();
        let key_bytes = &hash_output.as_bytes()[..32];

        let cipher = Aes256Gcm::new_from_slice(key_bytes).unwrap();
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_bytes())
            .unwrap();

        let mut out = Vec::new();
        out.extend_from_slice(ENCRYPTED_V1_MARKER);
        out.extend_from_slice(&salt_bytes);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        out
    }

    const MNEMONIC: &str =
        "vault upset spike foil chef aerobic solve prefer angry ripple wagon fabric";

    #[test]
    fn v2_roundtrip() {
        let blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        assert_eq!(format_version(&blob), Some(2));
        assert!(is_encrypted(&blob));
        let out = decrypt(&blob, "1234", "cube-a").unwrap();
        assert_eq!(out.as_slice(), MNEMONIC.as_bytes());
    }

    #[test]
    fn v2_wrong_password_rejected() {
        let blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        assert!(matches!(
            decrypt(&blob, "4321", "cube-a"),
            Err(SignerError::InvalidPassword)
        ));
    }

    #[test]
    fn v2_is_bound_to_its_cube() {
        // Sealed to cube-a: cube-b must not open it even with the right PIN.
        let blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        assert!(matches!(
            decrypt(&blob, "1234", "cube-b"),
            Err(SignerError::InvalidPassword)
        ));
        // ...and neither may an id-less reader.
        assert!(matches!(
            decrypt(&blob, "1234", ""),
            Err(SignerError::InvalidPassword)
        ));
    }

    #[test]
    fn unbound_file_opens_under_any_cube_id() {
        // The installer writes before the Cube id exists; that file must stay
        // readable once the Cube is minted.
        let blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "").unwrap();
        let out = decrypt(&blob, "1234", "cube-a").unwrap();
        assert_eq!(out.as_slice(), MNEMONIC.as_bytes());
    }

    #[test]
    fn v1_written_by_legacy_code_still_decrypts() {
        // The regression guard for every Cube already on disk. Uses the
        // production v1 parameters because that is what real files carry.
        let blob = legacy_v1_encrypt(MNEMONIC, "1234", KdfParams::SEED);
        assert_eq!(format_version(&blob), Some(1));
        let out = decrypt(&blob, "1234", "cube-a").unwrap();
        assert_eq!(out.as_slice(), MNEMONIC.as_bytes());
        // cube_id is irrelevant to v1 — it has no AAD.
        let out = decrypt(&blob, "1234", "").unwrap();
        assert_eq!(out.as_slice(), MNEMONIC.as_bytes());
    }

    #[test]
    fn v1_wrong_password_rejected() {
        let blob = legacy_v1_encrypt(MNEMONIC, "1234", KdfParams::SEED);
        assert!(matches!(
            decrypt(&blob, "4321", ""),
            Err(SignerError::InvalidPassword)
        ));
    }

    #[test]
    fn memory_kib_262144_survives_the_header_roundtrip() {
        // The u16 trap: the Recovery Kit envelope declares `memory_kib: u16`,
        // which silently truncates 262144 to 0. If this ever fails, someone
        // narrowed the field and every seed file written since is unopenable.
        let header = header_bytes(SEED_FILE_VERSION_V2, KDF_ID_ARGON2ID, KdfParams::SEED);
        let (v, k, params) = parse_header(&header).unwrap();
        assert_eq!(v, SEED_FILE_VERSION_V2);
        assert_eq!(k, KDF_ID_ARGON2ID);
        assert_eq!(params.memory_kib, 262_144);
        assert_eq!(params, KdfParams::SEED);
    }

    #[test]
    fn seed_params_are_the_documented_cost() {
        // I1's anchor: these are the numbers the whole design is measured
        // against. Changing them is a deliberate act, not a drive-by.
        assert_eq!(KdfParams::SEED.memory_kib, 262_144);
        assert_eq!(KdfParams::SEED.t_cost, 3);
        assert_eq!(KdfParams::SEED.p_cost, 4);
    }

    #[test]
    fn flipped_param_byte_fails_authentication() {
        // The v1 hole this closes: with the header unauthenticated, flipping a
        // cost byte just made the reader derive a different (cheaper) key and
        // nothing noticed. Under v2 it must fail — and specifically it must
        // never succeed at the rewritten cost.
        //
        // Flip a bit inside `memory_kib` that leaves the value in range, so the
        // range check can't be what rejects it and the AAD binding has to do
        // the work. 64 KiB -> 192 KiB here.
        let mut blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        blob[MARKER_LEN + 5] ^= 0x80;
        let err = decrypt(&blob, "1234", "cube-a").unwrap_err();
        assert!(
            matches!(err, SignerError::InvalidPassword),
            "expected authentication failure, got {:?}",
            err
        );

        // A flip that lands out of range is rejected earlier still, before any
        // Argon2 work happens.
        let mut blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        blob[MARKER_LEN + 6] ^= 0x01; // t_cost 1 -> 0
        assert!(matches!(
            decrypt(&blob, "1234", "cube-a"),
            Err(SignerError::ArgonParamsError(_))
        ));
    }

    #[test]
    fn flipped_ciphertext_byte_fails_authentication() {
        let mut blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        let ct_start = MARKER_LEN + HEADER_LEN + SALT_LEN + NONCE_LEN;
        blob[ct_start] ^= 0x01;
        assert!(matches!(
            decrypt(&blob, "1234", "cube-a"),
            Err(SignerError::InvalidPassword)
        ));
    }

    #[test]
    fn v2_retagged_as_v1_is_rejected() {
        let mut blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        blob[..MARKER_LEN].copy_from_slice(ENCRYPTED_V1_MARKER);
        // Read as v1 the header bytes become salt, so this can only fail.
        assert!(decrypt(&blob, "1234", "cube-a").is_err());
    }

    #[test]
    fn unknown_version_or_kdf_rejected() {
        let mut blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        blob[MARKER_LEN] = 0x99;
        assert!(matches!(
            decrypt(&blob, "1234", "cube-a"),
            Err(SignerError::InvalidFileFormat)
        ));

        let mut blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        blob[MARKER_LEN + 1] = 0xFE;
        assert!(matches!(
            decrypt(&blob, "1234", "cube-a"),
            Err(SignerError::InvalidFileFormat)
        ));
    }

    #[test]
    fn absurd_memory_cost_is_refused_before_allocating() {
        // A hostile header must not be able to ask for 4 TiB of RAM.
        let mut blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        blob[MARKER_LEN + 2..MARKER_LEN + 6].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(
            decrypt(&blob, "1234", "cube-a"),
            Err(SignerError::ArgonParamsError(_))
        ));

        // ...nor for a trivially cheap one.
        let mut blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        blob[MARKER_LEN + 2..MARKER_LEN + 6].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(
            decrypt(&blob, "1234", "cube-a"),
            Err(SignerError::ArgonParamsError(_))
        ));
    }

    #[test]
    fn truncated_input_rejected() {
        let blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        for cut in [0, MARKER_LEN, MIN_V2_LEN - 1] {
            assert!(decrypt(&blob[..cut], "1234", "cube-a").is_err());
        }
    }

    #[test]
    fn plaintext_is_not_an_encrypted_file() {
        assert!(!is_encrypted(MNEMONIC.as_bytes()));
        assert_eq!(format_version(MNEMONIC.as_bytes()), None);
        assert!(matches!(
            decrypt(MNEMONIC.as_bytes(), "1234", ""),
            Err(SignerError::NotEncryptedFile)
        ));
    }

    #[test]
    fn wire_layout_is_pinned() {
        let salt = [0x11u8; SALT_LEN];
        let nonce = [0x22u8; NONCE_LEN];
        let blob =
            encrypt_with(b"KAT", "pw", "cube-a", None, &salt, &nonce, KdfParams::TEST).unwrap();

        assert_eq!(&blob[..MARKER_LEN], ENCRYPTED_V2_MARKER);
        assert_eq!(blob[MARKER_LEN], SEED_FILE_VERSION_V2);
        assert_eq!(blob[MARKER_LEN + 1], KDF_ID_ARGON2ID);
        assert_eq!(
            u32::from_be_bytes(blob[MARKER_LEN + 2..MARKER_LEN + 6].try_into().unwrap()),
            KdfParams::TEST.memory_kib
        );
        assert_eq!(blob[MARKER_LEN + 6], KdfParams::TEST.t_cost);
        assert_eq!(blob[MARKER_LEN + 7], KdfParams::TEST.p_cost);
        assert_eq!(
            &blob[MARKER_LEN + HEADER_LEN..MARKER_LEN + HEADER_LEN + SALT_LEN],
            &salt
        );
        assert_eq!(
            &blob[MARKER_LEN + HEADER_LEN + SALT_LEN
                ..MARKER_LEN + HEADER_LEN + SALT_LEN + NONCE_LEN],
            &nonce
        );
        assert_eq!(blob.len(), MIN_V2_LEN + 3);
    }

    fn secret(byte: u8) -> DeviceSecret {
        Zeroizing::new([byte; 32])
    }

    #[test]
    fn v3_roundtrip() {
        let s = secret(0xAB);
        let blob = encrypt(MNEMONIC.as_bytes(), "1234", "cube-a", Some(&s)).unwrap();
        assert_eq!(format_version(&blob), Some(3));
        let out = decrypt_with(&blob, "1234", "cube-a", Some(&s)).unwrap();
        assert_eq!(out.as_slice(), MNEMONIC.as_bytes());
    }

    /// **This is the security claim of Tier 1.** A datadir copied to another
    /// machine carries the seed file but not the keyring entry, so it must not
    /// open — even with the correct PIN.
    #[test]
    fn copied_datadir_without_the_device_secret_does_not_decrypt() {
        let s = secret(0xAB);
        let blob = encrypt(MNEMONIC.as_bytes(), "1234", "cube-a", Some(&s)).unwrap();

        // No keyring entry at all on the new machine.
        assert!(matches!(
            decrypt_with(&blob, "1234", "cube-a", None),
            Err(SignerError::DeviceSecretRequired)
        ));
        // ...and a *different* machine's secret is no help either.
        assert!(matches!(
            decrypt_with(&blob, "1234", "cube-a", Some(&secret(0xCD))),
            Err(SignerError::InvalidPassword)
        ));
    }

    #[test]
    fn v3_missing_secret_is_not_reported_as_a_wrong_pin() {
        // I7: three distinguishable failures. A locked keychain must never
        // read as "wrong PIN" — that is what makes a user believe their wallet
        // is gone.
        let s = secret(0xAB);
        let blob = encrypt(MNEMONIC.as_bytes(), "1234", "cube-a", Some(&s)).unwrap();

        let missing = decrypt_with(&blob, "1234", "cube-a", None).unwrap_err();
        let wrong_pin = decrypt_with(&blob, "9999", "cube-a", Some(&s)).unwrap_err();
        assert!(matches!(missing, SignerError::DeviceSecretRequired));
        assert!(matches!(wrong_pin, SignerError::InvalidPassword));
        assert_ne!(missing.to_string(), wrong_pin.to_string());
    }

    #[test]
    fn v3_is_bound_to_its_cube_and_its_pin() {
        let s = secret(0xAB);
        let blob = encrypt(MNEMONIC.as_bytes(), "1234", "cube-a", Some(&s)).unwrap();
        assert!(matches!(
            decrypt_with(&blob, "1234", "cube-b", Some(&s)),
            Err(SignerError::InvalidPassword)
        ));
        assert!(matches!(
            decrypt_with(&blob, "4321", "cube-a", Some(&s)),
            Err(SignerError::InvalidPassword)
        ));
    }

    #[test]
    fn two_cubes_sharing_a_device_secret_get_unrelated_file_keys() {
        // The Cube id is in the HKDF info, so one Cube's file key tells you
        // nothing about another's even on the same machine and same PIN.
        let s = secret(0xAB);
        let a = encrypt(b"same plaintext", "1234", "cube-a", Some(&s)).unwrap();
        let b = encrypt(b"same plaintext", "1234", "cube-b", Some(&s)).unwrap();
        assert!(decrypt_with(&a, "1234", "cube-b", Some(&s)).is_err());
        assert!(decrypt_with(&b, "1234", "cube-a", Some(&s)).is_err());
    }

    #[test]
    fn v2_still_opens_while_v3_is_being_rolled_out() {
        // Retained for one release so a rollback doesn't strand anyone. A
        // caller holding a device secret must still be able to open a v2 file.
        let s = secret(0xAB);
        let blob = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        assert_eq!(format_version(&blob), Some(2));
        let out = decrypt_with(&blob, "1234", "cube-a", Some(&s)).unwrap();
        assert_eq!(out.as_slice(), MNEMONIC.as_bytes());
    }

    #[test]
    fn v3_retagged_as_v2_is_rejected() {
        // Stripping the marker back to v2 must not downgrade the file to
        // PIN-only — the header version is inside the AAD, and the version
        // check catches it before that anyway.
        let s = secret(0xAB);
        let mut blob = encrypt(MNEMONIC.as_bytes(), "1234", "cube-a", Some(&s)).unwrap();
        blob[..MARKER_LEN].copy_from_slice(ENCRYPTED_V2_MARKER);
        assert!(matches!(
            decrypt_with(&blob, "1234", "cube-a", None),
            Err(SignerError::InvalidFileFormat)
        ));
    }

    #[test]
    fn distinct_salt_and_nonce_per_call() {
        let a = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        let b = encrypt_v2(MNEMONIC.as_bytes(), "1234", "cube-a").unwrap();
        assert_ne!(a, b, "two sealings collided — RNG not feeding salt/nonce");
    }
}
