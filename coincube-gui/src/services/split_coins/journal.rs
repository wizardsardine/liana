//! The Vault's split records, next to its daemon data. Display state only:
//! a record says which transaction this Cube built as a split, never that it
//! may be signed or broadcast.

use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::SplitRecord;
use crate::{
    app::{
        settings::{BitcoinImport, CubeSettings, WalletId},
        wallet::Wallet,
    },
    chain::ChainId,
    dir::CoincubeDirectory,
};

const FILE: &str = "splits.json";
const VERSION: u32 = 1;
/// Far more records than a Vault can accumulate inside their lifetime.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Record {
    version: u32,
    splits: Vec<SplitRecord>,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// Records for a Vault on `chain`. A missing, oversized or unreadable file,
/// and records for another chain, read as none.
pub fn load(data_dir: &Path, chain: ChainId) -> Vec<SplitRecord> {
    let file = path(data_dir);
    if std::fs::metadata(&file).map_or(true, |m| m.len() > MAX_BYTES) {
        return Vec::new();
    }
    match std::fs::read(&file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
    {
        Some(record) if record.version == VERSION => record
            .splits
            .into_iter()
            .filter(|split| split.chain == chain)
            .collect(),
        _ => {
            tracing::warn!("ignoring unreadable {}", file.display());
            Vec::new()
        }
    }
}

pub fn save(data_dir: &Path, splits: &[SplitRecord]) -> io::Result<()> {
    let bytes = serde_json::to_vec(&Record {
        version: VERSION,
        splits: splits.to_vec(),
    })
    .map_err(io::Error::other)?;
    crate::services::chain_status::store::write_atomically(&path(data_dir), &bytes)
}

/// The data directory of the twin Cube's wallet: the Cube on the twin chain
/// holding the same wallet. For a Vault that is the Vault with the same
/// descriptor; for an imported Vault, the imported wallet with the same
/// public branches (its identity differs by chain, so its checksum does too).
/// `None` unless exactly one matches.
pub fn twin_data_dir(datadir: &CoincubeDirectory, wallet: &Wallet) -> Option<PathBuf> {
    let twin = crate::services::entangled::twin_chain(wallet.chain)?;
    let network = datadir.network_directory(twin);
    let id = twin_wallet_id(datadir, wallet)?;
    Some(network.coincubed_data_directory(&id).path().to_path_buf())
}

/// The twin Cube's wallet id, as [`twin_data_dir`] finds it.
pub fn twin_wallet_id(datadir: &CoincubeDirectory, wallet: &Wallet) -> Option<WalletId> {
    let twin = crate::services::entangled::twin_chain(wallet.chain)?;
    let network = datadir.network_directory(twin);
    if !network
        .path()
        .join(crate::app::settings::SETTINGS_FILE_NAME)
        .is_file()
    {
        return None;
    }
    let settings = crate::app::settings::Settings::from_file(&network).ok()?;
    let mut matches = settings
        .cubes
        .iter()
        .filter(|cube| cube.network == twin)
        .filter_map(|cube| twin_of(cube, wallet, twin));
    let id = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(id)
}

/// `cube`'s wallet id if it holds the twin of `wallet` on `twin`.
fn twin_of(cube: &CubeSettings, wallet: &Wallet, twin: ChainId) -> Option<WalletId> {
    match wallet.main_descriptor.as_imported() {
        None => cube
            .vault_wallet_id
            .as_ref()
            .filter(|id| id.descriptor_checksum == wallet.descriptor_checksum)
            .cloned(),
        Some(mine) => {
            let attachment = cube
                .bitcoin_import
                .as_deref()
                .and_then(BitcoinImport::active_attachment)?;
            let theirs = &attachment.descriptor;
            (cube.vault_wallet_id.is_none()
                && theirs.chain() == twin
                && theirs.receive_descriptor() == mine.receive_descriptor()
                && theirs.change_descriptor() == mine.change_descriptor()
                && attachment.validate(twin).is_ok())
            .then(|| attachment.wallet_id.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::split_coins::Method;
    use coincube_core::miniscript::bitcoin::{OutPoint, Txid};
    use std::str::FromStr;

    /// Write `cubes` as the settings of `chain`'s network directory.
    fn write_cubes(root: &CoincubeDirectory, chain: ChainId, cubes: Vec<CubeSettings>) {
        let dir = root.network_directory(chain);
        std::fs::create_dir_all(dir.path()).unwrap();
        let settings = crate::app::settings::Settings {
            cubes,
            ..Default::default()
        };
        std::fs::write(
            dir.path().join(crate::app::settings::SETTINGS_FILE_NAME),
            serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();
    }

    /// The Cube holding `wallet` as a ready imported wallet.
    fn imported_cube(wallet: &Wallet) -> CubeSettings {
        let mut cube = CubeSettings::new_with_raw_id(
            uuid::Uuid::new_v4().to_string(),
            "Imported".into(),
            wallet.chain,
        );
        let attachment = crate::app::settings::ImportedWalletAttachment::new(
            wallet.main_descriptor.as_imported().unwrap().clone(),
            wallet.chain,
            0,
            20,
        )
        .unwrap();
        cube.bitcoin_import = Some(Box::new(BitcoinImport {
            operation_id: uuid::Uuid::new_v4(),
            stage: crate::app::settings::BitcoinImportStage::Ready { wallet: attachment },
            last_error: None,
        }));
        cube
    }

    /// An imported Vault's twin is the Cube on the twin chain holding the
    /// same public branches, whatever its identity there, from either side;
    /// another wallet, or two candidates, is no twin.
    #[test]
    fn an_imported_vault_finds_its_twin_from_either_chain() {
        use crate::services::split_test_wallets::{
            imported_fork_vault, imported_vault, Shape, SHAPES,
        };
        for shape in SHAPES {
            let path = std::env::temp_dir().join(format!("coincube-twin-{}", uuid::Uuid::new_v4()));
            let root = CoincubeDirectory::new(path.clone());
            let bitcoin = imported_vault(shape);
            let fork = imported_fork_vault(shape);
            assert_ne!(bitcoin.descriptor_checksum, fork.descriptor_checksum);
            assert_eq!(twin_data_dir(&root, &bitcoin), None, "no twin yet");

            let other = imported_fork_vault(if shape == Shape::Wpkh {
                Shape::Pkh
            } else {
                Shape::Wpkh
            });
            write_cubes(&root, ChainId::BitcoinBlake2b, vec![imported_cube(&other)]);
            assert_eq!(twin_data_dir(&root, &bitcoin), None, "another wallet");

            write_cubes(
                &root,
                ChainId::BitcoinBlake2b,
                vec![imported_cube(&other), imported_cube(&fork)],
            );
            write_cubes(&root, ChainId::Bitcoin, vec![imported_cube(&bitcoin)]);
            let expected = |wallet: &Wallet| {
                root.network_directory(wallet.chain)
                    .coincubed_data_directory(&wallet.id())
                    .path()
                    .to_path_buf()
            };
            assert_eq!(twin_data_dir(&root, &bitcoin), Some(expected(&fork)));
            assert_eq!(twin_wallet_id(&root, &bitcoin), Some(fork.id()));
            assert_eq!(twin_data_dir(&root, &fork), Some(expected(&bitcoin)));

            write_cubes(
                &root,
                ChainId::BitcoinBlake2b,
                vec![imported_cube(&fork), imported_cube(&fork)],
            );
            assert_eq!(twin_data_dir(&root, &bitcoin), None, "ambiguous");
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn round_trips_and_filters_by_chain() {
        let dir = std::env::temp_dir().join(format!("coincube-splits-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let txid = Txid::from_str(&"ab".repeat(32)).unwrap();
        let record = |chain| SplitRecord {
            txid,
            chain,
            method: Method::SplitKey,
            inputs: vec![OutPoint::new(txid, 1)],
            key: Some(OutPoint::new(txid, 2)),
            key_path: Some("00".into()),
            created_at: 1,
            confirmations: 3,
        };
        save(
            &dir,
            &[record(ChainId::Bitcoin), record(ChainId::BitcoinBlake2b)],
        )
        .unwrap();
        assert_eq!(load(&dir, ChainId::Bitcoin), vec![record(ChainId::Bitcoin)]);
        std::fs::write(path(&dir), b"{").unwrap();
        assert!(load(&dir, ChainId::Bitcoin).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
