//! Persistent public-wallet attachment. This never contains a seed or an unlock credential.
use coincube_core::wallet_descriptor::{ImportedWalletDescriptor, ImportedWalletError};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{CubeSettings, WalletId};
use crate::chain::ChainId;

/// The import operation belongs to one Cube for its entire lifetime. Retrying
/// must reuse this id, rather than creating or selecting another empty Cube.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BitcoinImport {
    pub operation_id: Uuid,
    pub stage: BitcoinImportStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case", deny_unknown_fields)]
pub enum BitcoinImportStage {
    PendingSource,
    Installing { wallet: ImportedWalletAttachment },
    Syncing { wallet: ImportedWalletAttachment },
    Ready { wallet: ImportedWalletAttachment },
}

/// Both public branches are retained independently of the daemon so an
/// interrupted installation and a Recovery Kit can reconstruct the same wallet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedWalletAttachment {
    pub wallet_id: WalletId,
    pub descriptor: ImportedWalletDescriptor,
    /// Unix seconds. Zero means scan from genesis, never "use import time".
    pub birthday: u32,
    /// Initial address discovery window on each branch. The daemon persists
    /// subsequent coverage and allocation; this value is not a scan result.
    pub initial_lookahead: u32,
}

impl BitcoinImport {
    pub fn pending() -> Self {
        Self {
            operation_id: Uuid::new_v4(),
            stage: BitcoinImportStage::PendingSource,
            last_error: None,
        }
    }

    /// Includes the staged attachment so restart repair can recover it.
    pub fn attachment(&self) -> Option<&ImportedWalletAttachment> {
        match &self.stage {
            BitcoinImportStage::PendingSource => None,
            BitcoinImportStage::Installing { wallet }
            | BitcoinImportStage::Syncing { wallet }
            | BitcoinImportStage::Ready { wallet } => Some(wallet),
        }
    }

    /// Only an installed attachment may be opened as a wallet.
    pub fn active_attachment(&self) -> Option<&ImportedWalletAttachment> {
        match &self.stage {
            BitcoinImportStage::Syncing { wallet } | BitcoinImportStage::Ready { wallet } => {
                Some(wallet)
            }
            _ => None,
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(
            self.stage,
            BitcoinImportStage::PendingSource | BitcoinImportStage::Installing { .. }
        )
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.stage, BitcoinImportStage::Ready { .. })
    }
}

impl ImportedWalletAttachment {
    pub fn new(
        descriptor: ImportedWalletDescriptor,
        chain: ChainId,
        birthday: u32,
        initial_lookahead: u32,
    ) -> Result<Self, String> {
        let attachment = Self {
            wallet_id: WalletId::for_imported(&descriptor, chain).map_err(|e| e.to_string())?,
            descriptor,
            birthday,
            initial_lookahead,
        };
        attachment.validate(chain)?;
        Ok(attachment)
    }

    /// Reject a mismatched identity before using it as a directory name.
    pub fn validate(&self, chain: ChainId) -> Result<(), String> {
        if self.wallet_id
            != WalletId::for_imported(&self.descriptor, chain).map_err(|e| e.to_string())?
        {
            return Err(
                "The imported wallet identity does not match its descriptors and chain.".into(),
            );
        }
        if self.initial_lookahead == 0 || self.initial_lookahead > 10_000 {
            return Err("Choose an initial address discovery window between 1 and 10,000.".into());
        }
        Ok(())
    }
}

impl CubeSettings {
    pub fn has_imported_bitcoin(&self) -> bool {
        self.bitcoin_import
            .as_deref()
            .and_then(BitcoinImport::active_attachment)
            .is_some()
    }

    /// Whether this Cube holds a Vault for Connect's `hasVault`: a Vault
    /// or an imported Vault.
    pub fn holds_vault(&self) -> bool {
        self.vault_wallet_id.is_some() || self.has_imported_bitcoin()
    }

    pub fn bitcoin_import_pending(&self) -> bool {
        self.bitcoin_import
            .as_deref()
            .is_some_and(BitcoinImport::is_pending)
    }

    /// A pending import reserves the Bitcoin slot as well. Callers must not
    /// silently replace its intent with Vault setup.
    pub fn can_attach_vault(&self) -> bool {
        self.bitcoin_import.is_none() && self.vault_wallet_id.is_none()
    }

    /// Imported signing keys have no supported duress-recovery contract yet.
    pub fn supports_duress(&self) -> bool {
        self.bitcoin_import.is_none()
    }
}

impl WalletId {
    pub fn for_imported(
        descriptor: &ImportedWalletDescriptor,
        chain: ChainId,
    ) -> Result<Self, ImportedWalletError> {
        Ok(Self {
            timestamp: None,
            descriptor_checksum: format!("imported-v1-{}", descriptor.identity(chain)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxed_import_keeps_existing_settings_json_shape() {
        let mut cube = CubeSettings::new("Importing".into(), ChainId::Bitcoin);
        cube.bitcoin_import = Some(Box::new(BitcoinImport {
            operation_id: Uuid::nil(),
            stage: BitcoinImportStage::PendingSource,
            last_error: None,
        }));

        let json = serde_json::to_value(&cube).unwrap();
        assert_eq!(
            json["bitcoin_import"],
            serde_json::json!({
                "operation_id": Uuid::nil().to_string(),
                "stage": { "stage": "pending_source" },
            })
        );
        let restored: CubeSettings = serde_json::from_value(json).unwrap();
        assert_eq!(restored.bitcoin_import.unwrap().operation_id, Uuid::nil());
    }

    #[test]
    fn an_attached_import_holds_a_vault_for_connect_but_a_pending_one_does_not() {
        let mut cube = CubeSettings::new("Imported".into(), ChainId::Bitcoin);
        assert!(!cube.holds_vault());
        cube.bitcoin_import = Some(Box::new(BitcoinImport {
            operation_id: Uuid::nil(),
            stage: BitcoinImportStage::PendingSource,
            last_error: None,
        }));
        assert!(!cube.holds_vault(), "no descriptor is attached yet");
        let descriptor = ImportedWalletDescriptor::parse(
            "wpkh([6cb116eb/84h/0h/0h]xpub6C6c6ShFaUMjohRwYGnrDr5kbrnUjBeGh5U8BPAjtZnZ2D7WpRidu2SUNpBVfmMaw8TpDA7quYNW4iNwGYaq9iAwp5fzHLCXJwTqe76WpmF/<0;1>/*)",
            None,
            ChainId::Bitcoin,
        )
        .unwrap();
        let wallet = ImportedWalletAttachment::new(descriptor, ChainId::Bitcoin, 0, 20).unwrap();
        cube.bitcoin_import.as_mut().unwrap().stage = BitcoinImportStage::Syncing { wallet };
        assert!(
            cube.holds_vault(),
            "a scanning import already holds its Vault"
        );
    }
}

/// Immutable source binding. This is public wallet metadata, never signing authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedClaimPair {
    pub source_cube_id: String,
    pub source_operation_id: Uuid,
    pub source_wallet: ImportedWalletAttachment,
}
