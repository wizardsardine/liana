//! The two LAN signing policies share an encrypted transport, but not a
//! descriptor parser or a persisted wallet identity.

use coincube_core::{descriptors::CoincubeDescriptor, wallet_descriptor::ImportedWalletDescriptor};
use std::str::FromStr;

use crate::{
    app::{settings::WalletId, wallet::Wallet},
    chain::ChainId,
    split_hardware,
};

#[derive(Debug, Clone)]
pub enum PhonePolicyKind {
    Vault(CoincubeDescriptor),
    Imported {
        wallet: ImportedWalletDescriptor,
        wallet_id: WalletId,
    },
}

/// The descriptor bytes sent to Keychain are fixed when this profile is made.
/// `transport_id` is only the first four bytes of their SHA-256; an imported
/// wallet's persisted, chain-bound `WalletId` remains a separate authority.
#[derive(Debug, Clone)]
pub struct PhonePolicy {
    kind: PhonePolicyKind,
    chain: ChainId,
    transport_descriptor: String,
}

impl PhonePolicy {
    pub fn vault(descriptor: &str, chain: ChainId) -> Result<Self, String> {
        let vault = CoincubeDescriptor::from_str(descriptor).map_err(|e| e.to_string())?;
        Ok(Self {
            kind: PhonePolicyKind::Vault(vault),
            chain,
            transport_descriptor: descriptor.to_owned(),
        })
    }

    pub fn from_wallet(wallet: &Wallet) -> Result<Self, String> {
        match &wallet.main_descriptor {
            coincube_core::wallet_descriptor::WalletDescriptor::Vault(vault) => Ok(Self {
                kind: PhonePolicyKind::Vault(vault.clone()),
                chain: wallet.chain,
                transport_descriptor: vault.to_string(),
            }),
            coincube_core::wallet_descriptor::WalletDescriptor::Imported(imported) => {
                Self::imported(imported.clone(), wallet.chain, wallet.id())
            }
        }
    }

    pub fn imported(
        wallet: ImportedWalletDescriptor,
        chain: ChainId,
        wallet_id: WalletId,
    ) -> Result<Self, String> {
        if chain.is_blake2b() || wallet.chain() != chain {
            return Err("Mobile LAN signing is unavailable on this Bitcoin chain.".into());
        }
        let expected = WalletId::for_imported(&wallet, chain).map_err(|e| e.to_string())?;
        if wallet_id != expected {
            return Err("Imported wallet storage identity changed; pair again.".into());
        }
        let transport_descriptor = split_hardware::policy::imported_descriptor(&wallet)
            .map_err(|e| e.to_string())?
            .to_string();
        Ok(Self {
            kind: PhonePolicyKind::Imported { wallet, wallet_id },
            chain,
            transport_descriptor,
        })
    }

    pub fn descriptor(&self) -> &str {
        &self.transport_descriptor
    }

    pub fn kind(&self) -> &PhonePolicyKind {
        &self.kind
    }

    pub fn chain(&self) -> ChainId {
        self.chain
    }

    /// A cached LAN handle must still represent the same imported wallet,
    /// beyond merely sharing the short descriptor routing ID.
    pub fn same_imported_authority(&self, other: &Self) -> bool {
        match (&self.kind, &other.kind) {
            (
                PhonePolicyKind::Imported {
                    wallet_id: left, ..
                },
                PhonePolicyKind::Imported {
                    wallet_id: right, ..
                },
            ) => {
                self.chain == other.chain
                    && self.transport_descriptor == other.transport_descriptor
                    && left == right
            }
            _ => false,
        }
    }

    pub fn transport_id(&self) -> coincube_core::miniscript::bitcoin::bip32::Fingerprint {
        crate::app::wallet::descriptor_id_fingerprint_of_bytes(self.transport_descriptor.as_bytes())
    }
}
