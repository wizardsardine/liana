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
        /// Optional extra guidance shown on its own line, for cases where the
        /// bare version isn't self-explanatory (e.g. Coldcard's Edge branch).
        /// Kept separate from `minimal_supported_version` so the latter stays
        /// version-like for the "Install version {v} or later" templates.
        note: Option<&'static str>,
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

    pub(crate) fn id(&self) -> &String {
        match self {
            Self::Locked { id, .. } => id,
            Self::Unsupported { id, .. } => id,
            Self::Supported { id, .. } => id,
        }
    }

    /// True for a Keychain phone paired over the local network. It joins this
    /// list so it signs through the same path as a USB device, but it is not a
    /// hardware wallet and must not be presented as one.
    pub fn is_lan_phone(&self) -> bool {
        self.id().starts_with(LAN_PHONE_ID_PREFIX)
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

/// A Split device policy bound into the device listing (#568 B4b-2). A
/// named (multisig) policy goes into each Coldcard handle as its wallet name
/// and into each BitBox02 handle as its policy when the device is opened, so
/// the listed handle is the one that signs it. An unnamed (singlesig) policy
/// binds nothing: those devices sign singlesig on a plain handle, and a
/// forced singlesig policy is not what BitBox02 expects. Ledger is bound by
/// the Split binder instead (`split_hardware::bind`). Nothing is persisted.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SplitPolicyBinding {
    name: String,
    descriptor: String,
}

impl SplitPolicyBinding {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn descriptor(&self) -> &str {
        &self.descriptor
    }

    /// The name and descriptor bound into Coldcard and BitBox02 handles:
    /// `None` for an unnamed (singlesig) policy.
    pub fn bound(&self) -> Option<(&str, &str)> {
        (!self.name.is_empty()).then_some((self.name.as_str(), self.descriptor.as_str()))
    }
}

/// The device policy binding for an imported wallet: `None` for a Vault, or
/// when the imported descriptor has no device policy (its signing session
/// then refuses with the reason).
fn imported_wallet_binding(wallet: &Wallet) -> Option<SplitPolicyBinding> {
    let imported = wallet.main_descriptor.as_imported()?;
    let policy = crate::split_hardware::policy::imported_policy(imported).ok()?;
    Some(SplitPolicyBinding {
        name: policy.name().to_owned(),
        descriptor: policy.descriptor().to_owned(),
    })
}

/// The registration a listed device reports. An imported wallet's signing
/// session (`split_hardware`) checks its device policy and registers it when
/// needed, so the listing doesn't know and must not report `Some(false)`,
/// which the signer picker treats as blocked. Jade reports the same.
fn listed_registration(wallet: Option<&Wallet>, registered: bool) -> Option<bool> {
    if wallet.is_some_and(|wallet| wallet.main_descriptor.as_imported().is_some()) {
        None
    } else {
        Some(registered)
    }
}

/// No descriptor (xpubs) in logs or message traces.
impl std::fmt::Debug for SplitPolicyBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplitPolicyBinding")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

pub struct HardwareWallets {
    network: Network,
    pub list: Vec<HardwareWallet>,
    pub aliases: HashMap<Fingerprint, String>,
    wallet: Option<Arc<Wallet>>,
    datadir_path: CoincubeDirectory,
    /// False for session-only consumers (the Split tool): BitBox02 pairing
    /// then stays in memory and nothing is written under `datadir_path`.
    persist_pairing: bool,
    /// The Split policy bound into Coldcard and BitBox02 handles at listing
    /// (#568 B4b-2), or a loaded imported wallet's device policy; never used
    /// for a loaded Vault, which binds its own descriptor.
    split_policy: Option<SplitPolicyBinding>,
    /// Devices whose unlock failed, shared with the refresh stream. The
    /// stream counts such a device as still connected and would never list
    /// it again, so once its backoff ends the stream forgets it and pairs it
    /// afresh, without restarting the scan for every other device.
    unlock_retry: UnlockRetry,
}

/// When each device that failed to unlock may be picked up again (`None`
/// once released, so it is released once per failure). Failures in a row
/// back off from 2s to 60s; a successful unlock clears the count.
#[derive(Clone, Default)]
pub(crate) struct UnlockRetry(Arc<Mutex<HashMap<String, Retry>>>);

/// When the device may be picked up again, and its failures in a row.
type Retry = (Option<std::time::Instant>, u32);

impl UnlockRetry {
    const FIRST: std::time::Duration = std::time::Duration::from_secs(2);
    const MAX: std::time::Duration = std::time::Duration::from_secs(60);

    fn failed(&self, id: &str, now: std::time::Instant) {
        let mut retries = self.0.lock().unwrap();
        let failures = retries.get(id).map_or(1, |(_, failures)| failures + 1);
        let delay = Self::FIRST
            .saturating_mul(1 << (failures - 1).min(5))
            .min(Self::MAX);
        retries.insert(id.to_owned(), (Some(now + delay), failures));
    }

    fn unlocked(&self, id: &str) {
        self.0.lock().unwrap().remove(id);
    }

    /// Drop the devices whose backoff has ended from `connected`, once, so
    /// the scan finds them as new. Their failure count is kept for the next
    /// backoff.
    fn release_due(&self, connected: &mut Vec<String>, now: std::time::Instant) {
        let mut retries = self.0.lock().unwrap();
        connected.retain(|id| match retries.get_mut(id) {
            Some((retry_at, _)) if retry_at.is_some_and(|at| at <= now) => {
                *retry_at = None;
                false
            }
            _ => true,
        });
    }
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
            persist_pairing: true,
            split_policy: None,
            unlock_retry: UnlockRetry::default(),
        }
    }

    /// Session-only device listing: no BitBox02 pairing persistence. Combined
    /// with no loaded wallet (so no LAN phone discovery or identity creation),
    /// the refresh loop never writes to the datadir.
    pub fn ephemeral(mut self) -> Self {
        self.persist_pairing = false;
        self
    }

    /// Session-only listing for signing a Split policy (#568 B4b-2): Coldcard
    /// and BitBox02 handles are bound to a named (multisig) policy when the
    /// device is opened, and no BitBox02 pairing is written, since a Split
    /// listing is always ephemeral. With a loaded wallet, the wallet's own
    /// binding wins.
    pub fn with_split_policy(mut self, name: String, descriptor: String) -> Self {
        self.persist_pairing = false;
        self.split_policy = Some(SplitPolicyBinding { name, descriptor });
        self
    }

    /// Session-only listing bound to a public wallet policy. Imported
    /// wallets use the same device transport without acquiring a Split name.
    pub fn with_public_policy(self, name: String, descriptor: String) -> Self {
        self.with_split_policy(name, descriptor)
    }

    pub fn split_policy_binding(&self) -> Option<&SplitPolicyBinding> {
        self.split_policy.as_ref()
    }

    #[cfg(test)]
    pub fn persists_pairing(&self) -> bool {
        self.persist_pairing
    }

    /// Loads the wallet whose devices this listing shows. An imported wallet
    /// has no Vault descriptor to bind, so its device policy (the one its
    /// signing session registers) is bound into Coldcard and BitBox02
    /// handles instead, as for a Split listing. Pairing still persists.
    pub fn with_wallet(mut self, wallet: Arc<Wallet>) -> Self {
        self.aliases.clone_from(&wallet.keys_aliases);
        self.split_policy = imported_wallet_binding(&wallet).or(self.split_policy);
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
            HardwareWalletMessage::List(list) => Ok(self.update_connected_list(list)),
            HardwareWalletMessage::Unlocked(id, res) => {
                self.apply_unlock_result(id, res);
                Ok(Task::none())
            }
        }
    }

    fn update_connected_list(
        &mut self,
        ConnectedList { still, mut new }: ConnectedList,
    ) -> Task<HardwareWalletMessage> {
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
                HardwareWallet::Locked { device, id, .. } => match device.lock().unwrap().take() {
                    None => {}
                    Some(LockedDevice::BitBox02(bb)) => {
                        let id = id.to_string();
                        let network = self.network;
                        let wallet = self.wallet.clone();
                        let split_policy = self.split_policy.clone();
                        cmds.push(Task::perform(
                            async move {
                                (
                                    id.clone(),
                                    unlock_bitbox(id, network, bb, wallet, split_policy).await,
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
                            |(id_cloned, res)| HardwareWalletMessage::Unlocked(id_cloned, res),
                        ));
                    }
                },
                _ => {}
            }
        }
        if cmds.is_empty() {
            Task::none()
        } else {
            Task::batch(cmds)
        }
    }

    fn apply_unlock_result(&mut self, id: String, res: Result<HardwareWallet, async_hwi::Error>) {
        match res {
            Err(e) => {
                warn!("Pairing failed with an external device {}", e);
                self.list.retain(|hw| hw.id() != &id);
                // E.g. a BitBox02 found while locked: once unlocked,
                // the scan pairs it again after a backoff.
                self.unlock_retry.failed(&id, std::time::Instant::now());
            }
            Ok(hw) => {
                self.unlock_retry.unlocked(hw.id());
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
    }

    pub fn refresh(&self) -> iced::Subscription<HardwareWalletMessage> {
        iced::Subscription::run_with(self.refresh_state(), make_refresh_stream)
    }

    fn refresh_state(&self) -> RefreshState {
        RefreshState {
            network: self.network,
            keys_aliases: self.aliases.clone(),
            wallet: self.wallet.clone(),
            datadir_path: self.datadir_path.clone(),
            persist_pairing: self.persist_pairing,
            split_policy: self.split_policy.clone(),
            unlock_retry: self.unlock_retry.clone(),
        }
    }
}

async fn unlock_bitbox(
    id: String,
    network: Network,
    bb: Box<PairingBitbox02<runtime::TokioRuntime>>,
    wallet: Option<Arc<Wallet>>,
    split_policy: Option<SplitPolicyBinding>,
) -> Result<HardwareWallet, async_hwi::Error> {
    let paired_bb = bb.wait_confirm().await?;
    let mut bitbox2 = BitBox02::from(paired_bb).with_network(network);
    let fingerprint = bitbox2.get_master_fingerprint().await?;
    let mut registered = false;
    let version = bitbox2.get_version().await.ok();
    if let Some((wallet, descriptor)) = wallet.as_ref().and_then(|wallet| {
        wallet
            .inheritance_descriptor()
            .map(|descriptor| (wallet, descriptor))
    }) {
        let desc = descriptor.to_string();
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
        // Split (#568 B4b-2): a named policy is bound into the handle so the
        // listed handle signs it; whether it is registered is the signing
        // session's question.
        if let Some((_, policy)) = split_policy.as_ref().and_then(SplitPolicyBinding::bound) {
            bitbox2 = bitbox2.with_policy(policy)?;
        }
        Ok(HardwareWallet::Supported {
            id: id.clone(),
            kind: DeviceKind::BitBox02,
            fingerprint,
            device: bitbox2.into(),
            version,
            registered: listed_registration(wallet.as_deref(), registered),
            alias: None,
        })
    }
}

/// State for hardware wallet refresh subscription.
/// Subscription identity includes the exact wallet and storage context.
struct RefreshState {
    network: Network,
    keys_aliases: HashMap<Fingerprint, String>,
    wallet: Option<Arc<Wallet>>,
    datadir_path: CoincubeDirectory,
    persist_pairing: bool,
    split_policy: Option<SplitPolicyBinding>,
    /// Shared with the listing; not part of the stream's identity.
    unlock_retry: UnlockRetry,
}

impl std::hash::Hash for RefreshState {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.network.hash(state);
        self.wallet
            .as_ref()
            .map(|wallet| (wallet.chain, wallet.id_fingerprint()))
            .hash(state);
        self.datadir_path.path().hash(state);
        self.persist_pairing.hash(state);
        self.split_policy.hash(state);
    }
}

struct State {
    network: Network,
    keys_aliases: HashMap<Fingerprint, String>,
    wallet: Option<Arc<Wallet>>,
    connected_supported_hws: Vec<String>,
    api: Option<ledger::HidApi>,
    datadir_path: CoincubeDirectory,
    persist_pairing: bool,
    split_policy: Option<SplitPolicyBinding>,
    unlock_retry: UnlockRetry,
    /// Per-phone retry cooldowns keyed by `fp8`. Set on a failed
    /// dial so the next refresh tick skips the phone (and pays no
    /// `CONNECT_TIMEOUT`) until the window has elapsed. Cleared on
    /// a subsequent successful dial.
    phone_cooldowns: HashMap<String, PhoneCooldown>,
    /// Live `PhoneSigner` handles keyed by the same `phone-{fp8}` id
    /// surfaced to the consumer. Lets the refresh tick probe a
    /// previously-issued signer with
    /// [`crate::phone_signer::PhoneSigner::is_alive`] before
    /// short-circuiting a re-dial — without this, a dead TLS session
    /// whose mDNS record is still being advertised would stay listed
    /// as Supported while every sign attempt fails. Pruned at the end
    /// of every tick to drop ids that no longer survive into
    /// `connected_supported_hws`.
    phone_signers: HashMap<String, Arc<crate::phone_signer::PhoneSigner>>,
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
        persist_pairing: rs.persist_pairing,
        split_policy: rs.split_policy.clone(),
        unlock_retry: rs.unlock_retry.clone(),
        phone_cooldowns: HashMap::new(),
        phone_signers: HashMap::new(),
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

        // A device whose unlock failed is found afresh once its backoff ends.
        state.unlock_retry.release_due(
            &mut state.connected_supported_hws,
            std::time::Instant::now(),
        );

        let mut hws: Vec<HardwareWallet> = Vec::new();
        let mut still: Vec<String> = Vec::new();
        let physical = PhysicalDiscovery {
            network: state.network,
            keys_aliases: &state.keys_aliases,
            wallet: &state.wallet,
            connected_supported_hws: &state.connected_supported_hws,
            datadir_path: &state.datadir_path,
            persist_pairing: state.persist_pairing,
            split_policy: &state.split_policy,
        };
        physical.scan_specter_simulator(&mut hws, &mut still).await;
        physical.scan_specter_ports(&mut hws, &mut still).await;
        physical.scan_jade_ports(&mut hws, &mut still).await;
        physical.scan_ledger_simulator(&mut hws, &mut still).await;
        physical.scan_hid_devices(api, &mut hws, &mut still).await;
        physical.scan_ledger_hid(api, &mut hws, &mut still).await;

        scan_lan_phones(&mut state, &mut hws, &mut still).await;

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
        // Drop phone handles whose ids didn't survive into the
        // rebuilt list (offline, wallet-fp mismatch, removed from
        // store). Without this we'd keep dead `Arc<PhoneSigner>`
        // entries growing forever across reconnects.
        let surviving_ids = state.connected_supported_hws.clone();
        state
            .phone_signers
            .retain(|id, _| surviving_ids.contains(id));
        let _ = output
            .send(HardwareWalletMessage::List(ConnectedList {
                new: hws,
                still,
            }))
            .await;
    })
}

async fn scan_lan_phones(
    state: &mut State,
    hws: &mut Vec<HardwareWallet>,
    still: &mut Vec<String>,
) {
    // LAN-paired phones via the local-signer protocol. We browse
    // mDNS for `_coincube-signer._tcp.local.`, match the TXT
    // `fp=` against persisted `pairing_store` entries, and dial
    // each match over TLS. On a successful pinned-cert handshake
    // we surface a `PhoneSigner` as `Supported`; phones that are
    // paired but currently unreachable (mDNS silent or dial
    // failed) surface as `Unsupported(AppIsNotOpen)` so the user
    // sees "phone offline" rather than silence.
    let lan_chain = state.wallet.as_ref().map(|w| w.chain);
    let phone_store = if lan_chain.is_some_and(crate::phone_signer::lan_signing_allowed) {
        crate::phone_signer::pairing_store::load(&state.datadir_path)
    } else {
        // No discovery, identity creation or dial for an unsupported chain.
        Ok(Default::default())
    };
    match phone_store {
        Ok(store) if !store.phones.is_empty() || !store.pending_unpairs.is_empty() => {
            let discovered = crate::phone_signer::mdns::browse();
            let identity = match crate::phone_signer::identity::load_or_create(&state.datadir_path)
            {
                Ok(id) => Some(id),
                Err(e) => {
                    warn!("local-signer identity unavailable: {}", e);
                    None
                }
            };
            if let Some(identity) = identity {
                scan_lan_phone_store(state, hws, still, store, discovered, identity).await;
            }
        }
        Ok(_) => {}
        Err(e) => debug!("local-signer pairing store: {}", e),
    }
}

async fn scan_lan_phone_store(
    state: &mut State,
    hws: &mut Vec<HardwareWallet>,
    still: &mut Vec<String>,
    store: crate::phone_signer::pairing_store::PairingStoreFile,
    discovered: Vec<crate::phone_signer::mdns::DiscoveredPhone>,
    identity: crate::phone_signer::identity::DesktopIdentity,
) {
    // Sharing the identity across N concurrent dials —
    // `PairedTransport::connect` only borrows for the
    // duration of the rustls config build, so an Arc
    // clone per future is cheap.
    let identity = Arc::new(identity);
    let now = std::time::Instant::now();
    let mut dials = Vec::new();
    // The transport descriptor is distinct from an imported
    // wallet's persisted, chain-bound storage identity.
    let lan_policy = state
        .wallet
        .as_ref()
        .and_then(|wallet| crate::phone_signer::policy::PhonePolicy::from_wallet(wallet).ok());
    let active_transport_id = lan_policy.as_ref().map(|policy| policy.transport_id());
    // What every LAN session gets sealed with. Loaded once per
    // tick rather than per phone: `load_or_create` touches the
    // sidecar, and a tick can dial several phones.
    //
    // The descriptor comes from the same `Wallet` that scopes
    // the dial below, and `PhoneSigner::new` derives the
    // session's `descriptor_id` from it, so the LAN rail and
    // the Cubes list are the same digest of the same string by
    // construction, not by coincidence.
    // A phone can still be listed and dialled without this —
    // it just can't be sent a session. `sign_tx` turns the
    // absence into an actionable error rather than a silent
    // plaintext downgrade.
    let lan_transport_key =
        match crate::services::connect::crypto::DeviceTransportKey::load_or_create(
            &state.datadir_path.network_directory(state.network),
        ) {
            Ok(k) => Some(Arc::new(k)),
            Err(e) => {
                warn!(
                    "local-signer transport key unavailable, \
                     LAN signing will refuse to seal: {}",
                    e
                );
                None
            }
        };
    for paired in &store.phones {
        // Skip phones paired for a different transport policy
        // (or all phones when no wallet is loaded). Exact
        // imported account, origin, chain and storage identity
        // checks follow below; this short display ID never
        // grants signing authority on its own.
        if active_transport_id != Some(paired.vault_fingerprint) {
            continue;
        }
        let Some(policy) = lan_policy.as_ref() else {
            continue;
        };
        if let Err(error) = store.unique_signer_for(paired, policy) {
            debug!("{}: {}", paired.name, error);
            continue;
        }
        let fp8 = crate::phone_signer::identity::pin_hex8(&paired.cert_pin);
        let id = format!("{}{}", LAN_PHONE_ID_PREFIX, fp8);
        if remove_peer_unpaired_phone(state, paired, &fp8, &id) {
            continue;
        }
        // Resolve a target address: prefer the
        // mDNS-discovered one; fall back to the
        // user-entered `fallback_addr` when mDNS is
        // blocked or the phone isn't broadcasting.
        let target = resolve_phone_target(&fp8, paired, &discovered);
        if phone_already_connected(state, &store, policy, &id, target) {
            still.push(id);
            continue;
        }
        let Some((id, target)) = prepare_lan_phone_dial(state, hws, &fp8, id, target, now) else {
            continue;
        };
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
            Ok(t) => accept_lan_phone(
                state,
                hws,
                &store,
                lan_policy.as_ref(),
                &lan_transport_key,
                (fp8, id, paired, t),
            ),
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
    // Tell phones the user removed here, once reachable.
    if state.persist_pairing {
        crate::phone_signer::unpair_sync::deliver_pending(
            &state.datadir_path,
            &identity,
            &store,
            &discovered,
        )
        .await;
    }
}

fn remove_peer_unpaired_phone(
    state: &mut State,
    paired: &crate::phone_signer::pairing_store::PairedPhone,
    fp8: &str,
    id: &str,
) -> bool {
    // The phone said it removed this desktop. Drop the
    // pairing it was dialled for (not a newer re-pair
    // of the same phone) instead of redialling a phone
    // that would only say so again.
    if let Some(signer) = state
        .phone_signers
        .get(id)
        .filter(|s| s.peer_unpaired())
        .cloned()
    {
        state.phone_signers.remove(id);
        state.phone_cooldowns.remove(fp8);
        if state.persist_pairing {
            if let Err(e) = crate::phone_signer::pairing_store::remove_unpaired_by_peer(
                &state.datadir_path,
                &signer.paired_phone.cert_pin,
                signer.paired_phone.paired_at_unix,
                signer.paired_phone.imported_pairing_revision(),
            ) {
                warn!("could not remove {} after it unpaired: {}", paired.name, e);
            }
        }
        return true;
    }
    false
}

fn phone_already_connected(
    state: &State,
    store: &crate::phone_signer::pairing_store::PairingStoreFile,
    policy: &crate::phone_signer::policy::PhonePolicy,
    id: &str,
    target: Option<std::net::SocketAddr>,
) -> bool {
    // "Already connected" short-circuit. Unlike
    // USB devices — which the HID-API path
    // re-enumerates each tick — the
    // paired-phone store is persistent, so an
    // entry in `connected_supported_hws` alone
    // isn't proof the phone is still up. We
    // re-gate the short-circuit on:
    //   (a) current mDNS / fallback reachability
    //       (`target.is_some()`) — catches a
    //       phone whose Wi-Fi dropped.
    //   (b) the previously-issued `PhoneSigner`
    //       still being alive — catches a phone
    //       whose TLS session died while mDNS
    //       keeps advertising (e.g. signer app
    //       suspended, TCP reset). Without this
    //       the consumer would keep a stale
    //       handle and every sign attempt would
    //       hit `Disconnected`.
    //   (c) for imported wallets, its captured policy and
    //       exact pairing revision still matching the
    //       current store. A live socket to a re-paired
    //       phone is not a current signing handle.
    let prior_alive = state.phone_signers.get(id).is_some_and(|s| {
        s.is_alive()
            && (matches!(
                policy.kind(),
                crate::phone_signer::policy::PhonePolicyKind::Vault(_)
            ) || store.cached_imported_matches(&s.paired_phone, &s.policy, policy))
    });
    state
        .connected_supported_hws
        .iter()
        .any(|known| known == id)
        && target.is_some()
        && prior_alive
}

fn prepare_lan_phone_dial(
    state: &mut State,
    hws: &mut Vec<HardwareWallet>,
    fp8: &str,
    id: String,
    target: Option<std::net::SocketAddr>,
    now: std::time::Instant,
) -> Option<(String, std::net::SocketAddr)> {
    // We're about to re-dial, so the prior
    // handle (if any) is being replaced — drop
    // it now so its TLS session can close as
    // soon as no in-flight signing op still
    // holds a clone.
    state.phone_signers.remove(&id);
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
        return None;
    };
    // Cooldown gate: a phone that just failed
    // shouldn't pay another full `CONNECT_TIMEOUT`
    // this tick. Surface as offline; the window
    // expires on a later tick.
    if state
        .phone_cooldowns
        .get(fp8)
        .is_some_and(|cd| cd.is_cooling(now))
    {
        hws.push(HardwareWallet::Unsupported {
            id,
            kind: DeviceKind::Specter,
            version: None,
            reason: UnsupportedReason::AppIsNotOpen,
        });
        return None;
    }
    Some((id, target))
}

fn accept_lan_phone(
    state: &mut State,
    hws: &mut Vec<HardwareWallet>,
    store: &crate::phone_signer::pairing_store::PairingStoreFile,
    lan_policy: Option<&crate::phone_signer::policy::PhonePolicy>,
    lan_transport_key: &Option<Arc<crate::services::connect::crypto::DeviceTransportKey>>,
    (fp8, id, paired, t): (
        String,
        String,
        crate::phone_signer::pairing_store::PairedPhone,
        crate::phone_signer::transport::PairedTransport,
    ),
) {
    state.phone_cooldowns.remove(&fp8);
    let Some(policy) = lan_policy else {
        return;
    };
    let Ok(binding) = store.unique_signer_for(&paired, policy) else {
        return;
    };
    let fingerprint = binding.fingerprint;
    let signer = Arc::new(crate::phone_signer::PhoneSigner::new(
        t,
        fingerprint,
        None,
        paired.clone(),
        policy.clone(),
        state.datadir_path.clone(),
        (*lan_transport_key).clone(),
    ));
    // Stash a clone so the next refresh
    // tick can probe `is_alive()` before
    // deciding whether to re-dial.
    state.phone_signers.insert(id.clone(), signer.clone());
    let device: Arc<dyn HWI + Send + Sync> = signer;
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

struct PhysicalDiscovery<'a> {
    network: Network,
    keys_aliases: &'a HashMap<Fingerprint, String>,
    wallet: &'a Option<Arc<Wallet>>,
    connected_supported_hws: &'a [String],
    datadir_path: &'a CoincubeDirectory,
    persist_pairing: bool,
    split_policy: &'a Option<SplitPolicyBinding>,
}

impl PhysicalDiscovery<'_> {
    async fn scan_specter_simulator(&self, hws: &mut Vec<HardwareWallet>, still: &mut Vec<String>) {
        match specter::SpecterSimulator::try_connect().await {
            Ok(device) => {
                let id = "specter-simulator".to_string();
                if self.connected_supported_hws.contains(&id) {
                    still.push(id);
                } else {
                    match HardwareWallet::new(id, Arc::new(device), Some(self.keys_aliases)).await {
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
    }

    async fn scan_specter_ports(&self, hws: &mut Vec<HardwareWallet>, still: &mut Vec<String>) {
        match specter::SerialTransport::enumerate_potential_ports() {
            Ok(ports) => {
                for port in ports {
                    let id = format!("specter-{}", port);
                    if self.connected_supported_hws.contains(&id) {
                        still.push(id);
                    } else {
                        self.discover_specter_port(id, port, hws).await;
                    }
                }
            }
            Err(e) => warn!("Error while listing specter wallets: {}", e),
        }
    }

    async fn discover_specter_port(&self, id: String, port: String, hws: &mut Vec<HardwareWallet>) {
        match specter::Specter::<specter::SerialTransport>::new(port.clone()) {
            Err(e) => {
                warn!("{}", e);
            }
            Ok(device) => {
                if tokio::time::timeout(std::time::Duration::from_millis(500), device.fingerprint())
                    .await
                    .is_ok()
                {
                    match HardwareWallet::new(id, Arc::new(device), Some(self.keys_aliases)).await {
                        Ok(hw) => hws.push(hw),
                        Err(e) => {
                            debug!("{}", e);
                        }
                    }
                }
            }
        }
    }

    async fn scan_jade_ports(&self, hws: &mut Vec<HardwareWallet>, still: &mut Vec<String>) {
        match jade::SerialTransport::enumerate_potential_ports() {
            Ok(ports) => {
                for port in ports {
                    let id = format!("jade-{}", port);
                    if self.connected_supported_hws.contains(&id) {
                        still.push(id);
                    } else {
                        self.discover_jade_port(id, port, hws).await;
                    }
                }
            }
            Err(e) => warn!("Error while listing jade devices: {}", e),
        }
    }

    async fn discover_jade_port(&self, id: String, port: String, hws: &mut Vec<HardwareWallet>) {
        match jade::SerialTransport::new(port) {
            Err(e) => {
                warn!("{:?}", e);
            }
            Ok(device) => {
                match handle_jade_device(
                    id,
                    self.network,
                    Jade::new(device).with_network(self.network),
                    self.wallet.as_ref().map(|w| w.as_ref()),
                    Some(self.keys_aliases),
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

    async fn scan_ledger_simulator(&self, hws: &mut Vec<HardwareWallet>, still: &mut Vec<String>) {
        match ledger::LedgerSimulator::try_connect().await {
            Ok(device) => {
                let id = "ledger-simulator".to_string();
                if self.connected_supported_hws.contains(&id) {
                    still.push(id);
                } else {
                    match handle_ledger_device(
                        id,
                        device,
                        self.wallet.as_ref().map(|w| w.as_ref()),
                        self.keys_aliases,
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
    }

    async fn scan_hid_devices(
        &self,
        api: &ledger::HidApi,
        hws: &mut Vec<HardwareWallet>,
        still: &mut Vec<String>,
    ) {
        for device_info in api.device_list() {
            if async_hwi::bitbox::is_bitbox02(device_info) {
                let id = format!(
                    "bitbox-{:?}-{}-{}",
                    device_info.path(),
                    device_info.vendor_id(),
                    device_info.product_id()
                );
                if self.connected_supported_hws.contains(&id) {
                    still.push(id);
                    continue;
                }
                self.discover_bitbox(id, device_info, api, hws).await;
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
                if self.connected_supported_hws.contains(&id) {
                    still.push(id);
                    continue;
                }
                self.discover_coldcard(id, device_info, api, hws).await;
            }
        }
    }

    async fn discover_bitbox(
        &self,
        id: String,
        device_info: &ledger::DeviceInfo,
        api: &ledger::HidApi,
        hws: &mut Vec<HardwareWallet>,
    ) {
        if let Ok(device) = device_info.open_device(api) {
            let noise: Option<Box<dyn async_hwi::bitbox::NoiseConfig>> = if self.persist_pairing {
                Some(Box::new(settings::global::PersistedBitboxNoiseConfig::new(
                    self.datadir_path,
                )))
            } else {
                None
            };
            if let Ok(device) = PairingBitbox02::connect(device, noise).await {
                hws.push(HardwareWallet::Locked {
                    id,
                    kind: DeviceKind::BitBox02,
                    pairing_code: device.pairing_code().map(|s| s.replace('\n', " ")),
                    device: Arc::new(Mutex::new(Some(LockedDevice::BitBox02(Box::new(device))))),
                });
            }
        }
    }

    async fn discover_coldcard(
        &self,
        id: String,
        device_info: &ledger::DeviceInfo,
        api: &ledger::HidApi,
        hws: &mut Vec<HardwareWallet>,
    ) {
        if let Some(sn) = device_info.serial_number() {
            if let Ok((cc, _)) = coldcard::api::Coldcard::open(AsRefWrap { inner: api }, sn, None) {
                let device = self.bind_coldcard(cc);
                self.inspect_coldcard(id, device, hws).await;
            }
        }
    }

    fn bind_coldcard(&self, cc: coldcard::api::Coldcard) -> Arc<dyn HWI + Send + Sync> {
        if let Some(wallet) = self
            .wallet
            .as_ref()
            .filter(|wallet| wallet.inheritance_descriptor().is_some())
        {
            coldcard::Coldcard::from(cc)
                .with_wallet_name(wallet.name.clone())
                .into()
        } else if let Some((name, _)) = self
            .split_policy
            .as_ref()
            .and_then(SplitPolicyBinding::bound)
        {
            // Split (#568 B4b-2): the named policy's handle.
            coldcard::Coldcard::from(cc)
                .with_wallet_name(name.to_string())
                .into()
        } else {
            coldcard::Coldcard::from(cc).into()
        }
    }

    async fn inspect_coldcard(
        &self,
        id: String,
        device: Arc<dyn HWI + Send + Sync>,
        hws: &mut Vec<HardwareWallet>,
    ) {
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
                        alias: self.keys_aliases.get(&fingerprint).cloned(),
                    });
                } else {
                    hws.push(HardwareWallet::Unsupported {
                        id,
                        kind: device.device_kind(),
                        version: Some(version),
                        reason: UnsupportedReason::Version {
                            minimal_supported_version: "Edge firmware v6.2.1",
                            note: Some(
                                "This is the experimental Edge branch, Mk4/Q \
                                 only — not available on the Mk3.",
                            ),
                        },
                    });
                }
            }
            _ => tracing::error!("Failed to connect to coldcard"),
        }
    }

    async fn scan_ledger_hid(
        &self,
        api: &ledger::HidApi,
        hws: &mut Vec<HardwareWallet>,
        still: &mut Vec<String>,
    ) {
        for detected in ledger::Ledger::<ledger::TransportHID>::enumerate(api) {
            let id = format!(
                "ledger-{:?}-{}-{}",
                detected.path(),
                detected.vendor_id(),
                detected.product_id()
            );
            if self.connected_supported_hws.contains(&id) {
                still.push(id);
                continue;
            }

            match ledger::Ledger::<ledger::TransportHID>::connect(api, detected) {
                Ok(device) => match handle_ledger_device(
                    id,
                    device,
                    self.wallet.as_ref().map(|w| w.as_ref()),
                    self.keys_aliases,
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
    }
}

async fn handle_ledger_device<'a, T: async_hwi::ledger::Transport + Sync + Send + 'static>(
    id: String,
    device: ledger::Ledger<T>,
    wallet: Option<&'a Wallet>,
    keys_aliases: &'a HashMap<Fingerprint, String>,
) -> Result<HardwareWallet, HWIError> {
    match (
        device.get_master_fingerprint().await,
        device.get_version().await,
    ) {
        (Ok(fingerprint), Ok(version)) => {
            if ledger_version_supported(&version) {
                let (device, registered) = bind_ledger_wallet(device, wallet, fingerprint);
                Ok(HardwareWallet::Supported {
                    id,
                    kind: device.device_kind(),
                    fingerprint,
                    device: Arc::new(device),
                    version: Some(version),
                    registered: listed_registration(wallet, registered),
                    alias: keys_aliases.get(&fingerprint).cloned(),
                })
            } else {
                Ok(HardwareWallet::Unsupported {
                    id,
                    kind: device.device_kind(),
                    version: Some(version),
                    reason: UnsupportedReason::Version {
                        minimal_supported_version: "2.1.0",
                        note: None,
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

fn bind_ledger_wallet<T: async_hwi::ledger::Transport + Sync + Send + 'static>(
    mut device: ledger::Ledger<T>,
    wallet: Option<&Wallet>,
    fingerprint: Fingerprint,
) -> (ledger::Ledger<T>, bool) {
    let mut registered = false;
    if let Some(w) = &wallet {
        if let Some(cfg) = w
            .hardware_wallets
            .iter()
            .find(|cfg| cfg.fingerprint == fingerprint)
        {
            if let Some(descriptor) = w.inheritance_descriptor() {
                device = device
                    .with_wallet(&w.name, &descriptor.to_string(), Some(cfg.token()))
                    .expect("Configuration must be correct");
                registered = true;
            }
        }
    }
    (device, registered)
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
                handle_ready_jade(id, device, wallet, keys_aliases).await
            }
        }
    }
}

async fn handle_ready_jade(
    id: String,
    device: Jade<async_hwi::jade::SerialTransport>,
    wallet: Option<&Wallet>,
    keys_aliases: Option<&HashMap<Fingerprint, String>>,
) -> Result<HardwareWallet, HWIError> {
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
            let Some(descriptor) = wallet.inheritance_descriptor() else {
                // An imported wallet registers its own device
                // policy when it signs (`split_hardware`), so
                // whether it is registered is not known here.
                return Ok(HardwareWallet::Supported {
                    id: id.clone(),
                    kind,
                    fingerprint,
                    device: Arc::new(device),
                    version,
                    registered: None,
                    alias,
                });
            };
            let desc = descriptor.to_string();
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

/// Device-list id prefix of a LAN-paired Keychain phone (`phone-{fp8}`).
/// USB devices use their vendor name (`ledger-…`, `jade-…`), so the prefix is
/// what [`HardwareWallet::is_lan_phone`] keys off.
pub(crate) const LAN_PHONE_ID_PREFIX: &str = "phone-";

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
    crate::phone_signer::unpair_sync::resolve_target(
        fp8,
        paired.fallback_addr.as_deref(),
        discovered,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phone_signer::mdns::DiscoveredPhone;
    use crate::phone_signer::pairing_store::PairedPhone;
    use std::net::SocketAddr;

    const DESC: &str = "wsh(or_d(multi(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[de6eb005/48'/1'/0'/2']tpubDFGuYfS2JwiUSEXiQuNGdT3R7WTDhbaE6jbUhgYSSdhmfQcSx7ZntMPPv7nrkvAqjpj3jX9wbhSGMeKVao4qAzhbNyBi7iQmv5xxQk6H6jz/<0;1>/*),and_v(v:pkh([ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*),older(3))))#p9ax3xxp";

    /// An imported wallet's devices are never listed as unregistered: its
    /// signing session registers the policy, and the picker would otherwise
    /// disable them. A Vault and a wallet-less listing keep the device's
    /// answer.
    #[test]
    fn an_imported_wallets_devices_are_not_listed_as_unregistered() {
        let imported = crate::services::split_test_wallets::imported_vault(
            crate::services::split_test_wallets::Shape::WshSortedMulti,
        );
        assert_eq!(listed_registration(Some(&imported), false), None);
        assert_eq!(listed_registration(Some(&imported), true), None);
        use coincube_core::descriptors::CoincubeDescriptor;
        use std::str::FromStr;
        let vault = Wallet::new(CoincubeDescriptor::from_str(DESC).unwrap());
        assert_eq!(listed_registration(Some(&vault), false), Some(false));
        assert_eq!(listed_registration(Some(&vault), true), Some(true));
        assert_eq!(listed_registration(None, false), Some(false));
    }

    fn refresh_hash(wallet: Option<Arc<Wallet>>, root: &str) -> u64 {
        refresh_hash_with(wallet, root, None)
    }

    fn refresh_hash_with(
        wallet: Option<Arc<Wallet>>,
        root: &str,
        split_policy: Option<SplitPolicyBinding>,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        let state = RefreshState {
            network: Network::Testnet4,
            keys_aliases: HashMap::new(),
            wallet,
            datadir_path: CoincubeDirectory::new(root.into()),
            persist_pairing: true,
            split_policy,
            unlock_retry: UnlockRetry::default(),
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        state.hash(&mut hasher);
        hasher.finish()
    }

    fn listing_hash(listing: &HardwareWallets) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        listing.refresh_state().hash(&mut hasher);
        hasher.finish()
    }

    /// A device that fails to unlock leaves the list and is found again
    /// after a backoff (a BitBox02 plugged in while locked otherwise vanished
    /// until replugged). The scan itself does not restart, so other devices
    /// stay listed, and failures in a row wait longer.
    #[test]
    fn a_failed_unlock_is_retried_after_a_backoff() {
        use std::time::{Duration, Instant};
        let mut listing = HardwareWallets::new(
            CoincubeDirectory::new("/synthetic/one".into()),
            Network::Testnet4,
        );
        let before = listing_hash(&listing);
        let fail = |listing: &mut HardwareWallets| {
            let _ = listing.update(HardwareWalletMessage::Unlocked(
                "bitbox-synthetic".into(),
                Err(async_hwi::Error::Device(
                    "error querying device info".into(),
                )),
            ));
        };
        let connected = || {
            vec![
                "bitbox-synthetic".to_string(),
                "coldcard-synthetic".to_string(),
            ]
        };
        let released = |listing: &HardwareWallets, after: Duration| {
            let mut ids = connected();
            listing
                .unlock_retry
                .release_due(&mut ids, Instant::now() + after);
            ids
        };

        fail(&mut listing);
        assert!(listing.list.is_empty());
        assert_eq!(listing_hash(&listing), before, "the scan keeps running");
        // Held back until the backoff ends; other devices are untouched.
        assert_eq!(released(&listing, Duration::ZERO), connected());
        assert_eq!(
            released(&listing, Duration::from_secs(3)),
            vec!["coldcard-synthetic".to_string()]
        );
        // Released once: the retry in flight is not started twice.
        assert_eq!(released(&listing, Duration::from_secs(4)), connected());
        // A second failure waits longer, and the wait is capped.
        fail(&mut listing);
        assert_eq!(released(&listing, Duration::from_secs(3)), connected());
        assert_eq!(released(&listing, Duration::from_secs(5)).len(), 1);
        for _ in 0..10 {
            fail(&mut listing);
        }
        assert_eq!(released(&listing, Duration::from_secs(59)), connected());
        assert_eq!(released(&listing, Duration::from_secs(61)).len(), 1);
        // A successful unlock clears the count: the next failure waits 2s.
        listing.unlock_retry.unlocked("bitbox-synthetic");
        fail(&mut listing);
        assert_eq!(released(&listing, Duration::from_secs(3)).len(), 1);
    }

    /// #568 B4b-2: the Split policy binding is part of the subscription
    /// identity, so a listing opened for another policy, or none, restarts
    /// its stream; a Split listing never persists pairing; an unnamed
    /// policy binds nothing into the handles; Debug carries no descriptor.
    #[test]
    fn refresh_subscription_restarts_for_split_policy_changes() {
        let split = |name: &str| {
            HardwareWallets::new(
                CoincubeDirectory::new("/synthetic/one".into()),
                Network::Testnet4,
            )
            .with_split_policy(name.to_string(), DESC.to_string())
        };
        let bare = refresh_hash_with(None, "/synthetic/one", None);
        let listing = split("Split0000abcd");
        assert!(!listing.persists_pairing());
        let binding = listing.split_policy_binding().cloned().unwrap();
        assert_eq!(binding.bound(), Some(("Split0000abcd", DESC)));
        let bound = refresh_hash_with(None, "/synthetic/one", Some(binding.clone()));
        assert_ne!(bare, bound);
        assert_eq!(
            bound,
            refresh_hash_with(None, "/synthetic/one", Some(binding.clone()))
        );
        let other = split("Split0000abce")
            .split_policy_binding()
            .cloned()
            .unwrap();
        assert_ne!(
            bound,
            refresh_hash_with(None, "/synthetic/one", Some(other))
        );
        assert!(!format!("{binding:?}").contains("tpub"));

        let unnamed = split("");
        assert!(!unnamed.persists_pairing());
        let binding = unnamed.split_policy_binding().unwrap();
        assert_eq!(binding.bound(), None);
        assert_eq!(binding.descriptor(), DESC);
        assert!(!format!("{binding:?}").contains("tpub"));
    }

    /// An imported Vault's device list binds the device policy its signing
    /// session registers, so Coldcard and BitBox02 multisig handles can sign
    /// it; singlesig binds nothing; pairing still persists; a Vault binds
    /// its own descriptor, not a Split policy.
    #[test]
    fn an_imported_vaults_device_list_binds_its_device_policy() {
        use crate::services::split_test_wallets::{imported_vault, Shape, SHAPES};
        let root = || CoincubeDirectory::new("/synthetic/imported".into());
        for shape in SHAPES {
            let wallet = Arc::new(imported_vault(shape));
            let policy = crate::split_hardware::policy::imported_policy(
                wallet.main_descriptor.as_imported().unwrap(),
            )
            .unwrap();
            let listing = HardwareWallets::new(root(), Network::Bitcoin).with_wallet(wallet);
            assert!(listing.persists_pairing(), "{:?}", shape);
            let binding = listing.split_policy_binding().unwrap();
            assert_eq!(binding.descriptor(), policy.descriptor(), "{:?}", shape);
            if matches!(shape, Shape::WshMulti | Shape::WshSortedMulti) {
                assert_eq!(
                    binding.bound(),
                    Some((policy.name(), policy.descriptor())),
                    "{:?}",
                    shape
                );
            } else {
                assert_eq!(binding.bound(), None, "{:?}", shape);
            }
        }

        let vault = Arc::new(Wallet::new(
            crate::app::state::vault::test_support::unified::fixture().descriptor,
        ));
        let listing = HardwareWallets::new(root(), Network::Bitcoin).with_wallet(vault);
        assert!(listing.split_policy_binding().is_none());
    }

    #[test]
    fn refresh_subscription_restarts_for_chain_descriptor_and_storage_changes() {
        use coincube_core::{chain::ChainId, descriptors::CoincubeDescriptor};
        use std::str::FromStr;
        let descriptor = CoincubeDescriptor::from_str(DESC).unwrap();
        let bitcoin = Arc::new(Wallet::new(descriptor.clone()).with_chain(ChainId::Testnet4));
        let fork = Arc::new(Wallet::new(descriptor).with_chain(ChainId::BitcoinBlake2bTestnet4));
        assert_eq!(
            bitcoin.chain.bitcoin_network(),
            fork.chain.bitcoin_network()
        );
        let baseline = refresh_hash(Some(bitcoin.clone()), "/synthetic/one");
        assert_eq!(baseline, refresh_hash(Some(bitcoin), "/synthetic/one"));
        let fork_hash = refresh_hash(Some(fork.clone()), "/synthetic/one");
        assert_ne!(baseline, fork_hash);
        let other_descriptor = DESC
            .split('#')
            .next()
            .unwrap()
            .replace("older(3)", "older(4)");
        let other = Arc::new(
            Wallet::new(CoincubeDescriptor::from_str(&other_descriptor).unwrap())
                .with_chain(ChainId::BitcoinBlake2bTestnet4),
        );
        assert_ne!(fork_hash, refresh_hash(Some(other), "/synthetic/one"));
        assert_ne!(fork_hash, refresh_hash(Some(fork), "/synthetic/two"));
        assert_ne!(fork_hash, refresh_hash(None, "/synthetic/one"));
    }

    fn phone_with_fallback(pin: [u8; 32], fallback: Option<&str>) -> PairedPhone {
        PairedPhone {
            signer_binding: None,
            cert_pin: pin,
            name: "Test".into(),
            paired_at_unix: 0,
            wallet_fingerprints: Vec::new(),
            vault_fingerprint: Default::default(),
            transport_pubkey: Vec::new(),
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
