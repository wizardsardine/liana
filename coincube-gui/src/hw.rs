use iced::Task;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::{
    app::{settings, wallet::Wallet},
    dir::CoincubeDirectory,
};
use async_hwi::{
    bitbox::{api::runtime, BitBox02, PairingBitbox02},
    coldcard,
    jade::{self, Jade},
    ledger, specter, DeviceKind, Error as HWIError, Version, HWI,
};
use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, hashes::hex::FromHex, Network};
use iced::futures::{SinkExt, Stream};
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
    datadir_path: CoincubeDirectory,
}

impl std::fmt::Debug for HardwareWallets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaitingConfirmBitBox").finish()
    }
}

impl HardwareWallets {
    pub fn new(datadir_path: CoincubeDirectory, network: Network) -> Self {
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
        let state = RefreshState {
            network: self.network,
            keys_aliases: self.aliases.clone(),
            wallet: self.wallet.clone(),
            datadir_path: self.datadir_path.clone(),
        };
        iced::Subscription::run_with(state, make_refresh_stream)
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

/// State for hardware wallet refresh subscription.
/// Implements Hash based only on network for subscription identity.
struct RefreshState {
    network: Network,
    keys_aliases: HashMap<Fingerprint, String>,
    wallet: Option<Arc<Wallet>>,
    datadir_path: CoincubeDirectory,
}

impl std::hash::Hash for RefreshState {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Only hash network for subscription identity
        self.network.hash(state);
    }
}

struct State {
    network: Network,
    keys_aliases: HashMap<Fingerprint, String>,
    wallet: Option<Arc<Wallet>>,
    connected_supported_hws: Vec<String>,
    api: Option<ledger::HidApi>,
    datadir_path: CoincubeDirectory,
    /// Per-phone retry cooldowns keyed by `fp8`. Set on a failed
    /// dial so the next refresh tick skips the phone (and pays no
    /// `CONNECT_TIMEOUT`) until the window has elapsed. Cleared on
    /// a subsequent successful dial.
    phone_cooldowns: HashMap<String, PhoneCooldown>,
}

/// Function pointer for Subscription::run_with - creates the refresh stream from RefreshState
fn make_refresh_stream(rs: &RefreshState) -> impl Stream<Item = HardwareWalletMessage> {
    let state = State {
        network: rs.network,
        keys_aliases: rs.keys_aliases.clone(),
        wallet: rs.wallet.clone(),
        connected_supported_hws: Vec::new(),
        api: None,
        datadir_path: rs.datadir_path.clone(),
        phone_cooldowns: HashMap::new(),
    };
    refresh(state)
}

fn refresh(mut state: State) -> impl Stream<Item = HardwareWalletMessage> {
    iced::stream::channel(100, async move |mut output| loop {
        let api = if let Some(api) = &mut state.api {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if let Err(e) = api.refresh_devices() {
                let _ = output
                    .send(HardwareWalletMessage::Error(e.to_string()))
                    .await;
                continue;
            }
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
                    match HardwareWallet::new(id, Arc::new(device), Some(&state.keys_aliases)).await
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
                            device: Arc::new(Mutex::new(Some(LockedDevice::BitBox02(Box::new(
                                device,
                            ))))),
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
                        let device: Arc<dyn HWI + Send + Sync> = if let Some(wallet) = &state.wallet
                        {
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

        // LAN-paired phones via the local-signer protocol. We browse
        // mDNS for `_coincube-signer._tcp.local.`, match the TXT
        // `fp=` against persisted `pairing_store` entries, and dial
        // each match over TLS. On a successful pinned-cert handshake
        // we surface a `PhoneSigner` as `Supported`; phones that are
        // paired but currently unreachable (mDNS silent or dial
        // failed) surface as `Unsupported(AppIsNotOpen)` so the user
        // sees "phone offline" rather than silence.
        match crate::phone_signer::pairing_store::load(&state.datadir_path) {
            Ok(store) if !store.phones.is_empty() => {
                let discovered = crate::phone_signer::mdns::browse();
                let identity =
                    match crate::phone_signer::identity::load_or_create(&state.datadir_path) {
                        Ok(id) => Some(id),
                        Err(e) => {
                            warn!("local-signer identity unavailable: {}", e);
                            None
                        }
                    };
                if let Some(identity) = identity {
                    // Sharing the identity across N concurrent dials —
                    // `PairedTransport::connect` only borrows for the
                    // duration of the rustls config build, so an Arc
                    // clone per future is cheap.
                    let identity = Arc::new(identity);
                    let now = std::time::Instant::now();
                    let mut dials = Vec::new();
                    for paired in &store.phones {
                        let fp8 = crate::phone_signer::identity::pin_hex8(&paired.cert_pin);
                        let id = format!("phone-{}", fp8);
                        // Resolve a target address: prefer the
                        // mDNS-discovered one; fall back to the
                        // user-entered `fallback_addr` when mDNS is
                        // blocked or the phone isn't broadcasting.
                        let target = resolve_phone_target(&fp8, paired, &discovered);
                        // "Already connected" short-circuit. Unlike
                        // USB devices — which the HID-API path
                        // re-enumerates each tick — the
                        // paired-phone store is persistent, so an
                        // entry in `connected_supported_hws` alone
                        // isn't proof the phone is still up. Re-gate
                        // the short-circuit on current
                        // discoverability so a phone whose Wi-Fi
                        // dropped gets downgraded to
                        // `Unsupported(AppIsNotOpen)` on the next
                        // tick instead of staying mislabelled as
                        // Supported forever.
                        if state.connected_supported_hws.contains(&id) && target.is_some() {
                            still.push(id);
                            continue;
                        }
                        let Some(target) = target else {
                            // Paired but not visible. Surface offline
                            // without touching the cooldown — no dial
                            // attempt happened.
                            hws.push(HardwareWallet::Unsupported {
                                id,
                                kind: DeviceKind::Specter,
                                version: None,
                                reason: UnsupportedReason::AppIsNotOpen,
                            });
                            continue;
                        };
                        // Cooldown gate: a phone that just failed
                        // shouldn't pay another full `CONNECT_TIMEOUT`
                        // this tick. Surface as offline; the window
                        // expires on a later tick.
                        if state
                            .phone_cooldowns
                            .get(&fp8)
                            .is_some_and(|cd| cd.is_cooling(now))
                        {
                            hws.push(HardwareWallet::Unsupported {
                                id,
                                kind: DeviceKind::Specter,
                                version: None,
                                reason: UnsupportedReason::AppIsNotOpen,
                            });
                            continue;
                        }
                        let identity = identity.clone();
                        let phone_pin = paired.cert_pin;
                        let paired = paired.clone();
                        dials.push(async move {
                            let res = crate::phone_signer::transport::PairedTransport::connect(
                                target, &identity, phone_pin,
                            )
                            .await;
                            (fp8, id, paired, res)
                        });
                    }
                    // Run the dials concurrently so one slow / offline
                    // phone can't stall the whole refresh tick.
                    let results = iced::futures::future::join_all(dials).await;
                    for (fp8, id, paired, res) in results {
                        match res {
                            Ok(t) => {
                                state.phone_cooldowns.remove(&fp8);
                                let fingerprint = paired
                                    .wallet_fingerprints
                                    .first()
                                    .copied()
                                    .unwrap_or_default();
                                let signer = crate::phone_signer::PhoneSigner::new(
                                    t,
                                    fingerprint,
                                    None,
                                    paired.clone(),
                                );
                                let device: Arc<dyn HWI + Send + Sync> = Arc::new(signer);
                                hws.push(HardwareWallet::Supported {
                                    id,
                                    device,
                                    kind: DeviceKind::Specter,
                                    fingerprint,
                                    version: None,
                                    registered: Some(true),
                                    alias: Some(paired.name.clone()),
                                });
                            }
                            Err(e) => {
                                debug!("phone {} dial failed: {}", paired.name, e);
                                state
                                    .phone_cooldowns
                                    .entry(fp8)
                                    .or_default()
                                    .record_failure(std::time::Instant::now());
                                hws.push(HardwareWallet::Unsupported {
                                    id,
                                    kind: DeviceKind::Specter,
                                    version: None,
                                    reason: UnsupportedReason::AppIsNotOpen,
                                });
                            }
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(e) => debug!("local-signer pairing store: {}", e),
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
                    // Conservative: even when the table entry has
                    // no minimum-version requirement (Specter /
                    // SpecterSimulator), require the device to
                    // actually report a version. The PhoneSigner is
                    // surfaced with `kind: Specter, version: None`
                    // because async_hwi 0.0.29 has no `Phone`
                    // variant — claiming it's tap-script-capable
                    // because the table says "Specter, any
                    // version" would let the descriptor editor
                    // include a phone-signed key in a taproot
                    // multisig, where the phone may not actually
                    // support BIP-371 tap-script signing.
                    //
                    // Real Specter devices populate `version` via
                    // `device.get_version().await` in
                    // `HardwareWallet::new`, so they still match.
                    // The only regression is "real Specter whose
                    // `get_version` happened to fail" — which is a
                    // degraded state where being conservative is
                    // the right default.
                    (None, None) => false,
                }
        })
}

/// Initial delay between consecutive failed dials of the same phone.
/// The window doubles on each subsequent failure, up to
/// [`PHONE_RETRY_MAX`]. Cleared on successful connect.
pub(crate) const PHONE_RETRY_INITIAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Upper bound on the per-phone retry window so a long-offline
/// phone doesn't stay invisible for hours after coming back.
pub(crate) const PHONE_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(60);

/// Per-phone retry/backoff state. Held in `State::phone_cooldowns`
/// keyed by `fp8` so the refresh loop can skip phones whose last
/// dial attempt failed recently without paying the full
/// `CONNECT_TIMEOUT` (750ms) every tick. Cleared on the next
/// successful connect.
#[derive(Debug, Clone)]
pub(crate) struct PhoneCooldown {
    /// Instant before which we won't attempt another dial.
    pub next_retry_at: std::time::Instant,
    /// Most recently applied backoff window. The next failure
    /// doubles it (capped at [`PHONE_RETRY_MAX`]); empty on a fresh
    /// `Default` so the first call to `record_failure` settles on
    /// [`PHONE_RETRY_INITIAL`].
    pub current_window: std::time::Duration,
}

impl Default for PhoneCooldown {
    fn default() -> Self {
        Self {
            next_retry_at: std::time::Instant::now(),
            current_window: std::time::Duration::ZERO,
        }
    }
}

impl PhoneCooldown {
    /// Caller skips the dial when this is `true`.
    pub fn is_cooling(&self, now: std::time::Instant) -> bool {
        now < self.next_retry_at
    }

    /// Push `next_retry_at` out by a doubled (capped) window. First
    /// call settles on [`PHONE_RETRY_INITIAL`].
    pub fn record_failure(&mut self, now: std::time::Instant) {
        self.current_window = if self.current_window.is_zero() {
            PHONE_RETRY_INITIAL
        } else {
            std::cmp::min(self.current_window.saturating_mul(2), PHONE_RETRY_MAX)
        };
        self.next_retry_at = now + self.current_window;
    }
}

/// Resolve a paired phone's reachable address. Prefers the
/// mDNS-discovered endpoint (TXT `fp` matches the phone's cert pin
/// fingerprint); falls back to the user-entered `host:port` for
/// networks that block mDNS.
///
/// `None` means "paired but unreachable" — the caller surfaces that
/// as `HardwareWallet::Unsupported(AppIsNotOpen)`.
///
/// Pulled out of the discovery loop so the resolution decision is
/// pure and unit-testable.
pub(crate) fn resolve_phone_target(
    fp8: &str,
    paired: &crate::phone_signer::pairing_store::PairedPhone,
    discovered: &[crate::phone_signer::mdns::DiscoveredPhone],
) -> Option<std::net::SocketAddr> {
    let mdns_addr = discovered
        .iter()
        .find(|d| d.cert_fp8 == fp8)
        .map(|d| d.addr);
    let fallback_addr = paired
        .fallback_addr
        .as_deref()
        .and_then(|s| s.parse::<std::net::SocketAddr>().ok());
    mdns_addr.or(fallback_addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phone_signer::mdns::DiscoveredPhone;
    use crate::phone_signer::pairing_store::PairedPhone;
    use std::net::SocketAddr;

    fn phone_with_fallback(pin: [u8; 32], fallback: Option<&str>) -> PairedPhone {
        PairedPhone {
            cert_pin: pin,
            name: "Test".into(),
            paired_at_unix: 0,
            wallet_fingerprints: Vec::new(),
            fallback_addr: fallback.map(|s| s.to_string()),
        }
    }

    fn discovered(fp8: &str, addr: &str) -> DiscoveredPhone {
        DiscoveredPhone {
            cert_fp8: fp8.into(),
            addr: addr.parse::<SocketAddr>().expect("parse addr"),
            instance_name: "test".into(),
        }
    }

    #[test]
    fn resolve_prefers_mdns_when_both_available() {
        let phone = phone_with_fallback([1u8; 32], Some("10.0.0.5:8443"));
        let discoveries = vec![discovered("01010101", "192.168.1.20:7777")];
        let target = resolve_phone_target("01010101", &phone, &discoveries);
        assert_eq!(target.unwrap().to_string(), "192.168.1.20:7777");
    }

    #[test]
    fn resolve_falls_back_when_no_mdns_match() {
        let phone = phone_with_fallback([1u8; 32], Some("10.0.0.5:8443"));
        // The discovery is for a different phone.
        let discoveries = vec![discovered("ffffffff", "192.168.1.20:7777")];
        let target = resolve_phone_target("01010101", &phone, &discoveries);
        assert_eq!(target.unwrap().to_string(), "10.0.0.5:8443");
    }

    #[test]
    fn resolve_returns_none_when_neither_mdns_nor_fallback() {
        let phone = phone_with_fallback([1u8; 32], None);
        let target = resolve_phone_target("01010101", &phone, &[]);
        assert!(target.is_none(), "expected None, got {:?}", target);
    }

    #[test]
    fn resolve_returns_none_when_fallback_is_malformed() {
        let phone = phone_with_fallback([1u8; 32], Some("not-an-addr"));
        let target = resolve_phone_target("01010101", &phone, &[]);
        assert!(
            target.is_none(),
            "malformed fallback should not crash; got {:?}",
            target
        );
    }

    #[test]
    fn resolve_uses_only_mdns_when_no_fallback() {
        let phone = phone_with_fallback([1u8; 32], None);
        let discoveries = vec![discovered("01010101", "192.168.1.20:7777")];
        let target = resolve_phone_target("01010101", &phone, &discoveries);
        assert_eq!(target.unwrap().to_string(), "192.168.1.20:7777");
    }

    #[test]
    fn cooldown_default_is_not_cooling() {
        // A fresh default cooldown has `next_retry_at == created_at`,
        // so a clock that's advanced at all is past the deadline.
        let cd = PhoneCooldown::default();
        let later = cd.next_retry_at + std::time::Duration::from_millis(1);
        assert!(!cd.is_cooling(later));
    }

    #[test]
    fn cooldown_first_failure_uses_initial_window() {
        let mut cd = PhoneCooldown::default();
        let t0 = std::time::Instant::now();
        cd.record_failure(t0);
        assert_eq!(cd.current_window, PHONE_RETRY_INITIAL);
        assert_eq!(cd.next_retry_at, t0 + PHONE_RETRY_INITIAL);
        assert!(cd.is_cooling(t0));
        assert!(cd.is_cooling(t0 + PHONE_RETRY_INITIAL - std::time::Duration::from_millis(1)));
        assert!(!cd.is_cooling(t0 + PHONE_RETRY_INITIAL));
    }

    #[test]
    fn tapminiscript_compat_rejects_specter_without_version() {
        // The bug this guards against: PhoneSigner is surfaced as
        // (DeviceKind::Specter, version: None) because async_hwi has
        // no Phone variant. Without the (None, None) => false arm,
        // the compat check would falsely report taproot capability
        // for paired phones.
        assert!(!is_compatible_with_tapminiscript(
            &DeviceKind::Specter,
            None
        ));
        assert!(!is_compatible_with_tapminiscript(
            &DeviceKind::SpecterSimulator,
            None,
        ));
    }

    #[test]
    fn tapminiscript_compat_accepts_specter_with_any_reported_version() {
        // Real Specter devices populate `version` via
        // `device.get_version().await`. Any reported version
        // satisfies the "no minimum required" table entry.
        let v = async_hwi::Version {
            major: 1,
            minor: 0,
            patch: 0,
            prerelease: None,
        };
        assert!(is_compatible_with_tapminiscript(
            &DeviceKind::Specter,
            Some(&v),
        ));
        assert!(is_compatible_with_tapminiscript(
            &DeviceKind::SpecterSimulator,
            Some(&v),
        ));
    }

    #[test]
    fn tapminiscript_compat_still_enforces_minimum_versions() {
        // Sanity: per-device minimums weren't disturbed by the
        // (None, None) tightening.
        let too_old = async_hwi::Version {
            major: 2,
            minor: 0,
            patch: 0,
            prerelease: None,
        };
        let new_enough = async_hwi::Version {
            major: 2,
            minor: 2,
            patch: 0,
            prerelease: None,
        };
        assert!(!is_compatible_with_tapminiscript(
            &DeviceKind::Ledger,
            Some(&too_old),
        ));
        assert!(is_compatible_with_tapminiscript(
            &DeviceKind::Ledger,
            Some(&new_enough),
        ));
        // Devices outside the table are never compatible regardless
        // of version reporting.
        assert!(!is_compatible_with_tapminiscript(&DeviceKind::Jade, None));
        assert!(!is_compatible_with_tapminiscript(
            &DeviceKind::Jade,
            Some(&new_enough),
        ));
    }

    #[test]
    fn cooldown_subsequent_failures_double_until_cap() {
        let mut cd = PhoneCooldown::default();
        let t0 = std::time::Instant::now();
        cd.record_failure(t0);
        let mut prev = cd.current_window;
        for step in 0..6 {
            cd.record_failure(t0 + std::time::Duration::from_secs(step * 1000));
            let expected_double = prev.saturating_mul(2);
            let expected = std::cmp::min(expected_double, PHONE_RETRY_MAX);
            assert_eq!(
                cd.current_window, expected,
                "step {} expected {:?} got {:?}",
                step, expected, cd.current_window
            );
            prev = cd.current_window;
        }
        // After enough doublings we sit at the cap.
        assert_eq!(cd.current_window, PHONE_RETRY_MAX);
    }
}
