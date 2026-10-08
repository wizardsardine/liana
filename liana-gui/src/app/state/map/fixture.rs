use std::{
    collections::{BTreeMap, HashMap},
    str::FromStr,
};

use liana::{
    label::Label,
    miniscript::bitcoin::{
        absolute, bip32::ChildNumber, transaction, Address, Amount, Network, OutPoint, ScriptBuf,
        Sequence, Transaction, TxIn, TxOut, Txid, Witness,
    },
};
use lianad::commands::LCSpendInfo;

use crate::{
    app::state::map::graph::TxGraph,
    daemon::model::{Coin, HistoryTransaction, LabelItem},
};

const BASE_TIME: u32 = 1_700_000_000;
const DAY: u32 = 86_400;

pub fn address(n: u16) -> Address {
    Address::p2wsh(
        &ScriptBuf::from_bytes(n.to_be_bytes().to_vec()),
        Network::Bitcoin,
    )
}

/// An address of ours.
pub fn ours(k: u16) -> Address {
    address(100 + k)
}

pub fn foreign(n: u8) -> OutPoint {
    OutPoint::new(Txid::from_str(&format!("{n:0>64x}")).unwrap(), 0)
}

struct BuiltTx {
    tx: Transaction,
    day: Option<u32>,
    owned: Vec<usize>,
}

/// Builds a small wallet history, a transaction at a time.
#[derive(Default)]
pub struct Builder {
    txs: Vec<BuiltTx>,
    coins: HashMap<OutPoint, Coin>,
    labels: HashMap<String, String>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// `day` is the number of days after the base time, also used as block height; `None` is
    /// unconfirmed. Outputs are `(address, sats, ours)`.
    pub fn tx(
        &mut self,
        day: Option<u32>,
        inputs: &[OutPoint],
        outputs: &[(Address, u64, bool)],
    ) -> Txid {
        let tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: inputs
                .iter()
                .map(|outpoint| TxIn {
                    previous_output: *outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                })
                .collect(),
            output: outputs
                .iter()
                .map(|(address, sats, _)| TxOut {
                    value: Amount::from_sat(*sats),
                    script_pubkey: address.script_pubkey(),
                })
                .collect(),
        };
        let txid = tx.compute_txid();
        let mut owned = Vec::new();
        for (index, (address, sats, ours)) in outputs.iter().enumerate() {
            if !ours {
                continue;
            }
            let outpoint = OutPoint::new(txid, index as u32);
            self.coins.insert(
                outpoint,
                Coin {
                    outpoint,
                    amount: Amount::from_sat(*sats),
                    address: address.clone(),
                    derivation_index: ChildNumber::Normal {
                        index: index as u32,
                    },
                    block_height: day.map(|d| d as i32),
                    is_immature: false,
                    is_change: false,
                    is_from_self: false,
                    default_label: Label::None,
                    spend_info: None,
                },
            );
            owned.push(index);
        }
        self.txs.push(BuiltTx { tx, day, owned });
        txid
    }

    pub fn label(&mut self, item: impl Into<LabelItem>, value: &str) {
        self.labels
            .insert(item.into().to_string(), value.to_string());
    }

    /// Stored default label of one of our coins, set before `finish`.
    pub fn default_label(&mut self, outpoint: OutPoint, label: Label) {
        if let Some(coin) = self.coins.get_mut(&outpoint) {
            coin.default_label = label;
        }
    }

    pub fn finish(mut self) -> (Vec<HistoryTransaction>, Vec<Coin>) {
        for built in &self.txs {
            let txid = built.tx.compute_txid();
            for input in &built.tx.input {
                if let Some(coin) = self.coins.get_mut(&input.previous_output) {
                    coin.spend_info = Some(LCSpendInfo {
                        txid,
                        height: built.day.map(|d| d as i32),
                    });
                }
            }
        }
        let txs = self
            .txs
            .iter()
            .map(|built| {
                let txid = built.tx.compute_txid();
                let spent = built
                    .tx
                    .input
                    .iter()
                    .filter_map(|input| self.coins.get(&input.previous_output).cloned())
                    .collect();
                let owned_outputs: BTreeMap<usize, Label> = built
                    .owned
                    .iter()
                    .map(|index| {
                        let outpoint = OutPoint::new(txid, *index as u32);
                        (*index, self.coins[&outpoint].default_label.clone())
                    })
                    .collect();
                let mut history = HistoryTransaction::new(
                    built.tx.clone(),
                    built.day.map(|d| d as i32),
                    built.day.map(|d| BASE_TIME + d * DAY),
                    spent,
                    owned_outputs,
                    Network::Bitcoin,
                    Label::None,
                );
                history.labels = self.labels.clone();
                history
            })
            .collect();
        (txs, self.coins.into_values().collect())
    }
}

pub struct SampleTxids {
    pub salary: Txid,
    pub incoming_change: Txid,
    pub incoming_four: Txid,
    pub rent: [Txid; 4],
    pub consolidation: Txid,
    pub self_transfer: Txid,
    pub batch: Txid,
    pub payjoin: Txid,
    pub unconfirmed: Txid,
}

pub struct Fixture {
    pub txs: Vec<HistoryTransaction>,
    pub coins: Vec<Coin>,
    pub ids: SampleTxids,
    pub landlord: Address,
    pub reused_payee: Address,
}

/// The sample wallet of the spec.
pub fn sample_wallet() -> Fixture {
    let landlord = address(1);
    let reused_payee = address(10);
    let out = |txid: Txid, vout: u32| OutPoint::new(txid, vout);
    let mut b = Builder::new();

    let salary = b.tx(Some(1), &[foreign(1)], &[(ours(0), 2_000_000, true)]);
    let incoming_change = b.tx(
        Some(2),
        &[foreign(2), foreign(3), foreign(4)],
        &[(ours(1), 800_000, true), (address(50), 120_000, false)],
    );
    let incoming_four = b.tx(
        Some(3),
        &[foreign(5)],
        &[
            (ours(2), 100_000, true),
            (ours(3), 200_000, true),
            (ours(4), 300_000, true),
            (ours(5), 400_000, true),
            (address(51), 50_000, false),
        ],
    );
    let rent0 = b.tx(
        Some(4),
        &[out(salary, 0)],
        &[
            (landlord.clone(), 500_000, false),
            (ours(6), 1_499_000, true),
        ],
    );
    let consolidation = b.tx(
        Some(5),
        &(0..4).map(|v| out(incoming_four, v)).collect::<Vec<_>>(),
        &[(ours(7), 998_000, true)],
    );
    let rent1 = b.tx(
        Some(6),
        &[out(rent0, 1)],
        &[(landlord.clone(), 500_000, false), (ours(8), 998_000, true)],
    );
    let self_transfer = b.tx(
        Some(7),
        &[out(incoming_change, 0)],
        &[(ours(9), 799_000, true)],
    );
    let rent2 = b.tx(
        Some(8),
        &[out(rent1, 1)],
        &[
            (landlord.clone(), 500_000, false),
            (ours(10), 497_000, true),
        ],
    );
    let mut batch_outputs: Vec<(Address, u64, bool)> =
        (10..=22).map(|n| (address(n), 10_000, false)).collect();
    batch_outputs.push((ours(11), 867_000, true));
    let batch = b.tx(Some(9), &[out(consolidation, 0)], &batch_outputs);
    let rent3 = b.tx(
        Some(10),
        &[out(rent2, 1)],
        &[
            (landlord.clone(), 300_000, false),
            (ours(12), 196_000, true),
        ],
    );
    let payjoin = b.tx(
        Some(11),
        &[out(rent3, 1), foreign(6)],
        &[(address(30), 250_000, false), (ours(13), 145_000, true)],
    );
    let unconfirmed = b.tx(
        None,
        &[out(payjoin, 1)],
        &[
            (reused_payee.clone(), 50_000, false),
            (ours(14), 94_000, true),
        ],
    );

    b.label(salary, "Salary");
    b.label(landlord.clone(), "Landlord");
    b.label(rent0, "Rent January");
    for (vout, name) in ["a", "b", "c", "d"].iter().enumerate() {
        b.label(out(incoming_four, vout as u32), name);
    }
    b.label(out(incoming_change, 1), "Alice change");
    b.default_label(out(self_transfer, 0), Label::Funding("Savings".to_string()));

    let (txs, coins) = b.finish();
    Fixture {
        txs,
        coins,
        ids: SampleTxids {
            salary,
            incoming_change,
            incoming_four,
            rent: [rent0, rent1, rent2, rent3],
            consolidation,
            self_transfer,
            batch,
            payjoin,
            unconfirmed,
        },
        landlord,
        reused_payee,
    }
}

pub fn graph() -> TxGraph {
    let fixture = sample_wallet();
    TxGraph::new(fixture.txs, &fixture.coins)
}
