use std::collections::HashMap;
use std::str::FromStr;

use coincube_core::{
    descriptors::CoincubeDescriptor,
    miniscript::bitcoin::{self, bip32, consensus, hashes::hex::FromHex, Amount, OutPoint, Txid},
};
use serde::{de, Deserialize, Deserializer};

use crate::utils::serde::deser_fromstr;

/// Deserialize an address from string, assuming the network was checked.
pub fn deser_addr_assume_checked<'de, D>(deserializer: D) -> Result<bitcoin::Address, D::Error>
where
    D: Deserializer<'de>,
{
    let string = String::deserialize(deserializer)?;
    bitcoin::Address::from_str(&string)
        .map(|addr| addr.assume_checked())
        .map_err(de::Error::custom)
}

/// Deserialize an amount from sats
pub fn deser_amount_from_sats<'de, D>(deserializer: D) -> Result<bitcoin::Amount, D::Error>
where
    D: Deserializer<'de>,
{
    let a = u64::deserialize(deserializer)?;
    Ok(bitcoin::Amount::from_sat(a))
}

pub fn deser_hex<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: consensus::Decodable,
{
    let s = String::deserialize(d)?;
    let s = Vec::from_hex(&s).map_err(de::Error::custom)?;
    consensus::deserialize(&s).map_err(de::Error::custom)
}

/// The maximum number of item to return.
pub const DEFAULT_LIMIT: usize = 20;
/// The maximum number of outpoints that can be provided as a filter.
pub const DEFAULT_OUTPOINTS_LIMIT: usize = 50;
/// The maximum number of items that can be provided as a filter.
pub const DEFAULT_LABEL_ITEMS_LIMIT: usize = 50;

#[derive(Deserialize)]
pub struct Claims {
    pub sub: String,
}

#[derive(Deserialize)]
pub struct NetworkInfo {
    pub feerate: Feerate,
    pub rates: HashMap<String, f32>,
}

#[derive(Deserialize)]
pub struct Feerate {
    pub low: Option<i32>,
    pub high: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WalletBalance {
    /// Total of funds that present in a block.
    pub confirmed: u64,
    /// Total of funds that is not yet in a block.
    pub unconfirmed: u64,
    /// Total of funds that are mined but not yet available
    pub immature: u64,
    /// Total of funds that are unconfirmed but are coming from
    /// the wallet
    pub unconfirmed_change: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WalletStatus {
    Normal,
    Recovering,
    Recovered,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecoveryPath {
    pub sequence: u16,
    pub available_balance: u64,
    pub total_coins: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Wallet {
    pub id: String,
    pub name: String,
    #[serde(deserialize_with = "deser_fromstr")]
    pub descriptor: CoincubeDescriptor,
    pub deposit_derivation_index: u32,
    pub change_derivation_index: u32,
    pub recovery_paths: Vec<RecoveryPath>,
    pub biggest_remaining_sequence: Option<u32>,
    pub smallest_remaining_sequence: Option<u32>,
    pub metadata: WalletMetadata,
    pub created_at: i64,
    pub balance: WalletBalance,
    pub status: WalletStatus,
    pub tip_height: Option<i32>,
}

#[derive(Deserialize)]
pub struct ListWallets {
    pub wallets: Vec<Wallet>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Owner,
    Member,
}

#[derive(Deserialize)]
pub struct ListWalletMembers {
    pub members: Vec<Member>,
}

#[derive(Deserialize)]
pub struct Member {
    pub user_id: String,
    pub role: UserRole,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    pub uuid: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderKey {
    #[serde(deserialize_with = "deser_fromstr")]
    pub fingerprint: bip32::Fingerprint,
    pub uuid: String,
    pub token: String,
    pub provider: Provider,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WalletMetadata {
    pub wallet_alias: Option<String>,
    pub ledger_hmacs: Vec<LedgerHmac>,
    pub fingerprint_aliases: Vec<FingerprintAlias>,
    pub provider_keys: Vec<ProviderKey>,
}

pub const WALLET_ALIAS_MAXIMUM_LENGTH: usize = 64;

#[derive(Debug, Clone, Deserialize)]
pub struct LedgerHmac {
    #[serde(deserialize_with = "deser_fromstr")]
    pub fingerprint: bip32::Fingerprint,
    pub user_id: String,
    pub hmac: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FingerprintAlias {
    #[serde(deserialize_with = "deser_fromstr")]
    pub fingerprint: bip32::Fingerprint,
    pub user_id: String,
    pub alias: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WalletInvitationStatus {
    Pending,
    Accepted,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WalletInvitation {
    pub id: String,
    pub wallet_name: String,
    pub wallet_id: String,
    pub status: WalletInvitationStatus,
}

#[derive(Deserialize)]
pub struct WalletLabels {
    pub labels: HashMap<String, String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaymentKind {
    Outgoing,
    Incoming,
}

#[derive(Deserialize)]
pub struct Payment {
    pub txuuid: String,
    pub txid: String,
    pub vout: u32,
    pub amount: u64,
    pub block_height: Option<i32>,
    pub confirmed_at: Option<i64>,
    pub label: Option<String>,
    pub address_label: Option<String>,
    pub transaction_label: Option<String>,
    pub kind: PaymentKind,
    pub is_single: bool,
}

#[derive(Deserialize)]
pub struct ListPayments {
    pub payments: Vec<Payment>,
}

#[derive(Clone, Deserialize)]
pub struct Coin {
    #[serde(deserialize_with = "deser_addr_assume_checked")]
    pub address: bitcoin::Address,
    #[serde(deserialize_with = "deser_amount_from_sats")]
    pub amount: Amount,
    pub derivation_index: bip32::ChildNumber,
    pub outpoint: OutPoint,
    pub block_height: Option<i32>,
    pub spend_info: Option<CoinSpendInfo>,
    pub is_immature: bool,
    pub is_change_address: bool,
    pub is_from_self: bool,
}

#[derive(Clone, Deserialize)]
pub struct CoinSpendInfo {
    pub txid: Txid,
    pub height: Option<i32>,
}

#[derive(Deserialize)]
pub struct ListCoins {
    pub coins: Vec<Coin>,
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UTXOKind {
    Deposit,
    Change,
    External,
}

#[derive(Clone, Deserialize)]
pub struct Transaction {
    pub uuid: String,
    pub txid: String,
    pub fee: u64,
    pub fee_rate: u64,
    pub block_height: Option<i32>,
    pub confirmed_at: Option<i64>,
    pub label: Option<String>,
    #[serde(deserialize_with = "deser_hex")]
    pub raw: bitcoin::Transaction,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    /// If the transaction has multiple incoming or outgoing payments.
    pub is_batch: bool,
}

#[derive(Deserialize)]
pub struct ListTransactions {
    pub transactions: Vec<Transaction>,
}

#[derive(Clone, Deserialize)]
pub struct Output {
    pub address: Option<String>,
    pub label: Option<String>,
    pub address_label: Option<String>,
    pub amount: u64,
    pub kind: UTXOKind,
    pub coin: Option<Coin>,
}

#[derive(Clone, Deserialize)]
pub struct Input {
    pub txid: String,
    pub vout: usize,
    pub amount: Option<u64>,
    pub label: Option<String>,
    pub kind: UTXOKind,
    pub coin: Option<Coin>,
}

#[derive(Clone, Deserialize)]
pub struct Psbt {
    pub uuid: String,
    pub txid: Txid,
    pub fee: Option<u64>,
    pub fee_rate: Option<u64>,
    pub label: Option<String>,
    #[serde(deserialize_with = "deser_fromstr")]
    pub raw: bitcoin::Psbt,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub is_batch: bool,
    pub updated_at: i64,
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub enum DraftPsbtResult {
    Success(DraftPsbt),
    InsufficientFunds(InsufficientFundsInfo),
    Error(DraftPsbtError),
}

#[derive(Clone, Deserialize)]
pub struct InsufficientFundsInfo {
    pub missing: u64,
}

#[derive(Clone, Deserialize)]
pub struct DraftPsbtError {
    pub error: String,
}

#[derive(Clone, Deserialize)]
pub struct DraftPsbt {
    pub uuid: Option<String>,
    pub txid: Txid,
    pub fee: u64,
    pub fee_rate: u64,
    pub label: Option<String>,
    #[serde(deserialize_with = "deser_fromstr")]
    pub raw: bitcoin::Psbt,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub warnings: Vec<String>,
}

#[derive(Deserialize)]
pub struct ListPsbts {
    pub psbts: Vec<Psbt>,
}

#[derive(Deserialize)]
pub struct Labels {
    pub labels: coincubed::bip329::Labels,
}

#[derive(Deserialize)]
pub struct Address {
    #[serde(deserialize_with = "deser_addr_assume_checked")]
    pub address: bitcoin::Address,
    pub derivation_index: bip32::ChildNumber,
}

#[derive(Deserialize)]
pub struct RevealedAddress {
    #[serde(deserialize_with = "deser_addr_assume_checked")]
    pub address: bitcoin::Address,
    pub derivation_index: bip32::ChildNumber,
    pub label: Option<String>,
    pub used_count: u32,
}

#[derive(Deserialize)]
pub struct ListRevealedAddresses {
    pub addresses: Vec<RevealedAddress>,
    pub continue_from: Option<bip32::ChildNumber>,
}

pub mod payload {
    use coincube_core::{descriptors::CoincubeDescriptor, miniscript::bitcoin};
    use serde::{Serialize, Serializer};

    pub fn ser_to_string<T: std::fmt::Display, S: Serializer>(
        field: T,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        s.serialize_str(&field.to_string())
    }

    #[derive(Serialize)]
    pub struct Provider {
        pub uuid: String,
        pub name: String,
    }

    #[derive(Serialize)]
    pub struct ProviderKey {
        pub fingerprint: String,
        pub uuid: String,
        pub token: String,
        pub provider: Provider,
    }

    #[derive(Serialize)]
    pub struct CreateWallet<'a> {
        pub name: &'a str,
        #[serde(serialize_with = "ser_to_string")]
        pub descriptor: &'a CoincubeDescriptor,
        pub provider_keys: &'a Vec<ProviderKey>,
    }

    #[derive(Serialize)]
    pub struct CreateWalletInvitation<'a> {
        pub email: &'a str,
    }

    #[derive(Serialize)]
    pub struct ImportPsbt {
        pub psbt: String,
    }

    #[derive(Serialize)]
    pub struct Recipient {
        /// Recipient cannot have an empty amount and is_max set to false
        /// Amount cannot be less that the DUST limit.
        pub amount: Option<u64>,
        pub address: bitcoin::Address<bitcoin::address::NetworkUnchecked>,
        /// If is_max is set to true, API will calculate the remaining funds and
        /// use it for psbt output amount.
        /// Only one recipient can have is_max set to true
        pub is_max: bool,
    }

    #[derive(Serialize)]
    pub struct GeneratePsbt<'a> {
        pub recipients: Vec<Recipient>,
        /// The outpoints of coins to use as transaction inputs. If empty,
        /// coins will be selected automatically from the set of confirmed coins
        /// and those unconfirmed coins that are from self, excluding immature
        /// coins.
        pub inputs: &'a [bitcoin::OutPoint],
        // The feerate to use for this transaction.
        pub feerate: u64,
        /// If save is set to true, API will save in database the generated psbt
        /// and store the generated change address.
        pub save: bool,
    }

    #[derive(Serialize)]
    pub struct GenerateRecoveryPsbt<'a> {
        /// The address to sweep funds to.
        pub address: bitcoin::Address<bitcoin::address::NetworkUnchecked>,
        /// The outpoints of coins to use as transaction inputs. If empty, all
        /// coins that are recoverable on the chosen recovery path will be used.
        pub inputs: &'a [bitcoin::OutPoint],
        // The feerate to use for this transaction.
        pub feerate: u64,
        /// Timelock of the recovery path to use.
        pub timelock: u16,
        /// If save is set to true, API will save in database the generated psbt
        /// and store the generated change address.
        pub save: bool,
    }

    #[derive(Serialize)]
    pub struct Labels {
        pub labels: Vec<Label>,
    }

    #[derive(Serialize)]
    pub struct Label {
        pub item: String,
        pub value: Option<String>,
    }

    #[derive(Serialize)]
    pub struct GenerateRbfPsbt {
        /// ID of the transaction to be replaced.
        #[serde(serialize_with = "ser_to_string")]
        pub txid: bitcoin::Txid,
        /// The target feerate (sat/vb) to use for the replacement transaction
        /// in order to bump the fee of the transaction being replaced.
        ///
        /// Must be provided if and only if `is_cancel` is `false`.
        pub feerate: Option<u64>,
        /// Whether to cancel the transaction.
        ///
        /// If `true`, the feerate of the replacement transaction will be set
        /// automatically to the lowest possible feerate that satisfies all
        /// RBF policies.
        ///
        /// If `false`, the transaction will be replaced by another at the target
        /// `feerate` in order to bump its fee.
        pub is_cancel: bool,
        /// If save is set to true, API will save in database the generated psbt
        /// and, if a new change address is generated for the replacement, store
        /// this also. Note that if the transaction being replaced has a change
        /// output, then its corresponding change address will be reused in the
        /// replacement.
        pub save: bool,
    }

    #[derive(Serialize)]
    pub struct UpdateWallet {
        pub alias: Option<String>,
        pub ledger_hmac: Option<UpdateLedgerHmac>,
        pub fingerprint_aliases: Option<Vec<UpdateFingerprintAlias>>,
    }

    #[derive(Serialize)]
    pub struct UpdateLedgerHmac {
        pub fingerprint: String,
        pub hmac: String,
    }

    #[derive(Serialize)]
    pub struct UpdateFingerprintAlias {
        pub fingerprint: String,
        pub alias: String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::{
        absolute, consensus::encode::serialize_hex, transaction::Version as TxVersion, Address,
        ScriptBuf, Sequence, Transaction as BitcoinTransaction, TxIn, TxOut, Witness,
    };
    use serde_json::json;

    const MAINNET_ADDRESS: &str = "bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv";
    const ZERO_TXID: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const ONE_TXID: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    #[derive(Deserialize)]
    struct AddressProbe {
        #[serde(deserialize_with = "deser_addr_assume_checked")]
        address: bitcoin::Address,
    }

    #[derive(Deserialize)]
    struct AmountProbe {
        #[serde(deserialize_with = "deser_amount_from_sats")]
        amount: Amount,
    }

    #[derive(Deserialize)]
    struct TransactionProbe {
        #[serde(deserialize_with = "deser_hex")]
        tx: BitcoinTransaction,
    }

    fn dummy_transaction() -> BitcoinTransaction {
        BitcoinTransaction {
            version: TxVersion::TWO,
            lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
            input: vec![TxIn {
                previous_output: OutPoint::from_str(&format!("{ZERO_TXID}:0")).unwrap(),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(1_000),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    #[test]
    fn custom_deserializers_parse_checked_address_amount_and_raw_tx_hex() {
        let address: AddressProbe = serde_json::from_value(json!({
            "address": MAINNET_ADDRESS
        }))
        .unwrap();
        assert_eq!(address.address.to_string(), MAINNET_ADDRESS);

        let amount: AmountProbe = serde_json::from_value(json!({ "amount": 12_345 })).unwrap();
        assert_eq!(amount.amount, Amount::from_sat(12_345));

        let tx = dummy_transaction();
        let parsed: TransactionProbe = serde_json::from_value(json!({
            "tx": serialize_hex(&tx)
        }))
        .unwrap();
        assert_eq!(parsed.tx, tx);
    }

    #[test]
    fn custom_deserializers_reject_invalid_wire_values() {
        assert!(serde_json::from_value::<AddressProbe>(json!({
            "address": "not an address"
        }))
        .is_err());
        assert!(serde_json::from_value::<AmountProbe>(json!({
            "amount": -1
        }))
        .is_err());
        assert!(serde_json::from_value::<TransactionProbe>(json!({
            "tx": "not hex"
        }))
        .is_err());
    }

    #[test]
    fn enum_wire_values_are_lowercase() {
        let status: WalletStatus = serde_json::from_str("\"recovering\"").unwrap();
        assert!(matches!(status, WalletStatus::Recovering));

        let role: UserRole = serde_json::from_str("\"owner\"").unwrap();
        assert!(matches!(role, UserRole::Owner));

        let invitation_status: WalletInvitationStatus =
            serde_json::from_str("\"accepted\"").unwrap();
        assert!(matches!(
            invitation_status,
            WalletInvitationStatus::Accepted
        ));

        let payment_kind: PaymentKind = serde_json::from_str("\"incoming\"").unwrap();
        assert!(matches!(payment_kind, PaymentKind::Incoming));

        let utxo_kind: UTXOKind = serde_json::from_str("\"change\"").unwrap();
        assert!(matches!(utxo_kind, UTXOKind::Change));
    }

    #[test]
    fn coin_deserialization_maps_backend_fields_to_typed_values() {
        let coin: Coin = serde_json::from_value(json!({
            "address": MAINNET_ADDRESS,
            "amount": 50_000,
            "derivation_index": 7,
            "outpoint": format!("{ZERO_TXID}:1"),
            "block_height": 800_000,
            "spend_info": {
                "txid": ONE_TXID,
                "height": 800_100
            },
            "is_immature": false,
            "is_change_address": true,
            "is_from_self": true
        }))
        .unwrap();

        assert_eq!(coin.address.to_string(), MAINNET_ADDRESS);
        assert_eq!(coin.amount, Amount::from_sat(50_000));
        assert_eq!(coin.derivation_index, bip32::ChildNumber::from(7));
        assert_eq!(coin.outpoint.to_string(), format!("{ZERO_TXID}:1"));
        assert_eq!(coin.block_height, Some(800_000));
        assert_eq!(coin.spend_info.as_ref().unwrap().txid.to_string(), ONE_TXID);
        assert_eq!(coin.spend_info.as_ref().unwrap().height, Some(800_100));
        assert!(coin.is_change_address);
        assert!(coin.is_from_self);
        assert!(!coin.is_immature);
    }

    #[test]
    fn revealed_addresses_deserialize_checked_addresses_and_continue_cursor() {
        let list: ListRevealedAddresses = serde_json::from_value(json!({
            "addresses": [{
                "address": MAINNET_ADDRESS,
                "derivation_index": 5,
                "label": "savings",
                "used_count": 2
            }],
            "continue_from": 4
        }))
        .unwrap();

        assert_eq!(list.addresses.len(), 1);
        assert_eq!(list.addresses[0].address.to_string(), MAINNET_ADDRESS);
        assert_eq!(
            list.addresses[0].derivation_index,
            bip32::ChildNumber::from(5)
        );
        assert_eq!(list.addresses[0].label.as_deref(), Some("savings"));
        assert_eq!(list.addresses[0].used_count, 2);
        assert_eq!(list.continue_from, Some(bip32::ChildNumber::from(4)));
    }

    #[test]
    fn transaction_deserialization_decodes_raw_hex_and_nested_io() {
        let tx = dummy_transaction();
        let transaction: Transaction = serde_json::from_value(json!({
            "uuid": "tx-uuid",
            "txid": ZERO_TXID,
            "fee": 123,
            "fee_rate": 4,
            "block_height": null,
            "confirmed_at": null,
            "label": "payroll",
            "raw": serialize_hex(&tx),
            "inputs": [{
                "txid": ONE_TXID,
                "vout": 0,
                "amount": 1000,
                "label": null,
                "kind": "external",
                "coin": null
            }],
            "outputs": [{
                "address": MAINNET_ADDRESS,
                "label": null,
                "address_label": "deposit",
                "amount": 877,
                "kind": "deposit",
                "coin": null
            }],
            "is_batch": false
        }))
        .unwrap();

        assert_eq!(transaction.uuid, "tx-uuid");
        assert_eq!(transaction.txid, ZERO_TXID);
        assert_eq!(transaction.raw, tx);
        assert!(matches!(transaction.inputs[0].kind, UTXOKind::External));
        assert!(matches!(transaction.outputs[0].kind, UTXOKind::Deposit));
        assert!(!transaction.is_batch);
    }

    #[test]
    fn payload_serializers_preserve_backend_wire_shape() {
        let address = Address::from_str(MAINNET_ADDRESS).unwrap();
        let recipient = payload::Recipient {
            amount: Some(10_000),
            address,
            is_max: false,
        };
        assert_eq!(
            serde_json::to_value(&recipient).unwrap(),
            json!({
                "amount": 10_000,
                "address": MAINNET_ADDRESS,
                "is_max": false
            })
        );

        let rbf = payload::GenerateRbfPsbt {
            txid: Txid::from_str(ZERO_TXID).unwrap(),
            feerate: None,
            is_cancel: true,
            save: true,
        };
        assert_eq!(
            serde_json::to_value(&rbf).unwrap(),
            json!({
                "txid": ZERO_TXID,
                "feerate": null,
                "is_cancel": true,
                "save": true
            })
        );

        let update = payload::UpdateWallet {
            alias: Some("Family Vault".to_string()),
            ledger_hmac: Some(payload::UpdateLedgerHmac {
                fingerprint: "aabbccdd".to_string(),
                hmac: "hmac-token".to_string(),
            }),
            fingerprint_aliases: Some(vec![payload::UpdateFingerprintAlias {
                fingerprint: "11223344".to_string(),
                alias: "Alice".to_string(),
            }]),
        };
        assert_eq!(
            serde_json::to_value(&update).unwrap(),
            json!({
                "alias": "Family Vault",
                "ledger_hmac": {
                    "fingerprint": "aabbccdd",
                    "hmac": "hmac-token"
                },
                "fingerprint_aliases": [{
                    "fingerprint": "11223344",
                    "alias": "Alice"
                }]
            })
        );
    }
}
