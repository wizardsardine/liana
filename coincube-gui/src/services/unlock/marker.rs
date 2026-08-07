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
//! It is also named like a master-seed file, with a random fingerprint drawn
//! at arm time. Someone who copies the datadir sees two AES-GCM blobs of
//! identical size with identical KDF parameters and matching filename grammar.
//! Nothing in the file, its name, its length or its metadata says "duress".
//!
//! # Why the name is random, and recorded
//!
//! It used to be `SHA-256("coincube/duress-marker/v1" ‖ cube_id)` truncated to
//! four bytes, plus `created_at` — chosen so the file could be found again
//! without recording its name, on the reasoning that recording it would be the
//! tell. That was backwards. Both inputs sit in plaintext in `settings.json`,
//! so anyone who could read the recorded name could equally well *compute* the
//! derived one and `stat` it. The derivation bought nothing and cost the
//! property outright: armed-vs-unarmed was decidable from the datadir alone.
//!
//! A random name is not decidable that way. What it did not close was the
//! *count*: a second blob in `mnemonics/` only existed on armed Cubes. Unit 6b
//! closes that — see below.
//!
//! # The second slot, and how the app tells a marker from a decoy
//!
//! **Every** Cube carries a second slot from creation. On an armed Cube it is
//! the marker, sealed under the duress PIN. On an unarmed one it is a *decoy*:
//! the same plaintext sealed under [`OsRng`] key material that is generated,
//! used once and dropped ([`write_decoy`]). No PIN opens it, because no PIN
//! was ever involved.
//!
//! That is the whole mechanism, and the answer to "how does the app tell them
//! apart": **it never does.** There is no type byte, no length difference, no
//! naming rule and no lookup table, because the app has no use for one. The
//! only question ever asked of the slot is `verify(pin)` — a trial decryption
//! — and a decoy answers `false` for every PIN by construction. Nothing
//! classifies the slot, so nothing an imager reads can classify it either. A
//! design that stored "this one is real" anywhere would be exactly the tell
//! this unit exists to remove, and the mistake would be invisible until
//! someone imaged a datadir.
//!
//! Arming overwrites the slot in place; disarming overwrites it with a fresh
//! decoy. The file count, the name, the size, the permissions and the mtime
//! are identical before and after, so the transition leaves no trace either.
//!
//! Two consequences worth stating:
//!
//! * **The armed/unarmed timing oracle closes.** [`verify`] used to return
//!   instantly on a Cube with no marker, so a wrong PIN cost one Argon2 pass
//!   on an unarmed Cube and two on an armed one. With a slot always present it
//!   is always two. This is the property [`verify`]'s own doc has been
//!   promising since the marker landed.
//! * **A decoy costs one Argon2 derivation to write.** It goes through the
//!   real [`seed_crypt::encrypt`] rather than emitting random bytes of the
//!   right length, so its header, wire version and envelope cannot drift from
//!   a real marker's — they are produced by the same call.
//!
//! # What this still does not hide
//!
//! Armed-ness remains readable from `duress-state.json` (`enrolled: true`) and
//! from the presence of `duress.key`, both plaintext at the **datadir root**.
//! An attacker who images the whole datadir learns that duress is enrolled
//! regardless of what `mnemonics/` looks like, and the decoy does not change
//! that. What the decoy buys is a `mnemonics/` folder that is uniform on its
//! own — the case where an attacker sees the Cube's files without the root, or
//! is timing a running process rather than reading a disk. Closing the root
//! files is a separate change and is not in this plan.

use std::path::{Path, PathBuf};

use coincube_core::miniscript::bitcoin::{secp256k1::rand::RngCore, Network};
use coincube_core::seed_crypt::{self, DeviceSecret};
use coincube_core::signer::{MasterSigner, MnemonicFileName, MASTER_SEED_LABEL};

use super::UnlockError;

/// The sealed plaintext.
///
/// Fixed and known — its secrecy is not what protects anything; the point is
/// that only the duress PIN produces a valid GCM tag over it.
///
/// Its length no longer matters: [`seed_crypt`] pads every plaintext to one
/// fixed envelope, so this and a 24-word mnemonic seal to the same number of
/// bytes. It used to be hand-tuned to 93 bytes to match *a common 12-word*
/// mnemonic, which left it distinguishable from every other Cube shape.
///
/// It is deliberately **not** a valid BIP39 mnemonic: a real one would be
/// tidier still, but a publicly-known valid mnemonic is a wallet, and a wallet
/// is somewhere a confused user could send funds.
const MARKER_PLAINTEXT: &[u8] =
    b"coincube/duress-marker/v1 -- this file is not a seed phrase and holds no key material";

/// Mint a fresh marker file name, shaped exactly like a master-seed file
/// (`mnemonic-<8 hex>-master_<ts>-<ts>.txt`).
///
/// The four leading bytes are random rather than derived, so the name cannot
/// be recomputed from anything in `settings.json`. They occupy the slot a real
/// `Fingerprint` occupies and are the same width, so the two names are the
/// same grammar and the same length.
///
/// `seed_timestamp` must be the timestamp already on this Cube's seed file —
/// see [`seed_timestamp`]. A marker stamped with anything else is a
/// distinguisher on its own.
pub fn new_file_name(seed_timestamp: i64) -> String {
    let mut fp_bytes = [0u8; 4];
    coincube_core::miniscript::bitcoin::secp256k1::rand::thread_rng().fill_bytes(&mut fp_bytes);
    let fp: String = fp_bytes.iter().map(|b| format!("{:02x}", b)).collect();
    format!(
        "mnemonic-{}-{}{}-{}.txt",
        fp, MASTER_SEED_LABEL, seed_timestamp, seed_timestamp
    )
}

/// Domain tag for the **pre-6a** derived marker name. Kept only so migration
/// can recognise and remove those files; nothing writes this scheme any more.
const LEGACY_MARKER_FP_DOMAIN: &[u8] = b"coincube/duress-marker/v1";

/// The name a marker would have had before unit 6a made names random.
///
/// `SHA-256("coincube/duress-marker/v1" ‖ cube_id)` truncated to four bytes,
/// plus `created_at` twice. Both inputs sit in plaintext in `settings.json`,
/// which is exactly why the scheme was replaced: anyone who could read that
/// file could compute this name and learn that duress was armed.
///
/// It survives as a *detector*. A legacy marker's name was never recorded
/// anywhere, so `duress_slot_file` is `None` for those Cubes and nothing else
/// can find the file — [`crate::services::unlock::ensure_second_slot`] would
/// mint a second slot beside it and leave the Cube with three blobs. Migration
/// recomputes this, removes what it finds, and lets the backfill produce a
/// correctly-shaped slot.
pub fn legacy_file_name(cube_id: &str, cube_created_at: i64) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(LEGACY_MARKER_FP_DOMAIN);
    hasher.update(cube_id.as_bytes());
    let digest = hasher.finalize();
    let fp: String = digest[..4].iter().map(|b| format!("{:02x}", b)).collect();
    format!(
        "mnemonic-{}-{}{}-{}.txt",
        fp, MASTER_SEED_LABEL, cube_created_at, cube_created_at
    )
}

/// The timestamp on this Cube's master-seed file, for the marker to share.
///
/// **X7.** The marker used to stamp itself with `cube.created_at` while the
/// seed file stamped itself with `Utc::now()` at store time — two separate
/// clock reads with an Argon2 derivation between them. On a slow machine they
/// differ, and the file whose two timestamps match `created_at` in
/// `settings.json` exactly is the marker. Creation now takes one timestamp for
/// both (see `home.rs::finalize_cube_creation`), but Cubes created before that
/// still have the split, so read the real value off disk rather than assume.
///
/// Falls back to `fallback` (the Cube's `created_at`) when no seed file can be
/// identified — a Cube whose seed lives on another device, for instance.
pub fn seed_timestamp(
    datadir_root: &Path,
    network: Network,
    master_signer_fingerprint: Option<coincube_core::miniscript::bitcoin::bip32::Fingerprint>,
    fallback: i64,
) -> i64 {
    use std::str::FromStr;

    let Some(fp) = master_signer_fingerprint else {
        return fallback;
    };
    let folder = MasterSigner::mnemonics_folder(datadir_root, network);
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return fallback;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(|n| n.to_owned()) else {
            continue;
        };
        let Ok(parsed) = MnemonicFileName::from_str(&name) else {
            continue;
        };
        if parsed.fingerprint != fp {
            continue;
        }
        if let Some((checksum, ts)) = parsed.descriptor_info {
            if checksum.starts_with(MASTER_SEED_LABEL) {
                return ts;
            }
        }
    }
    fallback
}

/// Full path to a marker with this file name.
pub fn path(datadir_root: &Path, network: Network, file_name: &str) -> PathBuf {
    MasterSigner::mnemonics_folder(datadir_root, network).join(file_name)
}

/// Whether this Cube's second slot exists on disk.
///
/// **This is not "is duress armed".** Since unit 6b every Cube carries a slot
/// from creation, decoy or marker, so this is true for almost every Cube and
/// says nothing about enrolment — that lives in
/// [`crate::services::duress::DuressLocalState`]. Deciding armed-ness from a
/// file's presence is exactly the oracle the decoy removes; anything that
/// wants the answer must ask the state file.
///
/// It stays useful for the one question it does answer: does this Cube still
/// need a slot backfilled (migration, restore)?
pub fn exists(datadir_root: &Path, network: Network, file_name: Option<&str>) -> bool {
    file_name.is_some_and(|name| path(datadir_root, network, name).exists())
}

/// Write this Cube's slot as a **decoy** — indistinguishable from a marker,
/// openable by nothing.
///
/// The plaintext and the code path are the real ones; only the passphrase
/// differs, and it is 32 bytes of [`OsRng`] output that is dropped before this
/// returns. So the blob has a genuine header at the Cube's own wire version, a
/// genuine salt and nonce, and a genuine GCM tag — it simply has no key in the
/// world that opens it.
///
/// Emitting random bytes of the right length would be cheaper (this pays one
/// Argon2 derivation) and would be wrong: the header, version and envelope
/// framing would then be *reimplemented* next to the real ones, free to drift
/// apart on the next format change. Sharing the call makes drift impossible.
///
/// `device_secret` must be the Cube's own — a v3 Cube's decoy must be v3, or
/// the wire version singles it out.
pub fn write_decoy(
    datadir_root: &Path,
    network: Network,
    cube_id: &str,
    file_name: &str,
    device_secret: Option<&DeviceSecret>,
) -> Result<(), UnlockError> {
    use coincube_core::miniscript::bitcoin::secp256k1::rand::rngs::OsRng;

    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(key.as_mut());
    let passphrase =
        zeroize::Zeroizing::new(key.iter().map(|b| format!("{:02x}", b)).collect::<String>());
    write(
        datadir_root,
        network,
        cube_id,
        file_name,
        &passphrase,
        device_secret,
    )
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
    file_name: &str,
    duress_pin: &str,
    device_secret: Option<&DeviceSecret>,
) -> Result<(), UnlockError> {
    let folder = MasterSigner::mnemonics_folder(datadir_root, network);
    std::fs::create_dir_all(&folder).map_err(|e| UnlockError::Io(e.to_string()))?;

    let blob = seed_crypt::encrypt(MARKER_PLAINTEXT, duress_pin, cube_id, device_secret)
        .map_err(|e| UnlockError::Io(e.to_string()))?;

    let target = folder.join(file_name);

    // Write beside, flush to disk, rename over, then tighten permissions.
    //
    // The permission step is not hygiene — it is part of the indistinguishability
    // claim. Seed files are created `0o400` by `signer::create_file`. A marker
    // written with `std::fs::write` lands at `0o666 & !umask`, i.e. `0o644` on a
    // stock system, so a single `ls -l` of the mnemonics folder would pick the
    // duress marker out of a line-up that the wire format, the KDF parameters
    // and the filename grammar were all carefully built to make uniform (I2).
    // Metadata is part of the on-disk shape.
    let tmp = target.with_extension("tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).map_err(|e| UnlockError::Io(e.to_string()))?;
        f.write_all(&blob)
            .map_err(|e| UnlockError::Io(e.to_string()))?;
        // Without this the rename can be durable while the contents are not, so
        // a crash could leave a Cube armed with an empty marker that no PIN
        // opens — silently disarming duress on that Cube.
        f.sync_all().map_err(|e| UnlockError::Io(e.to_string()))?;
    }
    // Re-enrolling with a new duress PIN overwrites an existing marker, which by
    // then is read-only; on Windows that makes the rename fail. Harden *after*
    // the rename rather than on the temp file, so the rename's source is never
    // read-only either — see the matching note in `rewrite_file`.
    super::allow_overwrite(&target);
    std::fs::rename(&tmp, &target).map_err(|e| UnlockError::Io(e.to_string()))?;
    super::restrict_permissions(&target);
    match_seed_mtime(&folder, &target);
    Ok(())
}

/// Give the marker the same mtime as the Cube's seed files.
///
/// Permissions, size, wire format and filename grammar are all uniform by
/// now; without this, `ls -lT` still sorts the odd one out. A marker armed
/// weeks after the Cube was created carries that date, and "the file written
/// long after the others" is the whole answer.
///
/// Best-effort: a filesystem that refuses the update leaves a weaker property,
/// not a broken marker, so failures are swallowed rather than failing an
/// enrolment the user asked for. The seed file's mtime is the target because
/// it is the one an attacker compares against.
fn match_seed_mtime(folder: &Path, target: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let reference = entries
        .flatten()
        .filter(|e| e.path() != target)
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .filter_map(|m| m.modified().ok())
        .min();
    let Some(mtime) = reference else {
        return;
    };
    // The file is 0o400 by now; opening it for write needs the permission back
    // briefly, and `set_modified` needs a writable handle.
    super::allow_overwrite(target);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o600));
    }
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(target) {
        let _ = f.set_modified(mtime);
    }
    super::restrict_permissions(target);
}

/// Disarm duress on this Cube. Idempotent — a missing marker, or a Cube with
/// no recorded marker name, is success.
pub fn remove(
    datadir_root: &Path,
    network: Network,
    file_name: Option<&str>,
) -> Result<(), UnlockError> {
    let Some(file_name) = file_name else {
        return Ok(());
    };
    let target = path(datadir_root, network, file_name);
    super::allow_overwrite(&target);
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
/// # Timing
///
/// Every Cube carries a second slot from creation — [`write_decoy`] when duress
/// is not armed, [`write()`] when it is (unit 6b) — so this costs exactly one
/// Argon2id derivation at the seed file's parameters either way.
///
/// That is what keeps **wrong-vs-duress** indistinguishable on an armed Cube
/// (I2), and it is also what closed the older **armed-vs-unarmed** oracle: an
/// unarmed Cube pays the same derivation on a mistyped PIN, and `mnemonics/`
/// holds the same number of blobs, so neither channel separates the two. Unit
/// 6a had already made the name random and recorded rather than derivable, so
/// there is nothing left to `stat` for either.
///
/// One case is left, and it is transitional. A Cube with **no recorded slot** —
/// minted before 6b, or restored from a Recovery Kit — returns immediately
/// here, which marks it out as a Cube that cannot be armed. That lasts until
/// [`super::ensure_second_slot`] backfills it, which runs on the Cube's next
/// successful unlock.
pub fn verify(
    datadir_root: &Path,
    network: Network,
    cube_id: &str,
    file_name: Option<&str>,
    pin: &str,
    device_secret: Option<&DeviceSecret>,
) -> bool {
    let Some(file_name) = file_name else {
        return false;
    };
    let target = path(datadir_root, network, file_name);
    let Ok(blob) = std::fs::read(&target) else {
        return false;
    };
    match seed_crypt::decrypt_with(&blob, pin, cube_id, device_secret) {
        Ok(plaintext) => plaintext.as_slice() == MARKER_PLAINTEXT,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;

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
    const TS: i64 = 1_700_000_000;

    /// Arm a Cube the way enrollment does, returning the recorded name.
    fn arm(dir: &Path, cube_id: &str, pin: &str) -> String {
        let name = new_file_name(TS);
        write(dir, NET, cube_id, &name, pin, None).unwrap();
        name
    }

    /// Write a real master-seed file so the folder holds what a Cube's folder
    /// actually holds. `word_count` picks between a 12-word Cube — what both
    /// creation paths produce — and one restored from a 24-word phrase.
    fn store_seed(dir: &Path, cube_id: &str, word_count: usize) -> String {
        use coincube_core::signer::MasterSigner;

        let secp = Secp256k1::signing_only();
        let signer = match word_count {
            12 => MasterSigner::generate(NET).unwrap(),
            // A 24-word mnemonic — the largest a seed file can hold. No Cube
            // shape produces one any more (both creation paths make 12 words),
            // but the envelope must still cover the longest phrase a restore
            // could ever carry in.
            24 => MasterSigner::from_mnemonic(
                NET,
                coincube_core::bip39::Mnemonic::from_entropy(&[0x5A; 32]).unwrap(),
            )
            .unwrap(),
            other => panic!("unsupported word count {}", other),
        };
        assert_eq!(
            signer.words().len(),
            word_count,
            "fixture did not produce a {}-word mnemonic",
            word_count
        );
        signer
            .store_encrypted(
                dir,
                NET,
                &secp,
                Some((format!("{}{}", MASTER_SEED_LABEL, TS), TS)),
                "1234",
                cube_id,
                None,
            )
            .unwrap();
        MnemonicFileName {
            fingerprint: signer.fingerprint(&secp),
            descriptor_info: Some((format!("{}{}", MASTER_SEED_LABEL, TS), TS)),
        }
        .to_string()
    }

    fn folder_entries(dir: &Path) -> Vec<PathBuf> {
        use coincube_core::signer::MasterSigner;
        std::fs::read_dir(MasterSigner::mnemonics_folder(dir, NET))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect()
    }

    // -----------------------------------------------------------------
    // Behaviour
    // -----------------------------------------------------------------

    #[test]
    fn write_then_verify_round_trip() {
        let dir = tmp_dir("roundtrip");
        let name = arm(&dir, "cube-a", "9999");

        assert!(verify(&dir, NET, "cube-a", Some(&name), "9999", None));
        assert!(!verify(&dir, NET, "cube-a", Some(&name), "1234", None));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn absent_marker_never_verifies() {
        // The default that matters: a Cube with no duress enrolled must not be
        // wipeable by guessing.
        let dir = tmp_dir("absent");
        assert!(!exists(&dir, NET, None));
        let unarmed = new_file_name(TS);
        assert!(!exists(&dir, NET, Some(&unarmed)));
        for pin in ["0000", "1234", "9999", ""] {
            assert!(!verify(&dir, NET, "cube-a", None, pin, None));
            assert!(!verify(&dir, NET, "cube-a", Some(&unarmed), pin, None));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn marker_is_bound_to_its_cube() {
        let dir = tmp_dir("bound");
        let name = arm(&dir, "cube-a", "9999");
        // Same PIN, same file, different Cube: the AAD binding rejects it.
        assert!(!verify(&dir, NET, "cube-b", Some(&name), "9999", None));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rewrite_replaces_the_previous_pin() {
        let dir = tmp_dir("rewrite");
        let name = arm(&dir, "cube-a", "9999");
        // Re-enrolment reuses the recorded name, so the marker is replaced in
        // place rather than orphaned.
        write(&dir, NET, "cube-a", &name, "8888", None).unwrap();
        assert_eq!(folder_entries(&dir).len(), 1, "re-enrolment left an orphan");
        assert!(!verify(&dir, NET, "cube-a", Some(&name), "9999", None));
        assert!(verify(&dir, NET, "cube-a", Some(&name), "8888", None));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rewriting_a_hardened_marker_succeeds() {
        // The marker is written read-only. Re-enrolling has to replace it
        // anyway — on Windows a rename over a read-only destination fails with
        // ERROR_ACCESS_DENIED, so this is the regression guard for
        // `allow_overwrite`.
        let dir = tmp_dir("rewrite-hardened");
        let name = arm(&dir, "cube-a", "9999");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = path(&dir, NET, &name);
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o400
            );
        }
        write(&dir, NET, "cube-a", &name, "8888", None).expect("re-enrolment must replace it");
        assert!(verify(&dir, NET, "cube-a", Some(&name), "8888", None));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn remove_is_idempotent_and_disarms() {
        let dir = tmp_dir("remove");
        let name = arm(&dir, "cube-a", "9999");
        assert!(exists(&dir, NET, Some(&name)));
        remove(&dir, NET, Some(&name)).unwrap();
        assert!(!exists(&dir, NET, Some(&name)));
        // Second call on an already-clear Cube is not an error, and neither is
        // a Cube that never recorded a name.
        remove(&dir, NET, Some(&name)).unwrap();
        remove(&dir, NET, None).unwrap();
        assert!(!verify(&dir, NET, "cube-a", Some(&name), "9999", None));
        std::fs::remove_dir_all(dir).unwrap();
    }

    // -----------------------------------------------------------------
    // Indistinguishability — properties, not header equality
    // -----------------------------------------------------------------

    /// **Filename grammar.** The marker's name must parse as, and be shaped
    /// like, a master-seed file name — same prefix, same suffix, same
    /// fingerprint width, same timestamp pair.
    #[test]
    fn marker_name_is_grammatically_a_seed_name() {
        use std::str::FromStr;

        let dir = tmp_dir("grammar");
        let seed_name = store_seed(&dir, "cube-a", 12);
        let marker_name = new_file_name(TS);

        let seed = MnemonicFileName::from_str(&seed_name).expect("seed name parses");
        let marker = MnemonicFileName::from_str(&marker_name).expect("marker name must parse");

        assert_eq!(
            seed_name.len(),
            marker_name.len(),
            "names differ in length: {} vs {}",
            seed_name,
            marker_name
        );
        assert_eq!(
            seed.fingerprint.to_string().len(),
            marker.fingerprint.to_string().len(),
            "fingerprint fields differ in width"
        );
        assert_eq!(
            seed.descriptor_info, marker.descriptor_info,
            "the timestamp halves must match the seed file's exactly"
        );
        assert!(
            !marker_name.to_lowercase().contains("duress"),
            "the filename names the thing it is meant to hide: {}",
            marker_name
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// **Filename entropy.** The name must not be derivable from anything an
    /// attacker can read. The old scheme was
    /// `SHA-256(domain ‖ cube_id)` + `created_at`, so the same Cube always got
    /// the same name — this asserts that is gone.
    #[test]
    fn marker_name_is_unpredictable() {
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..256 {
            seen.insert(new_file_name(TS));
        }
        assert_eq!(
            seen.len(),
            256,
            "marker names repeat — the fingerprint is not random"
        );

        // Every distinct name differs only in the fingerprint field, and the
        // field spans the full 4-byte space rather than a corner of it.
        let firsts: std::collections::BTreeSet<char> = seen
            .iter()
            .filter_map(|n| n.strip_prefix("mnemonic-"))
            .filter_map(|n| n.chars().next())
            .collect();
        assert!(
            firsts.len() >= 10,
            "fingerprints cluster: only {} distinct leading hex digits",
            firsts.len()
        );
    }

    /// **Byte length**, for both Cube shapes. A 24-word Cube is the case the
    /// old hand-tuned 93-byte marker plaintext could never match.
    #[test]
    fn marker_and_seed_are_the_same_size_for_every_cube_shape() {
        for word_count in [12, 24] {
            let dir = tmp_dir(&format!("len-{}", word_count));
            let seed_name = store_seed(&dir, "cube-a", word_count);
            let marker_name = arm(&dir, "cube-a", "9999");

            let seed_len = std::fs::read(path(&dir, NET, &seed_name)).unwrap().len();
            let marker_len = std::fs::read(path(&dir, NET, &marker_name)).unwrap().len();
            assert_eq!(
                seed_len, marker_len,
                "a {}-word Cube's seed file is {} bytes but its marker is {}",
                word_count, seed_len, marker_len
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    /// The two Cube shapes are also the same size as *each other*, so the
    /// folder does not say whether this is a passkey Cube.
    #[test]
    fn twelve_and_twenty_four_word_cubes_are_the_same_size() {
        let a = tmp_dir("shape-12");
        let b = tmp_dir("shape-24");
        let twelve = std::fs::read(path(&a, NET, &store_seed(&a, "cube-a", 12)))
            .unwrap()
            .len();
        let twentyfour = std::fs::read(path(&b, NET, &store_seed(&b, "cube-b", 24)))
            .unwrap()
            .len();
        assert_eq!(
            twelve, twentyfour,
            "seed-file length still reveals the mnemonic length"
        );
        std::fs::remove_dir_all(a).unwrap();
        std::fs::remove_dir_all(b).unwrap();
    }

    /// **Permissions and mtime.** Metadata is part of the on-disk shape: a
    /// marker at 0o644 next to seed files at 0o400, or one carrying the date
    /// duress was armed, is identifiable from `ls -l` alone.
    #[test]
    fn marker_metadata_matches_the_seed_file() {
        let dir = tmp_dir("metadata");
        let seed_name = store_seed(&dir, "cube-a", 12);

        // Age the seed file so "same mtime" cannot pass by both files simply
        // being written in the same instant.
        let seed_path = path(&dir, NET, &seed_name);
        let backdated =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(TS as u64);
        {
            super::super::allow_overwrite(&seed_path);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&seed_path, std::fs::Permissions::from_mode(0o600))
                    .unwrap();
            }
            let f = std::fs::OpenOptions::new()
                .write(true)
                .open(&seed_path)
                .unwrap();
            f.set_modified(backdated).unwrap();
            super::super::restrict_permissions(&seed_path);
        }

        let marker_name = arm(&dir, "cube-a", "9999");
        let marker_path = path(&dir, NET, &marker_name);

        let seed_meta = std::fs::metadata(&seed_path).unwrap();
        let marker_meta = std::fs::metadata(&marker_path).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                seed_meta.permissions().mode() & 0o777,
                marker_meta.permissions().mode() & 0o777,
                "permissions differ: seed {:o}, marker {:o}",
                seed_meta.permissions().mode() & 0o777,
                marker_meta.permissions().mode() & 0o777
            );
            assert_eq!(
                marker_meta.permissions().mode() & 0o777,
                0o400,
                "the marker must be as locked down as a seed file"
            );
        }
        assert_eq!(
            seed_meta.modified().unwrap(),
            marker_meta.modified().unwrap(),
            "mtime gives away when duress was armed"
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// **Directory order and enumeration order.** Neither raw `read_dir` order
    /// nor sorted order may single the marker out, and nothing in the wire
    /// format may either.
    #[test]
    fn neither_enumeration_nor_content_singles_out_the_marker() {
        let dir = tmp_dir("order");
        let seed_name = store_seed(&dir, "cube-a", 12);
        let marker_name = arm(&dir, "cube-a", "9999");

        let entries = folder_entries(&dir);
        assert_eq!(entries.len(), 2, "seed file and marker");

        // Sorted order is decided by the random fingerprint, so the marker is
        // as likely to come first as second — nothing positional to read.
        let mut names: Vec<String> = entries
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_owned())
            .collect();
        names.sort();
        assert!(names.contains(&seed_name));
        assert!(names.contains(&marker_name));
        assert!(
            names.iter().all(|n| n.starts_with("mnemonic-")),
            "an entry breaks the grammar: {:?}",
            names
        );

        // Same wire version and the same declared KDF cost, byte for byte. If
        // these ever differ, the marker becomes the cheap oracle again.
        let seed = std::fs::read(path(&dir, NET, &seed_name)).unwrap();
        let marker = std::fs::read(path(&dir, NET, &marker_name)).unwrap();
        assert_eq!(
            seed_crypt::format_version(&seed),
            seed_crypt::format_version(&marker),
            "wire versions differ"
        );
        let header = |b: &[u8]| {
            b[seed_crypt::MARKER_LEN..seed_crypt::MARKER_LEN + seed_crypt::HEADER_LEN].to_vec()
        };
        assert_eq!(header(&seed), header(&marker), "KDF headers differ");

        // And no plaintext tell anywhere in the bytes.
        assert!(
            !marker.windows(6).any(|w| w.eq_ignore_ascii_case(b"duress")),
            "the marker's ciphertext leaks the word it is named for"
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// **X7.** The marker takes the seed file's timestamp, not the Cube's
    /// `created_at`, so a slow creation cannot drive them apart.
    #[test]
    fn marker_timestamp_is_read_from_the_seed_file() {
        use std::str::FromStr;

        let dir = tmp_dir("x7");
        let seed_name = store_seed(&dir, "cube-a", 12);
        let seed = MnemonicFileName::from_str(&seed_name).unwrap();
        let fingerprint = seed.fingerprint;

        // `created_at` deliberately disagrees with the seed file, as it does on
        // any Cube created before the two reads were unified.
        let drifted_created_at = TS + 9;
        let resolved = seed_timestamp(&dir, NET, Some(fingerprint), drifted_created_at);
        assert_eq!(
            resolved, TS,
            "the marker would have been stamped {} while the seed file says {}",
            drifted_created_at, TS
        );

        // No fingerprint, or no matching file: fall back rather than guess.
        assert_eq!(
            seed_timestamp(&dir, NET, None, drifted_created_at),
            drifted_created_at
        );
        assert_eq!(
            seed_timestamp(
                &dir,
                NET,
                Some(
                    coincube_core::miniscript::bitcoin::bip32::Fingerprint::from([
                        0xDE, 0xAD, 0xBE, 0xEF
                    ])
                ),
                drifted_created_at
            ),
            drifted_created_at
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    // -----------------------------------------------------------------
    // Unit 6b — the decoy slot
    // -----------------------------------------------------------------

    /// **The headline property.** An unarmed Cube and an armed one are
    /// byte-indistinguishable on disk: same file count, same name grammar,
    /// same length, same permissions, same mtime, same enumeration order, and
    /// nothing in the wire format that differs.
    ///
    /// Only the ciphertext differs, and AES-GCM output is pseudorandom, so
    /// that difference carries no information.
    #[test]
    fn an_unarmed_cube_is_indistinguishable_from_an_armed_one() {
        let unarmed = tmp_dir("shape-unarmed");
        let armed = tmp_dir("shape-armed");

        // Same Cube shape on both sides; only the slot's contents differ.
        let unarmed_seed = store_seed(&unarmed, "cube-a", 12);
        let armed_seed = store_seed(&armed, "cube-a", 12);

        let unarmed_slot = new_file_name(TS);
        write_decoy(&unarmed, NET, "cube-a", &unarmed_slot, None).unwrap();
        let armed_slot = new_file_name(TS);
        write(&armed, NET, "cube-a", &armed_slot, "8765", None).unwrap();

        // File count.
        assert_eq!(
            folder_entries(&unarmed).len(),
            folder_entries(&armed).len(),
            "an unarmed Cube holds a different number of files than an armed one"
        );
        assert_eq!(folder_entries(&unarmed).len(), 2, "seed file and slot");

        // Name grammar and length.
        assert_eq!(
            unarmed_slot.len(),
            armed_slot.len(),
            "slot names differ in length: {} vs {}",
            unarmed_slot,
            armed_slot
        );
        use std::str::FromStr;
        assert!(MnemonicFileName::from_str(&unarmed_slot).is_ok());
        assert!(MnemonicFileName::from_str(&armed_slot).is_ok());

        let decoy_bytes = std::fs::read(path(&unarmed, NET, &unarmed_slot)).unwrap();
        let marker_bytes = std::fs::read(path(&armed, NET, &armed_slot)).unwrap();

        // Byte length, and the same length as the seed file beside it.
        assert_eq!(
            decoy_bytes.len(),
            marker_bytes.len(),
            "a decoy is {} bytes but a marker is {}",
            decoy_bytes.len(),
            marker_bytes.len()
        );
        assert_eq!(
            decoy_bytes.len(),
            std::fs::read(path(&unarmed, NET, &unarmed_seed))
                .unwrap()
                .len(),
            "the decoy is not the same size as its own Cube's seed file"
        );

        // Wire version and KDF header, byte for byte.
        assert_eq!(
            seed_crypt::format_version(&decoy_bytes),
            seed_crypt::format_version(&marker_bytes),
            "decoy and marker are different wire versions"
        );
        let header = |b: &[u8]| {
            b[seed_crypt::MARKER_LEN..seed_crypt::MARKER_LEN + seed_crypt::HEADER_LEN].to_vec()
        };
        assert_eq!(
            header(&decoy_bytes),
            header(&marker_bytes),
            "decoy and marker declare different KDF costs"
        );

        // Permissions and mtime, against each Cube's own seed file.
        for (dir, slot, seed) in [
            (&unarmed, &unarmed_slot, &unarmed_seed),
            (&armed, &armed_slot, &armed_seed),
        ] {
            let slot_meta = std::fs::metadata(path(dir, NET, slot)).unwrap();
            let seed_meta = std::fs::metadata(path(dir, NET, seed)).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    slot_meta.permissions().mode() & 0o777,
                    seed_meta.permissions().mode() & 0o777,
                    "slot and seed have different permissions in {:?}",
                    dir
                );
            }
            assert_eq!(
                slot_meta.modified().unwrap(),
                seed_meta.modified().unwrap(),
                "slot and seed have different mtimes in {:?}",
                dir
            );
        }

        // Enumeration order carries nothing: the slot sorts by its random
        // fingerprint, so it is as likely to come first as second.
        for dir in [&unarmed, &armed] {
            let mut names: Vec<String> = folder_entries(dir)
                .iter()
                .map(|p| p.file_name().unwrap().to_str().unwrap().to_owned())
                .collect();
            names.sort();
            assert!(
                names
                    .iter()
                    .all(|n| n.starts_with("mnemonic-") && n.ends_with(".txt")),
                "an entry breaks the grammar in {:?}: {:?}",
                dir,
                names
            );
        }

        std::fs::remove_dir_all(unarmed).unwrap();
        std::fs::remove_dir_all(armed).unwrap();
    }

    /// A decoy opens for nothing. It is sealed under discarded `OsRng` key
    /// material, so there is no PIN — not even the one that would open a real
    /// marker — that verifies against it.
    #[test]
    fn a_decoy_opens_for_no_pin() {
        let dir = tmp_dir("decoy-opens");
        let slot = new_file_name(TS);
        write_decoy(&dir, NET, "cube-a", &slot, None).unwrap();

        assert!(exists(&dir, NET, Some(&slot)), "the decoy must be on disk");
        for pin in ["0000", "1234", "8765", "9999", ""] {
            assert!(
                !verify(&dir, NET, "cube-a", Some(&slot), pin, None),
                "PIN {} opened a decoy",
                pin
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Two decoys never collide — the key material is drawn fresh each time,
    /// so a decoy is not a constant an attacker can recognise by sight.
    #[test]
    fn decoys_are_not_a_recognisable_constant() {
        let dir = tmp_dir("decoy-distinct");
        let a = new_file_name(TS);
        let b = new_file_name(TS + 1);
        write_decoy(&dir, NET, "cube-a", &a, None).unwrap();
        write_decoy(&dir, NET, "cube-a", &b, None).unwrap();

        let bytes_a = std::fs::read(path(&dir, NET, &a)).unwrap();
        let bytes_b = std::fs::read(path(&dir, NET, &b)).unwrap();
        assert_eq!(bytes_a.len(), bytes_b.len());
        assert_ne!(
            bytes_a, bytes_b,
            "two decoys are byte-identical — they would be recognisable as decoys"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// **Arming and disarming leave the on-disk shape unchanged.** The whole
    /// transition is an in-place overwrite of one file: same name, same size,
    /// same permissions, same mtime, same file count.
    #[test]
    fn arming_and_disarming_do_not_change_the_on_disk_shape() {
        let dir = tmp_dir("arm-disarm");
        let seed_name = store_seed(&dir, "cube-a", 12);
        let slot = new_file_name(TS);

        // Snapshot helper: everything an imager could compare.
        let shape = |tag: &str| {
            let mut names: Vec<String> = folder_entries(&dir)
                .iter()
                .map(|p| p.file_name().unwrap().to_str().unwrap().to_owned())
                .collect();
            names.sort();
            let meta = std::fs::metadata(path(&dir, NET, &slot)).unwrap_or_else(|e| {
                panic!("slot missing at stage '{}': {}", tag, e);
            });
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode() & 0o777
            };
            #[cfg(not(unix))]
            let mode = 0u32;
            (
                names,
                meta.len(),
                mode,
                meta.modified().unwrap(),
                std::fs::read(path(&dir, NET, &seed_name)).unwrap().len(),
            )
        };

        write_decoy(&dir, NET, "cube-a", &slot, None).unwrap();
        let unarmed = shape("unarmed");

        write(&dir, NET, "cube-a", &slot, "8765", None).unwrap();
        let armed = shape("armed");
        assert!(
            verify(&dir, NET, "cube-a", Some(&slot), "8765", None),
            "arming did not take effect"
        );
        assert_eq!(
            unarmed, armed,
            "arming changed the on-disk shape — the transition is visible"
        );

        write_decoy(&dir, NET, "cube-a", &slot, None).unwrap();
        let disarmed = shape("disarmed");
        assert!(
            !verify(&dir, NET, "cube-a", Some(&slot), "8765", None),
            "disarming left the duress PIN working"
        );
        assert_eq!(
            armed, disarmed,
            "disarming changed the on-disk shape — the transition is visible"
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The decoy's wire version tracks its Cube's seed file. A v3 Cube must
    /// not carry a v2 decoy, or the header alone singles it out.
    #[test]
    fn the_decoy_version_tracks_the_cubes_seed_version() {
        let secret: DeviceSecret = zeroize::Zeroizing::new([0x42u8; 32]);

        for (tag, device_secret, expected) in [("v2", None, 2u8), ("v3", Some(&secret), 3u8)] {
            let dir = tmp_dir(&format!("decoy-{}", tag));
            let slot = new_file_name(TS);
            write_decoy(&dir, NET, "cube-a", &slot, device_secret).unwrap();
            let bytes = std::fs::read(path(&dir, NET, &slot)).unwrap();
            assert_eq!(
                seed_crypt::format_version(&bytes),
                Some(expected),
                "a {} Cube's decoy is not {} on the wire",
                tag,
                tag
            );

            // …and a real marker at the same version is the same size.
            let marker_slot = new_file_name(TS);
            write(&dir, NET, "cube-a", &marker_slot, "8765", device_secret).unwrap();
            assert_eq!(
                bytes.len(),
                std::fs::read(path(&dir, NET, &marker_slot)).unwrap().len(),
                "decoy and marker differ in size at {}",
                tag
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
