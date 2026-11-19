//! Read-only proof that a connected device exports an exact imported account.
//! The result contains public identity only. It does not retain or bind the
//! device handle, admit a signer, persist a device, or grant authority to sign.

use std::{fmt, sync::Arc};

use async_hwi::HWI;
use coincube_core::{
    chain::ChainId,
    imported_keys::{account_in_both_branches, ImportedAccount},
    miniscript::bitcoin::{
        bip32::{Fingerprint, Xpub},
        hashes::sha256,
        NetworkKind,
    },
    wallet_descriptor::ImportedWalletDescriptor,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceIdentityError {
    AccountNotInWallet,
    Cancelled,
    Device,
    Fingerprint,
    Network,
    Xpub,
}

impl fmt::Display for DeviceIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AccountNotInWallet => "The selected account is not in both wallet branches.",
            Self::Cancelled => "The imported device check was cancelled.",
            Self::Device => "The connected device could not export this account.",
            Self::Fingerprint => "The connected device has a different master fingerprint.",
            Self::Network => "The device account belongs to another Bitcoin network.",
            Self::Xpub => "The device account does not match the selected wallet key.",
        })
    }
}

impl std::error::Error for DeviceIdentityError {}

/// Public proof scoped to the complete script identity of one imported wallet.
/// A future signer must recheck the same connected device and operation
/// generation at admission/sign time, then use the existing PSBT and
/// device-signature admission boundaries to perform any Send.
pub struct VerifiedDeviceAccount {
    account: ImportedAccount,
    chain: ChainId,
    wallet_identity: sha256::Hash,
}

impl fmt::Debug for VerifiedDeviceAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerifiedDeviceAccount([REDACTED])")
    }
}

impl VerifiedDeviceAccount {
    pub fn account(&self) -> &ImportedAccount {
        &self.account
    }

    pub fn matches_wallet(&self, wallet: &ImportedWalletDescriptor) -> bool {
        wallet.chain() == self.chain
            && wallet.identity(self.chain).ok() == Some(self.wallet_identity)
            && account_in_both_branches(wallet, &self.account)
    }
}

fn check_wallet(
    wallet: &ImportedWalletDescriptor,
    selected: &ImportedAccount,
) -> Result<sha256::Hash, DeviceIdentityError> {
    if !account_in_both_branches(wallet, selected) {
        return Err(DeviceIdentityError::AccountNotInWallet);
    }
    wallet
        .identity(wallet.chain())
        .map_err(|_| DeviceIdentityError::AccountNotInWallet)
}

fn check_export(
    wallet: &ImportedWalletDescriptor,
    selected: &ImportedAccount,
    reported: Fingerprint,
    exported: Xpub,
) -> Result<VerifiedDeviceAccount, DeviceIdentityError> {
    let wallet_identity = check_wallet(wallet, selected)?;
    if reported != selected.fingerprint {
        return Err(DeviceIdentityError::Fingerprint);
    }
    let expected_network = if wallet.chain().bitcoin_network()
        == coincube_core::miniscript::bitcoin::Network::Bitcoin
    {
        NetworkKind::Main
    } else {
        NetworkKind::Test
    };
    if exported.network != expected_network {
        return Err(DeviceIdentityError::Network);
    }
    // The descriptor parser already checked that the origin length and last
    // child number agree with this xpub. Equality also checks parent
    // fingerprint, depth, child number, chain code and public key.
    if exported != selected.xpub {
        return Err(DeviceIdentityError::Xpub);
    }
    Ok(VerifiedDeviceAccount {
        account: selected.clone(),
        chain: wallet.chain(),
        wallet_identity,
    })
}

/// Read the exact origin path from a fresh device handle and compare the full
/// exported xpub with the selected descriptor key. `is_current` must represent
/// the UI operation generation and selected Cube/wallet; it is checked across
/// both awaits so a late result cannot be admitted after cancellation.
pub async fn verify_device_account(
    device: Arc<dyn HWI + Send + Sync>,
    wallet: &ImportedWalletDescriptor,
    selected: &ImportedAccount,
    is_current: impl Fn() -> bool,
) -> Result<VerifiedDeviceAccount, DeviceIdentityError> {
    if !is_current() {
        return Err(DeviceIdentityError::Cancelled);
    }
    check_wallet(wallet, selected)?;
    let reported = device
        .get_master_fingerprint()
        .await
        .map_err(|_| DeviceIdentityError::Device)?;
    if !is_current() {
        return Err(DeviceIdentityError::Cancelled);
    }
    if reported != selected.fingerprint {
        return Err(DeviceIdentityError::Fingerprint);
    }
    let exported = device
        .get_extended_pubkey(&selected.origin)
        .await
        .map_err(|_| DeviceIdentityError::Device)?;
    if !is_current() {
        return Err(DeviceIdentityError::Cancelled);
    }
    check_export(wallet, selected, reported, exported)
}

#[cfg(test)]
mod tests;
