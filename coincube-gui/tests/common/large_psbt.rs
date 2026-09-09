//! Builds a real, authenticated (P2-A) Taproot spend through
//! `coincube_core::spend::create_spend`, funded by previous transactions with
//! many outputs, so transport tests can measure the exact production payload
//! rather than a synthetic blob. Every input carries its complete previous
//! transaction and a matching `witness_utxo`.
use coincube_core::{
    descriptors::CoincubeDescriptor,
    miniscript::bitcoin::{
        self, absolute::LockTime, bip32, consensus, secp256k1, Amount, Network, OutPoint,
        ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
    },
    spend::{create_spend, AddrInfo, CandidateCoin, SpendOutputAddress, SpendTxFees, TxGetter},
};
use std::{collections::HashMap, str::FromStr};

pub const TR_DESC: &str = "tr(tpubD6NzVbkrYhZ4YdBUPkUhDYj6Sd1QK8vgiCf5RwHnAnSNK5ozemAZzPTYZbgQq4diod7oxFJJYGa8FNRHzRo7URkixzQTuudh38xRRdSc4Hu/<0;1>/*,{and_v(v:multi_a(1,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*,[da2ee873/48'/1'/0'/2']tpubDEbXY6RbN9mxAvQW797WxReGGkrdyRfdYcehVVaQQcQ3kyfhxSMcnU9qGpUVRHXXALvBtc99jcuxx5tkzcLaJbAukSNpP9h2ti4XFRosv1g/<2;3>/*),older(2)),multi_a(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[da2ee873/48'/1'/0'/2']tpubDEbXY6RbN9mxAvQW797WxReGGkrdyRfdYcehVVaQQcQ3kyfhxSMcnU9qGpUVRHXXALvBtc99jcuxx5tkzcLaJbAukSNpP9h2ti4XFRosv1g/<0;1>/*)})";

struct MapGetter(HashMap<bitcoin::Txid, Transaction>);

impl TxGetter for MapGetter {
    fn get_tx(&mut self, txid: &bitcoin::Txid) -> Option<Transaction> {
        self.0.get(txid).cloned()
    }
}

fn script_at(desc: &CoincubeDescriptor, index: u32, is_change: bool) -> ScriptBuf {
    let secp = secp256k1::Secp256k1::verification_only();
    let d = if is_change {
        desc.change_descriptor()
    } else {
        desc.receive_descriptor()
    };
    d.derive(bip32::ChildNumber::from(index), &secp)
        .script_pubkey()
}

/// A consensus-serialized funding transaction identified by the txid of its
/// own bytes: output 0 pays the wallet, the rest are unrelated P2TR outputs.
fn funding_tx(desc: &CoincubeDescriptor, index: u32, prev_outputs: usize) -> Transaction {
    let mut output = vec![TxOut {
        value: Amount::from_sat(100_000),
        script_pubkey: script_at(desc, index, false),
    }];
    for _ in 1..prev_outputs {
        output.push(TxOut {
            value: Amount::from_sat(546),
            script_pubkey: ScriptBuf::from_bytes([vec![0x51, 0x20], vec![0xab; 32]].concat()),
        });
    }
    let tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::from_bytes((1_000 + index).to_le_bytes().to_vec()),
            sequence: Sequence::MAX,
            witness: Witness::default(),
        }],
        output,
    };
    let bytes = consensus::encode::serialize(&tx);
    consensus::encode::deserialize(&bytes).expect("real transaction bytes")
}

/// An authenticated Taproot spend of `inputs` coins, each funded by its own
/// `prev_outputs`-output transaction. Serialized size grows with both.
pub fn authenticated_taproot_psbt(inputs: u32, prev_outputs: usize) -> bitcoin::psbt::Psbt {
    let desc = CoincubeDescriptor::from_str(TR_DESC).expect("descriptor");
    let secp = secp256k1::Secp256k1::verification_only();
    let mut prevs = HashMap::new();
    let mut cands = Vec::new();
    for i in 0..inputs {
        let prev = funding_tx(&desc, i, prev_outputs);
        cands.push(CandidateCoin {
            outpoint: OutPoint::new(prev.compute_txid(), 0),
            amount: Amount::from_sat(100_000),
            deriv_index: bip32::ChildNumber::from(i),
            is_change: false,
            must_select: true,
            sequence: None,
            ancestor_info: None,
        });
        prevs.insert(prev.compute_txid(), prev);
    }
    let total: u64 = cands.iter().map(|c| c.amount.to_sat()).sum();
    let destination = SpendOutputAddress {
        addr: bitcoin::Address::from_script(&script_at(&desc, 7, false), Network::Regtest)
            .expect("address"),
        info: None,
    };
    let change = SpendOutputAddress {
        addr: bitcoin::Address::from_script(&script_at(&desc, 0, true), Network::Regtest)
            .expect("address"),
        info: Some(AddrInfo {
            index: bip32::ChildNumber::from(0),
            is_change: true,
        }),
    };
    let res = create_spend(
        &desc,
        &secp,
        &mut MapGetter(prevs),
        &[(destination, Amount::from_sat(total / 2))],
        &cands,
        SpendTxFees::Regular(1),
        change,
        LockTime::ZERO,
    )
    .expect("authenticated spend");
    for (input, txin) in res.psbt.inputs.iter().zip(&res.psbt.unsigned_tx.input) {
        let prev = input
            .non_witness_utxo
            .as_ref()
            .expect("previous transaction");
        assert_eq!(prev.compute_txid(), txin.previous_output.txid);
    }
    res.psbt
}
