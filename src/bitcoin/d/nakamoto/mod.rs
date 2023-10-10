//! Nakamoto Backend implementation
//!
//! Author: Vincenzo Palazzo <vincenzopalazzodev@gmail.com>
use std::net;
use std::path::PathBuf;
use std::thread::JoinHandle;

// FIXME: use the bitcoin exported inside the type
use nakamoto::common::bitcoin;

use nakamoto::client;
use nakamoto::net::poll::{Waker, Reactor};

use crate::bitcoin::BitcoinInterface;

/// Nakamoto client
pub struct Nakamoto {
    /// Nakamoto handler used interact with the client.
    handler: client::Handle<Waker>,
    /// Nakamoto main worked to avoid leave a pending project.
    worker: JoinHandle<Result<(), client::Error>>
}

impl Nakamoto {
    /// Create a new instance of nakamoto.
    pub fn new(network: &bitcoin::Network, connect: &[net::SocketAddr], data_dir: PathBuf) -> Result<Self, ()> {
        let mut config = client::Config::new(network.clone().into());
        config.root = data_dir;
        config.connect = connect.to_vec();
        config.user_agent = "Liana-Nakamoto-v1";
        let client = client::Client::<Reactor<net::TcpStream>>::new().unwrap();
        let handler = client.handle();
        let worker = std::thread::spawn(|| client.run(config));
        Ok(Self{ handler, worker })
    }
}

impl BitcoinInterface for Nakamoto {
    fn chain_tip(&self) -> crate::bitcoin::BlockChainTip {
        unimplemented!()
    }

    fn broadcast_tx(&self, tx: &miniscript::bitcoin::Transaction) -> Result<(), String> {
        unimplemented!()
    }

    fn common_ancestor(&self, tip: &crate::bitcoin::BlockChainTip) -> Option<crate::bitcoin::BlockChainTip> {
        todo!()
    }

    fn is_in_chain(&self, tip: &crate::bitcoin::BlockChainTip) -> bool {
        unimplemented!()
    }

    fn block_before_date(&self, timestamp: u32) -> Option<crate::bitcoin::BlockChainTip> {
        unimplemented!()
    }

    fn confirmed_coins(
        &self,
        outpoints: &[miniscript::bitcoin::OutPoint],
    ) -> (Vec<(miniscript::bitcoin::OutPoint, i32, u32)>, Vec<miniscript::bitcoin::OutPoint>) {
        unimplemented!()
    }

    fn genesis_block(&self) -> crate::bitcoin::BlockChainTip {
        unimplemented!()
    }

    fn received_coins(
        &self,
        tip: &crate::bitcoin::BlockChainTip,
        descs: &[crate::descriptors::SinglePathLianaDesc],
    ) -> Vec<crate::bitcoin::UTxO> {
        unimplemented!()
    }

    fn rescan_progress(&self) -> Option<f64> {
        unimplemented!()
    }

    fn spending_coins(
        &self,
        outpoints: &[miniscript::bitcoin::OutPoint],
    ) -> Vec<(miniscript::bitcoin::OutPoint, miniscript::bitcoin::Txid)> {
        unimplemented!()
    }

    fn spent_coins(
        &self,
        outpoints: &[(miniscript::bitcoin::OutPoint, miniscript::bitcoin::Txid)],
    ) -> Vec<(miniscript::bitcoin::OutPoint, miniscript::bitcoin::Txid, crate::bitcoin::Block)> {
        unimplemented!()
    }

    fn start_rescan(
        &self,
        desc: &crate::descriptors::LianaDescriptor,
        timestamp: u32,
    ) -> Result<(), String> {
        unimplemented!()
    }

    fn sync_progress(&self) -> f64 {
        unimplemented!()
    }

    fn tip_time(&self) -> u32 {
        unimplemented!()
    }

    fn wallet_transaction(
        &self,
        txid: &miniscript::bitcoin::Txid,
    ) -> Option<(miniscript::bitcoin::Transaction, Option<crate::bitcoin::Block>)> {
        unimplemented!()
    }
}
