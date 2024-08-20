use iced::{Command, Subscription};
use ledger_manager::utils::InstallStep;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    app::{settings, wallet::Wallet},
    ledger_upgrade::{
        ledger_upgrade_subscriptions, maybe_start_upgrade, update_upgrade_state, UpgradeMessage,
    },
};
use async_hwi::{
    bitbox::{api::runtime, BitBox02, PairingBitbox02},
    coldcard,
    jade::{self, Jade},
    ledger::{self, DeviceInfo, HidApi},
    specter, DeviceKind, Error as HWIError, Version, HWI,
};
use liana::miniscript::bitcoin::{bip32::Fingerprint, hashes::hex::FromHex, Network};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub enum UnsupportedReason {
    Version {
        minimal_supported_version: &'static str,
    },
    Method(&'static str),
    NotPartOfWallet(Fingerprint),
    WrongNetwork,
    Taproot,
}

// Todo drop the Clone, to remove the Mutex on HardwareWallet::Locked
#[derive(Debug, Clone)]
pub enum HardwareWallet {
    Unsupported {
        id: String,
        kind: DeviceKind,
        version: Option<Version>,
        reason: UnsupportedReason,
    },
    Locked {
        id: String,
        // None if the device is currently unlocking in a command.
        device: Arc<Mutex<Option<LockedDevice>>>,
        pairing_code: Option<String>,
        kind: DeviceKind,
    },
    Supported {
        id: String,
        device: Arc<dyn HWI + Sync + Send>,
        kind: DeviceKind,
        fingerprint: Fingerprint,
        version: Option<Version>,
        registered: Option<bool>,
        alias: Option<String>,
    },
    NeedUpgrade {
        id: String,
        kind: DeviceKind,
        fingerprint: Fingerprint,
        version: Option<Version>,
        upgrade_in_progress: bool,
        upgrade_step: Option<InstallStep<Version>>,
        upgrade_log: Vec<String>,
        upgrade_testnet: bool,
        upgraded_version: Option<Version>,
    },
}

pub enum LockedDevice {
    BitBox02(Box<PairingBitbox02<runtime::TokioRuntime>>),
    Jade(Jade<jade::SerialTransport>),
}

impl std::fmt::Debug for LockedDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaitingConfirmBitBox").finish()
    }
}

impl HardwareWallet {
    async fn new(
        id: String,
        device: Arc<dyn HWI + Send + Sync>,
        aliases: Option<&HashMap<Fingerprint, String>>,
    ) -> Result<Self, HWIError> {
        let kind = device.device_kind();
        let fingerprint = device.get_master_fingerprint().await?;
        let version = device.get_version().await.ok();
        Ok(Self::Supported {
            id,
            device,
            kind,
            fingerprint,
            version,
            registered: None,
            alias: aliases.and_then(|aliases| aliases.get(&fingerprint).cloned()),
        })
    }

    pub fn id(&self) -> &String {
        match self {
            Self::Locked { id, .. } => id,
            Self::Unsupported { id, .. } => id,
            Self::Supported { id, .. } => id,
            Self::NeedUpgrade { id, .. } => id,
        }
    }

    pub fn kind(&self) -> &DeviceKind {
        match self {
            Self::Locked { kind, .. } => kind,
            Self::Unsupported { kind, .. } => kind,
            Self::Supported { kind, .. } => kind,
            Self::NeedUpgrade { kind, .. } => kind,
        }
    }

    pub fn fingerprint(&self) -> Option<Fingerprint> {
        match self {
            Self::Locked { .. } => None,
            Self::Unsupported { .. } => None,
            Self::Supported { fingerprint, .. } => Some(*fingerprint),
            Self::NeedUpgrade { fingerprint, .. } => Some(*fingerprint),
        }
    }

    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Supported { .. })
    }

    pub fn is_upgrade_in_progress(&self) -> bool {
        if let Self::NeedUpgrade {
            upgrade_in_progress,
            ..
        } = self
        {
            *upgrade_in_progress
        } else {
            false
        }
    }

    pub fn start_upgrade(&mut self, network: Network) {
        if let Self::NeedUpgrade {
            upgrade_in_progress,
            upgrade_step,
            upgrade_testnet,
            ..
        } = self
        {
            *upgrade_step = None;
            *upgrade_in_progress = true;
            *upgrade_testnet = network != Network::Bitcoin;
        }
    }

    pub fn upgrade_ended(&mut self, version: Version) {
        if let Self::NeedUpgrade {
            upgrade_in_progress,
            upgrade_step,
            upgrade_log,
            upgraded_version,
            ..
        } = self
        {
            *upgrade_in_progress = false;
            *upgrade_step = Some(InstallStep::Completed);
            *upgrade_log = Vec::new();
            *upgraded_version = Some(version);
        }
    }

    pub fn upgrade_failed(&mut self) {
        if let Self::NeedUpgrade {
            upgrade_in_progress,
            upgrade_step,
            ..
        } = self
        {
            *upgrade_in_progress = false;
            *upgrade_step = Some(InstallStep::Error("Failed to install app".into()));
        }
    }

    pub fn push_log(&mut self, log: String) {
        if let Self::NeedUpgrade { upgrade_log, .. } = self {
            upgrade_log.push(log);
        }
    }

    pub fn logs(&self) -> Vec<String> {
        if let Self::NeedUpgrade { upgrade_log, .. } = self {
            upgrade_log.clone()
        } else {
            Vec::new()
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HardwareWalletConfig {
    pub kind: String,
    pub fingerprint: Fingerprint,
    pub token: String,
}

impl HardwareWalletConfig {
    pub fn new(kind: &async_hwi::DeviceKind, fingerprint: Fingerprint, token: &[u8; 32]) -> Self {
        Self {
            kind: kind.to_string(),
            fingerprint,
            token: hex::encode(token),
        }
    }

    fn token(&self) -> [u8; 32] {
        let mut res = [0x00; 32];
        res.copy_from_slice(&Vec::from_hex(&self.token).unwrap());
        res
    }
}

#[derive(Debug, Clone)]
pub enum HardwareWalletMessage {
    Error(String),
    List(ConnectedList),
    Unlocked(String, Result<HardwareWallet, async_hwi::Error>),
    Upgrade(UpgradeMessage),
    LockModal(bool),
    UpgradeLedger(String, Network),
}

#[derive(Debug, Clone)]
pub struct ConnectedList {
    new: Vec<HardwareWallet>,
    still: Vec<String>,
}

pub struct HardwareWallets {
    network: Network,
    pub list: Vec<HardwareWallet>,
    pub aliases: HashMap<Fingerprint, String>,
    wallet: Option<Arc<Wallet>>,
    datadir_path: PathBuf,
    refresh_index: u8,
}

impl std::fmt::Debug for HardwareWallets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaitingConfirmBitBox").finish()
    }
}

impl HardwareWallets {
    pub fn new(datadir_path: PathBuf, network: Network) -> Self {
        Self {
            network,
            list: Vec::new(),
            aliases: HashMap::new(),
            wallet: None,
            datadir_path,
            refresh_index: 0,
        }
    }

    pub fn with_wallet(mut self, wallet: Arc<Wallet>) -> Self {
        self.aliases.clone_from(&wallet.keys_aliases);
        self.wallet = Some(wallet);
        self
    }

    pub fn set_alias(&mut self, fg: Fingerprint, new_alias: String) {
        // remove all (fingerprint, alias) with same alias.
        self.aliases.retain(|_, a| *a != new_alias);
        for hw in &mut self.list {
            if let HardwareWallet::Supported {
                fingerprint, alias, ..
            } = hw
            {
                if *fingerprint == fg {
                    *alias = Some(new_alias.clone());
                } else if alias.as_ref() == Some(&new_alias) {
                    *alias = None;
                }
            }
        }
        self.aliases.insert(fg, new_alias);
    }

    pub fn load_aliases(&mut self, aliases: HashMap<Fingerprint, String>) {
        self.aliases = aliases;
    }

    pub fn set_network(&mut self, network: Network) {
        self.network = network;
        self.list = Vec::new();
    }

    pub fn update(
        &mut self,
        message: HardwareWalletMessage,
    ) -> Result<Command<HardwareWalletMessage>, async_hwi::Error> {
        fn upgrade_is_completed(device: &HardwareWallet) -> bool {
            if let HardwareWallet::NeedUpgrade {
                upgrade_step: Some(step),
                upgraded_version: Some(version),
                ..
            } = device
            {
                matches!(step, InstallStep::Completed)
                    && ledger_version_supported(Some(version), true)
            } else {
                false
            }
        }

        // reset the refresh state if upgrade completed
        if self.list.iter().any(upgrade_is_completed) {
            self.reset_refresh();
        }
        // then remove the device w/ upgrade completed from the list
        self.list.retain(|d| !upgrade_is_completed(d));

        match message {
            HardwareWalletMessage::Error(e) => Err(async_hwi::Error::Device(e)),
            HardwareWalletMessage::List(ConnectedList { still, new }) => {
                let hws_upgrading: Vec<_> = self
                    .list
                    .iter()
                    .filter(|dev| matches!(dev, HardwareWallet::NeedUpgrade { .. }))
                    .cloned()
                    .collect();
                // remove disconnected
                self.list.retain(|hw| still.contains(hw.id()));
                // Upgrading devices are not automaticaly removed
                for n in hws_upgrading {
                    if !self.list.iter().any(|d| *d.id() == *n.id()) {
                        self.list.push(n);
                    }
                }
                // avoid duplicates
                for n in new {
                    if !self.list.iter().any(|d| *d.id() == *n.id()) {
                        self.list.push(n);
                    }
                }
                let mut cmds = Vec::new();
                for hw in &mut self.list {
                    match hw {
                        HardwareWallet::Supported {
                            fingerprint, alias, ..
                        } => {
                            *alias = self.aliases.get(fingerprint).cloned();
                        }
                        HardwareWallet::Locked { device, id, .. } => {
                            match device.lock().unwrap().take() {
                                None => {}
                                Some(LockedDevice::BitBox02(bb)) => {
                                    let id = id.to_string();
                                    let id_cloned = id.clone();
                                    let network = self.network;
                                    let wallet = self.wallet.clone();
                                    cmds.push(Command::perform(
                                        async move {
                                            let paired_bb = bb.wait_confirm().await?;
                                            let mut bitbox2 =
                                                BitBox02::from(paired_bb).with_network(network);
                                            let fingerprint =
                                                bitbox2.get_master_fingerprint().await?;
                                            let mut registered = false;
                                            if let Some(wallet) = &wallet {
                                                let desc = wallet.main_descriptor.to_string();
                                                bitbox2 = bitbox2.with_policy(&desc)?;
                                                registered =
                                                    bitbox2.is_policy_registered(&desc).await?;
                                                if wallet.descriptor_keys().contains(&fingerprint) {
                                                    Ok(HardwareWallet::Supported {
                                                        id: id.clone(),
                                                        kind: DeviceKind::BitBox02,
                                                        fingerprint,
                                                        device: bitbox2.into(),
                                                        version: None,
                                                        registered: Some(registered),
                                                        alias: None,
                                                    })
                                                } else {
                                                    Ok(HardwareWallet::Unsupported {
                                                        id: id.clone(),
                                                        kind: DeviceKind::BitBox02,
                                                        version: None,
                                                        reason: UnsupportedReason::NotPartOfWallet(
                                                            fingerprint,
                                                        ),
                                                    })
                                                }
                                            } else {
                                                Ok(HardwareWallet::Supported {
                                                    id: id.clone(),
                                                    kind: DeviceKind::BitBox02,
                                                    fingerprint,
                                                    device: bitbox2.into(),
                                                    version: None,
                                                    registered: Some(registered),
                                                    alias: None,
                                                })
                                            }
                                        },
                                        |res| HardwareWalletMessage::Unlocked(id_cloned, res),
                                    ));
                                }
                                Some(LockedDevice::Jade(device)) => {
                                    let id = id.clone();
                                    let id_cloned = id.clone();
                                    let network = self.network;
                                    let wallet = self.wallet.clone();
                                    cmds.push(Command::perform(
                                        async move {
                                            device.auth().await?;
                                            handle_jade_device(
                                                id,
                                                network,
                                                device,
                                                wallet.as_ref().map(|w| w.as_ref()),
                                                None,
                                                false,
                                            )
                                            .await
                                        },
                                        |res| HardwareWalletMessage::Unlocked(id_cloned, res),
                                    ));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if cmds.is_empty() {
                    Ok(Command::none())
                } else {
                    Ok(Command::batch(cmds))
                }
            }
            HardwareWalletMessage::Unlocked(id, res) => {
                match res {
                    Err(e) => {
                        warn!("Pairing failed with an external device {}", e);
                        self.list.retain(|hw| hw.id() != &id);
                    }
                    Ok(hw) => {
                        if let Some(h) = self.list.iter_mut().find(|hw1| {
                            if let HardwareWallet::Locked { id, .. } = hw1 {
                                id == hw.id()
                            } else {
                                false
                            }
                        }) {
                            *h = hw;
                            if let HardwareWallet::Supported {
                                fingerprint, alias, ..
                            } = h
                            {
                                *alias = self.aliases.get(fingerprint).cloned();
                            }
                        }
                    }
                }
                Ok(Command::none())
            }
            HardwareWalletMessage::UpgradeLedger(id, network) => {
                let lock_modal = maybe_start_upgrade(id, self, network);
                Ok(if let Some(upgrading) = lock_modal {
                    Command::perform(
                        async move { HardwareWalletMessage::LockModal(upgrading) },
                        |msg| msg,
                    )
                } else {
                    Command::none()
                })
            }
            HardwareWalletMessage::Upgrade(m) => {
                let lock_modal = update_upgrade_state(m, self);
                Ok(if let Some(upgrading) = lock_modal {
                    Command::perform(
                        async move { HardwareWalletMessage::LockModal(upgrading) },
                        |msg| msg,
                    )
                } else {
                    Command::none()
                })
            }
            _ => Ok(Command::none()),
        }
    }

    pub fn reset_refresh(&mut self) {
        // In order to reset the subscription state we need to change the
        // subscription id
        self.refresh_index = self.refresh_index.wrapping_add(1);
    }

    pub fn refresh(&self, taproot: bool) -> iced::Subscription<HardwareWalletMessage> {
        let id = format!(
            "refresh-{}-{}-{}",
            self.network, self.refresh_index, taproot
        );
        iced::subscription::unfold(
            id,
            State {
                network: self.network,
                keys_aliases: self.aliases.clone(),
                wallet: self.wallet.clone(),
                connected_supported_hws: Vec::new(),
                hws_upgrade: Vec::new(),
                api: None,
                datadir_path: self.datadir_path.clone(),
                taproot,
                hws: Vec::new(),
                still: Vec::new(),
                still_upgrade: Vec::new(),
            },
            refresh,
        )
    }
}

pub fn hw_subscriptions(
    hws: &HardwareWallets,
    taproot: Option<bool>,
) -> Subscription<HardwareWalletMessage> {
    let mut subs = ledger_upgrade_subscriptions(hws);
    if let Some(taproot) = taproot {
        subs.push(hws.refresh(taproot))
    }
    Subscription::batch(subs)
}

pub struct State {
    network: Network,
    keys_aliases: HashMap<Fingerprint, String>,
    wallet: Option<Arc<Wallet>>,
    connected_supported_hws: Vec<String>,
    api: Option<ledger::HidApi>,
    datadir_path: PathBuf,
    taproot: bool,
    hws: Vec<HardwareWallet>,
    still: Vec<String>,
    hws_upgrade: Vec<HardwareWallet>,
    still_upgrade: Vec<String>,
}

async fn refresh(mut state: State) -> (HardwareWalletMessage, State) {
    // do not sleep on first call
    if state.api.is_some() {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let mut api = if let Some(api) = state.api.take() {
        api
    } else {
        match ledger::HidApi::new() {
            Ok(api) => api,
            Err(e) => {
                return (HardwareWalletMessage::Error(e.to_string()), state);
            }
        }
    };

    if let Err(e) = api.refresh_devices() {
        return (HardwareWalletMessage::Error(e.to_string()), state);
    };

    poll_specter_simulator(&mut state).await;
    poll_specter(&mut state).await;
    poll_jade(&mut state).await;
    poll_ledger_simulator(&mut state).await;

    for device_info in api.device_list() {
        if async_hwi::bitbox::is_bitbox02(device_info)
            && handle_bitbox02_device(&mut state, device_info, &api).await
        {
            continue;
        }
        if device_info.vendor_id() == coldcard::api::COINKITE_VID
            && device_info.product_id() == coldcard::api::CKCC_PID
            && handle_coldcard_device(&mut state, device_info, &api).await
        {
            continue;
        }
    }

    poll_ledger(&mut state, &api).await;

    if let Some(wallet) = &state.wallet {
        let wallet_keys = wallet.descriptor_keys();
        for hw in &mut state.hws {
            if let HardwareWallet::Supported {
                fingerprint,
                id,
                kind,
                version,
                ..
            } = &hw
            {
                if !wallet_keys.contains(fingerprint) {
                    *hw = HardwareWallet::Unsupported {
                        id: id.clone(),
                        kind: *kind,
                        version: version.clone(),
                        reason: UnsupportedReason::NotPartOfWallet(*fingerprint),
                    };
                }
            }
        }
    }

    state.connected_supported_hws = state
        .still
        .iter()
        .chain(state.hws.iter().filter_map(|hw| match hw {
            HardwareWallet::Locked { id, .. } => Some(id),
            HardwareWallet::Supported { id, .. } => Some(id),
            _ => None,
        }))
        .cloned()
        .collect();
    let mut new_upgrade = state
        .hws_upgrade
        .clone()
        .into_iter()
        .filter(|d| !state.still_upgrade.contains(d.id()))
        .collect();
    state.hws.append(&mut new_upgrade);
    state.still.append(&mut state.still_upgrade);
    let msg = HardwareWalletMessage::List(ConnectedList {
        new: state.hws,
        still: state.still,
    });
    (state.hws, state.still) = (Vec::new(), Vec::new());
    state.api = Some(api);
    (msg, state)
}

pub async fn poll_specter_simulator(state: &mut State) {
    match specter::SpecterSimulator::try_connect().await {
        Ok(device) => {
            let id = "specter-simulator".to_string();
            if state.connected_supported_hws.contains(&id) {
                state.still.push(id);
            } else {
                match HardwareWallet::new(id, Arc::new(device), Some(&state.keys_aliases)).await {
                    Ok(hw) => state.hws.push(hw),
                    Err(e) => {
                        debug!("{}", e);
                    }
                }
            }
        }
        Err(HWIError::DeviceNotFound) => {}
        Err(e) => {
            debug!("{}", e);
        }
    }
}

pub async fn poll_specter(state: &mut State) {
    match specter::SerialTransport::enumerate_potential_ports() {
        Ok(ports) => {
            for port in ports {
                let id = format!("specter-{}", port);
                if state.connected_supported_hws.contains(&id) {
                    state.still.push(id);
                } else {
                    match specter::Specter::<specter::SerialTransport>::new(port.clone()) {
                        Err(e) => {
                            warn!("{}", e);
                        }
                        Ok(device) => {
                            if tokio::time::timeout(
                                std::time::Duration::from_millis(500),
                                device.fingerprint(),
                            )
                            .await
                            .is_ok()
                            {
                                match HardwareWallet::new(
                                    id,
                                    Arc::new(device),
                                    Some(&state.keys_aliases),
                                )
                                .await
                                {
                                    Ok(hw) => state.hws.push(hw),
                                    Err(e) => {
                                        debug!("{}", e);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Err(e) => warn!("Error while listing specter wallets: {}", e),
    }
}

pub async fn poll_jade(state: &mut State) {
    match jade::SerialTransport::enumerate_potential_ports() {
        Ok(ports) => {
            for port in ports {
                let id = format!("jade-{}", port);
                if state.connected_supported_hws.contains(&id) {
                    state.still.push(id);
                } else {
                    match jade::SerialTransport::new(port) {
                        Err(e) => {
                            warn!("{:?}", e);
                        }
                        Ok(device) => {
                            match handle_jade_device(
                                id,
                                state.network,
                                Jade::new(device).with_network(state.network),
                                state.wallet.as_ref().map(|w| w.as_ref()),
                                Some(&state.keys_aliases),
                                state.taproot,
                            )
                            .await
                            {
                                Ok(hw) => {
                                    state.hws.push(hw);
                                }
                                Err(e) => {
                                    warn!("{:?}", e);
                                }
                            }
                        }
                    }
                }
            }
        }
        Err(e) => warn!("Error while listing jade devices: {}", e),
    }
}

pub async fn poll_ledger_simulator(state: &mut State) {
    match ledger::LedgerSimulator::try_connect().await {
        Ok(mut device) => {
            let id = "ledger-simulator".to_string();
            if state.connected_supported_hws.contains(&id) {
                state.still.push(id);
            } else {
                match device.get_master_fingerprint().await {
                    Ok(fingerprint) => {
                        let version = device.get_version().await.ok();
                        if ledger_version_supported(version.as_ref(), state.taproot) {
                            let mut registered = false;
                            if let Some(w) = &state.wallet {
                                if let Some(cfg) = w
                                    .hardware_wallets
                                    .iter()
                                    .find(|cfg| cfg.fingerprint == fingerprint)
                                {
                                    device = device
                                        .with_wallet(
                                            &w.name,
                                            &w.main_descriptor.to_string(),
                                            Some(cfg.token()),
                                        )
                                        .expect("Configuration must be correct");
                                    registered = true;
                                }
                            }
                            state.hws.push(HardwareWallet::Supported {
                                id,
                                kind: device.device_kind(),
                                fingerprint,
                                device: Arc::new(device),
                                version,
                                registered: Some(registered),
                                alias: state.keys_aliases.get(&fingerprint).cloned(),
                            });
                        } else {
                            state.hws.push(HardwareWallet::Unsupported {
                                id,
                                kind: device.device_kind(),
                                version,
                                reason: UnsupportedReason::Version {
                                    minimal_supported_version: "2.1.0",
                                },
                            });
                        }
                    }
                    Err(_) => {
                        state.hws.push(HardwareWallet::Unsupported {
                            id,
                            kind: device.device_kind(),
                            version: None,
                            reason: UnsupportedReason::Version {
                                minimal_supported_version: "2.1.0",
                            },
                        });
                    }
                }
            }
        }
        Err(HWIError::DeviceNotFound) => {}
        Err(e) => {
            debug!("{}", e);
        }
    }
}

pub async fn poll_ledger(state: &mut State, api: &HidApi) {
    for detected in ledger::Ledger::<ledger::TransportHID>::enumerate(api) {
        let id = ledger_id(detected);
        if state.hws_upgrade.iter().any(|d| *d.id() == id) {
            state.still_upgrade.push(id.clone());
            continue;
        }
        if state.connected_supported_hws.contains(&id) {
            state.still.push(id);
            continue;
        }
        match ledger::Ledger::<ledger::TransportHID>::connect(api, detected) {
            Ok(mut device) => match device.get_master_fingerprint().await {
                Ok(fingerprint) => {
                    let version = device.get_version().await.ok();
                    if ledger_version_supported(version.as_ref(), state.taproot) {
                        let mut registered = false;
                        if let Some(w) = &state.wallet {
                            if let Some(cfg) = w
                                .hardware_wallets
                                .iter()
                                .find(|cfg| cfg.fingerprint == fingerprint)
                            {
                                device = device
                                    .with_wallet(
                                        &w.name,
                                        &w.main_descriptor.to_string(),
                                        Some(cfg.token()),
                                    )
                                    .expect("Configuration must be correct");
                                registered = true;
                            }
                        }
                        state.hws.push(HardwareWallet::Supported {
                            id,
                            kind: device.device_kind(),
                            fingerprint,
                            device: Arc::new(device),
                            version,
                            registered: Some(registered),
                            alias: state.keys_aliases.get(&fingerprint).cloned(),
                        });
                    } else if ledger_need_taproot_upgrade(&version) {
                        state.hws_upgrade.push(HardwareWallet::NeedUpgrade {
                            id,
                            kind: device.device_kind(),
                            fingerprint,
                            version,
                            upgrade_in_progress: false,
                            upgrade_step: None,
                            upgrade_log: Vec::new(),
                            upgrade_testnet: state.network != Network::Bitcoin,
                            upgraded_version: None,
                        });
                    } else {
                        state.hws.push(HardwareWallet::Unsupported {
                            id,
                            kind: device.device_kind(),
                            version,
                            reason: UnsupportedReason::Version {
                                minimal_supported_version: "2.1.0",
                            },
                        });
                    }
                }
                Err(_) => {
                    state.hws.push(HardwareWallet::Unsupported {
                        id,
                        kind: device.device_kind(),
                        version: None,
                        reason: UnsupportedReason::Version {
                            minimal_supported_version: "2.1.0",
                        },
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

pub async fn handle_bitbox02_device(
    state: &mut State,
    device_info: &DeviceInfo,
    api: &HidApi,
) -> bool {
    let id = format!(
        "bitbox-{:?}-{}-{}",
        device_info.path(),
        device_info.vendor_id(),
        device_info.product_id()
    );
    if state.connected_supported_hws.contains(&id) {
        state.still.push(id);
        return true;
    }
    if let Ok(device) = device_info.open_device(api) {
        if let Ok(device) = PairingBitbox02::connect(
            device,
            Some(Box::new(settings::global::PersistedBitboxNoiseConfig::new(
                &state.datadir_path,
            ))),
        )
        .await
        {
            let hw = if !state.taproot {
                HardwareWallet::Locked {
                    id,
                    kind: DeviceKind::BitBox02,
                    pairing_code: device.pairing_code().map(|s| s.replace('\n', " ")),
                    device: Arc::new(Mutex::new(Some(LockedDevice::BitBox02(Box::new(device))))),
                }
            } else {
                HardwareWallet::Unsupported {
                    id,
                    kind: DeviceKind::BitBox02,
                    version: None,
                    reason: UnsupportedReason::Taproot,
                }
            };
            state.hws.push(hw);
            return true;
        }
    }
    false
}

async fn handle_jade_device(
    id: String,
    network: Network,
    device: Jade<async_hwi::jade::SerialTransport>,
    wallet: Option<&Wallet>,
    keys_aliases: Option<&HashMap<Fingerprint, String>>,
    taproot: bool,
) -> Result<HardwareWallet, HWIError> {
    let info = device.get_info().await?;
    let version = async_hwi::parse_version(&info.jade_version).ok();
    // Jade may not be setup for the current network
    if taproot {
        Ok(HardwareWallet::Unsupported {
            id,
            kind: device.device_kind(),
            version,
            reason: UnsupportedReason::Taproot,
        })
    } else if (network == Network::Bitcoin
        && info.jade_networks != jade::api::JadeNetworks::Main
        && info.jade_networks != jade::api::JadeNetworks::All)
        || (network != Network::Bitcoin && info.jade_networks == jade::api::JadeNetworks::Main)
    {
        Ok(HardwareWallet::Unsupported {
            id,
            kind: device.device_kind(),
            version,
            reason: UnsupportedReason::WrongNetwork,
        })
    } else {
        match info.jade_state {
            jade::api::JadeState::Locked
            | jade::api::JadeState::Temp
            | jade::api::JadeState::Uninit
            | jade::api::JadeState::Unsaved => Ok(HardwareWallet::Locked {
                id,
                kind: DeviceKind::Jade,
                pairing_code: None,
                device: Arc::new(Mutex::new(Some(LockedDevice::Jade(device)))),
            }),
            jade::api::JadeState::Ready => {
                let kind = device.device_kind();
                let version = device.get_version().await.ok();
                let fingerprint = match device.get_master_fingerprint().await {
                    Err(HWIError::NetworkMismatch) => {
                        return Ok(HardwareWallet::Unsupported {
                            id: id.clone(),
                            kind,
                            version,
                            reason: UnsupportedReason::WrongNetwork,
                        });
                    }
                    Err(e) => {
                        return Err(e);
                    }
                    Ok(fingerprint) => fingerprint,
                };
                let alias = keys_aliases.and_then(|aliases| aliases.get(&fingerprint).cloned());
                if let Some(wallet) = &wallet {
                    if wallet.descriptor_keys().contains(&fingerprint) {
                        let desc = wallet.main_descriptor.to_string();
                        let device = device.with_wallet(wallet.name.clone());
                        let registered = device.is_wallet_registered(&wallet.name, &desc).await?;
                        Ok(HardwareWallet::Supported {
                            id: id.clone(),
                            kind,
                            fingerprint,
                            device: Arc::new(device),
                            version,
                            registered: Some(registered),
                            alias,
                        })
                    } else {
                        Ok(HardwareWallet::Unsupported {
                            id: id.clone(),
                            kind,
                            version,
                            reason: UnsupportedReason::NotPartOfWallet(fingerprint),
                        })
                    }
                } else {
                    Ok(HardwareWallet::Supported {
                        id: id.clone(),
                        kind,
                        fingerprint,
                        device: Arc::new(device),
                        version,
                        registered: Some(false),
                        alias,
                    })
                }
            }
        }
    }
}

pub async fn handle_coldcard_device(
    state: &mut State,
    device_info: &DeviceInfo,
    api: &HidApi,
) -> bool {
    let id = format!(
        "coldcard-{:?}-{}-{}",
        device_info.path(),
        device_info.vendor_id(),
        device_info.product_id()
    );
    if state.connected_supported_hws.contains(&id) {
        state.still.push(id);
        return true;
    }
    if let Some(sn) = device_info.serial_number() {
        if let Ok((cc, _)) = coldcard::api::Coldcard::open(AsRefWrap { inner: api }, sn, None) {
            let device = if let Some(wallet) = &state.wallet {
                coldcard::Coldcard::from(cc).with_wallet_name(wallet.name.clone())
            } else {
                coldcard::Coldcard::from(cc)
            };
            let version = device.get_version().await.ok();
            match HardwareWallet::new(id, device.into(), Some(&state.keys_aliases)).await {
                Err(e) => tracing::error!("Failed to connect to coldcard: {}", e),
                Ok(hw) => {
                    let hw = if coldcard_version_supported(version.as_ref(), state.taproot) {
                        hw
                    } else {
                        HardwareWallet::Unsupported {
                            id: hw.id().clone(),
                            kind: *hw.kind(),
                            version,
                            reason: UnsupportedReason::Taproot,
                        }
                    };
                    state.hws.push(hw);
                    return true;
                }
            };
        }
    }
    false
}

struct AsRefWrap<'a, T> {
    inner: &'a T,
}

impl<'a, T> AsRef<T> for AsRefWrap<'a, T> {
    fn as_ref(&self) -> &T {
        self.inner
    }
}

pub fn ledger_id(info: &DeviceInfo) -> String {
    format!(
        "ledger-{:?}-{}-{}",
        info.path(),
        info.vendor_id(),
        info.product_id(),
    )
}

pub fn ledger_version_supported(version: Option<&Version>, taproot: bool) -> bool {
    if let Some(version) = version {
        if version.major >= 2 {
            return if version.major == 2 {
                if taproot {
                    version.minor >= 2
                } else {
                    version.minor >= 1
                }
            } else {
                true
            };
        }
    }
    false
}

fn coldcard_version_supported(version: Option<&Version>, taproot: bool) -> bool {
    if let Some(version) = version {
        if version.major >= 6 {
            return if version.major == 6 {
                if taproot {
                    version.minor >= 3
                } else {
                    version.minor >= 1
                }
            } else {
                true
            };
        }
    }
    false
}

pub fn is_compatible_with_tapminiscript(
    device_kind: &DeviceKind,
    version: Option<&Version>,
) -> bool {
    match device_kind {
        DeviceKind::BitBox02 => false,
        DeviceKind::Coldcard => coldcard_version_supported(version, true),
        DeviceKind::Specter => true,
        DeviceKind::SpecterSimulator => true,
        DeviceKind::Ledger => ledger_version_supported(version, true),
        DeviceKind::LedgerSimulator => ledger_version_supported(version, true),
        DeviceKind::Jade => false,
    }
}

pub fn ledger_need_taproot_upgrade(version: &Option<Version>) -> bool {
    ledger_version_supported(version.as_ref(), false)
        && !ledger_version_supported(version.as_ref(), true)
}
