//! `knots_verify <archive> <SHA256SUMS> <SHA256SUMS.asc>`
//!
//! Exit 0 iff (1) the archive's SHA-256 is listed for exactly its file name in
//! `SHA256SUMS`, and (2) `SHA256SUMS.asc` carries at least one signature that
//! verifies, over `SHA256SUMS`, against the Knots signing key vendored in
//! `coincube-gui/assets/knots_signing_key.asc` (fingerprint re-derived and
//! pinned). This is the same trust anchor and the same rule the desktop
//! installer applies (`coincube-gui/src/installer/step/node/bitcoind.rs`,
//! `verify_detached_signature` / `hash_listed_in_manifest`), so the functional
//! tests run a Knots binary the app itself would have accepted — without
//! depending on a keyserver or a system `gpg`.

use std::path::Path;
use std::process::exit;

use pgp::composed::{Deserializable, SignedPublicKey, StandaloneSignature};
use pgp::types::KeyDetails;
use sha2::{Digest, Sha256};

/// Same pin as `coincube_gui::node::bitcoind::KNOTS_SIGNING_KEY_FINGERPRINT`.
const KNOTS_SIGNING_KEY_FINGERPRINT: &str = "1A3E761F19D2CC7785C5502EA291A2C45D0C504A";
/// The vendored key itself, read from the single copy the desktop ships.
const KNOTS_SIGNING_KEY_ASC: &str =
    include_str!("../../../../coincube-gui/assets/knots_signing_key.asc");

fn hash_listed_in_manifest(bytes: &[u8], archive_filename: &str, sha256sums: &str) -> bool {
    let bytes_hash = hex::encode(Sha256::digest(bytes));
    sha256sums.lines().any(|line| {
        let mut fields = line.split_whitespace();
        matches!(
            (fields.next(), fields.next()),
            (Some(hash), Some(name))
                if name == archive_filename && hash.eq_ignore_ascii_case(&bytes_hash)
        )
    })
}

/// Same rule as the installer's `verify_detached_signature`: the armored key
/// must re-derive to `expected_fingerprint`, and at least one signature in
/// `asc` must verify over `data` against that key or a subkey it has bound.
fn verify_detached_signature(
    data: &[u8],
    asc: &str,
    pubkey_armored: &str,
    expected_fingerprint: &str,
) -> Result<(), String> {
    if !asc
        .trim_start()
        .starts_with("-----BEGIN PGP SIGNATURE-----")
    {
        return Err("no PGP signature block".into());
    }
    let (pubkey, _) = SignedPublicKey::from_string(pubkey_armored)
        .map_err(|e| format!("vendored key unreadable: {e}"))?;
    if !hex::encode(pubkey.fingerprint().as_bytes()).eq_ignore_ascii_case(expected_fingerprint) {
        return Err("vendored key fingerprint does not match the pin".into());
    }
    let bound_subkeys: Vec<_> = pubkey
        .public_subkeys
        .iter()
        .filter(|sub| sub.verify(&pubkey.primary_key).is_ok())
        .collect();
    let (sig_iter, _) = StandaloneSignature::from_string_many(asc)
        .map_err(|e| format!("signature block unreadable: {e}"))?;
    let signatures: Vec<StandaloneSignature> = sig_iter.flatten().collect();
    let verifies = signatures.iter().any(|sig| {
        sig.verify(&pubkey, data).is_ok()
            || bound_subkeys
                .iter()
                .any(|sub| sig.verify(*sub, data).is_ok())
    });
    if verifies {
        Ok(())
    } else {
        Err(format!(
            "none of the {} signature(s) verify against {}",
            signatures.len(),
            expected_fingerprint
        ))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: knots_verify <archive> <SHA256SUMS> <SHA256SUMS.asc>");
        exit(2);
    }
    let archive = Path::new(&args[1]);
    let archive_name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let archive_bytes = std::fs::read(archive).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", archive.display());
        exit(2)
    });
    let sums_bytes = std::fs::read(&args[2]).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", args[2]);
        exit(2)
    });
    let asc = std::fs::read_to_string(&args[3]).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", args[3]);
        exit(2)
    });

    if let Err(e) = verify_detached_signature(
        &sums_bytes,
        &asc,
        KNOTS_SIGNING_KEY_ASC,
        KNOTS_SIGNING_KEY_FINGERPRINT,
    ) {
        eprintln!("SHA256SUMS.asc: {e}");
        exit(1);
    }
    let sums = String::from_utf8_lossy(&sums_bytes);
    if !hash_listed_in_manifest(&archive_bytes, archive_name, &sums) {
        eprintln!("{archive_name}: SHA-256 not listed in SHA256SUMS");
        exit(1);
    }
    println!(
        "ok: {archive_name} listed in SHA256SUMS; SHA256SUMS.asc verified against {}",
        KNOTS_SIGNING_KEY_FINGERPRINT
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // The real, multi-maintainer-signed manifest of the fork release, as
    // published at bitcoinknots.org/files/29.x/29.5.1.knots20260508/.
    const SUMS: &[u8] = include_bytes!("../tests/fixtures/29.5.1.knots20260508.SHA256SUMS");
    const ASC: &str = include_str!("../tests/fixtures/29.5.1.knots20260508.SHA256SUMS.asc");

    #[test]
    fn real_manifest_verifies_against_vendored_key() {
        verify_detached_signature(
            SUMS,
            ASC,
            KNOTS_SIGNING_KEY_ASC,
            KNOTS_SIGNING_KEY_FINGERPRINT,
        )
        .unwrap();
    }

    #[test]
    fn tampered_manifest_does_not_verify() {
        let mut tampered = SUMS.to_vec();
        tampered.extend_from_slice(
            b"\n0000000000000000000000000000000000000000000000000000000000000000  extra\n",
        );
        let err = verify_detached_signature(
            &tampered,
            ASC,
            KNOTS_SIGNING_KEY_ASC,
            KNOTS_SIGNING_KEY_FINGERPRINT,
        )
        .unwrap_err();
        assert!(err.contains("none of the"), "{err}");
    }

    #[test]
    fn single_flipped_byte_does_not_verify() {
        let mut flipped = SUMS.to_vec();
        flipped[0] ^= 0x01;
        assert!(verify_detached_signature(
            &flipped,
            ASC,
            KNOTS_SIGNING_KEY_ASC,
            KNOTS_SIGNING_KEY_FINGERPRINT
        )
        .is_err());
    }

    #[test]
    fn wrong_pin_rejects_the_vendored_key() {
        let err = verify_detached_signature(
            SUMS,
            ASC,
            KNOTS_SIGNING_KEY_ASC,
            "0000000000000000000000000000000000000000",
        )
        .unwrap_err();
        assert!(err.contains("fingerprint"), "{err}");
    }

    #[test]
    fn missing_signature_block_is_reported_as_such() {
        for asc in [
            "",
            "not a signature",
            "-----BEGIN PGP PUBLIC KEY BLOCK-----",
        ] {
            let err = verify_detached_signature(
                SUMS,
                asc,
                KNOTS_SIGNING_KEY_ASC,
                KNOTS_SIGNING_KEY_FINGERPRINT,
            )
            .unwrap_err();
            assert_eq!(err, "no PGP signature block");
        }
    }

    #[test]
    fn garbled_signature_block_is_invalid() {
        let garbled = "-----BEGIN PGP SIGNATURE-----\n\nnot-base64!\n-----END PGP SIGNATURE-----\n";
        assert!(verify_detached_signature(
            SUMS,
            garbled,
            KNOTS_SIGNING_KEY_ASC,
            KNOTS_SIGNING_KEY_FINGERPRINT
        )
        .is_err());
    }

    #[test]
    fn manifest_lists_hash_for_exact_filename_only() {
        let bytes = b"release archive bytes";
        let digest = hex::encode(Sha256::digest(bytes));
        let manifest = format!(
            "{digest}  bitcoin-x.tar.gz\n{}  other.tar.gz\n",
            "ab".repeat(32)
        );
        assert!(hash_listed_in_manifest(
            bytes,
            "bitcoin-x.tar.gz",
            &manifest
        ));
        assert!(!hash_listed_in_manifest(bytes, "other.tar.gz", &manifest));
        assert!(!hash_listed_in_manifest(bytes, "renamed.tar.gz", &manifest));
        assert!(!hash_listed_in_manifest(
            b"different bytes",
            "bitcoin-x.tar.gz",
            &manifest
        ));
        // Hex case does not matter; whitespace shape follows GNU coreutils.
        let upper = manifest
            .to_uppercase()
            .replace("BITCOIN-X.TAR.GZ", "bitcoin-x.tar.gz");
        assert!(hash_listed_in_manifest(bytes, "bitcoin-x.tar.gz", &upper));
    }

    #[test]
    fn real_archive_names_are_listed_in_the_real_manifest() {
        let sums = String::from_utf8_lossy(SUMS);
        for name in [
            "bitcoin-29.5.1.knots20260508-x86_64-linux-gnu.tar.gz",
            "bitcoin-29.5.1.knots20260508-arm64-apple-darwin.tar.gz",
        ] {
            assert!(
                sums.lines().any(|l| l.ends_with(&format!("  {name}"))),
                "{name}"
            );
        }
    }
}
