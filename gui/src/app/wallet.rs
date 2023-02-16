use std::collections::{HashMap, HashSet};

use crate::{hw::HardwareWalletConfig, signer::Signer};

use liana::descriptors::MultipathDescriptor;
use liana::miniscript::bitcoin::util::bip32::Fingerprint;

pub const DEFAULT_WALLET_NAME: &str = "Liana";

#[derive(Debug)]
pub struct Wallet {
    pub name: String,
    pub main_descriptor: MultipathDescriptor,
    pub keys_aliases: HashMap<Fingerprint, String>,
    pub hardware_wallets: Vec<HardwareWalletConfig>,
    pub signer: Option<Signer>,
}

impl Wallet {
    pub fn new(name: String, main_descriptor: MultipathDescriptor) -> Self {
        Self {
            name,
            main_descriptor,
            keys_aliases: HashMap::new(),
            hardware_wallets: Vec::new(),
            signer: None,
        }
    }

    pub fn legacy(main_descriptor: MultipathDescriptor) -> Self {
        Self {
            name: DEFAULT_WALLET_NAME.to_string(),
            main_descriptor,
            keys_aliases: HashMap::new(),
            hardware_wallets: Vec::new(),
            signer: None,
        }
    }

    pub fn with_key_aliases(mut self, aliases: HashMap<Fingerprint, String>) -> Self {
        self.keys_aliases = aliases;
        self
    }

    pub fn with_hardware_wallets(mut self, hardware_wallets: Vec<HardwareWalletConfig>) -> Self {
        self.hardware_wallets = hardware_wallets;
        self
    }

    pub fn with_signer(mut self, signer: Signer) -> Self {
        self.signer = Some(signer);
        self
    }

    pub fn descriptor_keys(&self) -> HashSet<Fingerprint> {
        let info = self.main_descriptor.info();
        let mut descriptor_keys = HashSet::new();
        for (fingerprint, _) in info.primary_path().thresh_origins().1.iter() {
            descriptor_keys.insert(*fingerprint);
        }
        for (fingerprint, _) in info.recovery_path().1.thresh_origins().1.iter() {
            descriptor_keys.insert(*fingerprint);
        }
        descriptor_keys
    }
}
