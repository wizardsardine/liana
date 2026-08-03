//! The duress marker.
//!
//! Replaces the `duress_pin_hash` field that used to live in `settings.json`.
//! That hash was Argon2id at m=19 MiB / t=2 / p=1 — 27 ms a guess against a
//! seed file that costs 831 ms. Any attacker with the datadir attacked the
//! cheap one, learned the duress PIN, and from there learned that duress was
//! armed at all. The marker costs exactly what the seed file costs, because it
//! goes through the same codec at the same parameters (invariant I2).
//!
//! # What it is
//!
//! A fixed, known plaintext sealed under the **duress** PIN. Verifying a duress
//! PIN means trial-decrypting it: the GCM tag is the verifier, so there is one
//! cost and no cheaper oracle.
//!
//! # Where it lives, and why there
//!
//! In the Cube's own `mnemonics/` folder, alongside the seed file. That is a
//! deliberate contrast with the duress retry queue and
//! `services/duress/cipher.rs`'s key, which sit *outside* the Cube data dir
//! precisely so a wipe cannot destroy them. The marker is the opposite: it is
//! *for* this Cube and should die with it. `duress_wipe_targets` already takes
//! the whole `mnemonics/` directory, so that happens with no extra code.
//!
//! It is also named like a master-seed file, with a Cube-derived pseudo
//! fingerprint. Someone who copies the datadir sees two AES-GCM blobs with
//! identical KDF parameters and matching filename grammar. Nothing in the file
//! or its name says "duress".

use std::path::{Path, PathBuf};

use coincube_core::miniscript::bitcoin::Network;
use coincube_core::seed_crypt::{self, DeviceSecret};
use coincube_core::signer::{MasterSigner, MASTER_SEED_LABEL};

use super::UnlockError;

/// Domain tag for the marker's pseudo-fingerprint. Versioned: changing it
/// changes every marker filename, which orphans existing markers.
const MARKER_FP_DOMAIN: &[u8] = b"coincube/duress-marker/v1";

/// The sealed plaintext.
///
/// Fixed and known — its secrecy is not what protects anything; the point is
/// that only the duress PIN produces a valid GCM tag over it.
///
/// It is 93 bytes, the length of a common 12-word English mnemonic, so the
/// marker's ciphertext is the same size as a seed file's. It is deliberately
/// **not** a valid BIP39 mnemonic: a real one would be tidier still, but a
/// publicly-known valid mnemonic is a wallet, and a wallet is somewhere a
/// confused user could send funds.
const MARKER_PLAINTEXT: &[u8; 93] =
    b"coincube/duress-marker/v1 -- this file is not a seed phrase and holds no key material........";

/// Deterministic filename for `cube_id`'s marker, matching the master-seed
/// grammar (`mnemonic-<fp>-master_<ts>-<ts>.txt`).
///
/// The fingerprint is derived, not random, so the file can be found again
/// without recording its name anywhere — recording it would be the tell.
pub fn file_name(cube_id: &str, cube_created_at: i64) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(MARKER_FP_DOMAIN);
    hasher.update(cube_id.as_bytes());
    let digest = hasher.finalize();
    let fp: String = digest[..4].iter().map(|b| format!("{:02x}", b)).collect();
    format!(
        "mnemonic-{}-{}{}-{}.txt",
        fp, MASTER_SEED_LABEL, cube_created_at, cube_created_at
    )
}

/// Full path to `cube_id`'s marker.
pub fn path(
    datadir_root: &Path,
    network: Network,
    cube_id: &str,
    cube_created_at: i64,
) -> PathBuf {
    MasterSigner::mnemonics_folder(datadir_root, network)
        .join(file_name(cube_id, cube_created_at))
}

/// Whether a marker exists for this Cube (i.e. duress is armed on it).
pub fn exists(
    datadir_root: &Path,
    network: Network,
    cube_id: &str,
    cube_created_at: i64,
) -> bool {
    path(datadir_root, network, cube_id, cube_created_at).exists()
}

/// Arm duress on this Cube by writing its marker. Replaces any existing one so
/// re-enrolment with a new duress PIN works.
///
/// `device_secret` must be the same one the Cube's seed file uses, so the two
/// files are indistinguishable in wire version as well as in parameters.
pub fn write(
    datadir_root: &Path,
    network: Network,
    cube_id: &str,
    cube_created_at: i64,
    duress_pin: &str,
    device_secret: Option<&DeviceSecret>,
) -> Result<(), UnlockError> {
    let folder = MasterSigner::mnemonics_folder(datadir_root, network);
    std::fs::create_dir_all(&folder).map_err(|e| UnlockError::Io(e.to_string()))?;

    let blob = seed_crypt::encrypt(MARKER_PLAINTEXT, duress_pin, cube_id, device_secret)
        .map_err(|e| UnlockError::Io(e.to_string()))?;

    let target = folder.join(file_name(cube_id, cube_created_at));
    // Write beside and rename, so a crash mid-write can't leave a Cube armed
    // with a truncated marker that no PIN opens.
    let tmp = target.with_extension("tmp");
    std::fs::write(&tmp, &blob).map_err(|e| UnlockError::Io(e.to_string()))?;
    std::fs::rename(&tmp, &target).map_err(|e| UnlockError::Io(e.to_string()))?;
    Ok(())
}

/// Disarm duress on this Cube. Idempotent — a missing marker is success.
pub fn remove(
    datadir_root: &Path,
    network: Network,
    cube_id: &str,
    cube_created_at: i64,
) -> Result<(), UnlockError> {
    let target = path(datadir_root, network, cube_id, cube_created_at);
    match std::fs::remove_file(&target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(UnlockError::Io(e.to_string())),
    }
}

/// Does `pin` open this Cube's duress marker?
///
/// `false` for a Cube with no marker — never a permissive default. A duress
/// activation is destructive, so it must only ever follow an explicit,
/// deliberate enrolment.
///
/// Costs exactly one Argon2id derivation at the seed file's parameters, which
/// is what keeps wrong-vs-duress timing-indistinct.
pub fn verify(
    datadir_root: &Path,
    network: Network,
    cube_id: &str,
    cube_created_at: i64,
    pin: &str,
    device_secret: Option<&DeviceSecret>,
) -> bool {
    let target = path(datadir_root, network, cube_id, cube_created_at);
    let Ok(blob) = std::fs::read(&target) else {
        return false;
    };
    match seed_crypt::decrypt_with(&blob, pin, cube_id, device_secret) {
        Ok(plaintext) => plaintext.as_slice() == MARKER_PLAINTEXT.as_slice(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "coincube-marker-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const NET: Network = Network::Bitcoin;

    #[test]
    fn write_then_verify_round_trip() {
        let dir = tmp_dir("roundtrip");
        write(&dir, NET, "cube-a", 1000, "9999", None).unwrap();

        assert!(verify(&dir, NET, "cube-a", 1000, "9999", None));
        assert!(!verify(&dir, NET, "cube-a", 1000, "1234", None));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn absent_marker_never_verifies() {
        // The default that matters: a Cube with no duress enrolled must not be
        // wipeable by guessing.
        let dir = tmp_dir("absent");
        assert!(!exists(&dir, NET, "cube-a", 1000));
        for pin in ["0000", "1234", "9999", ""] {
            assert!(!verify(&dir, NET, "cube-a", 1000, pin, None));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn marker_is_bound_to_its_cube() {
        let dir = tmp_dir("bound");
        write(&dir, NET, "cube-a", 1000, "9999", None).unwrap();
        // Same PIN, different Cube: the AAD binding rejects it.
        assert!(!verify(&dir, NET, "cube-b", 1000, "9999", None));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn marker_is_indistinguishable_from_a_seed_file() {
        // I2 on disk. Same wire version, same KDF parameters, same filename
        // grammar, and nothing in the name saying what it is.
        use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;
        use coincube_core::signer::MasterSigner;

        let dir = tmp_dir("shape");
        let secp = Secp256k1::signing_only();
        let signer = MasterSigner::generate(NET).unwrap();
        signer
            .store_encrypted(
                &dir,
                NET,
                &secp,
                Some((format!("{}{}", MASTER_SEED_LABEL, 1000), 1000)),
                "1234",
                "cube-a",
                None,
            )
            .unwrap();
        write(&dir, NET, "cube-a", 1000, "9999", None).unwrap();

        let folder = MasterSigner::mnemonics_folder(&dir, NET);
        let files: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(files.len(), 2, "seed file and marker");

        for f in &files {
            let name = f.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with("mnemonic-"), "{}", name);
            assert!(name.ends_with(".txt"), "{}", name);
            assert!(
                !name.to_lowercase().contains("duress"),
                "the filename names the thing it is meant to hide: {}",
                name
            );

            let data = std::fs::read(f).unwrap();
            // Same wire version...
            assert_eq!(seed_crypt::format_version(&data), Some(2), "{}", name);
            // ...and the same declared KDF cost, byte for byte. If these ever
            // differ, the marker becomes the cheap oracle again.
            let header = &data[seed_crypt::MARKER_LEN
                ..seed_crypt::MARKER_LEN + seed_crypt::HEADER_LEN];
            let other = std::fs::read(files.iter().find(|o| o != &f).unwrap()).unwrap();
            let other_header = &other[seed_crypt::MARKER_LEN
                ..seed_crypt::MARKER_LEN + seed_crypt::HEADER_LEN];
            assert_eq!(header, other_header, "KDF headers differ: {}", name);
        }

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rewrite_replaces_the_previous_pin() {
        let dir = tmp_dir("rewrite");
        write(&dir, NET, "cube-a", 1000, "9999", None).unwrap();
        write(&dir, NET, "cube-a", 1000, "8888", None).unwrap();
        assert!(!verify(&dir, NET, "cube-a", 1000, "9999", None));
        assert!(verify(&dir, NET, "cube-a", 1000, "8888", None));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn remove_is_idempotent_and_disarms() {
        let dir = tmp_dir("remove");
        write(&dir, NET, "cube-a", 1000, "9999", None).unwrap();
        assert!(exists(&dir, NET, "cube-a", 1000));
        remove(&dir, NET, "cube-a", 1000).unwrap();
        assert!(!exists(&dir, NET, "cube-a", 1000));
        // Second call on an already-clear Cube is not an error.
        remove(&dir, NET, "cube-a", 1000).unwrap();
        assert!(!verify(&dir, NET, "cube-a", 1000, "9999", None));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_name_is_deterministic_and_cube_specific() {
        assert_eq!(file_name("cube-a", 1000), file_name("cube-a", 1000));
        assert_ne!(file_name("cube-a", 1000), file_name("cube-b", 1000));
        assert_ne!(file_name("cube-a", 1000), file_name("cube-a", 1001));
        // Parses as a MnemonicFileName, like everything else in the folder.
        use std::str::FromStr;
        assert!(coincube_core::signer::MnemonicFileName::from_str(&file_name("cube-a", 1000)).is_ok());
    }

    #[test]
    fn marker_plaintext_is_not_a_valid_mnemonic() {
        // If it were, a mis-routed marker would look like a real (and
        // publicly-known) wallet.
        use std::str::FromStr;
        let as_str = std::str::from_utf8(MARKER_PLAINTEXT).unwrap();
        assert!(coincube_core::bip39::Mnemonic::from_str(as_str).is_err());
    }
}
