use crate::descriptors::InheritanceDescriptor;

use std::{convert::TryFrom, str::FromStr};

use miniscript::bitcoin::{
    self,
    consensus::encode,
    util::{bip32, psbt::PartiallySignedTransaction as Psbt},
};

pub const SCHEMA: &str = "\
CREATE TABLE version (
    version INTEGER NOT NULL
);

/* About the Bitcoin network. */
CREATE TABLE tip (
    network TEXT NOT NULL,
    blockheight INTEGER,
    blockhash BLOB
);

/* This stores metadata about our wallet. We only support single wallet for
 * now (and the foreseeable future).
 */
CREATE TABLE wallets (
    id INTEGER PRIMARY KEY NOT NULL,
    timestamp INTEGER NOT NULL,
    main_descriptor TEXT NOT NULL,
    deposit_derivation_index INTEGER NOT NULL
);

/* Our (U)TxOs.
 *
 * The 'spend_block_height' and 'spend_block.time' are only present if the spending
 * transaction for this coin exists and was confirmed.
 */
CREATE TABLE coins (
    id INTEGER PRIMARY KEY NOT NULL,
    wallet_id INTEGER NOT NULL,
    blockheight INTEGER,
    blocktime INTEGER,
    txid BLOB NOT NULL,
    vout INTEGER NOT NULL,
    amount_sat INTEGER NOT NULL,
    derivation_index INTEGER NOT NULL,
    spend_txid BLOB,
    spend_block_height INTEGER,
    spend_block_time INTEGER,
    UNIQUE (txid, vout),
    FOREIGN KEY (wallet_id) REFERENCES wallets (id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

/* A mapping from descriptor address to derivation index. Necessary until
 * we can get the derivation index from the parent descriptor from bitcoind.
 */
CREATE TABLE addresses (
    address TEXT NOT NULL UNIQUE,
    derivation_index INTEGER NOT NULL UNIQUE
);

/* Transactions we created that spend some of our coins. */
CREATE TABLE spend_transactions (
    id INTEGER PRIMARY KEY NOT NULL,
    psbt BLOB UNIQUE NOT NULL,
    txid BLOB UNIQUE NOT NULL
);
";

/// A row in the "tip" table.
#[derive(Clone, Debug)]
pub struct DbTip {
    pub network: bitcoin::Network,
    pub block_height: Option<i32>,
    pub block_hash: Option<bitcoin::BlockHash>,
}

impl TryFrom<&rusqlite::Row<'_>> for DbTip {
    type Error = rusqlite::Error;

    fn try_from(row: &rusqlite::Row) -> Result<Self, Self::Error> {
        let network: String = row.get(0)?;
        let network = bitcoin::Network::from_str(&network)
            .expect("Insane database: can't parse network string");

        let block_height: Option<i32> = row.get(1)?;
        let block_hash: Option<Vec<u8>> = row.get(2)?;
        let block_hash: Option<bitcoin::BlockHash> = block_hash
            .map(|h| encode::deserialize(&h).expect("Insane database: can't parse network string"));

        Ok(DbTip {
            network,
            block_height,
            block_hash,
        })
    }
}

/// A row in the "wallets" table.
#[derive(Clone, Debug)]
pub struct DbWallet {
    pub id: i64,
    pub timestamp: u32,
    pub main_descriptor: InheritanceDescriptor,
    pub deposit_derivation_index: bip32::ChildNumber,
}

impl TryFrom<&rusqlite::Row<'_>> for DbWallet {
    type Error = rusqlite::Error;

    fn try_from(row: &rusqlite::Row) -> Result<Self, Self::Error> {
        let id = row.get(0)?;
        let timestamp = row.get(1)?;

        let desc_str: String = row.get(2)?;
        let main_descriptor = InheritanceDescriptor::from_str(&desc_str)
            .expect("Insane database: can't parse deposit descriptor");

        let der_idx: u32 = row.get(3)?;
        let deposit_derivation_index = bip32::ChildNumber::from(der_idx);

        Ok(DbWallet {
            id,
            timestamp,
            main_descriptor,
            deposit_derivation_index,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DbSpendBlock {
    pub height: i32,
    pub time: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DbCoin {
    pub id: i64,
    pub wallet_id: i64,
    pub outpoint: bitcoin::OutPoint,
    pub block_height: Option<i32>,
    pub block_time: Option<u32>,
    pub amount: bitcoin::Amount,
    pub derivation_index: bip32::ChildNumber,
    pub spend_txid: Option<bitcoin::Txid>,
    pub spend_block: Option<DbSpendBlock>,
}

impl TryFrom<&rusqlite::Row<'_>> for DbCoin {
    type Error = rusqlite::Error;

    fn try_from(row: &rusqlite::Row) -> Result<Self, Self::Error> {
        let id = row.get(0)?;
        let wallet_id = row.get(1)?;

        let block_height = row.get(2)?;
        let block_time = row.get(3)?;
        let txid: Vec<u8> = row.get(4)?;
        let txid: bitcoin::Txid = encode::deserialize(&txid).expect("We only store valid txids");
        let vout = row.get(5)?;
        let outpoint = bitcoin::OutPoint { txid, vout };

        let amount = row.get(6)?;
        let amount = bitcoin::Amount::from_sat(amount);
        let der_idx: u32 = row.get(7)?;
        let derivation_index = bip32::ChildNumber::from(der_idx);

        let spend_txid: Option<Vec<u8>> = row.get(8)?;
        let spend_txid =
            spend_txid.map(|txid| encode::deserialize(&txid).expect("We only store valid txids"));
        let spend_height: Option<i32> = row.get(9)?;
        let spend_time: Option<u32> = row.get(10)?;
        assert_eq!(spend_height.is_none(), spend_time.is_none());
        let spend_block = spend_height.map(|height| DbSpendBlock {
            height,
            time: spend_time.expect("Must be there if height is"),
        });

        Ok(DbCoin {
            id,
            wallet_id,
            outpoint,
            block_height,
            block_time,
            amount,
            derivation_index,
            spend_txid,
            spend_block,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbAddress {
    pub address: bitcoin::Address,
    pub derivation_index: bip32::ChildNumber,
}

impl TryFrom<&rusqlite::Row<'_>> for DbAddress {
    type Error = rusqlite::Error;

    fn try_from(row: &rusqlite::Row) -> Result<Self, Self::Error> {
        let address: String = row.get(0)?;
        let address = bitcoin::Address::from_str(&address).expect("We only store valid addresses");

        let derivation_index: u32 = row.get(1)?;
        let derivation_index = bip32::ChildNumber::from(derivation_index);
        assert!(derivation_index.is_normal());

        Ok(DbAddress {
            address,
            derivation_index,
        })
    }
}

/// A row in the "spend_transactions" table
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbSpendTransaction {
    pub id: i64,
    pub psbt: Psbt,
    pub txid: bitcoin::Txid,
}

impl TryFrom<&rusqlite::Row<'_>> for DbSpendTransaction {
    type Error = rusqlite::Error;

    fn try_from(row: &rusqlite::Row) -> Result<Self, Self::Error> {
        let id: i64 = row.get(0)?;

        let psbt: Vec<u8> = row.get(1)?;
        let psbt: Psbt = encode::deserialize(&psbt).expect("We only store valid PSBTs");

        let txid: Vec<u8> = row.get(2)?;
        let txid: bitcoin::Txid = encode::deserialize(&txid).expect("We only store valid txids");
        assert_eq!(txid, psbt.unsigned_tx.txid());

        Ok(DbSpendTransaction { id, psbt, txid })
    }
}
