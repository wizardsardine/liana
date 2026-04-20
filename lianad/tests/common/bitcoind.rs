use std::thread::sleep;
use std::time::Duration;

use electrsd::corepc_client::client_sync::v29 as rpc;
use electrsd::corepc_node::{Conf, Node, P2P};
use miniscript::bitcoin::{self, address::NetworkUnchecked, Address};

use crate::common::utils::wait_for;

pub fn start_bitcoind() -> anyhow::Result<Node> {
    let mut conf = Conf::default();

    conf.args.push("-printtoconsole");
    conf.args.push("-server");
    conf.args.push("-debug");
    conf.args.push("-debugexclude=libevent");
    conf.args.push("-debugexclude=tor");
    conf.args.push("-txindex=1"); // enable txindex for electrs
    conf.args.push("-peertimeout=172800"); // = 2 days (2 * 24 * 60 * 60)
    conf.args.push("-rpcthreads=32");

    conf.p2p = P2P::Yes; // electrs requires p2p port open

    // Check for custom bitcoind binary path
    let exe = match std::env::var("BITCOIND_PATH") {
        Ok(path) if !path.is_empty() => {
            println!("Using custom bitcoind binary");
            path
        }
        _ => {
            println!("Using downloaded bitcoind binary");
            electrsd::corepc_node::downloaded_exe_path()?
        }
    };
    println!("bitcoind binary: {}", exe);
    let bitcoind = Node::with_conf(&exe, &conf)?;
    Ok(bitcoind)
}

pub fn new_address_unchecked(client: &rpc::Client) -> Address<NetworkUnchecked> {
    client.new_address().unwrap().into_unchecked()
}

/// Create bitcoind process and set up wallet.
pub fn setup_bitcoind() -> anyhow::Result<Node> {
    let bitcoind = start_bitcoind().expect("start_bitcoind");

    bitcoind
        .client
        .generate_to_address(101, &bitcoind.client.new_address().unwrap())
        .unwrap();

    while bitcoind.client.get_balance().unwrap().0 < 50.0 {
        sleep(Duration::from_millis(100));
    }

    Ok(bitcoind)
}

pub fn generate_blocks(client: &rpc::Client, num_blocks: usize, wait_for_txs: &[bitcoin::Txid]) {
    assert!(wait_for(|| {
        let mempool = client.get_raw_mempool().unwrap();
        for txid in wait_for_txs {
            if !mempool.0.contains(&txid.to_string()) {
                return false;
            }
        }
        true
    }));
    let old_block_count = client.get_block_count().unwrap();
    let addr = client.new_address().unwrap();
    client.generate_to_address(num_blocks, &addr).unwrap();
    assert!(wait_for(
        || client.get_block_count().unwrap().0 == old_block_count.0 + num_blocks as u64
    ));
}
