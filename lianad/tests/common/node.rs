use electrsd::corepc_node::Node as BitcoinD;
use electrsd::{self, ElectrsD};

use crate::common::electrs::start_electrs;

/// Node kind used by lianad process.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Bitcoind,
    Electrs, // we may also want to test against other electrum servers in the future
}

/// Node instance used by lianad process.
pub enum Node<'a> {
    Bitcoind(&'a BitcoinD),
    Electrs(Box<ElectrsD>),
}

impl<'a> Node<'a> {
    pub fn new(kind: NodeKind, bitcoind: &'a BitcoinD) -> anyhow::Result<Self> {
        match kind {
            NodeKind::Bitcoind => Ok(Node::Bitcoind(bitcoind)),
            NodeKind::Electrs => start_electrs(bitcoind).map(|e| Node::Electrs(Box::new(e))),
        }
    }
}
