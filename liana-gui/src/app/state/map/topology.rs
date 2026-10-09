use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::app::state::map::{
    graph::{InputSlot, OutputSlot, TxGraph},
    wallets::WalletKey,
};

pub const TOPOLOGY_VERSION: u32 = 2;

/// The map graph with every identifier replaced by an index: no txid, outpoint, address,
/// amount, label, height, time or wallet name.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Topology {
    pub version: u32,
    pub wallets: usize,
    pub txs: Vec<TopoTx>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct TopoTx {
    /// Position in the left to right order of the lanes placement.
    pub order: usize,
    /// Lane index of the primary wallet.
    pub wallet: usize,
    pub inputs: Vec<TopoInput>,
    pub outputs: Vec<TopoOutput>,
    pub unconfirmed: bool,
}

/// The other end of a coin link: a transaction `order` and its slot index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Peer {
    pub tx: usize,
    pub slot: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TopoInput {
    /// Coin of the wallet at lane `wallet`, created by output `from` when it is on the map.
    Own { wallet: usize, from: Option<Peer> },
    /// A counterparty coin.
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TopoOutput {
    /// Coin of the wallet at lane `wallet`, spent by input `to` when it is on the map.
    Own {
        wallet: usize,
        to: Option<Peer>,
    },
    Payment,
    /// A counterparty output.
    External,
}

/// `None` when a wallet of the graph is missing from `lane_order`.
pub fn topology(graph: &TxGraph, lane_order: &[WalletKey]) -> Option<Topology> {
    let lane = |wallet: &WalletKey| lane_order.iter().position(|key| key == wallet);
    let mut spent_by = HashMap::new();
    let mut funded_by = HashMap::new();
    for edge in graph.coin_edges() {
        let (from, to) = (
            Peer {
                tx: edge.from.tx,
                slot: edge.from.index,
            },
            Peer {
                tx: edge.to.tx,
                slot: edge.to.index,
            },
        );
        spent_by.insert(from, to);
        funded_by.insert(to, from);
    }
    // The lanes placement walks the transactions in graph order.
    let txs = graph
        .txs()
        .iter()
        .enumerate()
        .map(|(order, tx)| {
            let inputs = tx
                .inputs
                .iter()
                .enumerate()
                .map(|(slot, input)| match input {
                    InputSlot::OurCoin { wallet, .. } => Some(TopoInput::Own {
                        wallet: lane(wallet)?,
                        from: funded_by.get(&Peer { tx: order, slot }).copied(),
                    }),
                    InputSlot::CounterpartyCoin { .. } => Some(TopoInput::External),
                })
                .collect::<Option<_>>()?;
            let outputs = tx
                .outputs
                .iter()
                .enumerate()
                .map(|(slot, output)| match output {
                    OutputSlot::OurCoin { wallet, .. } => Some(TopoOutput::Own {
                        wallet: lane(wallet)?,
                        to: spent_by.get(&Peer { tx: order, slot }).copied(),
                    }),
                    OutputSlot::Payment { .. } => Some(TopoOutput::Payment),
                    OutputSlot::CounterpartyOutput { .. } => Some(TopoOutput::External),
                })
                .collect::<Option<_>>()?;
            Some(TopoTx {
                order,
                wallet: lane(tx.primary())?,
                inputs,
                outputs,
                unconfirmed: tx.history().time.is_none(),
            })
        })
        .collect::<Option<_>>()?;
    Some(Topology {
        version: TOPOLOGY_VERSION,
        wallets: lane_order.len(),
        txs,
    })
}

#[cfg(test)]
mod tests {
    use liana::miniscript::bitcoin::{Address, OutPoint};

    use super::*;
    use crate::app::state::map::fixture::{self, Fixture};

    fn sample() -> (Fixture, Topology) {
        let f = fixture::sample_wallet();
        let graph = fixture::current_graph(f.txs.clone(), f.coins.clone());
        let topology = topology(&graph, &[WalletKey::Current]).unwrap();
        (f, topology)
    }

    fn two_wallets(lane_order: impl Fn(&WalletKey) -> Vec<WalletKey>) -> Topology {
        let mut two = fixture::two_wallets("aaaa", "bbbb");
        let graph = TxGraph::new(std::mem::take(&mut two.wallets));
        topology(&graph, &lane_order(&two.b)).unwrap()
    }

    fn peer(tx: usize, slot: usize) -> Option<Peer> {
        Some(Peer { tx, slot })
    }

    #[test]
    fn no_identifier_in_the_json() {
        let (f, topology) = sample();
        let json = serde_json::to_string_pretty(&topology).unwrap();
        let mut secrets: Vec<String> = Vec::new();
        for history in &f.txs {
            secrets.push(history.txid.to_string());
            secrets.extend(history.time.map(|time| time.to_string()));
            for input in &history.tx.input {
                secrets.push(input.previous_output.to_string());
                secrets.push(input.previous_output.txid.to_string());
            }
            for (vout, txout) in history.tx.output.iter().enumerate() {
                secrets.push(OutPoint::new(history.txid, vout as u32).to_string());
                secrets.push(txout.value.to_sat().to_string());
                secrets.push(
                    Address::from_script(&txout.script_pubkey, history.network)
                        .unwrap()
                        .to_string(),
                );
            }
        }
        for coin in &f.coins {
            secrets.push(coin.address.to_string());
            secrets.push(coin.amount.to_sat().to_string());
        }
        secrets.extend(["Salary", "Landlord", "Alice change"].map(String::from));
        assert_eq!(secrets.len(), 200);
        for secret in secrets {
            assert!(!json.contains(&secret), "{} found in the export", secret);
        }
    }

    #[test]
    fn sample_slots_point_to_each_other() {
        let (_, topology) = sample();
        assert_eq!(topology.version, 2);
        assert_eq!(topology.wallets, 1);
        assert_eq!(topology.txs.len(), 12);
        assert!(topology.txs.iter().enumerate().all(|(i, tx)| tx.order == i));
        let unconfirmed: Vec<usize> = topology
            .txs
            .iter()
            .filter(|tx| tx.unconfirmed)
            .map(|tx| tx.order)
            .collect();
        assert_eq!(unconfirmed, [11]);
        let mut links = 0;
        for tx in &topology.txs {
            for (slot, output) in tx.outputs.iter().enumerate() {
                if let TopoOutput::Own { to: Some(to), .. } = output {
                    links += 1;
                    assert_eq!(
                        topology.txs[to.tx].inputs[to.slot],
                        TopoInput::Own {
                            wallet: 0,
                            from: peer(tx.order, slot)
                        }
                    );
                }
            }
        }
        assert_eq!(links, 12);
        assert_eq!(
            topology.txs[3].outputs,
            [
                TopoOutput::Payment,
                TopoOutput::Own {
                    wallet: 0,
                    to: peer(5, 0)
                }
            ]
        );
        assert_eq!(
            topology.txs[10].inputs,
            [
                TopoInput::Own {
                    wallet: 0,
                    from: peer(9, 1)
                },
                TopoInput::External
            ]
        );
    }

    #[test]
    fn two_wallets_follow_the_lane_order() {
        let current_first = two_wallets(|b| vec![WalletKey::Current, b.clone()]);
        assert_eq!(current_first.wallets, 2);
        assert_eq!(
            current_first.txs[1].outputs,
            [
                TopoOutput::Own {
                    wallet: 1,
                    to: peer(2, 0)
                },
                TopoOutput::Own {
                    wallet: 0,
                    to: None
                }
            ]
        );
        assert_eq!(current_first.txs[2].wallet, 1);
        let b_first = two_wallets(|b| vec![b.clone(), WalletKey::Current]);
        assert_eq!(b_first.txs[2].wallet, 0);
        assert_eq!(
            b_first.txs[2].inputs,
            [TopoInput::Own {
                wallet: 0,
                from: peer(1, 0)
            }]
        );
    }

    #[test]
    fn missing_wallet_has_no_topology() {
        let mut two = fixture::two_wallets("aaaa", "bbbb");
        let graph = TxGraph::new(std::mem::take(&mut two.wallets));
        assert_eq!(topology(&graph, &[WalletKey::Current]), None);
    }

    #[test]
    fn slot_json() {
        let inputs = [
            TopoInput::Own {
                wallet: 1,
                from: peer(4, 2),
            },
            TopoInput::External,
        ];
        let outputs = [
            TopoOutput::Own {
                wallet: 0,
                to: None,
            },
            TopoOutput::Payment,
        ];
        assert_eq!(
            serde_json::to_string(&inputs).unwrap(),
            r#"[{"kind":"own","wallet":1,"from":{"tx":4,"slot":2}},{"kind":"external"}]"#
        );
        assert_eq!(
            serde_json::to_string(&outputs).unwrap(),
            r#"[{"kind":"own","wallet":0,"to":null},{"kind":"payment"}]"#
        );
    }
}
