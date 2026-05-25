use std::collections::HashMap;
use std::str::FromStr;

use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{self, bip32, consensus, hashes::hex::FromHex, Amount, OutPoint, Txid},
};
use serde::{de, Deserialize, Deserializer, Serialize};

pub fn deser_fromstr<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: std::fmt::Display,
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

/// Serialize any `Display` type as its string form.
pub fn ser_to_string<T: std::fmt::Display, S: serde::Serializer>(
    field: &T,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_str(&field.to_string())
}

/// Serialize an amount as integer sats.
pub fn ser_amount_to_sats<S: serde::Serializer>(amount: &Amount, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(amount.to_sat())
}

/// Serialize a derivation index as a bare integer.
pub fn ser_childnumber<S: serde::Serializer>(
    index: &bip32::ChildNumber,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_u32(u32::from(*index))
}

/// Serialize an optional derivation index as an optional bare integer.
pub fn ser_opt_childnumber<S: serde::Serializer>(
    index: &Option<bip32::ChildNumber>,
    s: S,
) -> Result<S::Ok, S::Error> {
    match index {
        Some(i) => s.serialize_some(&u32::from(*i)),
        None => s.serialize_none(),
    }
}

/// Serialize a consensus-encodable value as a lowercase hex string.
pub fn ser_hex<T: consensus::Encodable, S: serde::Serializer>(
    value: &T,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_str(&consensus::encode::serialize_hex(value))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum FiatCurrency {
    None,
    USD,
    EUR,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct UserSettings {
    pub fiat_currency: FiatCurrency,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct NetworkInfo {
    pub feerate: Feerate,
    #[cfg_attr(feature = "schema", schema(example = "{\"BTCEUR\": 34033.98 }"))]
    pub rates: HashMap<String, f32>,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Feerate {
    #[cfg_attr(feature = "schema", schema(example = "1"))]
    pub low: Option<i32>,
    #[cfg_attr(feature = "schema", schema(example = "null"))]
    pub high: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct WalletBalance {
    /// Total of funds that present in a block.
    #[cfg_attr(feature = "schema", schema(example = "213546879"))]
    pub confirmed: u64,
    /// Total of funds that is not yet in a block.
    #[cfg_attr(feature = "schema", schema(example = "213546"))]
    pub unconfirmed: u64,
    /// Total of funds that are mined but not yet available
    #[cfg_attr(feature = "schema", schema(example = "213546"))]
    pub immature: u64,
    /// Total of funds that are unconfirmed but are coming from
    /// the wallet
    #[cfg_attr(feature = "schema", schema(example = "213546"))]
    pub unconfirmed_change: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum WalletStatus {
    Normal,
    Recovering,
    Recovered,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct RecoveryPath {
    pub sequence: u16,
    pub available_balance: u64,
    pub total_coins: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Wallet {
    #[cfg_attr(
        feature = "schema",
        schema(example = "4fe7201d-a3ae-4d01-aee0-f03dd2ae22c9")
    )]
    pub id: String,
    #[cfg_attr(
        feature = "schema",
        schema(example = "f2e240ea-7eae-4a2f-a8b3-31f06e6fb7ce")
    )]
    pub org_id: String,
    #[cfg_attr(feature = "schema", schema(example = "My first liana wallet"))]
    pub name: String,
    #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
    #[cfg_attr(
        feature = "schema",
        schema(
            value_type = String,
            example = "wsh(andor(pk([abcdef01]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(65535),pk([abcdef01]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#lvqp9ta4"
        )
    )]
    pub descriptor: LianaDescriptor,
    pub deposit_derivation_index: u32,
    pub change_derivation_index: u32,
    pub recovery_paths: Vec<RecoveryPath>,
    pub biggest_remaining_sequence: Option<u32>,
    pub smallest_remaining_sequence: Option<u32>,
    pub metadata: WalletMetadata,
    #[cfg_attr(feature = "schema", schema(example = 1689000284))]
    pub created_at: i64,
    #[cfg_attr(feature = "schema", schema(example = 213546879))]
    pub balance: WalletBalance,
    #[cfg_attr(feature = "schema", schema(example = "normal"))]
    pub status: WalletStatus,
    #[cfg_attr(feature = "schema", schema(example = "841353"))]
    pub tip_height: Option<i32>,
    pub participants: HashMap<String, UserRole>,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ListWallets {
    pub wallets: Vec<Wallet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Owner,
    Member,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ListWalletMembers {
    pub members: Vec<Member>,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Member {
    #[cfg_attr(
        feature = "schema",
        schema(example = "4fe7201d-a3ae-4d01-aee0-f03dd2ae22c9")
    )]
    pub user_id: String,
    #[cfg_attr(feature = "schema", schema(example = "owner"))]
    pub role: UserRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Provider {
    /// Provider ID.
    #[cfg_attr(
        feature = "schema",
        schema(example = "4fe7201d-a3ae-4d01-aee0-f03dd2ae22c9")
    )]
    pub uuid: String,
    /// Provider name.
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ProviderKey {
    /// Key fingerprint.
    #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "ea48bfd2"))]
    pub fingerprint: bip32::Fingerprint,
    /// Key ID.
    #[cfg_attr(
        feature = "schema",
        schema(example = "4fe7201d-a3ae-4d01-aee0-f03dd2ae22c9")
    )]
    pub uuid: String,
    /// Token used to fetch and redeem key.
    pub token: String,
    /// Key provider.
    pub provider: Provider,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct WalletMetadata {
    pub wallet_alias: Option<String>,
    pub ledger_hmacs: Vec<LedgerHmac>,
    pub fingerprint_aliases: Vec<FingerprintAlias>,
    pub provider_keys: Vec<ProviderKey>,
    pub registrations: Vec<Registration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Registration {
    pub did_register: bool,
    pub device_fingerprint: String,
    pub registered_wallet_alias: Option<String>,
}

pub const WALLET_ALIAS_MAXIMUM_LENGTH: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct LedgerHmac {
    #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "ea48bfd2"))]
    pub fingerprint: bip32::Fingerprint,
    #[cfg_attr(
        feature = "schema",
        schema(example = "4fe7201d-a3ae-4d01-aee0-f03dd2ae22c9")
    )]
    pub user_id: String,
    #[cfg_attr(
        feature = "schema",
        schema(example = "dae925660e20859ed8833025d46444483ce264fdb77e34569aabe9d590da8fb7")
    )]
    pub hmac: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct FingerprintAlias {
    #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "ea48bfd2"))]
    pub fingerprint: bip32::Fingerprint,
    /// User id of the author of the alias, not necessarily the fingerprint key owner.
    #[cfg_attr(
        feature = "schema",
        schema(example = "4fe7201d-a3ae-4d01-aee0-f03dd2ae22c9")
    )]
    pub user_id: String,
    #[cfg_attr(feature = "schema", schema(example = "Edouard key"))]
    pub alias: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum WalletInvitationStatus {
    Pending,
    Accepted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct WalletInvitation {
    #[cfg_attr(
        feature = "schema",
        schema(example = "4fe7201d-a3ae-4d01-aee0-f03dd2ae22c9")
    )]
    pub id: String,
    #[cfg_attr(
        feature = "schema",
        schema(example = "4fe7201d-a3ae-4d01-aee0-f03dd2ae22c9")
    )]
    pub wallet_id: String,
    #[cfg_attr(feature = "schema", schema(example = "My first liana wallet"))]
    pub wallet_name: String,
    /// A descriptor policy without the xpubs
    #[cfg_attr(
        feature = "schema",
        schema(
            example = "wsh(andor(pk([abcdef01]/<0;1>/*),older(65535),pk([abcdef01]/<0;1>/*)))#lvqp9ta4"
        )
    )]
    pub descriptor_summary: String,
    #[cfg_attr(feature = "schema", schema(example = "expired"))]
    pub status: WalletInvitationStatus,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct WalletLabels {
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum PaymentKind {
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Payment {
    #[cfg_attr(
        feature = "schema",
        schema(example = "4fe7201d-a3ae-4d01-aee0-f03dd2ae22c9")
    )]
    pub txuuid: String,
    #[cfg_attr(
        feature = "schema",
        schema(example = "c7e9bd82a6218aaf50d86034da7f4afee01222d09c69a4568c9c12a8aa8bd7bf")
    )]
    pub txid: String,
    #[cfg_attr(feature = "schema", schema(example = 1))]
    pub vout: u32,
    #[cfg_attr(feature = "schema", schema(example = 69420))]
    pub amount: u64,
    #[cfg_attr(feature = "schema", schema(example = 798123))]
    pub block_height: Option<i32>,
    #[cfg_attr(feature = "schema", schema(example = 1689000284))]
    pub confirmed_at: Option<i64>,
    #[cfg_attr(feature = "schema", schema(example = "Baguette"))]
    pub label: Option<String>,
    #[cfg_attr(feature = "schema", schema(example = "Baguette"))]
    pub address_label: Option<String>,
    #[cfg_attr(feature = "schema", schema(example = "Baguette"))]
    pub transaction_label: Option<String>,
    #[cfg_attr(feature = "schema", schema(example = "outgoing"))]
    pub kind: PaymentKind,
    /// false, if the payment is part of a batch transaction.
    #[cfg_attr(feature = "schema", schema(example = "false"))]
    pub is_single: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ListPayments {
    pub payments: Vec<Payment>,
    /// The timestamp to use to get the next page.
    /// Timestamp of the last transaction in the list.
    pub next_cursor: Option<i64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Coin {
    #[serde(
        serialize_with = "ser_to_string",
        deserialize_with = "deser_addr_assume_checked"
    )]
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "tb1qsecwgyr0ltmvk5ty2x7jg8egp2z3heuzat879j"))]
    pub address: bitcoin::Address,
    #[serde(
        serialize_with = "ser_amount_to_sats",
        deserialize_with = "deser_amount_from_sats"
    )]
    #[cfg_attr(feature = "schema", schema(value_type = u64, example = 1_023_423))]
    pub amount: Amount,
    #[serde(serialize_with = "ser_childnumber")]
    #[cfg_attr(feature = "schema", schema(value_type = u32, example = 45))]
    pub derivation_index: bip32::ChildNumber,
    #[serde(serialize_with = "ser_to_string")]
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "c7e9bd82a6218aaf50d86034da7f4afee01222d09c69a4568c9c12a8aa8bd7bf:3"))]
    pub outpoint: OutPoint,
    #[cfg_attr(feature = "schema", schema(example = 812_034))]
    pub block_height: Option<i32>,
    pub spend_info: Option<CoinSpendInfo>,
    #[cfg_attr(feature = "schema", schema(example = false))]
    pub is_immature: bool,
    #[cfg_attr(feature = "schema", schema(example = true))]
    pub is_change_address: bool,
    #[cfg_attr(feature = "schema", schema(example = false))]
    pub is_from_self: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CoinSpendInfo {
    #[serde(serialize_with = "ser_to_string")]
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "6f0dc85a369b44458eba3a1f0ea5b5935d563afb6994f70f5b0094e05be1676c"))]
    pub txid: Txid,
    #[cfg_attr(feature = "schema", schema(example = 814_491))]
    pub height: Option<i32>,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ListCoins {
    pub coins: Vec<Coin>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum UTXOKind {
    Deposit,
    Change,
    External,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Transaction {
    #[cfg_attr(
        feature = "schema",
        schema(example = "4fe7201d-a3ae-4d01-aee0-f03dd2ae22c9")
    )]
    pub uuid: String,
    #[cfg_attr(
        feature = "schema",
        schema(example = "c7e9bd82a6218aaf50d86034da7f4afee01222d09c69a4568c9c12a8aa8bd7bf")
    )]
    pub txid: String,
    #[cfg_attr(feature = "schema", schema(example = 420))]
    pub fee: u64,
    #[cfg_attr(feature = "schema", schema(example = 10))]
    pub fee_rate: u64,
    #[cfg_attr(feature = "schema", schema(example = 798123))]
    pub block_height: Option<i32>,
    #[cfg_attr(feature = "schema", schema(example = 1689000284))]
    pub confirmed_at: Option<i64>,
    #[cfg_attr(feature = "schema", schema(example = "Baguette"))]
    pub label: Option<String>,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    /// hex encoding format of the transaction.
    #[serde(serialize_with = "ser_hex", deserialize_with = "deser_hex")]
    #[cfg_attr(feature = "schema", schema(value_type = String))]
    pub raw: bitcoin::Transaction,
    /// If the transaction has multiple incoming or ougoing payment.
    pub is_batch: bool,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ListTransactions {
    pub transactions: Vec<Transaction>,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Output {
    #[cfg_attr(feature = "schema", schema(example = ""))]
    pub address: Option<String>,
    #[cfg_attr(feature = "schema", schema(example = "Salary John Smith"))]
    pub label: Option<String>,
    #[cfg_attr(feature = "schema", schema(example = "Salary John Smith"))]
    pub address_label: Option<String>,
    #[cfg_attr(feature = "schema", schema(example = 123456))]
    pub amount: u64,
    #[cfg_attr(feature = "schema", schema(example = "external"))]
    pub kind: UTXOKind,
    /// Coin is null if the parent resource is a psbt or the output is external of the wallet.
    pub coin: Option<Coin>,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Input {
    #[cfg_attr(
        feature = "schema",
        schema(example = "c7e9bd82a6218aaf50d86034da7f4afee01222d09c69a4568c9c12a8aa8bd7bf")
    )]
    pub txid: String,
    #[cfg_attr(feature = "schema", schema(example = 1))]
    pub vout: usize,
    #[cfg_attr(feature = "schema", schema(example = 123456))]
    pub amount: Option<u64>,
    #[cfg_attr(feature = "schema", schema(example = "my salary of last month"))]
    pub label: Option<String>,
    #[cfg_attr(feature = "schema", schema(example = "deposit"))]
    pub kind: UTXOKind,
    /// Coin is null if the parent resource is a draft psbt or the input is external of the wallet.
    pub coin: Option<Coin>,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Psbt {
    #[cfg_attr(
        feature = "schema",
        schema(example = "4fe7201d-a3ae-4d01-aee0-f03dd2ae22c9")
    )]
    pub uuid: String,
    #[serde(serialize_with = "ser_to_string")]
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "c7e9bd82a6218aaf50d86034da7f4afee01222d09c69a4568c9c12a8aa8bd7bf"))]
    pub txid: Txid,
    #[cfg_attr(feature = "schema", schema(example = 420))]
    pub fee: Option<u64>,
    #[cfg_attr(feature = "schema", schema(example = 10))]
    pub fee_rate: Option<u64>,
    #[cfg_attr(feature = "schema", schema(example = 798123))]
    pub label: Option<String>,
    #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "BcHNidP8BAH0CAAAAAetVCFeFtc4auSBP31tQlsSckSIcC8noPd6los5CUw6mAQAAAAD9////AoA4AQAAAAAAFgAUQnxKBIi3WlQ2ZDZoKxxdraH+yCYMRAAAAAAAACIAIGftqBNLg5w5HQL9fOJ/tM/tuwaWhpI/Yw95vjuKsEmiAAAAAAABAM0CAAAAAAEBMNemgjcYcofbyKepwTrGJvXCHdmZ4zYXiFLTXtWGByAAAAAAAP3///8CIQ0MAQAAAAAiUSAObLFvxb/Ru+wErab2Kn1ZQBlkOnTNHbn4gG1CqjfrjqCGAQAAAAAAIgAgPln1Iwu6CQsQ0HLsI7mspT8nFVHupAZdUbN3GOt8yokBQKLjVYtEsR3TftWIjUcNCWosIqKl0dATawTM8IaQ7WytEukM3wq5PvPAx52VTwR7fXDM644RF91vxJwZLXxZJl3gMAIAAQEroIYBAAAAAAAiACA+WfUjC7oJCxDQcuwjuaylPycVUe6kBl1Rs3cY63zKiSICAgsJFuUFq1kmsGZHyOBj5LkHzmb9vnSnkvwnfqx3lTf8SDBFAiEAwfKO6TdvtRFsFfGO15S8KAItZu3pwQN3lMXHUhO7OlsCIAYNsUBVPHCGMyocpLiVFx5ZiuLHiZWtB59R8JmBxvq+ASICAvFlw9KXZJK7Qr0ifD1vq1NeRxYt6/wfKCfFlZyJwOzaRzBEAiASjgdnh/ZkR05kVGCDdi/Z9Q5PsPhoIZvTsMHKV1dKxQIgVuCPr5tGd0ikrpg5nC5/eDPazLkkUTPhoT74yDMCXJcBIgIDXp6KYklUUZyRaDoaPTcYw9IqT2bRk2TUmWSwpjGjYFhIMEUCIQDFJ8ueAvyyxZyT36aRP+eL5AV++dOf3mymFuKPoET2SAIgP036F7LTjf0jp3pX/oR8vgQL/fEINQCMJy4MxCsz9dwBIgIDacUDu9MDmexcRnW8mAYYpoWcPjVdZcBvJv96B3RY7PtIMEUCIQCVgqf0QGVdJd62v6o40wjhHzII/pS107PrvOr8sHU1iQIgTCZE5ZZVSWqGNMnVWDFOPgiQFpioo5+rro+xKzG+inUBAQWGUiECCwkW5QWrWSawZkfI4GPkuQfOZv2+dKeS/Cd+rHeVN/whA16eimJJVFGckWg6Gj03GMPSKk9m0ZNk1JlksKYxo2BYUq5zZHapFFMC9YAQLbOckH7Gdk5vLsx5zXNoiKxrdqkU2CGMFd5bvvBje05ZERRUkCdJwouIrGyTUYgDYYoAsmgiBgILCRblBatZJrBmR8jgY+S5B85m/b50p5L8J36sd5U3/BzBENoTMAAAgAEAAIAAAACAAgAAgAAAAAAAAAAAIgYC8WXD0pdkkrtCvSJ8PW+rU15HFi3r/B8oJ8WVnInA7Noc/9Y8jTAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA16eimJJVFGckWg6Gj03GMPSKk9m0ZNk1JlksKYxo2BYHP/WPI0wAACAAQAAgAEAAIACAACAAAAAAAAAAAAiBgNpxQO70wOZ7FxGdbyYBhimhZw+NV1lwG8m/3oHdFjs+xzBENoTMAAAgAEAAIABAACAAgAAgAAAAAAAAAAAAAAiAgJ5DBmvNgzqYu7WrTrNOv8I2+Tm5tj+Mg7e1180g8x0WBz/1jyNMAAAgAEAAIAAAACAAgAAgAEAAAAUAAAAIgICiChgQxYuslWyXviiCFjZWpIYLKOyojdWpAbibRzvEIIcwRDaEzAAAIABAACAAAAAgAIAAIABAAAAFAAAACICA4br5TDELJzaqRzWnsIU5+e1GYQxh5L5/iIMo+LSpdo7HP/WPI0wAACAAQAAgAEAAIACAACAAQAAABQAAAAiAgP4nuKX/J/9tmBm51wcaTk2OAquPtiq4HrU8lrC8aM8GRzBENoTMAAAgAEAAIABAACAAgAAgAEAAAAUAAAAAA=="))]
    pub raw: bitcoin::Psbt,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub is_batch: bool,
    #[cfg_attr(feature = "schema", schema(example = 1689000284))]
    pub updated_at: i64,
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub enum DraftPsbtErrors {
    InsufficientFunds(InsufficientFundsInfo),
    Error(DraftPsbtError),
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct InsufficientFundsInfo {
    #[cfg_attr(feature = "schema", schema(example = 120))]
    pub missing: u64,
}

#[derive(Clone, Deserialize)]
pub struct DraftPsbtError {
    pub error: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DraftPsbt {
    #[cfg_attr(
        feature = "schema",
        schema(example = "4fe7201d-a3ae-4d01-aee0-f03dd2ae22c9")
    )]
    pub uuid: Option<String>,
    #[serde(serialize_with = "ser_to_string")]
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "c7e9bd82a6218aaf50d86034da7f4afee01222d09c69a4568c9c12a8aa8bd7bf"))]
    pub txid: Txid,
    #[cfg_attr(feature = "schema", schema(example = 420))]
    pub fee: u64,
    #[cfg_attr(feature = "schema", schema(example = 10))]
    pub fee_rate: u64,
    #[cfg_attr(feature = "schema", schema(example = 798123))]
    pub label: Option<String>,
    #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "BcHNidP8BAH0CAAAAAetVCFeFtc4auSBP31tQlsSckSIcC8noPd6los5CUw6mAQAAAAD9////AoA4AQAAAAAAFgAUQnxKBIi3WlQ2ZDZoKxxdraH+yCYMRAAAAAAAACIAIGftqBNLg5w5HQL9fOJ/tM/tuwaWhpI/Yw95vjuKsEmiAAAAAAABAM0CAAAAAAEBMNemgjcYcofbyKepwTrGJvXCHdmZ4zYXiFLTXtWGByAAAAAAAP3///8CIQ0MAQAAAAAiUSAObLFvxb/Ru+wErab2Kn1ZQBlkOnTNHbn4gG1CqjfrjqCGAQAAAAAAIgAgPln1Iwu6CQsQ0HLsI7mspT8nFVHupAZdUbN3GOt8yokBQKLjVYtEsR3TftWIjUcNCWosIqKl0dATawTM8IaQ7WytEukM3wq5PvPAx52VTwR7fXDM644RF91vxJwZLXxZJl3gMAIAAQEroIYBAAAAAAAiACA+WfUjC7oJCxDQcuwjuaylPycVUe6kBl1Rs3cY63zKiSICAgsJFuUFq1kmsGZHyOBj5LkHzmb9vnSnkvwnfqx3lTf8SDBFAiEAwfKO6TdvtRFsFfGO15S8KAItZu3pwQN3lMXHUhO7OlsCIAYNsUBVPHCGMyocpLiVFx5ZiuLHiZWtB59R8JmBxvq+ASICAvFlw9KXZJK7Qr0ifD1vq1NeRxYt6/wfKCfFlZyJwOzaRzBEAiASjgdnh/ZkR05kVGCDdi/Z9Q5PsPhoIZvTsMHKV1dKxQIgVuCPr5tGd0ikrpg5nC5/eDPazLkkUTPhoT74yDMCXJcBIgIDXp6KYklUUZyRaDoaPTcYw9IqT2bRk2TUmWSwpjGjYFhIMEUCIQDFJ8ueAvyyxZyT36aRP+eL5AV++dOf3mymFuKPoET2SAIgP036F7LTjf0jp3pX/oR8vgQL/fEINQCMJy4MxCsz9dwBIgIDacUDu9MDmexcRnW8mAYYpoWcPjVdZcBvJv96B3RY7PtIMEUCIQCVgqf0QGVdJd62v6o40wjhHzII/pS107PrvOr8sHU1iQIgTCZE5ZZVSWqGNMnVWDFOPgiQFpioo5+rro+xKzG+inUBAQWGUiECCwkW5QWrWSawZkfI4GPkuQfOZv2+dKeS/Cd+rHeVN/whA16eimJJVFGckWg6Gj03GMPSKk9m0ZNk1JlksKYxo2BYUq5zZHapFFMC9YAQLbOckH7Gdk5vLsx5zXNoiKxrdqkU2CGMFd5bvvBje05ZERRUkCdJwouIrGyTUYgDYYoAsmgiBgILCRblBatZJrBmR8jgY+S5B85m/b50p5L8J36sd5U3/BzBENoTMAAAgAEAAIAAAACAAgAAgAAAAAAAAAAAIgYC8WXD0pdkkrtCvSJ8PW+rU15HFi3r/B8oJ8WVnInA7Noc/9Y8jTAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA16eimJJVFGckWg6Gj03GMPSKk9m0ZNk1JlksKYxo2BYHP/WPI0wAACAAQAAgAEAAIACAACAAAAAAAAAAAAiBgNpxQO70wOZ7FxGdbyYBhimhZw+NV1lwG8m/3oHdFjs+xzBENoTMAAAgAEAAIABAACAAgAAgAAAAAAAAAAAAAAiAgJ5DBmvNgzqYu7WrTrNOv8I2+Tm5tj+Mg7e1180g8x0WBz/1jyNMAAAgAEAAIAAAACAAgAAgAEAAAAUAAAAIgICiChgQxYuslWyXviiCFjZWpIYLKOyojdWpAbibRzvEIIcwRDaEzAAAIABAACAAAAAgAIAAIABAAAAFAAAACICA4br5TDELJzaqRzWnsIU5+e1GYQxh5L5/iIMo+LSpdo7HP/WPI0wAACAAQAAgAEAAIACAACAAQAAABQAAAAiAgP4nuKX/J/9tmBm51wcaTk2OAquPtiq4HrU8lrC8aM8GRzBENoTMAAAgAEAAIABAACAAgAAgAEAAAAUAAAAAA=="))]
    pub raw: bitcoin::Psbt,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub warnings: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ListPsbts {
    pub psbts: Vec<Psbt>,
}

#[derive(Deserialize)]
pub struct Labels {
    pub labels: bip329::Labels,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Address {
    #[serde(
        serialize_with = "ser_to_string",
        deserialize_with = "deser_addr_assume_checked"
    )]
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "tb1qsecwgyr0ltmvk5ty2x7jg8egp2z3heuzat879j"))]
    pub address: bitcoin::Address,
    #[serde(serialize_with = "ser_childnumber")]
    #[cfg_attr(feature = "schema", schema(value_type = u32, example = "42"))]
    pub derivation_index: bip32::ChildNumber,
}

/// A wallet address that has been revealed to the user.
///
/// Whether it is a receive or change address will depend on the request.
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct RevealedAddress {
    /// The index used to derive this address from the wallet's descriptor.
    #[serde(serialize_with = "ser_childnumber")]
    #[cfg_attr(feature = "schema", schema(value_type = u32, example = 42))]
    pub derivation_index: bip32::ChildNumber,
    /// The address.
    #[serde(
        serialize_with = "ser_to_string",
        deserialize_with = "deser_addr_assume_checked"
    )]
    #[cfg_attr(feature = "schema", schema(value_type = String, example = "tb1qsecwgyr0ltmvk5ty2x7jg8egp2z3heuzat879j"))]
    pub address: bitcoin::Address,
    /// The label for this address.
    #[cfg_attr(feature = "schema", schema(example = "Baguette"))]
    pub label: Option<String>,
    /// The number of coins currently in the wallet that are using this address.
    #[cfg_attr(feature = "schema", schema(value_type = usize, example = "1"))]
    pub used_count: u32,
}

/// List of revealed addresses.
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ListRevealedAddresses {
    /// Revealed addresses.
    pub addresses: Vec<RevealedAddress>,
    /// For pagination of revealed addresses, this value indicates the value of `start_derivation_index` to pass to the next request.
    /// If `None`, it means that there are no further revealed addresses to list.
    #[serde(serialize_with = "ser_opt_childnumber")]
    #[cfg_attr(feature = "schema", schema(value_type = Option<u32>, example = "42"))]
    pub continue_from: Option<bip32::ChildNumber>,
}

pub mod payload {
    use std::str::FromStr;

    use liana::{
        descriptors::LianaDescriptor,
        miniscript::{
            bitcoin::{
                address::NetworkUnchecked, bip32::Fingerprint, psbt::Psbt, Address, OutPoint, Txid,
            },
            descriptor::DescriptorPublicKey,
        },
    };
    use serde::{de, ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};
    use uuid::Uuid;

    use super::FiatCurrency;

    /// Serialize any `Display` type as its string form.
    pub fn ser_to_string<T: std::fmt::Display, S: Serializer>(
        field: &T,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        s.serialize_str(&field.to_string())
    }

    /// Serialize an unchecked address as its string form.
    pub fn ser_addr<S: Serializer>(
        addr: &Address<NetworkUnchecked>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        s.serialize_str(&addr.clone().assume_checked().to_string())
    }

    /// Deserialize a `FromStr` type from its string form.
    pub fn deser_str<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: FromStr,
        <T as FromStr>::Err: std::fmt::Display,
    {
        let a = String::deserialize(deserializer)?;
        T::from_str(&a).map_err(de::Error::custom)
    }

    /// Deserialize an optional `FromStr` type from its string form.
    #[allow(unused)]
    pub fn deser_opt_str<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: FromStr,
        <T as FromStr>::Err: std::fmt::Display,
    {
        if let Some(a) = Option::<String>::deserialize(deserializer)? {
            Ok(Some(T::from_str(&a).map_err(de::Error::custom)?))
        } else {
            Ok(None)
        }
    }

    /// Information about a provider.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct Provider {
        #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_str")]
        pub uuid: Uuid,
        pub name: String,
    }

    /// Information about a provider's key.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct ProviderKey {
        #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_str")]
        #[cfg_attr(feature = "schema", schema(value_type = String))]
        pub fingerprint: Fingerprint,
        #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_str")]
        pub uuid: Uuid,
        pub token: String,
        pub provider: Provider,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct CreateWallet {
        pub name: String,
        #[serde(flatten)]
        pub template: CreateWalletTemplate,
        #[serde(default)]
        pub provider_keys: Vec<ProviderKey>,
        pub org_id: Option<Uuid>,
    }

    #[allow(clippy::large_enum_variant)]
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    #[serde(untagged)]
    pub enum CreateWalletTemplate {
        RecoveryBuddy {
            #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_str")]
            #[cfg_attr(feature = "schema", schema(value_type = String))]
            xpub: DescriptorPublicKey,
            #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_str")]
            #[cfg_attr(feature = "schema", schema(value_type = String))]
            recovery_xpub: DescriptorPublicKey,
        },
        Descriptor {
            #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_str")]
            #[cfg_attr(feature = "schema", schema(value_type = String))]
            descriptor: LianaDescriptor,
        },
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    #[cfg_attr(feature = "schema", schema(as = CreateInvitation))]
    pub struct CreateWalletInvitation {
        pub email: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct ImportPsbt {
        #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_str")]
        #[cfg_attr(feature = "schema", schema(value_type = String))]
        pub psbt: Psbt,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct Recipient {
        /// Recipient cannot have an empty amount and is_max set to false
        /// Amount cannot be less that the DUST limit.
        pub amount: Option<u64>,
        #[serde(serialize_with = "ser_addr", deserialize_with = "deser_str")]
        #[cfg_attr(feature = "schema", schema(value_type = String))]
        pub address: Address<NetworkUnchecked>,
        /// If is_max is set to true, API will calculate the remaining funds and
        /// use it for psbt output amount.
        /// Only one recipient can have is_max set to true
        #[serde(default)]
        pub is_max: bool,
    }

    /// Feerate to use for a transaction.
    #[derive(Debug, Clone)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub enum FeeratePayload {
        /// Feerate as sat/vb.
        #[cfg_attr(feature = "schema", schema(rename = "feerate"))]
        SatVb(i32),
        /// A high or low feerate as set by the backend.
        #[cfg_attr(feature = "schema", schema(rename = "urgent"))]
        Urgent(bool),
    }

    impl Serialize for FeeratePayload {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut s = serializer.serialize_struct("FeeratePayload", 1)?;
            match self {
                FeeratePayload::SatVb(rate) => s.serialize_field("feerate", rate)?,
                FeeratePayload::Urgent(urgent) => s.serialize_field("urgent", urgent)?,
            }
            s.end()
        }
    }

    pub fn deser_feerate<'de, D>(deserializer: D) -> Result<FeeratePayload, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct FeerateHelper {
            feerate: Option<i32>,
            urgent: Option<bool>,
        }
        let FeerateHelper { feerate, urgent } = FeerateHelper::deserialize(deserializer)?;
        let feerate = match (feerate, urgent) {
            (Some(_), Some(_)) => {
                return Err(de::Error::custom(
                    "must not set both `feerate` and `urgent`",
                ));
            }
            (Some(rate), None) => FeeratePayload::SatVb(rate),
            (None, Some(urgent)) => FeeratePayload::Urgent(urgent),
            (None, None) => {
                return Err(de::Error::custom("must set either `feerate` or `urgent`"));
            }
        };
        Ok(feerate)
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct GeneratePsbt {
        pub recipients: Vec<Recipient>,
        /// The outpoints of coins to use as transaction inputs. If empty,
        /// coins will be selected automatically from the set of confirmed coins
        /// and those unconfirmed coins at a change address, excluding immature
        /// coins.
        #[serde(default)]
        #[cfg_attr(feature = "schema", schema(value_type = Vec<String>))]
        pub inputs: Vec<OutPoint>,
        // The feerate to use for this transaction.
        #[serde(flatten, deserialize_with = "deser_feerate")]
        pub feerate: FeeratePayload,
        /// If save is set to true, API will save in database the generated psbt
        /// and store the generated change address.
        #[serde(default)]
        pub save: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct GenerateRecoveryPsbt {
        #[serde(serialize_with = "ser_addr", deserialize_with = "deser_str")]
        #[cfg_attr(feature = "schema", schema(value_type = String))]
        /// The address to sweep funds to.
        pub address: Address<NetworkUnchecked>,
        /// The outpoints of coins to sweep.
        ///
        /// All outpoints specified must be for coins that are
        /// currently recoverable on the recovery path corresponding
        /// to `timelock`. If no outpoints are specified, all coins
        /// that are currently recoverable on this path will be used.
        #[serde(default)]
        #[cfg_attr(feature = "schema", schema(value_type = Vec<String>))]
        pub inputs: Vec<OutPoint>,
        #[serde(flatten, deserialize_with = "deser_feerate")]
        // The feerate to use for this transaction.
        pub feerate: FeeratePayload,
        /// Timelock of the recovery path to use.
        pub timelock: u16,
        /// If save is set to true, API will save in database the generated psbt
        /// and store the generated change address.
        #[serde(default)]
        pub save: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct Labels {
        pub labels: Vec<Label>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct Label {
        #[cfg_attr(feature = "schema", schema(value_type = String))]
        pub item: String,
        pub value: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct RbfKindPayload {
        /// The target feerate (sat/vb) to use for the replacement transaction
        /// in order to bump the fee of the transaction being replaced.
        ///
        /// Must be provided if and only if `is_cancel` is `false`.
        pub feerate: Option<i32>,
        /// Whether to cancel the transaction.
        ///
        /// If `true`, the feerate of the replacement transaction will be set
        /// automatically to the lowest possible feerate that satisfies all
        /// RBF policies.
        ///
        /// If `false`, the transaction will be replaced by another at the target
        /// `feerate` in order to bump its fee.
        pub is_cancel: bool,
    }

    /// RBF kind.
    #[derive(Debug, Clone)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub enum RbfKind {
        /// Bump transaction fee by replacing with another at the given feerate (sat/vb).
        BumpFee(i32),
        /// Cancel transaction.
        Cancel,
    }

    impl RbfKind {
        pub fn is_cancel(&self) -> bool {
            matches!(self, RbfKind::Cancel)
        }
    }

    impl Serialize for RbfKind {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut s = serializer.serialize_struct("RbfKind", 2)?;
            match self {
                RbfKind::BumpFee(rate) => {
                    s.serialize_field("feerate", &Some(*rate))?;
                    s.serialize_field("is_cancel", &false)?;
                }
                RbfKind::Cancel => {
                    s.serialize_field("feerate", &Option::<i32>::None)?;
                    s.serialize_field("is_cancel", &true)?;
                }
            }
            s.end()
        }
    }

    pub fn deser_rbf_kind<'de, D>(deserializer: D) -> Result<RbfKind, D::Error>
    where
        D: Deserializer<'de>,
    {
        let RbfKindPayload { feerate, is_cancel } = RbfKindPayload::deserialize(deserializer)?;
        let rbf_type = match (feerate, is_cancel) {
            (Some(_), true) => {
                return Err(de::Error::custom(
                    "must not set `feerate` when `is_cancel` is true",
                ));
            }
            (Some(rate), false) => RbfKind::BumpFee(rate),
            (None, true) => RbfKind::Cancel,
            (None, false) => {
                return Err(de::Error::custom(
                    "`feerate` is required when `is_cancel` is false",
                ));
            }
        };
        Ok(rbf_type)
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct GenerateRbfPsbt {
        /// ID of the transaction to be replaced.
        #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_str")]
        #[cfg_attr(feature = "schema", schema(value_type = String, example = "c7e9bd82a6218aaf50d86034da7f4afee01222d09c69a4568c9c12a8aa8bd7bf"))]
        pub txid: Txid,
        #[serde(flatten, deserialize_with = "deser_rbf_kind")]
        #[cfg_attr(feature = "schema", schema(value_type = RbfKindPayload))]
        pub rbf_kind: RbfKind,
        /// If save is set to true, API will save in database the generated psbt
        /// and, if a new change address is generated for the replacement, store
        /// this also. Note that if the transaction being replaced has a change
        /// output, then its corresponding change address will be reused in the
        /// replacement.
        #[serde(default)]
        pub save: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct UpdateWallet {
        // In contrary of name, alias is not linked to descriptor registration on HWs.
        pub alias: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub registration: Option<Registration>,
        pub ledger_hmac: Option<UpdateLedgerHmac>,
        pub fingerprint_aliases: Option<Vec<UpdateFingerprintAlias>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub status: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct Registration {
        pub did_register: bool,
        pub device_fingerprint: String,
        pub registered_wallet_alias: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct UpdateLedgerHmac {
        pub fingerprint: String,
        pub hmac: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct UpdateFingerprintAlias {
        pub fingerprint: String,
        pub alias: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
    pub struct UpdateSettings {
        pub fiat_currency: FiatCurrency,
    }
}
