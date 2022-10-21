use async_hwi::{ledger, specter, DeviceKind, Error as HWIError, HWI};
use log::debug;
use minisafe::miniscript::bitcoin::util::bip32::Fingerprint;
use std::sync::Arc;

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

pub async fn list_hardware_wallets() -> Vec<HardwareWallet> {
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
    match ledger::Ledger::try_connect_hid() {
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
    hws
}
