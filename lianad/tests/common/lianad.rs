use electrsd;
use electrsd::corepc_node::Node as BitcoinD;
use liana::descriptors::LianaDescriptor;
use lianad::config::{
    BitcoinBackend, BitcoinConfig, BitcoindConfig, BitcoindRpcAuth, Config as LianadConfig,
    ElectrumConfig,
};
use lianad::datadir::DataDirectory;
use lianad::miniscript::bitcoin::Network;
use lianad::{DaemonControl, DaemonHandle};

use crate::common::descriptor::{DescriptorKind, TestDescriptor};
use crate::common::node::{Node, NodeKind};

pub struct LianaD<'a> {
    pub handle: DaemonHandle,
    pub desc: TestDescriptor,
    // keep node and temp dir alive while lianad is running
    _node: Node<'a>,
    _data_directory: tempfile::TempDir,
}

impl<'a> LianaD<'a> {
    pub fn new(
        bitcoind: &'a BitcoinD,
        node_kind: NodeKind,
        use_taproot: bool,
        descriptor_kind: DescriptorKind,
    ) -> Self {
        let node = Node::new(node_kind, bitcoind).expect("start node for lianad");
        let desc = TestDescriptor::new(descriptor_kind, use_taproot);
        let data_directory = tempfile::TempDir::new().unwrap();

        let cfg = lianad_config(
            Network::Regtest,
            desc.descriptor.clone(),
            &node,
            &data_directory,
        );
        let handle = DaemonHandle::start_default(cfg, false).expect("start daemon");
        Self {
            handle,
            desc,
            _node: node,
            _data_directory: data_directory,
        }
    }

    pub fn new_single_sig(bitcoind: &'a BitcoinD, node_kind: NodeKind, use_taproot: bool) -> Self {
        Self::new(bitcoind, node_kind, use_taproot, DescriptorKind::SingleSig)
    }

    pub fn control(&self) -> &DaemonControl {
        match &self.handle {
            DaemonHandle::Controller { control, .. } => control,
            _ => panic!("expected Controller handle in integration tests"),
        }
    }
}

fn lianad_config(
    network: Network,
    liana_desc: LianaDescriptor,
    node: &Node<'_>,
    data_directory: &tempfile::TempDir,
) -> LianadConfig {
    let bitcoin_config = BitcoinConfig {
        network,
        poll_interval_secs: std::time::Duration::from_secs(1),
    };

    let backend_config = match node {
        Node::Bitcoind(d) => BitcoinBackend::Bitcoind(BitcoindConfig {
            rpc_auth: BitcoindRpcAuth::CookieFile(d.params.cookie_file.clone()),
            addr: std::net::SocketAddr::V4(d.params.rpc_socket),
        }),
        Node::Electrs(e) => BitcoinBackend::Electrum(ElectrumConfig {
            addr: e.electrum_url.clone(),
            validate_domain: true,
        }),
    };

    LianadConfig::new(
        bitcoin_config,
        Some(backend_config),
        log::LevelFilter::Debug,
        liana_desc,
        DataDirectory::new(data_directory.path().to_path_buf()),
    )
}
