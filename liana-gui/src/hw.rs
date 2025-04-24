use iced::Task;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::{
    app::{settings, wallet::Wallet},
    dir::LianaDirectory,
};
use async_hwi::{
    bitbox::{api::runtime, BitBox02, PairingBitbox02},
    coldcard,
    jade::{self, Jade},
    ledger, specter, DeviceKind, Error as HWIError, Version, HWI,
};
use iced::futures::{SinkExt, Stream};
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
    AppIsNotOpen,
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

    fn id(&self) -> &String {
        match self {
            Self::Locked { id, .. } => id,
            Self::Unsupported { id, .. } => id,
            Self::Supported { id, .. } => id,
        }
    }

    pub fn kind(&self) -> &DeviceKind {
        match self {
            Self::Locked { kind, .. } => kind,
            Self::Unsupported { kind, .. } => kind,
            Self::Supported { kind, .. } => kind,
        }
    }

    pub fn fingerprint(&self) -> Option<Fingerprint> {
        match self {
            Self::Locked { .. } => None,
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
}

#[derive(Debug, Clone)]
pub struct ConnectedList {
    pub new: Vec<HardwareWallet>,
    still: Vec<String>,
}

pub struct HardwareWallets {
    network: Network,
    pub list: Vec<HardwareWallet>,
    pub aliases: HashMap<Fingerprint, String>,
    wallet: Option<Arc<Wallet>>,
    datadir_path: LianaDirectory,
}

impl std::fmt::Debug for HardwareWallets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaitingConfirmBitBox").finish()
    }
}

impl HardwareWallets {
    pub fn new(datadir_path: LianaDirectory, network: Network) -> Self {
        Self {
            network,
            list: Vec::new(),
            aliases: HashMap::new(),
            wallet: None,
            datadir_path,
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

    pub fn reset_watch_list(&mut self) {
        self.list = Vec::new();
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
    ) -> Result<Task<HardwareWalletMessage>, async_hwi::Error> {
        match message {
            HardwareWalletMessage::Error(e) => Err(async_hwi::Error::Device(e)),
            HardwareWalletMessage::List(ConnectedList { still, mut new }) => {
                // remove disconnected
                self.list.retain(|hw| still.contains(hw.id()));
                self.list.append(&mut new);
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
                                    let network = self.network;
                                    let wallet = self.wallet.clone();
                                    cmds.push(Task::perform(
                                        async move {
                                            (
                                                id.clone(),
                                                unlock_bitbox(id, network, bb, wallet).await,
                                            )
                                        },
                                        |(id, res)| HardwareWalletMessage::Unlocked(id, res),
                                    ));
                                }
                                Some(LockedDevice::Jade(device)) => {
                                    let id = id.clone();
                                    let id_cloned = id.clone();
                                    let network = self.network;
                                    let wallet = self.wallet.clone();
                                    cmds.push(Task::perform(
                                        async move {
                                            if let Err(e) = device.auth().await {
                                                return (id_cloned, Err(e.into()));
                                            }
                                            let res = handle_jade_device(
                                                id,
                                                network,
                                                device,
                                                wallet.as_ref().map(|w| w.as_ref()),
                                                None,
                                            )
                                            .await;
                                            (id_cloned, res)
                                        },
                                        |(id_cloned, res)| {
                                            HardwareWalletMessage::Unlocked(id_cloned, res)
                                        },
                                    ));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if cmds.is_empty() {
                    Ok(Task::none())
                } else {
                    Ok(Task::batch(cmds))
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
                Ok(Task::none())
            }
        }
    }

    pub fn refresh(&self) -> iced::Subscription<HardwareWalletMessage> {
        iced::Subscription::run_with_id(
            format!("refresh-{}", self.network),
            refresh(State {
                network: self.network,
                keys_aliases: self.aliases.clone(),
                wallet: self.wallet.clone(),
                connected_supported_hws: Vec::new(),
                api: None,
                datadir_path: self.datadir_path.clone(),
            }),
        )
    }
}

async fn unlock_bitbox(
    id: String,
    network: Network,
    bb: Box<PairingBitbox02<runtime::TokioRuntime>>,
    wallet: Option<Arc<Wallet>>,
) -> Result<HardwareWallet, async_hwi::Error> {
    let paired_bb = bb.wait_confirm().await?;
    let mut bitbox2 = BitBox02::from(paired_bb).with_network(network);
    let fingerprint = bitbox2.get_master_fingerprint().await?;
    let mut registered = false;
    let version = bitbox2.get_version().await.ok();
    if let Some(wallet) = &wallet {
        let desc = wallet.main_descriptor.to_string();
        bitbox2 = bitbox2.with_policy(&desc)?;
        registered = bitbox2.is_policy_registered(&desc).await?;
        if wallet.descriptor_keys().contains(&fingerprint) {
            Ok(HardwareWallet::Supported {
                id: id.clone(),
                kind: DeviceKind::BitBox02,
                fingerprint,
                device: bitbox2.into(),
                version,
                registered: Some(registered),
                alias: None,
            })
        } else {
            Ok(HardwareWallet::Unsupported {
                id: id.clone(),
                kind: DeviceKind::BitBox02,
                version,
                reason: UnsupportedReason::NotPartOfWallet(fingerprint),
            })
        }
    } else {
        Ok(HardwareWallet::Supported {
            id: id.clone(),
            kind: DeviceKind::BitBox02,
            fingerprint,
            device: bitbox2.into(),
            version,
            registered: Some(registered),
            alias: None,
        })
    }
}

struct State {
    network: Network,
    keys_aliases: HashMap<Fingerprint, String>,
    wallet: Option<Arc<Wallet>>,
    connected_supported_hws: Vec<String>,
    api: Option<ledger::HidApi>,
    datadir_path: LianaDirectory,
}

fn refresh(mut state: State) -> impl Stream<Item = HardwareWalletMessage> {
    iced::stream::channel(100, move |mut output| async move {
        loop {
            let api = if let Some(api) = &mut state.api {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                if let Err(e) = api.refresh_devices() {
                    let _ = output
                        .send(HardwareWalletMessage::Error(e.to_string()))
                        .await;
                    continue;
                };
                api
            } else {
                match ledger::HidApi::new() {
                    Ok(api) => {
                        state.api = Some(api);
                        state.api.as_mut().unwrap()
                    }
                    Err(e) => {
                        let _ = output
                            .send(HardwareWalletMessage::Error(e.to_string()))
                            .await;
                        continue;
                    }
                }
            };

            let mut hws: Vec<HardwareWallet> = Vec::new();
            let mut still: Vec<String> = Vec::new();
            match specter::SpecterSimulator::try_connect().await {
                Ok(device) => {
                    let id = "specter-simulator".to_string();
                    if state.connected_supported_hws.contains(&id) {
                        still.push(id);
                    } else {
                        match HardwareWallet::new(id, Arc::new(device), Some(&state.keys_aliases))
                            .await
                        {
                            Ok(hw) => hws.push(hw),
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

            match specter::SerialTransport::enumerate_potential_ports() {
                Ok(ports) => {
                    for port in ports {
                        let id = format!("specter-{}", port);
                        if state.connected_supported_hws.contains(&id) {
                            still.push(id);
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
                                            Ok(hw) => hws.push(hw),
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

            match jade::SerialTransport::enumerate_potential_ports() {
                Ok(ports) => {
                    for port in ports {
                        let id = format!("jade-{}", port);
                        if state.connected_supported_hws.contains(&id) {
                            still.push(id);
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
                                    )
                                    .await
                                    {
                                        Ok(hw) => {
                                            hws.push(hw);
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

            match ledger::LedgerSimulator::try_connect().await {
                Ok(device) => {
                    let id = "ledger-simulator".to_string();
                    if state.connected_supported_hws.contains(&id) {
                        still.push(id);
                    } else {
                        match handle_ledger_device(
                            id,
                            device,
                            state.wallet.as_ref().map(|w| w.as_ref()),
                            &state.keys_aliases,
                        )
                        .await
                        {
                            Ok(hw) => {
                                hws.push(hw);
                            }
                            Err(e) => {
                                warn!("{:?}", e);
                            }
                        }
                    }
                }
                Err(HWIError::DeviceNotFound) => {}
                Err(e) => {
                    debug!("{}", e);
                }
            }

            for device_info in api.device_list() {
                if async_hwi::bitbox::is_bitbox02(device_info) {
                    let id = format!(
                        "bitbox-{:?}-{}-{}",
                        device_info.path(),
                        device_info.vendor_id(),
                        device_info.product_id()
                    );
                    if state.connected_supported_hws.contains(&id) {
                        still.push(id);
                        continue;
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
                            hws.push(HardwareWallet::Locked {
                                id,
                                kind: DeviceKind::BitBox02,
                                pairing_code: device.pairing_code().map(|s| s.replace('\n', " ")),
                                device: Arc::new(Mutex::new(Some(LockedDevice::BitBox02(
                                    Box::new(device),
                                )))),
                            });
                        }
                    }
                }
                if device_info.vendor_id() == coldcard::api::COINKITE_VID
                    && device_info.product_id() == coldcard::api::CKCC_PID
                {
                    let id = format!(
                        "coldcard-{:?}-{}-{}",
                        device_info.path(),
                        device_info.vendor_id(),
                        device_info.product_id()
                    );
                    if state.connected_supported_hws.contains(&id) {
                        still.push(id);
                        continue;
                    }
                    if let Some(sn) = device_info.serial_number() {
                        if let Ok((cc, _)) =
                            coldcard::api::Coldcard::open(AsRefWrap { inner: api }, sn, None)
                        {
                            let device: Arc<dyn HWI + Send + Sync> =
                                if let Some(wallet) = &state.wallet {
                                    coldcard::Coldcard::from(cc)
                                        .with_wallet_name(wallet.name.clone())
                                        .into()
                                } else {
                                    coldcard::Coldcard::from(cc).into()
                                };
                            match (
                                device.get_master_fingerprint().await,
                                device.get_version().await,
                            ) {
                                (Ok(fingerprint), Ok(version)) => {
                                    if version
                                        >= (Version {
                                            major: 6,
                                            minor: 2,
                                            patch: 1,
                                            prerelease: None,
                                        })
                                    {
                                        hws.push(HardwareWallet::Supported {
                                            id,
                                            device,
                                            kind: DeviceKind::Coldcard,
                                            fingerprint,
                                            version: Some(version),
                                            registered: None,
                                            alias: state.keys_aliases.get(&fingerprint).cloned(),
                                        });
                                    } else {
                                        hws.push(HardwareWallet::Unsupported {
                                            id,
                                            kind: device.device_kind(),
                                            version: Some(version),
                                            reason: UnsupportedReason::Version {
                                                minimal_supported_version: "Edge firmware v6.2.1",
                                            },
                                        });
                                    }
                                }
                                _ => tracing::error!("Failed to connect to coldcard"),
                            }
                        }
                    }
                }
            }
            for detected in ledger::Ledger::<ledger::TransportHID>::enumerate(api) {
                let id = format!(
                    "ledger-{:?}-{}-{}",
                    detected.path(),
                    detected.vendor_id(),
                    detected.product_id()
                );
                if state.connected_supported_hws.contains(&id) {
                    still.push(id);
                    continue;
                }

                match ledger::Ledger::<ledger::TransportHID>::connect(api, detected) {
                    Ok(device) => match handle_ledger_device(
                        id,
                        device,
                        state.wallet.as_ref().map(|w| w.as_ref()),
                        &state.keys_aliases,
                    )
                    .await
                    {
                        Ok(hw) => {
                            hws.push(hw);
                        }
                        Err(e) => {
                            warn!("{:?}", e);
                        }
                    },
                    Err(HWIError::DeviceNotFound) => {}
                    Err(e) => {
                        debug!("{}", e);
                    }
                }
            }

            if let Some(wallet) = &state.wallet {
                let wallet_keys = wallet.descriptor_keys();
                for hw in &mut hws {
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

            state.connected_supported_hws = still
                .iter()
                .chain(hws.iter().filter_map(|hw| match hw {
                    HardwareWallet::Locked { id, .. } => Some(id),
                    HardwareWallet::Supported { id, .. } => Some(id),
                    HardwareWallet::Unsupported { .. } => None,
                }))
                .cloned()
                .collect();
            let _ = output
                .send(HardwareWalletMessage::List(ConnectedList {
                    new: hws,
                    still,
                }))
                .await;
        }
    })
}

async fn handle_ledger_device<'a, T: async_hwi::ledger::Transport + Sync + Send + 'static>(
    id: String,
    mut device: ledger::Ledger<T>,
    wallet: Option<&'a Wallet>,
    keys_aliases: &'a HashMap<Fingerprint, String>,
) -> Result<HardwareWallet, HWIError> {
    match (
        device.get_master_fingerprint().await,
        device.get_version().await,
    ) {
        (Ok(fingerprint), Ok(version)) => {
            if ledger_version_supported(&version) {
                let mut registered = false;
                if let Some(w) = &wallet {
                    if let Some(cfg) = w
                        .hardware_wallets
                        .iter()
                        .find(|cfg| cfg.fingerprint == fingerprint)
                    {
                        device = device
                            .with_wallet(&w.name, &w.main_descriptor.to_string(), Some(cfg.token()))
                            .expect("Configuration must be correct");
                        registered = true;
                    }
                }
                Ok(HardwareWallet::Supported {
                    id,
                    kind: device.device_kind(),
                    fingerprint,
                    device: Arc::new(device),
                    version: Some(version),
                    registered: Some(registered),
                    alias: keys_aliases.get(&fingerprint).cloned(),
                })
            } else {
                Ok(HardwareWallet::Unsupported {
                    id,
                    kind: device.device_kind(),
                    version: Some(version),
                    reason: UnsupportedReason::Version {
                        minimal_supported_version: "2.1.0",
                    },
                })
            }
        }
        (_, _) => Ok(HardwareWallet::Unsupported {
            id,
            kind: device.device_kind(),
            version: None,
            reason: UnsupportedReason::AppIsNotOpen,
        }),
    }
}

async fn handle_jade_device(
    id: String,
    network: Network,
    device: Jade<async_hwi::jade::SerialTransport>,
    wallet: Option<&Wallet>,
    keys_aliases: Option<&HashMap<Fingerprint, String>>,
) -> Result<HardwareWallet, HWIError> {
    let info = device.get_info().await?;
    let version = async_hwi::parse_version(&info.jade_version).ok();
    // Jade may not be setup for the current network
    if (network == Network::Bitcoin
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

struct AsRefWrap<'a, T> {
    inner: &'a T,
}

impl<'a, T> AsRef<T> for AsRefWrap<'a, T> {
    fn as_ref(&self) -> &T {
        self.inner
    }
}

fn ledger_version_supported(version: &Version) -> bool {
    if version.major >= 2 {
        if version.major == 2 {
            version.minor >= 1
        } else {
            true
        }
    } else {
        false
    }
}

// Kind and minimal version of devices supporting tapminiscript.
// We cannot use a lazy_static HashMap yet, because DeviceKind does not implement Hash.
const DEVICES_COMPATIBLE_WITH_TAPMINISCRIPT: [(DeviceKind, Option<Version>); 5] = [
    (
        DeviceKind::Ledger,
        Some(Version {
            major: 2,
            minor: 2,
            patch: 0,
            prerelease: None,
        }),
    ),
    (DeviceKind::Specter, None),
    (DeviceKind::SpecterSimulator, None),
    (
        DeviceKind::Coldcard,
        Some(Version {
            major: 6,
            minor: 3,
            patch: 3,
            prerelease: None,
        }),
    ),
    (
        DeviceKind::BitBox02,
        Some(Version {
            major: 9,
            minor: 21,
            patch: 0,
            prerelease: None,
        }),
    ),
];

pub fn is_compatible_with_tapminiscript(
    device_kind: &DeviceKind,
    version: Option<&Version>,
) -> bool {
    DEVICES_COMPATIBLE_WITH_TAPMINISCRIPT
        .iter()
        .any(|(kind, minimal_version)| {
            device_kind == kind
                && match (version, minimal_version) {
                    (Some(v1), Some(v2)) => v1 >= v2,
                    (None, Some(_)) => false,
                    (Some(_), None) => true,
                    (None, None) => true,
                }
        })
}
