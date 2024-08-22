use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    app::settings::KeySetting,
    bitcoind::{Bitcoind, InternalBitcoindConfig},
    lianalite::client::backend::{BackendClient, BackendWalletClient},
    signer::Signer,
};
use async_hwi::DeviceKind;
use liana::{
    config::{BitcoinConfig, BitcoindConfig},
    descriptors::LianaDescriptor,
    miniscript::bitcoin,
};

#[derive(Debug, Clone)]
pub enum RemoteBackend {
    // The installer will have to create a wallet from the created descriptor.
    WithoutWallet(BackendClient),
    // The installer will have to fetch the wallet and only install the missing configuration files.
    WithWallet(BackendWalletClient),
}

impl RemoteBackend {
    pub fn user_email(&self) -> &str {
        match self {
            Self::WithWallet(b) => b.user_email(),
            Self::WithoutWallet(b) => b.user_email(),
        }
    }
}

#[derive(Clone)]
pub struct Context {
    pub bitcoin_config: BitcoinConfig,
    pub bitcoind_config: Option<BitcoindConfig>,
    pub descriptor: Option<LianaDescriptor>,
    pub keys: Vec<KeySetting>,
    pub hws: Vec<(DeviceKind, bitcoin::bip32::Fingerprint, Option<[u8; 32]>)>,
    pub data_dir: PathBuf,
    pub network: bitcoin::Network,
    pub hw_is_used: bool,
    // In case a user entered a mnemonic,
    // we dont want to override the generated signer with it.
    pub recovered_signer: Option<Arc<Signer>>,
    pub bitcoind_is_external: bool,
    pub internal_bitcoind_config: Option<InternalBitcoindConfig>,
    pub internal_bitcoind: Option<Bitcoind>,
    pub remote_backend: Option<RemoteBackend>,
}

impl Context {
    pub fn new(
        network: bitcoin::Network,
        data_dir: PathBuf,
        remote_backend: Option<RemoteBackend>,
    ) -> Self {
        Self {
            bitcoin_config: BitcoinConfig {
                network,
                poll_interval_secs: Duration::from_secs(30),
            },
            hws: Vec::new(),
            keys: Vec::new(),
            bitcoind_config: None,
            descriptor: None,
            data_dir,
            network,
            hw_is_used: false,
            recovered_signer: None,
            bitcoind_is_external: true,
            internal_bitcoind_config: None,
            internal_bitcoind: None,
            remote_backend,
        }
    }
}
