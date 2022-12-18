use std::sync::Arc;

use async_hwi::{ledger, specter, DeviceKind, Error as HWIError, HWI};
use liana::miniscript::bitcoin::{
    hashes::hex::{FromHex, ToHex},
    util::bip32::Fingerprint,
};
use log::debug;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct HardwareWallet {
    pub device: Arc<dyn HWI + Send + Sync>,
    pub kind: DeviceKind,
    pub fingerprint: Fingerprint,
}

impl HardwareWallet {
    async fn new(device: Arc<dyn HWI + Send + Sync>) -> Result<Self, HWIError> {
        let kind = device.device_kind();
        let fingerprint = device.get_master_fingerprint().await?;
        Ok(Self {
            device,
            kind,
            fingerprint,
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HardwareWalletConfig {
    pub kind: String,
    pub fingerprint: String,
    pub token: String,
}

impl HardwareWalletConfig {
    pub fn new(kind: &async_hwi::DeviceKind, fingerprint: &Fingerprint, token: &[u8; 32]) -> Self {
        Self {
            kind: kind.to_string(),
            fingerprint: fingerprint.to_string(),
            token: token.to_hex(),
        }
    }

    fn token(&self) -> [u8; 32] {
        let mut res = [0x00; 32];
        res.copy_from_slice(&Vec::from_hex(&self.token).unwrap());
        res
    }
}

pub async fn list_hardware_wallets(
    cfg: &[HardwareWalletConfig],
    wallet: Option<(&str, &str)>,
) -> Vec<HardwareWallet> {
    let mut hws: Vec<HardwareWallet> = Vec::new();
    match specter::SpecterSimulator::try_connect().await {
        Ok(device) => match HardwareWallet::new(Arc::new(device)).await {
            Ok(hw) => hws.push(hw),
            Err(e) => {
                debug!("{}", e);
            }
        },
        Err(HWIError::DeviceNotFound) => {}
        Err(e) => {
            debug!("{}", e);
        }
    }
    match specter::Specter::try_connect_serial().await {
        Ok(device) => match HardwareWallet::new(Arc::new(device)).await {
            Ok(hw) => hws.push(hw),
            Err(e) => {
                debug!("{}", e);
            }
        },
        Err(HWIError::DeviceNotFound) => {}
        Err(e) => {
            debug!("{}", e);
        }
    }
    match ledger::LedgerSimulator::try_connect().await {
        Ok(mut device) => match device.get_master_fingerprint().await {
            Ok(fingerprint) => {
                if let Some((name, descriptor)) = wallet {
                    device
                        .load_wallet(
                            name,
                            descriptor,
                            cfg.iter()
                                .find(|cfg| cfg.fingerprint == fingerprint.to_string())
                                .map(|cfg| cfg.token()),
                        )
                        .expect("Configuration must be correct");
                }

                hws.push(HardwareWallet {
                    kind: device.device_kind(),
                    fingerprint,
                    device: Arc::new(device),
                });
            }
            Err(e) => {
                debug!("{}", e);
            }
        },
        Err(HWIError::DeviceNotFound) => {}
        Err(e) => {
            debug!("{}", e);
        }
    }
    match ledger::Ledger::try_connect_hid() {
        Ok(mut device) => match device.get_master_fingerprint().await {
            Ok(fingerprint) => {
                if let Some((name, descriptor)) = wallet {
                    device
                        .load_wallet(
                            name,
                            descriptor,
                            cfg.iter()
                                .find(|cfg| cfg.fingerprint == fingerprint.to_string())
                                .map(|cfg| cfg.token()),
                        )
                        .expect("Configuration must be correct");
                }

                hws.push(HardwareWallet {
                    kind: device.device_kind(),
                    fingerprint,
                    device: Arc::new(device),
                });
            }
            Err(e) => {
                debug!("{}", e);
            }
        },
        Err(HWIError::DeviceNotFound) => {}
        Err(e) => {
            debug!("{}", e);
        }
    }
    hws
}
