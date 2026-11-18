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
/// behind one master fingerprint.
pub(crate) fn sign_with_cube_keys(
    datadir: &crate::dir::CoincubeDirectory,
    chain: crate::chain::ChainId,
    records: &[crate::services::imported_keys::LocalKeyRecord],
    current: &Psbt,
) -> Result<Psbt, String> {
    records.iter().try_fold(current.clone(), |psbt, record| {
        sign_with_cube_key(datadir, chain, record, &psbt)
    })
}

/// Sign `current` with one stored key. The unsigned construction is `current`
/// without its signatures; the key checks that every earlier signature is
/// kept and the transaction is unchanged.
pub(crate) fn sign_with_cube_key(
    datadir: &crate::dir::CoincubeDirectory,
    chain: crate::chain::ChainId,
    record: &crate::services::imported_keys::LocalKeyRecord,
    current: &Psbt,
) -> Result<Psbt, String> {
    let mut original = current.clone();
    for input in &mut original.inputs {
        input.partial_sigs.clear();
        input.final_script_sig = None;
        input.final_script_witness = None;
    }
    with_repository(datadir, chain, |repository| {
        repository.sign(
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
