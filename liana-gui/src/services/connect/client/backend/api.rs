use std::collections::HashMap;
use std::str::FromStr;

use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{self, bip32, consensus, hashes::hex::FromHex, Amount, OutPoint, Txid},
};
use serde::{de, Deserialize, Deserializer};

pub fn deser_fromstr<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    <T as FromStr>::Err: std::fmt::Display,
{
    let string = String::deserialize(deserializer)?;
    T::from_str(&string).map_err(de::Error::custom)
}

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
    pub descriptor: LianaDescriptor,
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
    /// If the transaction has multiple incoming or ougoing payment.
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
    pub labels: lianad::bip329::Labels,
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
    use liana::{descriptors::LianaDescriptor, miniscript::bitcoin};
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
        pub descriptor: &'a LianaDescriptor,
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
