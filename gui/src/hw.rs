use std::sync::Arc;

use async_hwi::{ledger, specter, DeviceKind, Error as HWIError, Version, HWI};
use liana::miniscript::bitcoin::{
    hashes::hex::{FromHex, ToHex},
    util::bip32::Fingerprint,
};
use log::debug;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub enum HardwareWallet {
    Unsupported {
        kind: DeviceKind,
        version: Option<Version>,
        message: String,
    },
    Supported {
        device: Arc<dyn HWI + Send + Sync>,
        kind: DeviceKind,
        fingerprint: Fingerprint,
        version: Option<Version>,
    },
}

impl HardwareWallet {
    async fn new(device: Arc<dyn HWI + Send + Sync>) -> Result<Self, HWIError> {
        let kind = device.device_kind();
        let fingerprint = device.get_master_fingerprint().await?;
        let version = device.get_version().await.ok();
        Ok(Self::Supported {
            device,
            kind,
            fingerprint,
            version,
        })
    }

    pub fn kind(&self) -> &DeviceKind {
        match self {
            Self::Unsupported { kind, .. } => kind,
            Self::Supported { kind, .. } => kind,
        }
    }

    pub fn fingerprint(&self) -> Option<Fingerprint> {
        match self {
            Self::Unsupported { .. } => None,
            Self::Supported { fingerprint, .. } => Some(*fingerprint),
        }
    }

    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Supported { .. })
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

                let version = device.get_version().await.ok();
                if ledger_version_supported(version.as_ref()) {
                    hws.push(HardwareWallet::Supported {
                        kind: device.device_kind(),
                        fingerprint,
                        device: Arc::new(device),
                        version,
                    });
                } else {
                    hws.push(HardwareWallet::Unsupported {
                        kind: device.device_kind(),
                        version,
                        message: "Minimal supported app version is 2.1.0".to_string(),
                    });
                }
            }
            Err(_) => {
                hws.push(HardwareWallet::Unsupported {
                    kind: device.device_kind(),
                    version: None,
                    message: "Minimal supported app version is 2.1.0".to_string(),
                });
            }
        },
        Err(HWIError::DeviceNotFound) => {}
        Err(e) => {
            debug!("{}", e);
        }
    }
    match ledger::HidApi::new() {
        Err(e) => {
            debug!("{}", e);
        }
        Ok(api) => {
            for detected in ledger::Ledger::<ledger::TransportHID>::enumerate(&api) {
                match ledger::Ledger::<ledger::TransportHID>::connect(&api, detected) {
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

                            let version = device.get_version().await.ok();
                            if ledger_version_supported(version.as_ref()) {
                                hws.push(HardwareWallet::Supported {
                                    kind: device.device_kind(),
                                    fingerprint,
                                    device: Arc::new(device),
                                    version,
                                });
                            } else {
                                hws.push(HardwareWallet::Unsupported {
                                    kind: device.device_kind(),
                                    version,
                                    message: "Minimal supported app version is 2.1.0".to_string(),
                                });
                            }
                        }
                        Err(_) => {
                            hws.push(HardwareWallet::Unsupported {
                                kind: device.device_kind(),
                                version: None,
                                message: "Minimal supported app version is 2.1.0".to_string(),
                            });
                        }
                    },
                    Err(HWIError::DeviceNotFound) => {}
                    Err(e) => {
                        debug!("{}", e);
                    }
                }
            }
        }
    }
    hws
}

fn ledger_version_supported(version: Option<&Version>) -> bool {
    if let Some(version) = version {
        if version.major >= 2 {
            if version.major == 2 {
                version.minor >= 1
            } else {
                true
            }
        } else {
            false
        }
    } else {
        false
    }
}
