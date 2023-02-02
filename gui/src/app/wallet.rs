use std::collections::HashMap;

use crate::hw::HardwareWalletConfig;

use liana::descriptors::MultipathDescriptor;
use liana::miniscript::bitcoin::util::bip32::Fingerprint;

pub const DEFAULT_WALLET_NAME: &str = "Liana";

#[derive(Debug, Clone)]
pub struct Wallet {
    pub name: String,
    pub main_descriptor: MultipathDescriptor,
    pub keys_aliases: HashMap<Fingerprint, String>,
    pub hardware_wallets: Vec<HardwareWalletConfig>,
}

impl Wallet {
    pub fn new(name: String, main_descriptor: MultipathDescriptor) -> Self {
        Self {
            name,
            main_descriptor,
            keys_aliases: HashMap::new(),
            hardware_wallets: Vec::new(),
        }
    }

    pub fn legacy(main_descriptor: MultipathDescriptor) -> Self {
        Self {
            name: DEFAULT_WALLET_NAME.to_string(),
            main_descriptor,
            keys_aliases: HashMap::new(),
            hardware_wallets: Vec::new(),
        }
    }

    pub fn with_key_aliases(mut self, aliases: HashMap<Fingerprint, String>) -> Self {
        self.keys_aliases = aliases;
        self
    }

    pub fn with_harware_wallets(mut self, hardware_wallets: Vec<HardwareWalletConfig>) -> Self {
        self.hardware_wallets = hardware_wallets;
        self
    }
}
