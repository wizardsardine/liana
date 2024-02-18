//! Nakamoto Backend implementation
//!
//! Author: Vincenzo Palazzo <vincenzopalazzodev@gmail.com>
use std::net;
use std::path::PathBuf;
use std::str::FromStr;
use std::thread::JoinHandle;

use nakamoto::client::traits::Handle;
use nakamoto::client;
use nakamoto::common::bitcoin_hashes::hex::FromHex;
use nakamoto::net::poll::{Waker, Reactor};
use miniscript::bitcoin::hashes::Hash;
use miniscript::bitcoin;

use crate::BitcoindError;
use crate::bitcoin::BitcoinInterface;

use super::SyncProgress;

/// Nakamoto client
pub struct Nakamoto {
    /// Nakamoto handler used interact with the client.
    handler: client::Handle<Waker>,
    /// Nakamoto main worked to avoid leave a pending project.
    _worker: JoinHandle<Result<(), client::Error>>,
    _network: client::Network,
}

impl Nakamoto {
    /// Create a new instance of nakamoto.
    pub fn new(network: &bitcoin::Network, connect: &[net::SocketAddr], data_dir: PathBuf) -> Result<Self, ()> {
        let network = client::Network::from_str(&network.to_string()).map_err(|_| ())?;
        let mut config = client::Config::new(network);
        config.root = data_dir;
        config.connect = connect.to_vec();
        config.user_agent = "Liana-Nakamoto-v1";
        let client = client::Client::<Reactor<net::TcpStream>>::new().unwrap();
        let handler = client.handle();
        let worker = std::thread::spawn(|| client.run(config));
        Ok(Self{ handler, _worker: worker, _network: network })
    }

    /// Stop the nakamoto node
    #[allow(dead_code)]
    pub fn stop(self) -> Result<(), BitcoindError> {
        self.handler.shutdown().map_err(|_| BitcoindError::GenericError)?;
        let _ = self._worker.join().map_err(|_| BitcoindError::GenericError)?;
        Ok(())
    }
}

impl BitcoinInterface for Nakamoto {
    fn mempool_spenders(&self, outpoints: &[miniscript::bitcoin::OutPoint]) -> Vec<super::MempoolEntry> {
        unimplemented!()
    }

    fn chain_tip(&self) -> crate::bitcoin::BlockChainTip {
        // FIXME: we should check the error and maybe return
        // it to the caller.
        let (height, header, _) = self.handler.get_tip().unwrap();
        let block_hash = header.block_hash();
        let hash = bitcoin::BlockHash::from_slice(&block_hash.as_hash().to_vec()).unwrap();
        crate::bitcoin::BlockChainTip{ height: height as i32, hash }
    }

    fn broadcast_tx(&self, tx: &miniscript::bitcoin::Transaction) -> Result<(), String> {
        use miniscript::bitcoin::consensus::serialize;
        use nakamoto::common::bitcoin::consensus::deserialize;

        let tx = serialize(tx);
        let tx: nakamoto::common::bitcoin::Transaction = deserialize(&tx).map_err(|err| format!("{err}"))?;
        self.handler.submit_transaction(tx).map_err(|err| format!("{err}"))?;
        Ok(())
    }

    fn common_ancestor(&self, tip: &crate::bitcoin::BlockChainTip) -> Option<crate::bitcoin::BlockChainTip> {
        None
    }

    fn is_in_chain(&self, tip: &crate::bitcoin::BlockChainTip) -> bool {
        true
    }

    fn block_before_date(&self, timestamp: u32) -> Option<crate::bitcoin::BlockChainTip> {
        None
    }

    fn confirmed_coins(
        &self,
        outpoints: &[miniscript::bitcoin::OutPoint],
    ) -> (Vec<(miniscript::bitcoin::OutPoint, i32, u32)>, Vec<miniscript::bitcoin::OutPoint>) {
        unimplemented!()
    }

    fn genesis_block(&self) -> crate::bitcoin::BlockChainTip {
        let height = 0;
        let block = self.handler.get_block_by_height(height).unwrap();
        let block = block.unwrap();
        let block_hash = block.block_hash().as_hash().to_string();
        let block_hash = miniscript::bitcoin::BlockHash::from_str(&block_hash).unwrap();
        crate::bitcoin::BlockChainTip{ height: height as i32, hash: block_hash  }
    }

    fn received_coins(
        &self,
        tip: &crate::bitcoin::BlockChainTip,
        descs: &[crate::descriptors::SinglePathLianaDesc],
    ) -> Vec<crate::bitcoin::UTxO> {
        unimplemented!()
    }

    fn rescan_progress(&self) -> Option<f64> {
        None
    }

    fn spending_coins(
        &self,
        outpoints: &[miniscript::bitcoin::OutPoint],
    ) -> Vec<(miniscript::bitcoin::OutPoint, miniscript::bitcoin::Txid)> {
        unimplemented!()
    }

    fn start_rescan(
        &self,
        desc: &crate::descriptors::LianaDescriptor,
        timestamp: u32,
    ) -> Result<(), String> {
        // We do not care for the moment, because we are tracking with nakamoto
        // all the transactions submitted
        Ok(())
    }

    fn wallet_transaction(
        &self,
        txid: &miniscript::bitcoin::Txid,
    ) -> Option<(miniscript::bitcoin::Transaction, Option<crate::bitcoin::Block>)> {
        use nakamoto::common::bitcoin::consensus::serialize;
        use miniscript::bitcoin::consensus::deserialize;

        let txid = txid.to_string();
        let txid = nakamoto::common::bitcoin::Txid::from_hex(&txid).unwrap();
        let Ok(Some(tx)) = self.handler.get_submitted_transaction(&txid) else {
            return None;
        };
        let tx = serialize(&tx);
        let tx: miniscript::bitcoin::Transaction = deserialize(&tx).unwrap();
        // FIXME: we do not know what is the block that it is confirmed, we should
        // keep this in our db maybe?
        Some((tx, None))
    }

    fn spent_coins(
        &self,
        outpoints: &[(miniscript::bitcoin::OutPoint, miniscript::bitcoin::Txid)],
    ) -> (
        Vec<(miniscript::bitcoin::OutPoint, miniscript::bitcoin::Txid, crate::bitcoin::Block)>,
        Vec<miniscript::bitcoin::OutPoint>,
    ) {
        unimplemented!()
    }

    fn sync_progress(&self) -> super::SyncProgress {
        // FIXME: call tip and try to simulate the bitcoin intergace
        SyncProgress::new(100.0, 0, 0)
    }

    fn tip_time(&self) -> Option<u32> {
        // This is a little bit hard to track at the moment
        None
    }
}
