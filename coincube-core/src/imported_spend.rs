//! Ordinary spends from a complete imported public Bitcoin wallet.
//! The Cube's Vault policy and seed are never used for these transactions.

use std::{collections::BTreeSet, convert::TryInto, fmt};

use miniscript::{
    bitcoin::{
        self, absolute::LockTime, bip32::ChildNumber, psbt::Psbt, secp256k1, Amount, ScriptBuf,
        Sequence, Transaction, TxIn, TxOut,
    },
    descriptor::DefiniteDescriptorKey,
    psbt::PsbtExt,
    Descriptor,
};

use crate::{
    public_wallet_finalize::{self, FinalizeError},
    spend::{
        self, AddrInfo, CandidateCoin, CoinSelectionRes, CreateSpendRes, CreateSpendWarning,
        InputAuthError, InsaneFeeInfo, SpendCreationError, SpendOutputAddress, SpendTxFees,
        TxGetter,
    },
    wallet_descriptor::ImportedWalletDescriptor,
};

fn invalid(reason: &'static str) -> SpendCreationError {
    SpendCreationError::InvalidImportedWallet(reason)
}

fn branch(
    wallet: &ImportedWalletDescriptor,
    info: AddrInfo,
) -> Result<Descriptor<DefiniteDescriptorKey>, SpendCreationError> {
    let index = match info.index {
        ChildNumber::Normal { index } => index,
        ChildNumber::Hardened { .. } => return Err(invalid("hardened address index")),
    };
    let descriptor = if info.is_change {
        wallet.change_descriptor()
    } else {
        wallet.receive_descriptor()
    };
    descriptor
        .at_derivation_index(index)
        .map_err(|_| invalid("address derivation failed"))
}

fn own_output(
    wallet: &ImportedWalletDescriptor,
    address: &SpendOutputAddress,
) -> Result<Option<Descriptor<DefiniteDescriptorKey>>, SpendCreationError> {
    match address.info {
        Some(info) => {
            let derived = branch(wallet, info)?;
            if derived.script_pubkey() != address.addr.script_pubkey() {
                return Err(invalid("owned address does not match wallet descriptor"));
            }
            Ok(Some(derived))
        }
        None => Ok(None),
    }
}

fn segwit(descriptor: &Descriptor<DefiniteDescriptorKey>) -> bool {
    !matches!(descriptor, Descriptor::Pkh(_))
}

/// Build an ordinary signed-later PSBT with authenticated full previous
/// transactions, exact descriptor scripts and signer origins. The caller must
/// reserve a fresh owned change address durably. For an explicit sweep of
/// selected coins, `change_addr` is the sole payment address.
#[allow(clippy::too_many_arguments)]
pub fn create_imported_spend(
    wallet: &ImportedWalletDescriptor,
    _secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    tx_getter: &mut impl TxGetter,
    destinations: &[(SpendOutputAddress, Amount)],
    candidate_coins: &[CandidateCoin],
    fees: SpendTxFees,
    change_addr: SpendOutputAddress,
    locktime: LockTime,
) -> Result<CreateSpendRes, SpendCreationError> {
    let (feerate, replaced_fee) = match fees {
        SpendTxFees::Regular(rate) => (rate, None),
        SpendTxFees::Rbf(rate, fee) => (rate, Some(fee)),
    };
    if feerate == 0 {
        return Err(SpendCreationError::InvalidFeerate(feerate));
    }
    if feerate > spend::MAX_FEERATE {
        return Err(SpendCreationError::InsaneFees(
            InsaneFeeInfo::TooHighFeerate(feerate),
        ));
    }
    let owned_change = matches!(
        change_addr.info,
        Some(AddrInfo {
            is_change: true,
            ..
        })
    );
    let explicit_sweep = destinations.is_empty()
        && !candidate_coins.is_empty()
        && candidate_coins.iter().all(|coin| coin.must_select);
    if !owned_change && !explicit_sweep {
        return Err(invalid("change must be an owned change-branch address"));
    }
    let change_desc = own_output(wallet, &change_addr)?;
    let mut seen = BTreeSet::new();
    for coin in candidate_coins {
        if !seen.insert(coin.outpoint) {
            return Err(invalid("duplicate candidate outpoint"));
        }
        branch(
            wallet,
            AddrInfo {
                index: coin.deriv_index,
                is_change: coin.is_change,
            },
        )?;
    }

    let mut tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: locktime,
        input: Vec::new(),
        output: Vec::with_capacity(destinations.len() + 1),
    };
    let mut owned_outputs = Vec::with_capacity(destinations.len() + 1);
    for (address, amount) in destinations {
        spend::check_output_value(*amount)?;
        let owned = own_output(wallet, address)?;
        tx.output.push(TxOut {
            value: *amount,
            script_pubkey: address.addr.script_pubkey(),
        });
        owned_outputs.push(owned);
    }
    let mut change_txo = TxOut {
        value: Amount::MAX,
        script_pubkey: change_addr.addr.script_pubkey(),
    };
    let representative = wallet
        .receive_descriptor()
        .at_derivation_index(0)
        .map_err(|_| invalid("descriptor derivation failed"))?;
    let is_segwit = segwit(&representative);
    let max_sat_weight = representative
        .max_weight_to_satisfy()
        .map_err(|_| invalid("descriptor cannot be satisfied"))?
        .to_wu()
        .checked_add(u64::from(is_segwit))
        .ok_or_else(|| invalid("satisfaction weight overflow"))?;
    let selection_rate: u16 = feerate
        .try_into()
        .map_err(|_| SpendCreationError::InsaneFees(InsaneFeeInfo::TooHighFeerate(feerate)))?;
    // A sweep payment to an external or receive address is required.
    // Only change is subject to the economic change threshold.
    let CoinSelectionRes {
        selected,
        change_amount,
        max_change_amount,
        fee_for_ancestors,
    } = spend::select_coins_for_spend(
        candidate_coins,
        tx.clone(),
        change_txo.clone(),
        f32::from(selection_rate),
        replaced_fee,
        max_sat_weight,
        is_segwit,
        destinations.is_empty(),
        spend::DUST_OUTPUT_SATS,
        owned_change,
    )
    .map_err(SpendCreationError::CoinSelection)?;
    let has_change = change_amount.to_sat() > 0;
    let mut warnings = Vec::new();
    if has_change {
        spend::check_output_value(change_amount)?;
        change_txo.value = change_amount;
        tx.output.push(change_txo);
        owned_outputs.push(change_desc);
    } else if max_change_amount.to_sat() > 0 {
        warnings.push(CreateSpendWarning::ChangeAddedToFee(
            max_change_amount.to_sat(),
        ));
    }
    if fee_for_ancestors.to_sat() > 0 {
        warnings.push(CreateSpendWarning::AdditionalFeeForAncestors(
            fee_for_ancestors.to_sat(),
        ));
    }
    for coin in &selected {
        tx.input.push(TxIn {
            previous_output: coin.outpoint,
            sequence: coin.sequence.unwrap_or(Sequence::ENABLE_RBF_NO_LOCKTIME),
            ..TxIn::default()
        });
    }
    let mut psbt =
        Psbt::from_unsigned_tx(tx).map_err(|_| invalid("unsigned transaction is invalid"))?;
    for (i, desc) in owned_outputs.iter().enumerate() {
        if let Some(desc) = desc {
            psbt.update_output_with_descriptor(i, desc)
                .map_err(|_| invalid("owned output descriptor does not match"))?;
        }
    }
    for (i, coin) in selected.iter().enumerate() {
        let definite = branch(
            wallet,
            AddrInfo {
                index: coin.deriv_index,
                is_change: coin.is_change,
            },
        )?;
        let previous = tx_getter.get_tx(&coin.outpoint.txid);
        let authenticated =
            spend::authenticate_previous_output(&coin.outpoint, previous.as_ref(), None)
                .map_err(|e| SpendCreationError::InputAuthentication(coin.outpoint, e))?;
        if authenticated.value != coin.amount {
            return Err(SpendCreationError::InputAuthentication(
                coin.outpoint,
                InputAuthError::AmountMismatch {
                    expected: coin.amount,
                    actual: authenticated.value,
                },
            ));
        }
        if authenticated.script_pubkey != definite.script_pubkey() {
            return Err(SpendCreationError::InputAuthentication(
                coin.outpoint,
                InputAuthError::ScriptMismatch,
            ));
        }
        psbt.inputs[i].non_witness_utxo = previous;
        if is_segwit {
            psbt.inputs[i].witness_utxo = Some(authenticated);
        }
        psbt.update_input_with_descriptor(i, &definite)
            .map_err(|_| invalid("input descriptor metadata is invalid"))?;
    }
    verify_imported_spend_before_broadcast(wallet, &psbt)?;
    Ok(CreateSpendRes {
        psbt,
        has_change,
        warnings,
    })
}

/// Recheck every previous transaction, descriptor script, output and fee before
/// handing a PSBT to a signer or broadcasting its finalized transaction.
pub fn verify_imported_spend_before_broadcast(
    wallet: &ImportedWalletDescriptor,
    psbt: &Psbt,
) -> Result<(), SpendCreationError> {
    let tx = &psbt.unsigned_tx;
    if tx.input.is_empty()
        || tx.output.is_empty()
        || psbt.inputs.len() != tx.input.len()
        || psbt.outputs.len() != tx.output.len()
    {
        return Err(SpendCreationError::SanityCheckFailure(psbt.clone()));
    }
    let mut total = 0u64;
    let mut sat_weight = 0u64;
    let mut any_segwit = false;
    for (i, txin) in tx.input.iter().enumerate() {
        let input = &psbt.inputs[i];
        let output = spend::authenticate_previous_output(
            &txin.previous_output,
            input.non_witness_utxo.as_ref(),
            input.witness_utxo.as_ref(),
        )
        .map_err(|e| SpendCreationError::InputAuthentication(txin.previous_output, e))?;
        let desc = locate_input(wallet, input, &output.script_pubkey).ok_or({
            SpendCreationError::InputAuthentication(
                txin.previous_output,
                InputAuthError::ScriptMismatch,
            )
        })?;
        total = total
            .checked_add(output.value.to_sat())
            .ok_or_else(|| SpendCreationError::SanityCheckFailure(psbt.clone()))?;
        sat_weight = sat_weight
            .checked_add(
                desc.max_weight_to_satisfy()
                    .map_err(|_| invalid("input descriptor cannot be satisfied"))?
                    .to_wu(),
            )
            .ok_or_else(|| invalid("satisfaction weight overflow"))?;
        any_segwit |= segwit(&desc);
        let mut expected = psbt.clone();
        expected.inputs[i].bip32_derivation.clear();
        expected.inputs[i].redeem_script = None;
        expected.inputs[i].witness_script = None;
        expected
            .update_input_with_descriptor(i, &desc)
            .map_err(|_| invalid("input descriptor metadata is invalid"))?;
        if expected.inputs[i].bip32_derivation != input.bip32_derivation
            || expected.inputs[i].redeem_script != input.redeem_script
            || expected.inputs[i].witness_script != input.witness_script
        {
            return Err(invalid(
                "input signer metadata differs from wallet descriptor",
            ));
        }
    }
    let out = tx
        .output
        .iter()
        .try_fold(0u64, |sum, txout| sum.checked_add(txout.value.to_sat()))
        .ok_or_else(|| SpendCreationError::SanityCheckFailure(psbt.clone()))?;
    let fee = total
        .checked_sub(out)
        .ok_or(SpendCreationError::InsaneFees(InsaneFeeInfo::NegativeFee))?;
    if fee > spend::MAX_FEE.to_sat() {
        return Err(SpendCreationError::InsaneFees(InsaneFeeInfo::TooHighFee(
            fee,
        )));
    }
    let signed_weight = tx
        .weight()
        .to_wu()
        .checked_add(sat_weight)
        .and_then(|weight| {
            weight.checked_add(if any_segwit {
                2 + tx.input.len() as u64
            } else {
                0
            })
        })
        .ok_or_else(|| invalid("signed weight overflow"))?;
    let maximum_vbytes = signed_weight.saturating_add(3) / 4;
    let feerate = fee
        .checked_div(maximum_vbytes)
        .ok_or(SpendCreationError::InsaneFees(
            InsaneFeeInfo::InvalidFeerate,
        ))?;
    if !(1..=spend::MAX_FEERATE).contains(&feerate) {
        return Err(SpendCreationError::InsaneFees(
            InsaneFeeInfo::TooHighFeerate(feerate),
        ));
    }
    for output in &tx.output {
        if output.value < output.script_pubkey.minimal_non_dust() {
            return Err(SpendCreationError::SanityCheckFailure(psbt.clone()));
        }
    }
    Ok(())
}

fn locate_input(
    wallet: &ImportedWalletDescriptor,
    input: &bitcoin::psbt::Input,
    script: &ScriptBuf,
) -> Option<Descriptor<DefiniteDescriptorKey>> {
    for (_, path) in input.bip32_derivation.values() {
        let mut numbers = path.as_ref().iter().rev();
        let index = match numbers.next()? {
            ChildNumber::Normal { index } => *index,
            ChildNumber::Hardened { .. } => continue,
        };
        let is_change = match numbers.next()? {
            ChildNumber::Normal { index: 0 } => false,
            ChildNumber::Normal { index: 1 } => true,
            _ => continue,
        };
        let descriptor = if is_change {
            wallet.change_descriptor()
        } else {
            wallet.receive_descriptor()
        };
        if let Ok(derived) = descriptor.at_derivation_index(index) {
            if derived.script_pubkey() == *script {
                return Some(derived);
            }
        }
    }
    None
}

#[derive(Debug)]
pub enum ImportedFinalizeError {
    Construction(SpendCreationError),
    Signatures(FinalizeError),
}

impl fmt::Display for ImportedFinalizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Construction(reason) => write!(f, "Imported spend is invalid: {reason}"),
            Self::Signatures(reason) => write!(f, "Imported wallet signature refused: {reason:?}"),
        }
    }
}

impl std::error::Error for ImportedFinalizeError {}

fn verified_partial_outputs<C: secp256k1::Verification>(
    wallet: &ImportedWalletDescriptor,
    construction: &Psbt,
    signed: &Psbt,
    secp: &secp256k1::Secp256k1<C>,
) -> Result<Vec<TxOut>, ImportedFinalizeError> {
    verify_imported_spend_before_broadcast(wallet, construction)
        .map_err(ImportedFinalizeError::Construction)?;
    check_imported_signed_construction(construction, signed)
        .map_err(ImportedFinalizeError::Signatures)?;
    public_wallet_finalize::verify_partial_signatures_for(signed, secp, |_, input, output| {
        locate_input(wallet, input, &output.script_pubkey)
    })
    .map_err(ImportedFinalizeError::Signatures)
}

/// Bitcoin Core may return the funding transaction in a signed PSBT without
/// its witnesses. Those witnesses are not part of its txid and do not affect
/// the output being spent. Permit only that difference; the shared
/// construction check is otherwise byte-exact. Signature verification still
/// authenticates the actual funding transaction supplied by the signer.
fn check_imported_signed_construction(
    construction: &Psbt,
    signed: &Psbt,
) -> Result<(), FinalizeError> {
    if construction.inputs.len() != signed.inputs.len() {
        return Err(FinalizeError::ConstructionChanged);
    }
    let mut normalized = signed.clone();
    for (index, (original, incoming)) in construction.inputs.iter().zip(&signed.inputs).enumerate()
    {
        match (&original.non_witness_utxo, &incoming.non_witness_utxo) {
            (Some(expected), Some(actual)) => {
                let mut expected_base = expected.clone();
                let mut actual_base = actual.clone();
                for input in &mut expected_base.input {
                    input.witness.clear();
                }
                for input in &mut actual_base.input {
                    input.witness.clear();
                }
                if expected_base != actual_base {
                    return Err(FinalizeError::ConstructionChanged);
                }
                normalized.inputs[index].non_witness_utxo = Some(expected.clone());
            }
            (None, None) => {}
            _ => return Err(FinalizeError::ConstructionChanged),
        }
    }
    public_wallet_finalize::check_signed_construction(construction, &normalized)
}

/// Number of cryptographically valid signatures per input on an exact partial
/// PSBT. For multisig this can be shown before the threshold is complete.
pub fn verify_imported_signatures<C: secp256k1::Verification>(
    wallet: &ImportedWalletDescriptor,
    construction: &Psbt,
    signed: &Psbt,
    secp: &secp256k1::Secp256k1<C>,
) -> Result<Vec<usize>, ImportedFinalizeError> {
    verified_partial_outputs(wallet, construction, signed, secp)?;
    Ok(signed
        .inputs
        .iter()
        .map(|input| input.partial_sigs.len())
        .collect())
}

pub struct VerifiedImportedSpend {
    transaction: Transaction,
    fee: Amount,
    signatures_per_input: Vec<usize>,
}

impl VerifiedImportedSpend {
    pub fn transaction(&self) -> &Transaction {
        &self.transaction
    }

    pub fn fee(&self) -> Amount {
        self.fee
    }

    pub fn signatures_per_input(&self) -> &[usize] {
        &self.signatures_per_input
    }
}

/// Finalize only signatures on the exact PSBT created by the imported-wallet
/// adapter. Every supplied signature is checked, then Miniscript finalizes and
/// replays the actual scriptSig/witness before a transaction is returned.
pub fn finalize_imported_spend<C: secp256k1::Verification>(
    wallet: &ImportedWalletDescriptor,
    construction: &Psbt,
    signed: &Psbt,
    secp: &secp256k1::Secp256k1<C>,
) -> Result<VerifiedImportedSpend, ImportedFinalizeError> {
    let outputs = verified_partial_outputs(wallet, construction, signed, secp)?;
    let total = outputs
        .iter()
        .try_fold(0u64, |sum, output| sum.checked_add(output.value.to_sat()))
        .ok_or(ImportedFinalizeError::Signatures(FinalizeError::Economics))?;
    let paid = signed
        .unsigned_tx
        .output
        .iter()
        .try_fold(0u64, |sum, output| sum.checked_add(output.value.to_sat()))
        .ok_or(ImportedFinalizeError::Signatures(FinalizeError::Economics))?;
    let fee = total
        .checked_sub(paid)
        .ok_or(ImportedFinalizeError::Signatures(FinalizeError::Economics))?;
    let (transaction, signatures_per_input) =
        public_wallet_finalize::finalize_and_replay(construction, signed, &outputs, secp)
            .map_err(ImportedFinalizeError::Signatures)?;
    let actual_feerate = fee / (transaction.vsize() as u64);
    if !(1..=spend::MAX_FEERATE).contains(&actual_feerate) {
        return Err(ImportedFinalizeError::Signatures(FinalizeError::Economics));
    }
    Ok(VerifiedImportedSpend {
        transaction,
        fee: Amount::from_sat(fee),
        signatures_per_input,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::ChainId;
    use miniscript::bitcoin::{
        bip32::{DerivationPath, Xpriv, Xpub},
        ecdsa,
        key::PublicKey,
        sighash::{EcdsaSighashType, SighashCache},
        Address, Network, OutPoint,
    };
    use std::str::FromStr;

    fn account(seed: u8, purpose: &str) -> (Xpriv, String) {
        let secp = secp256k1::Secp256k1::new();
        let master = Xpriv::new_master(Network::Regtest, &[seed; 32]).unwrap();
        let path = DerivationPath::from_str(purpose).unwrap();
        let xpriv = master.derive_priv(&secp, &path).unwrap();
        let xpub = Xpub::from_priv(&secp, &xpriv);
        (
            xpriv,
            format!(
                "[{}/{}]{xpub}",
                master.fingerprint(&secp),
                purpose.trim_start_matches("m/")
            ),
        )
    }

    fn wallet_on(kind: &str, chain: ChainId) -> (ImportedWalletDescriptor, Vec<Xpriv>) {
        let path = if kind.starts_with("wsh") {
            "m/48h/1h/0h/2h"
        } else {
            "m/84h/1h/0h"
        };
        let count = if kind.starts_with("wsh") { 3 } else { 1 };
        let accounts: Vec<_> = (1..=count).map(|seed| account(seed, path)).collect();
        let keys = accounts
            .iter()
            .map(|(_, key)| key.as_str())
            .collect::<Vec<_>>();
        let render = |branch| match kind {
            "pkh" => format!("pkh({}/{branch}/*)", keys[0]),
            "wpkh" => format!("wpkh({}/{branch}/*)", keys[0]),
            "sh(wpkh)" => format!("sh(wpkh({}/{branch}/*))", keys[0]),
            "wsh(multi)" => format!(
                "wsh(multi(2,{}))",
                keys.iter()
                    .map(|key| format!("{key}/{branch}/*"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            "wsh(sortedmulti)" => format!(
                "wsh(sortedmulti(2,{}))",
                keys.iter()
                    .map(|key| format!("{key}/{branch}/*"))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            _ => unreachable!(),
        };
        (
            ImportedWalletDescriptor::parse(&render(0), Some(&render(1)), chain).unwrap(),
            accounts.into_iter().map(|(key, _)| key).collect(),
        )
    }

    struct OneTx(Transaction);
    impl TxGetter for OneTx {
        fn get_tx(&mut self, txid: &bitcoin::Txid) -> Option<Transaction> {
            (self.0.compute_txid() == *txid).then(|| self.0.clone())
        }
    }

    fn draft(kind: &str) -> (ImportedWalletDescriptor, Vec<Xpriv>, CreateSpendRes) {
        draft_on(kind, ChainId::Regtest)
    }

    fn draft_on(
        kind: &str,
        chain: ChainId,
    ) -> (ImportedWalletDescriptor, Vec<Xpriv>, CreateSpendRes) {
        let (wallet, signers) = wallet_on(kind, chain);
        let address =
            |script: ScriptBuf| Address::from_script(&script, chain.bitcoin_network()).unwrap();
        let receive = wallet
            .receive_descriptor()
            .at_derivation_index(0)
            .unwrap()
            .script_pubkey();
        let previous = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn::default()],
            output: vec![TxOut {
                value: Amount::from_sat(150_000),
                script_pubkey: receive,
            }],
        };
        let candidate = CandidateCoin {
            outpoint: OutPoint {
                txid: previous.compute_txid(),
                vout: 0,
            },
            amount: Amount::from_sat(150_000),
            deriv_index: ChildNumber::Normal { index: 0 },
            is_change: false,
            must_select: true,
            sequence: None,
            ancestor_info: None,
        };
        let change = SpendOutputAddress {
            addr: address(
                wallet
                    .change_descriptor()
                    .at_derivation_index(1)
                    .unwrap()
                    .script_pubkey(),
            ),
            info: Some(AddrInfo {
                index: ChildNumber::Normal { index: 1 },
                is_change: true,
            }),
        };
        let (_, recipient_key) = account(99, "m/84h/1h/0h");
        let recipient = Descriptor::<miniscript::DescriptorPublicKey>::from_str(&format!(
            "wpkh({recipient_key}/0/*)"
        ))
        .unwrap()
        .at_derivation_index(0)
        .unwrap()
        .script_pubkey();
        let destinations = [(
            SpendOutputAddress {
                addr: address(recipient),
                info: None,
            },
            Amount::from_sat(50_000),
        )];
        let secp = secp256k1::Secp256k1::verification_only();
        let created = create_imported_spend(
            &wallet,
            &secp,
            &mut OneTx(previous),
            &destinations,
            &[candidate],
            SpendTxFees::Regular(5),
            change,
            LockTime::ZERO,
        )
        .unwrap();
        (wallet, signers, created)
    }

    #[test]
    fn imported_sweep_sizes_the_actual_external_payment_and_requires_selected_coins() {
        let (wallet, _, ordinary) = draft("wpkh");
        let previous = ordinary.psbt.inputs[0].non_witness_utxo.clone().unwrap();
        let (key, _) = account(99, "m/86h/1h/0h");
        let secp = secp256k1::Secp256k1::new();
        let (key, _) =
            secp256k1::PublicKey::from_secret_key(&secp, &key.private_key).x_only_public_key();
        let recipient = SpendOutputAddress {
            addr: Address::p2tr(&secp, key, None, Network::Regtest),
            info: None,
        };
        let build = |sats, must_select, destinations: &[(SpendOutputAddress, Amount)]| {
            let mut previous = previous.clone();
            let amount = Amount::from_sat(sats);
            previous.output[0].value = amount;
            let candidate = CandidateCoin {
                outpoint: OutPoint::new(previous.compute_txid(), 0),
                amount,
                deriv_index: ChildNumber::Normal { index: 0 },
                is_change: false,
                must_select,
                sequence: None,
                ancestor_info: None,
            };
            create_imported_spend(
                &wallet,
                &secp256k1::Secp256k1::verification_only(),
                &mut OneTx(previous),
                destinations,
                &[candidate],
                SpendTxFees::Regular(10),
                recipient.clone(),
                LockTime::ZERO,
            )
        };

        let large = build(150_000, true, &[]).unwrap();
        let fee = large.psbt.fee().unwrap().to_sat();
        let small = build(fee + 600, true, &[]).unwrap().psbt;
        assert_eq!(
            small.unsigned_tx.output,
            [TxOut {
                value: Amount::from_sat(600),
                script_pubkey: recipient.addr.script_pubkey(),
            }]
        );
        assert!(small.outputs[0].bip32_derivation.is_empty());
        assert_eq!(small.fee().unwrap().to_sat(), fee);
        assert!(build(150_000, false, &[]).is_err());
        assert!(build(
            150_000,
            true,
            &[(recipient.clone(), Amount::from_sat(50_000))]
        )
        .is_err());
    }

    /// A sweep recipient is a payment, not change. It is paid at the payment
    /// minimum even when that amount would be uneconomical as a change output,
    /// and one sat below the minimum is refused.
    #[test]
    fn imported_sweep_pays_the_dust_minimum_instead_of_the_change_threshold() {
        let (wallet, _, ordinary) = draft("wpkh");
        let previous = ordinary.psbt.inputs[0].non_witness_utxo.clone().unwrap();
        let (key, _) = account(99, "m/86h/1h/0h");
        let secp = secp256k1::Secp256k1::new();
        let (key, _) =
            secp256k1::PublicKey::from_secret_key(&secp, &key.private_key).x_only_public_key();
        let recipient = SpendOutputAddress {
            addr: Address::p2tr(&secp, key, None, Network::Regtest),
            info: None,
        };
        // High enough that an output of `DUST_OUTPUT_SATS` is uneconomical
        // change, and still a legal feerate.
        let feerate = 200;
        let build = |sats| {
            let mut previous = previous.clone();
            let amount = Amount::from_sat(sats);
            previous.output[0].value = amount;
            let candidate = CandidateCoin {
                outpoint: OutPoint::new(previous.compute_txid(), 0),
                amount,
                deriv_index: ChildNumber::Normal { index: 0 },
                is_change: false,
                must_select: true,
                sequence: None,
                ancestor_info: None,
            };
            create_imported_spend(
                &wallet,
                &secp256k1::Secp256k1::verification_only(),
                &mut OneTx(previous),
                &[],
                &[candidate],
                SpendTxFees::Regular(feerate),
                recipient.clone(),
                LockTime::ZERO,
            )
        };

        let large = build(150_000).unwrap();
        let fee = large.psbt.fee().unwrap().to_sat();
        let minimum = spend::DUST_OUTPUT_SATS;
        let paid = build(fee + minimum).unwrap();
        assert_eq!(
            paid.psbt.unsigned_tx.output,
            [TxOut {
                value: Amount::from_sat(minimum),
                script_pubkey: recipient.addr.script_pubkey(),
            }]
        );
        assert!(paid.psbt.outputs[0].bip32_derivation.is_empty());
        assert!(paid.warnings.is_empty());
        assert_eq!(paid.psbt.fee().unwrap().to_sat(), fee);

        assert!(matches!(
            build(fee + minimum - 1),
            Err(SpendCreationError::CoinSelection(_))
        ));
    }

    /// A Bitcoin Blake2b imported wallet spends like a Bitcoin one: Bitcoin
    /// Blake2b keeps Bitcoin's transactions and signatures, so every shape
    /// builds, passes the pre-broadcast check and finalises with its signers'
    /// ordinary signatures. Replay is the spend screen's warning, not a
    /// refusal here (Robert, 2026-11-17).
    #[test]
    fn imported_blake2b_wallets_build_sign_and_finalize_spends() {
        let secp = secp256k1::Secp256k1::new();
        for kind in ["pkh", "wpkh", "sh(wpkh)", "wsh(multi)", "wsh(sortedmulti)"] {
            let (wallet, signers, created) = draft_on(kind, ChainId::BitcoinBlake2bTestnet4);
            assert!(wallet.chain().is_blake2b());
            verify_imported_spend_before_broadcast(&wallet, &created.psbt)
                .unwrap_or_else(|e| panic!("{}: {:?}", kind, e));
            let mut signed = created.psbt.clone();
            for signer in signers
                .iter()
                .take(if kind.starts_with("wsh") { 2 } else { 1 })
            {
                add_signature(&mut signed, signer);
            }
            finalize_imported_spend(&wallet, &created.psbt, &signed, &secp)
                .unwrap_or_else(|e| panic!("{}: {:?}", kind, e));
        }
    }

    fn add_signature(psbt: &mut Psbt, signer: &Xpriv) {
        let secp = secp256k1::Secp256k1::new();
        let child = signer
            .derive_priv(&secp, &DerivationPath::from_str("m/0/0").unwrap())
            .unwrap();
        let pubkey = PublicKey::new(secp256k1::PublicKey::from_secret_key(
            &secp,
            &child.private_key,
        ));
        let mut cache = SighashCache::new(&psbt.unsigned_tx);
        let message = psbt.sighash_msg(0, &mut cache, None).unwrap().to_secp_msg();
        psbt.inputs[0].partial_sigs.insert(
            pubkey,
            ecdsa::Signature {
                signature: secp.sign_ecdsa(&message, &child.private_key),
                sighash_type: EcdsaSighashType::All,
            },
        );
    }

    #[test]
    fn only_sighash_all_signatures_finalize() {
        use miniscript::bitcoin::psbt::PsbtSighashType;
        let secp = secp256k1::Secp256k1::new();
        let (wallet, signers, created) = draft("wpkh");

        let mut explicit = created.psbt.clone();
        explicit.inputs[0].sighash_type = Some(PsbtSighashType::from(EcdsaSighashType::All));
        add_signature(&mut explicit, &signers[0]);
        assert!(finalize_imported_spend(&wallet, &created.psbt, &explicit, &secp).is_ok());

        for other in [
            EcdsaSighashType::None,
            EcdsaSighashType::Single,
            EcdsaSighashType::AllPlusAnyoneCanPay,
        ] {
            let mut requested = created.psbt.clone();
            requested.inputs[0].sighash_type = Some(PsbtSighashType::from(other));
            add_signature(&mut requested, &signers[0]);
            assert!(
                matches!(
                    finalize_imported_spend(&wallet, &created.psbt, &requested, &secp),
                    Err(ImportedFinalizeError::Signatures(
                        FinalizeError::UnsupportedSighash
                    ))
                ),
                "{:?}",
                other
            );

            let mut relabeled = created.psbt.clone();
            add_signature(&mut relabeled, &signers[0]);
            for signature in relabeled.inputs[0].partial_sigs.values_mut() {
                signature.sighash_type = other;
            }
            assert!(relabeled.inputs[0].sighash_type.is_none());
            assert!(
                matches!(
                    finalize_imported_spend(&wallet, &created.psbt, &relabeled, &secp),
                    Err(ImportedFinalizeError::Signatures(
                        FinalizeError::UnsupportedSighash
                    ))
                ),
                "{:?}",
                other
            );
        }
    }

    /// Every supported shape reads as a Vault policy with one path and no
    /// recovery paths, and its signature progress counts like a Vault's.
    #[test]
    fn imported_shapes_read_as_a_primary_only_policy() {
        use crate::descriptors::PathInfo;
        for (kind, threshold, keys) in [
            ("pkh", 1, 1),
            ("sh(wpkh)", 1, 1),
            ("wpkh", 1, 1),
            ("wsh(multi)", 2, 3),
            ("wsh(sortedmulti)", 2, 3),
        ] {
            let (wallet, signers, created) = draft(kind);
            let policy = wallet.policy().unwrap();
            assert!(policy.recovery_paths().is_empty(), "{}", kind);
            let (k, origins) = policy.primary_path().thresh_origins();
            assert_eq!((k, origins.len()), (threshold, keys), "{}", kind);
            match (policy.primary_path(), keys) {
                (PathInfo::Single(_), 1) | (PathInfo::Multi(..), 3) => {}
                (path, _) => panic!("{}: {:?}", kind, path),
            }

            let unsigned = wallet.partial_spend_info(&created.psbt).unwrap();
            assert_eq!(unsigned.primary_path().threshold, threshold, "{}", kind);
            assert_eq!(unsigned.primary_path().sigs_count, 0, "{}", kind);
            assert!(unsigned.recovery_paths().is_empty());

            let mut signed = created.psbt.clone();
            add_signature(&mut signed, &signers[0]);
            let info = wallet.partial_spend_info(&signed).unwrap();
            assert_eq!(info.primary_path().sigs_count, 1, "{}", kind);
            assert_eq!(info.primary_path().signed_pubkeys.len(), 1, "{}", kind);

            assert_eq!(wallet.change_indexes(&created.psbt), vec![1], "{}", kind);
            let vbytes = wallet.unsigned_tx_max_vbytes(&created.psbt.unsigned_tx);
            let fee = created.psbt.fee().unwrap().to_sat();
            // Built at 5 sat/vB against the same size estimate.
            assert!(
                fee >= 5 * vbytes && fee <= 5 * vbytes + 10,
                "{}: {} vs {}",
                kind,
                fee,
                vbytes
            );
        }
    }

    /// A signer counts only where it signed every input: a PSBT signed on
    /// some inputs reads as less signed instead of failing.
    #[test]
    fn imported_spend_info_counts_only_signers_on_every_input() {
        let (wallet, signers, created) = draft("wsh(sortedmulti)");
        let mut psbt = created.psbt.clone();
        let mut second = psbt.unsigned_tx.input[0].clone();
        second.previous_output.vout += 1;
        psbt.unsigned_tx.input.push(second);
        psbt.inputs.push(psbt.inputs[0].clone());

        add_signature(&mut psbt, &signers[0]);
        let partial = wallet.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial.primary_path().sigs_count, 0);
        assert!(partial.primary_path().signed_pubkeys.is_empty());

        let sigs = psbt.inputs[0].partial_sigs.clone();
        psbt.inputs[1].partial_sigs = sigs;
        let both = wallet.partial_spend_info(&psbt).unwrap();
        assert_eq!(both.primary_path().sigs_count, 1);
        assert_eq!(both.primary_path().signed_pubkeys.len(), 1);
        assert_eq!(both.primary_path().threshold, 2);
    }

    #[test]
    fn imported_spend_info_reaches_threshold_with_different_signers_per_input() {
        let (wallet, signers, created) = draft("wsh(sortedmulti)");
        let mut psbt = created.psbt.clone();
        let mut second = psbt.unsigned_tx.input[0].clone();
        second.previous_output.vout += 1;
        psbt.unsigned_tx.input.push(second);
        psbt.inputs.push(psbt.inputs[0].clone());

        // Input 0 signed by the first two keys, input 1 by the last two.
        let mut other = psbt.clone();
        add_signature(&mut psbt, &signers[0]);
        add_signature(&mut psbt, &signers[1]);
        add_signature(&mut other, &signers[1]);
        add_signature(&mut other, &signers[2]);
        psbt.inputs[1].partial_sigs = other.inputs[0].partial_sigs.clone();

        let partial = wallet.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial.primary_path().threshold, 2);
        assert_eq!(partial.primary_path().sigs_count, 2);
        // Only the key that signed both inputs reads as signed.
        assert_eq!(partial.primary_path().signed_pubkeys.len(), 1);

        // One input short of the threshold keeps the spend short.
        psbt.inputs[1].partial_sigs = {
            let mut short = created.psbt.clone();
            add_signature(&mut short, &signers[2]);
            short.inputs[0].partial_sigs.clone()
        };
        let short = wallet.partial_spend_info(&psbt).unwrap();
        assert_eq!(short.primary_path().sigs_count, 1);
        assert!(short.primary_path().signed_pubkeys.is_empty());
    }

    #[test]
    fn imported_multipath_descriptor_derives_both_branches() {
        let (wallet, _, _) = draft("wsh(sortedmulti)");
        let multipath = wallet.multipath_descriptor();
        let branches = multipath.into_single_descriptors().unwrap();
        assert_eq!(branches.len(), 2);
        assert_eq!(&branches[0], wallet.receive_descriptor());
        assert_eq!(&branches[1], wallet.change_descriptor());
    }

    #[test]
    fn all_supported_shapes_construct_with_authenticated_prevtx_and_own_change() {
        for kind in ["pkh", "sh(wpkh)", "wpkh", "wsh(multi)", "wsh(sortedmulti)"] {
            let (wallet, _, created) = draft(kind);
            assert!(created.has_change, "{}", kind);
            assert_eq!(created.psbt.inputs.len(), 1, "{kind}");
            assert!(
                created.psbt.inputs[0].non_witness_utxo.is_some(),
                "{}",
                kind
            );
            assert!(
                !created.psbt.inputs[0].bip32_derivation.is_empty(),
                "{}",
                kind
            );
            assert!(
                !created.psbt.outputs[1].bip32_derivation.is_empty(),
                "{}",
                kind
            );
            assert_eq!(
                created.psbt.inputs[0].witness_utxo.is_some(),
                kind != "pkh",
                "{kind}"
            );
            verify_imported_spend_before_broadcast(&wallet, &created.psbt).unwrap();
        }
    }

    #[test]
    fn independent_multisig_signatures_progress_and_finalize() {
        let (wallet, signers, created) = draft("wsh(multi)");
        let secp = secp256k1::Secp256k1::new();
        let mut partial = created.psbt.clone();
        add_signature(&mut partial, &signers[0]);
        assert_eq!(
            verify_imported_signatures(&wallet, &created.psbt, &partial, &secp).unwrap(),
            vec![1]
        );
        assert!(matches!(
            finalize_imported_spend(&wallet, &created.psbt, &partial, &secp),
            Err(ImportedFinalizeError::Signatures(
                FinalizeError::Unsatisfied
            ))
        ));
        add_signature(&mut partial, &signers[1]);
        assert_eq!(
            verify_imported_signatures(&wallet, &created.psbt, &partial, &secp).unwrap(),
            vec![2]
        );
        let finalized = finalize_imported_spend(&wallet, &created.psbt, &partial, &secp).unwrap();
        assert_eq!(finalized.signatures_per_input(), &[2]);
        assert!(finalized.fee().to_sat() > 0);
        assert_eq!(finalized.transaction().input.len(), 1);
    }

    #[test]
    fn all_shapes_finalize_with_full_sighash_all_signatures() {
        let secp = secp256k1::Secp256k1::new();
        for kind in ["pkh", "sh(wpkh)", "wpkh", "wsh(multi)", "wsh(sortedmulti)"] {
            let (wallet, signers, created) = draft(kind);
            let mut signed = created.psbt.clone();
            for signer in signers
                .iter()
                .take(if kind.starts_with("wsh") { 2 } else { 1 })
            {
                add_signature(&mut signed, signer);
            }
            let finalized =
                finalize_imported_spend(&wallet, &created.psbt, &signed, &secp).unwrap();
            assert!(finalized.transaction().vsize() > 0, "{}", kind);
            assert!(
                finalized.fee().to_sat() / (finalized.transaction().vsize() as u64) >= 5,
                "{}",
                kind
            );
        }
    }

    #[test]
    fn signed_psbt_may_strip_only_previous_transaction_witnesses() {
        let secp = secp256k1::Secp256k1::new();
        for kind in ["pkh", "sh(wpkh)", "wpkh", "wsh(multi)", "wsh(sortedmulti)"] {
            let (wallet, signers, created) = draft(kind);
            let mut construction = created.psbt;
            construction.inputs[0]
                .non_witness_utxo
                .as_mut()
                .unwrap()
                .input[0]
                .witness
                .push([1, 2, 3]);
            let mut signed = construction.clone();
            signed.inputs[0].non_witness_utxo.as_mut().unwrap().input[0]
                .witness
                .clear();
            for signer in signers
                .iter()
                .take(if kind.starts_with("wsh") { 2 } else { 1 })
            {
                add_signature(&mut signed, signer);
            }
            assert!(
                finalize_imported_spend(&wallet, &construction, &signed, &secp).is_ok(),
                "{}",
                kind
            );

            let mut altered_base = signed.clone();
            altered_base.inputs[0]
                .non_witness_utxo
                .as_mut()
                .unwrap()
                .output[0]
                .value -= Amount::from_sat(1);
            assert!(matches!(
                finalize_imported_spend(&wallet, &construction, &altered_base, &secp),
                Err(ImportedFinalizeError::Signatures(
                    FinalizeError::ConstructionChanged
                ))
            ));

            let mut altered_metadata = signed;
            altered_metadata.inputs[0].redeem_script = Some(ScriptBuf::new());
            assert!(matches!(
                finalize_imported_spend(&wallet, &construction, &altered_metadata, &secp),
                Err(ImportedFinalizeError::Signatures(
                    FinalizeError::ConstructionChanged
                ))
            ));
        }
    }

    #[test]
    fn signer_cannot_change_recipient_fee_change_or_previous_transaction() {
        let (wallet, signers, created) = draft("wpkh");
        let secp = secp256k1::Secp256k1::new();
        let mut signed = created.psbt.clone();
        add_signature(&mut signed, &signers[0]);
        assert!(finalize_imported_spend(&wallet, &created.psbt, &signed, &secp).is_ok());
        for output in [0, 1] {
            let mut changed = signed.clone();
            changed.unsigned_tx.output[output].value -= Amount::from_sat(1);
            assert!(finalize_imported_spend(&wallet, &created.psbt, &changed, &secp).is_err());
        }
        let mut changed = signed.clone();
        changed.unsigned_tx.output[0].script_pubkey = ScriptBuf::new();
        assert!(finalize_imported_spend(&wallet, &created.psbt, &changed, &secp).is_err());
        let mut changed = signed.clone();
        changed.inputs[0].non_witness_utxo.as_mut().unwrap().output[0].value -= Amount::from_sat(1);
        assert!(finalize_imported_spend(&wallet, &created.psbt, &changed, &secp).is_err());
        let mut corrupted_draft = created.psbt.clone();
        corrupted_draft.inputs[0]
            .non_witness_utxo
            .as_mut()
            .unwrap()
            .output[0]
            .value -= Amount::from_sat(1);
        assert!(matches!(
            verify_imported_spend_before_broadcast(&wallet, &corrupted_draft),
            Err(SpendCreationError::InputAuthentication(..))
        ));
        let mut changed = signed;
        changed.inputs[0]
            .partial_sigs
            .values_mut()
            .next()
            .unwrap()
            .sighash_type = EcdsaSighashType::None;
        assert!(finalize_imported_spend(&wallet, &created.psbt, &changed, &secp).is_err());
        let mut changed = created.psbt.clone();
        let child = signers[0]
            .derive_priv(&secp, &DerivationPath::from_str("m/0/0").unwrap())
            .unwrap();
        let pubkey = PublicKey::new(secp256k1::PublicKey::from_secret_key(
            &secp,
            &child.private_key,
        ));
        let unrelated = secp256k1::Message::from_digest([7; 32]);
        changed.inputs[0].partial_sigs.insert(
            pubkey,
            ecdsa::Signature {
                signature: secp.sign_ecdsa(&unrelated, &child.private_key),
                sighash_type: EcdsaSighashType::All,
            },
        );
        assert!(matches!(
            finalize_imported_spend(&wallet, &created.psbt, &changed, &secp),
            Err(ImportedFinalizeError::Signatures(
                FinalizeError::InvalidSignature { .. }
            ))
        ));
    }

    /// The shared public-wallet finalizer refuses every change to the
    /// construction other than added signatures. Ported from the removed
    /// foreign-Split finalizer tests, which covered these helpers through
    /// step 1.
    #[test]
    fn any_change_to_the_construction_refuses() {
        type Change = Box<dyn Fn(&mut Psbt)>;
        let secp = secp256k1::Secp256k1::new();
        let (wallet, signers, created) = draft("wpkh");
        let mut signed = created.psbt.clone();
        add_signature(&mut signed, &signers[0]);
        assert!(finalize_imported_spend(&wallet, &created.psbt, &signed, &secp).is_ok());
        let internal_key = PublicKey::new(secp256k1::PublicKey::from_secret_key(
            &secp,
            &account(9, "m/86h/1h/0h").0.private_key,
        ));
        let changes: Vec<Change> = vec![
            Box::new(|p| p.unsigned_tx.output[1].value = Amount::from_sat(1_000)),
            Box::new(|p| p.unsigned_tx.output.reverse()),
            Box::new(|p| p.inputs[0].bip32_derivation.clear()),
            Box::new(|p| p.inputs[0].witness_utxo = None),
            Box::new(|p| p.outputs[1].bip32_derivation.clear()),
            Box::new(move |p| p.inputs[0].tap_internal_key = Some(internal_key.inner.into())),
            Box::new(|p| p.inputs[0].final_script_witness = Some(bitcoin::Witness::new())),
            Box::new(|p| {
                p.inputs[0].unknown.insert(
                    bitcoin::psbt::raw::Key {
                        type_value: 0xfc,
                        key: vec![1],
                    },
                    vec![2],
                );
            }),
        ];
        for (index, change) in changes.iter().enumerate() {
            let mut tampered = signed.clone();
            change(&mut tampered);
            assert!(
                matches!(
                    finalize_imported_spend(&wallet, &created.psbt, &tampered, &secp),
                    Err(ImportedFinalizeError::Signatures(
                        FinalizeError::ConstructionChanged
                    ))
                ),
                "change {}",
                index
            );
        }
    }

    /// Every signature present must be by a key the input commits to and
    /// valid for it, even a surplus one the finalizer would ignore; an
    /// uncompressed key never signs a segwit input. Ported from the removed
    /// foreign-Split finalizer tests.
    #[test]
    fn every_signature_must_be_a_committed_key_over_this_input() {
        let secp = secp256k1::Secp256k1::new();
        let (wallet, signers, created) = draft("wpkh");
        let mut signed = created.psbt.clone();
        add_signature(&mut signed, &signers[0]);
        let (key, signature) = signed.inputs[0]
            .partial_sigs
            .iter()
            .next()
            .map(|(k, s)| (*k, *s))
            .unwrap();
        let refused = |psbt: &Psbt| {
            matches!(
                finalize_imported_spend(&wallet, &created.psbt, psbt, &secp),
                Err(ImportedFinalizeError::Signatures(
                    FinalizeError::InvalidSignature { input: 0 }
                ))
            )
        };

        // A signature valid over this input's digest, by a key the input
        // does not commit to.
        let mut surplus = signed.clone();
        add_signature(&mut surplus, &account(9, "m/84h/1h/0h").0);
        assert_eq!(surplus.inputs[0].partial_sigs.len(), 2);
        assert!(refused(&surplus));

        let mut uncompressed = signed.clone();
        uncompressed.inputs[0].partial_sigs.clear();
        uncompressed.inputs[0].partial_sigs.insert(
            PublicKey {
                compressed: false,
                inner: key.inner,
            },
            signature,
        );
        assert!(refused(&uncompressed));
    }
}
