//! Opaque, account-scoped local signing authority for imported Bitcoin wallets.
//! A Cube/Vault seed is never substituted for this independently admitted key.

use std::fmt;

use miniscript::{
    bitcoin::{
        bip32::{DerivationPath, Fingerprint, Xpub},
        hashes::sha256,
        psbt::Psbt,
        secp256k1::Secp256k1,
        sighash::EcdsaSighashType,
    },
    DescriptorPublicKey, ForEachKey,
};
use zeroize::Zeroizing;

use crate::{
    chain::ChainId, imported_spend::verify_imported_signatures, signer::SessionSigner,
    wallet_descriptor::ImportedWalletDescriptor,
};

pub use crate::imported_key_crypt::{MAX_MNEMONIC_BYTES, MAX_PASSPHRASE_BYTES};

/// Public account identity; all three components must match one descriptor
/// xpub in both branches. A fingerprint by itself grants no authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedAccount {
    pub fingerprint: Fingerprint,
    pub origin: DerivationPath,
    pub xpub: Xpub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportedKeyError {
    InvalidMnemonic,
    SecretTooLong,
    KeyDoesNotMatchWallet,
    WrongWallet,
    InvalidConstruction,
    SigningFailed,
    NoSignatureAdded,
    InvalidSignature,
}

impl fmt::Display for ImportedKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidMnemonic => "The imported recovery phrase is invalid.",
            Self::SecretTooLong => "The imported recovery phrase or passphrase is too long.",
            Self::KeyDoesNotMatchWallet => "This key does not match the selected wallet account.",
            Self::WrongWallet => "This key belongs to another imported wallet.",
            Self::InvalidConstruction => "The imported spend no longer matches its original draft.",
            Self::SigningFailed => "The imported spend could not be signed.",
            Self::NoSignatureAdded => "This key did not sign any imported wallet input.",
            Self::InvalidSignature => "The imported signature did not verify against the wallet.",
        })
    }
}
impl std::error::Error for ImportedKeyError {}

/// One selected account and wallet, with an owned root that scrubs on drop.
/// Deliberately neither Clone nor a raw-root accessor.
pub struct VerifiedImportedKey {
    signer: SessionSigner,
    account: ImportedAccount,
    chain: ChainId,
    wallet_identity: sha256::Hash,
}

impl fmt::Debug for VerifiedImportedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerifiedImportedKey([REDACTED])")
    }
}

impl VerifiedImportedKey {
    pub fn account(&self) -> &ImportedAccount {
        &self.account
    }

    /// Preserve the original draft and all previous valid signatures. Any
    /// upstream partial mutation is confined to a clone that is discarded on
    /// error; only a verified result is returned.
    pub fn sign(
        &self,
        wallet: &ImportedWalletDescriptor,
        original: &Psbt,
        current: &Psbt,
    ) -> Result<Psbt, ImportedKeyError> {
        if wallet.chain() != self.chain
            || wallet
                .identity(wallet.chain())
                .map_err(|_| ImportedKeyError::WrongWallet)?
                != self.wallet_identity
            || !account_in_both_branches(wallet, &self.account)
        {
            return Err(ImportedKeyError::WrongWallet);
        }
        if current.inputs.iter().any(|input| {
            input
                .sighash_type
                .is_some_and(|kind| kind != EcdsaSighashType::All.into())
        }) {
            return Err(ImportedKeyError::InvalidConstruction);
        }
        let secp = Secp256k1::verification_only();
        verify_imported_signatures(wallet, original, current, &secp)
            .map_err(|_| ImportedKeyError::InvalidConstruction)?;
        let signed = self
            .signer
            .sign_imported_scoped(current, self.account.fingerprint, &self.account.origin)
            .map_err(|_| ImportedKeyError::SigningFailed)?;
        if !signed
            .inputs
            .iter()
            .zip(&current.inputs)
            .any(|(after, before)| after.partial_sigs.len() > before.partial_sigs.len())
        {
            return Err(ImportedKeyError::NoSignatureAdded);
        }
        verify_imported_signatures(wallet, original, &signed, &secp)
            .map_err(|_| ImportedKeyError::InvalidSignature)?;
        Ok(signed)
    }
}

/// Check an exact public account against both validated branches. Public
/// account identity alone never grants signing authority.
pub fn account_in_both_branches(
    wallet: &ImportedWalletDescriptor,
    account: &ImportedAccount,
) -> bool {
    [wallet.receive_descriptor(), wallet.change_descriptor()]
        .iter()
        .all(|descriptor| {
            let mut found = false;
            descriptor.for_each_key(|key| {
                if let DescriptorPublicKey::XPub(key) = key {
                    found |= key.origin.as_ref()
                        == Some(&(account.fingerprint, account.origin.clone()))
                        && key.xkey == account.xpub;
                }
                true
            });
            found
        })
}

/// Admit an English BIP39 phrase and exact selected descriptor account. The
/// caller owns zeroizing inputs; no phrase or passphrase is retained after
/// derivation. `selected_origin` is the full master-to-account path.
pub fn verify_imported_account(
    wallet: &ImportedWalletDescriptor,
    selected_origin: &DerivationPath,
    words: Zeroizing<String>,
    passphrase: Zeroizing<String>,
) -> Result<VerifiedImportedKey, ImportedKeyError> {
    if words.len() > MAX_MNEMONIC_BYTES || passphrase.len() > MAX_PASSPHRASE_BYTES {
        return Err(ImportedKeyError::SecretTooLong);
    }
    let mut path_present = false;
    wallet.receive_descriptor().for_each_key(|key| {
        if let DescriptorPublicKey::XPub(key) = key {
            path_present |= key
                .origin
                .as_ref()
                .is_some_and(|(_, path)| path == selected_origin);
        }
        true
    });
    if !path_present || selected_origin.len() > 255 {
        return Err(ImportedKeyError::KeyDoesNotMatchWallet);
    }
    let mnemonic = bip39::Mnemonic::parse_in(bip39::Language::English, words.as_str())
        .map_err(|_| ImportedKeyError::InvalidMnemonic)?;
    let signer = SessionSigner::from_mnemonic(
        wallet.chain().bitcoin_network(),
        mnemonic,
        passphrase.as_str(),
    )
    .map_err(|_| ImportedKeyError::InvalidMnemonic)?;
    let secp = Secp256k1::new();
    let account = ImportedAccount {
        fingerprint: signer.fingerprint(&secp),
        origin: selected_origin.clone(),
        xpub: signer
            .try_xpub_at(selected_origin, &secp)
            .map_err(|_| ImportedKeyError::KeyDoesNotMatchWallet)?,
    };
    if !account_in_both_branches(wallet, &account) {
        return Err(ImportedKeyError::KeyDoesNotMatchWallet);
    }
    let wallet_identity = wallet
        .identity(wallet.chain())
        .map_err(|_| ImportedKeyError::WrongWallet)?;
    Ok(VerifiedImportedKey {
        signer,
        account,
        chain: wallet.chain(),
        wallet_identity,
    })
}
