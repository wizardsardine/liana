//! GUI delegation to the core's opaque imported-account authority.
//! This service never accepts or exposes a raw master private key.

use coincube_core::miniscript::bitcoin::psbt::Psbt;
#[cfg(test)]
use coincube_core::{
    imported_keys::VerifiedImportedKey, wallet_descriptor::ImportedWalletDescriptor,
};

#[cfg(test)]
pub use coincube_core::imported_keys::ImportedKeyError as ImportedSignError;

#[cfg(test)]
pub(crate) fn sign_imported_psbt(
    wallet: &ImportedWalletDescriptor,
    original: &Psbt,
    current: &Psbt,
    key: &VerifiedImportedKey,
) -> Result<Psbt, ImportedSignError> {
    key.sign(wallet, original, current)
}

/// The imported wallet's keys stored in the unlocked Cube, as the Vault's
/// signer picker offers them. Reads only public records. Restored keys are
/// left out: they sign only with current Connect authorization.
pub(crate) fn cube_key_records(
    datadir: &crate::dir::CoincubeDirectory,
    chain: crate::chain::ChainId,
) -> Result<Vec<crate::services::imported_keys::LocalKeyRecord>, String> {
    with_repository(datadir, chain, |repository| {
        repository.local_signing_records()
    })
}

/// Sign `current` with each of `records` in turn: every stored account
/// behind one master fingerprint. An account that has already signed adds
/// nothing and is skipped, so a later account can still add its signature;
/// the batch fails only when no account added one, or on any real failure.
pub(crate) fn sign_with_cube_keys(
    datadir: &crate::dir::CoincubeDirectory,
    chain: crate::chain::ChainId,
    records: &[crate::services::imported_keys::LocalKeyRecord],
    current: &Psbt,
) -> Result<Psbt, String> {
    sign_each(records, current, |record, psbt| {
        sign_with_cube_key(datadir, chain, record, psbt)
    })
}

/// Fold `sign` over `records`, carrying each result forward. `Ok(None)` is a
/// verified account with nothing to add; at least one must contribute.
fn sign_each<R>(
    records: &[R],
    current: &Psbt,
    mut sign: impl FnMut(&R, &Psbt) -> Result<Option<Psbt>, String>,
) -> Result<Psbt, String> {
    let mut psbt = current.clone();
    let mut contributed = false;
    for record in records {
        if let Some(signed) = sign(record, &psbt)? {
            psbt = signed;
            contributed = true;
        }
    }
    if contributed {
        Ok(psbt)
    } else {
        Err("Imported signing failed verification.".into())
    }
}

/// Sign `current` with one stored key. The unsigned construction is `current`
/// without its signatures; the key checks that every earlier signature is
/// kept and the transaction is unchanged. `Ok(None)` when the key verified
/// but had no signature left to add.
pub(crate) fn sign_with_cube_key(
    datadir: &crate::dir::CoincubeDirectory,
    chain: crate::chain::ChainId,
    record: &crate::services::imported_keys::LocalKeyRecord,
    current: &Psbt,
) -> Result<Option<Psbt>, String> {
    let mut original = current.clone();
    for input in &mut original.inputs {
        input.partial_sigs.clear();
        input.final_script_sig = None;
        input.final_script_witness = None;
    }
    with_repository(datadir, chain, |repository| {
        repository.sign_contribution(
            record.record_id,
            record.revision,
            &original,
            current,
            || true,
        )
    })
}

fn with_repository<T>(
    datadir: &crate::dir::CoincubeDirectory,
    chain: crate::chain::ChainId,
    run: impl FnOnce(&crate::services::imported_keys::LocalKeyRepository<'_>) -> Result<T, String>,
) -> Result<T, String> {
    let cube_id =
        crate::app::session::current_cube_id().ok_or("Unlock this Cube before using its keys.")?;
    let directory = datadir.network_directory(chain);
    let settings = crate::app::settings::Settings::from_file(&directory)
        .map_err(|_| "Cannot read this Cube's settings.".to_string())?;
    let cube = settings
        .cubes
        .iter()
        .find(|cube| cube.id == cube_id)
        .ok_or("This Cube no longer exists.")?;
    let attachment = cube
        .bitcoin_import
        .as_deref()
        .and_then(crate::app::settings::BitcoinImport::active_attachment)
        .ok_or("This Cube has no imported wallet.")?;
    let repository =
        crate::services::imported_keys::LocalKeyRepository::new(&directory, cube, attachment)?;
    run(&repository)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod sign_each_tests {
    use super::sign_each;
    use coincube_core::miniscript::bitcoin::{
        absolute::LockTime, psbt::Psbt, transaction::Version, Transaction,
    };

    fn psbt(lock: u32) -> Psbt {
        Psbt::from_unsigned_tx(Transaction {
            version: Version::TWO,
            lock_time: LockTime::from_consensus(lock),
            input: vec![],
            output: vec![],
        })
        .unwrap()
    }

    #[test]
    fn an_account_that_already_signed_does_not_stop_the_next() {
        let calls = std::cell::Cell::new(0);
        let signed = sign_each(&[1, 2], &psbt(0), |record, _| {
            calls.set(calls.get() + 1);
            Ok((*record == 2).then(|| psbt(2)))
        })
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(signed, psbt(2));
    }

    #[test]
    fn no_contribution_from_any_account_is_an_error() {
        assert!(sign_each(&[1, 2], &psbt(0), |_, _| Ok(None)).is_err());
    }

    #[test]
    fn a_real_failure_still_stops_the_batch() {
        let calls = std::cell::Cell::new(0);
        let result = sign_each(&[1, 2], &psbt(0), |_, _| {
            calls.set(calls.get() + 1);
            Err("bad key".to_string())
        });
        assert_eq!(result.unwrap_err(), "bad key");
        assert_eq!(calls.get(), 1);
    }
}
