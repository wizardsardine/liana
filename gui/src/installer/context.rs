use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    app::settings::KeySetting,
    lianalite::client::backend::{BackendClient, BackendWalletClient},
    node::bitcoind::{Bitcoind, InternalBitcoindConfig},
    signer::Signer,
};
use async_hwi::DeviceKind;
use liana::{
    config::{BitcoinBackend, BitcoinConfig},
    descriptors::LianaDescriptor,
    miniscript::bitcoin,
};

#[derive(Debug, Clone)]
pub enum RemoteBackend {
    Undefined,
    None,
    // The installer will have to create a wallet from the created descriptor.
    WithoutWallet(BackendClient),
    // The installer will have to fetch the wallet and only install the missing configuration files.
    WithWallet(BackendWalletClient),
}

impl RemoteBackend {
    pub fn user_email(&self) -> Option<&str> {
        match self {
            Self::WithWallet(b) => Some(b.user_email()),
            Self::WithoutWallet(b) => Some(b.user_email()),
            _ => None,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, RemoteBackend::None)
    }
    pub fn is_some(&self) -> bool {
        matches!(
            self,
            RemoteBackend::WithoutWallet { .. } | RemoteBackend::WithWallet { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorTemplate {
    SimpleInheritance,
    Custom,
}

#[derive(Clone)]
pub struct Context {
    pub bitcoin_config: BitcoinConfig,
    pub bitcoin_backend: Option<BitcoinBackend>,
    pub descriptor_template: DescriptorTemplate,
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
    pub remote_backend: RemoteBackend,
}

impl Context {
    pub fn new(
        network: bitcoin::Network,
        data_dir: PathBuf,
        remote_backend: RemoteBackend,
    ) -> Self {
        Self {
            descriptor_template: DescriptorTemplate::Custom,
            bitcoin_config: BitcoinConfig {
                network,
                poll_interval_secs: Duration::from_secs(30),
            },
            hws: Vec::new(),
            keys: Vec::new(),
            bitcoin_backend: None,
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
