//! Public hardware-wallet account source shared by Split and wallet import.
//!
//! A connected device exports one standard singlesig account xpub
//! (BIP84/BIP49/BIP44, `m/purpose'/0'/account'`). The xpub is checked against
//! the device's master fingerprint, mainnet, depth and child number before the
//! existing [`AccountXpubSource`] boundary derives public receive/change
//! descriptors. Nothing here signs, and nothing is persisted or logged: the
//! device list is session-only (no BitBox02 pairing written) and the account
//! lives in memory until cancel, disconnect or a changed choice clears it.
//! Taproot (BIP86) is deliberately not offered.
//!
//! Hardware signing lives separately in `split_hardware`; this source never
//! signs and its account material has no spend authority.

use std::sync::Arc;

use async_hwi::HWI;
use coincube_core::miniscript::bitcoin::{
    bip32::{ChildNumber, DerivationPath, Fingerprint, Xpub},
    Network, NetworkKind,
};
use coincube_ui::theme::tenshu::button::ActionKind;
use coincube_ui::{
    component::{tenshu::ActionButton, text::*},
    theme,
    widget::{Column, Element, Row},
};
use iced::advanced::widget::Id;
use iced::{widget::text_input, Alignment, Length, Subscription, Task};

use crate::{
    dir::CoincubeDirectory,
    hw::{HardwareWallet, HardwareWalletMessage, HardwareWallets},
    services::{
        foreign_scan::ScanDescriptor,
        public_account_source::{AccountXpubSource, StandardSinglesig},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Bip84,
    Bip49,
    Bip44,
}

pub const PURPOSES: [Purpose; 3] = [Purpose::Bip84, Purpose::Bip49, Purpose::Bip44];

impl Purpose {
    fn number(self) -> u32 {
        match self {
            Self::Bip84 => 84,
            Self::Bip49 => 49,
            Self::Bip44 => 44,
        }
    }

    fn standard(self) -> StandardSinglesig {
        match self {
            Self::Bip84 => StandardSinglesig::Bip84,
            Self::Bip49 => StandardSinglesig::Bip49,
            Self::Bip44 => StandardSinglesig::Bip44,
        }
    }

    /// `m/purpose'/0'/account'` (mainnet coin type only).
    pub fn path(self, account: u32) -> Result<DerivationPath, HardwareSourceError> {
        let account =
            ChildNumber::from_hardened_idx(account).map_err(|_| HardwareSourceError::Account)?;
        Ok(DerivationPath::from(vec![
            ChildNumber::Hardened {
                index: self.number(),
            },
            ChildNumber::Hardened { index: 0 },
            account,
        ]))
    }
}

impl std::fmt::Display for Purpose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bip84 => write!(f, "Native SegWit (BIP84, m/84'/0'/n')"),
            Self::Bip49 => write!(f, "Nested SegWit (BIP49, m/49'/0'/n')"),
            Self::Bip44 => write!(f, "Legacy (BIP44, m/44'/0'/n')"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareSourceError {
    /// The device reported a master fingerprint other than the listed one.
    Fingerprint,
    Network,
    /// Depth or child number do not match `m/purpose'/0'/account'`.
    Depth,
    /// The parent fingerprint cannot belong to a depth-3 account key.
    ParentFingerprint,
    Account,
    Descriptor,
    Device,
    Disconnected,
}

fn error_copy(error: HardwareSourceError) -> &'static str {
    match error {
        HardwareSourceError::Fingerprint => "The device reported a different wallet fingerprint than the one listed. Reconnect the device and try again.",
        HardwareSourceError::Network => "The device returned a non-mainnet key. Only Bitcoin mainnet accounts can be scanned.",
        HardwareSourceError::Depth | HardwareSourceError::ParentFingerprint => "The device returned a key that does not match the requested account path. Nothing was scanned.",
        HardwareSourceError::Account => "Enter an account number between 0 and 2147483647.",
        HardwareSourceError::Descriptor => "The account could not be turned into a supported public descriptor.",
        HardwareSourceError::Device => "The device did not return the account. Unlock it, open its Bitcoin app, and try again.",
        HardwareSourceError::Disconnected => "The device is no longer connected.",
    }
}

pub fn parse_account(value: &str) -> Result<u32, HardwareSourceError> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(HardwareSourceError::Account);
    }
    value
        .parse::<u32>()
        .ok()
        .filter(|account| *account < (1 << 31))
        .ok_or(HardwareSourceError::Account)
}

/// Public account material read from a device. Debug output is redacted so
/// the xpub never reaches a log line through message tracing.
pub struct HardwareAccount {
    pub device_id: String,
    pub fingerprint: Fingerprint,
    pub purpose: Purpose,
    pub account: u32,
    pub external: ScanDescriptor,
    pub internal: ScanDescriptor,
}

impl std::fmt::Debug for HardwareAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HardwareAccount")
            .field("fingerprint", &self.fingerprint)
            .field("purpose", &self.purpose)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// Checks a device-exported account xpub and derives its descriptors, with
/// the device's master fingerprint recorded as the key origin.
pub fn account_from_device(
    device_id: String,
    purpose: Purpose,
    account: u32,
    listed: Fingerprint,
    reported: Fingerprint,
    xpub: Xpub,
) -> Result<HardwareAccount, HardwareSourceError> {
    if reported != listed {
        return Err(HardwareSourceError::Fingerprint);
    }
    if xpub.network != NetworkKind::Main {
        return Err(HardwareSourceError::Network);
    }
    let expected = purpose.path(account)?;
    if xpub.depth != 3 || Some(&xpub.child_number) != expected.as_ref().last() {
        return Err(HardwareSourceError::Depth);
    }
    // A depth-3 key's parent is the depth-2 coin-type key: its fingerprint is
    // never zero, never the master's, and never the key's own.
    if xpub.parent_fingerprint == Fingerprint::default()
        || xpub.parent_fingerprint == listed
        || xpub.fingerprint() == listed
        || xpub.parent_fingerprint == xpub.fingerprint()
    {
        return Err(HardwareSourceError::ParentFingerprint);
    }
    let descriptors = AccountXpubSource::new(purpose.standard(), account, listed, xpub)
        .map_err(|_| HardwareSourceError::Depth)?
        .descriptors()
        .map_err(|_| HardwareSourceError::Descriptor)?;
    Ok(HardwareAccount {
        device_id,
        fingerprint: descriptors.fingerprint,
        purpose,
        account,
        external: descriptors.external,
        internal: descriptors.internal,
    })
}

/// Reads the account through the same HWI export the installer uses
/// (`installer::step::descriptor::editor::key::get_extended_pubkey`), after a
/// fresh master-fingerprint read so a swapped device is refused.
pub async fn read_account(
    device: Arc<dyn HWI + Send + Sync>,
    device_id: String,
    listed: Fingerprint,
    purpose: Purpose,
    account: u32,
) -> Result<HardwareAccount, HardwareSourceError> {
    let path = purpose.path(account)?;
    let reported = device
        .get_master_fingerprint()
        .await
        .map_err(|_| HardwareSourceError::Device)?;
    if reported != listed {
        return Err(HardwareSourceError::Fingerprint);
    }
    let xpub = device
        .get_extended_pubkey(&path)
        .await
        .map_err(|_| HardwareSourceError::Device)?;
    account_from_device(device_id, purpose, account, listed, reported, xpub)
}

#[derive(Debug, Clone)]
pub enum HardwareMessage {
    Focus(Id),
    Devices(HardwareWalletMessage, u64),
    PurposeSelected(Purpose),
    AccountEdited(String),
    Read(String),
    Loaded(Result<Arc<HardwareAccount>, HardwareSourceError>, u64, u64),
}

fn focus(id: Id) -> HardwareMessage {
    HardwareMessage::Focus(id)
}

const ACCOUNT_INPUT_ID: &str = "tenshu.hardware.account";
static NEXT_HARDWARE_ACTION_SCOPE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

fn read_action_name(device_id: &str) -> String {
    format!("tenshu.hardware.read:{}:{}", device_id.len(), device_id)
}

fn purpose_action_id(purpose: Purpose) -> &'static str {
    match purpose {
        Purpose::Bip84 => "tenshu.hardware.purpose.bip84",
        Purpose::Bip49 => "tenshu.hardware.purpose.bip49",
        Purpose::Bip44 => "tenshu.hardware.purpose.bip44",
    }
}

/// Restore focus after a control disappears only while its owner is mounted
/// and nothing else has focus. A result can arrive in another tab or overlay.
pub(crate) fn focus_if_unfocused<T: 'static>(target: Id) -> Task<T> {
    use iced::advanced::widget::operation::{focusable, Operation, Outcome};

    struct Restore {
        target: Id,
        mounted: bool,
        found: bool,
    }

    impl<T: 'static> Operation<T> for Restore {
        fn focusable(
            &mut self,
            id: Option<&Id>,
            _bounds: iced::Rectangle,
            state: &mut dyn focusable::Focusable,
        ) {
            self.mounted |= id == Some(&self.target);
            self.found |= state.is_focused();
        }

        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
            operate(self);
        }

        fn finish(&self) -> Outcome<T> {
            if self.found || !self.mounted {
                Outcome::None
            } else {
                Outcome::Chain(Box::new(focusable::focus(self.target.clone())))
            }
        }
    }

    iced_runtime::task::effect(iced_runtime::Action::widget(Restore {
        target,
        mounted: false,
        found: false,
    }))
}

#[derive(Debug, Clone)]
pub enum HardwareStatus {
    Idle,
    Reading(String),
    Ready(Arc<HardwareAccount>),
    Failed(HardwareSourceError),
}

pub struct HardwareSource {
    // Each source instance owns its focus targets, including imported Bitcoin.
    action_scope: u64,
    devices: Option<HardwareWallets>,
    pub purpose: Purpose,
    pub account: String,
    status: HardwareStatus,
    generation: u64,
}

impl Default for HardwareSource {
    fn default() -> Self {
        Self {
            action_scope: NEXT_HARDWARE_ACTION_SCOPE
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            devices: None,
            purpose: Purpose::Bip84,
            account: "0".to_string(),
            status: HardwareStatus::Idle,
            generation: 0,
        }
    }
}

impl HardwareSource {
    pub(crate) fn action_id(&self, name: impl AsRef<str>) -> Id {
        Id::from(format!("{}.source.{}", name.as_ref(), self.action_scope))
    }

    fn read_action_id(&self, device_id: &str) -> Id {
        self.action_id(read_action_name(device_id))
    }

    fn focus_account(&self) -> Task<HardwareMessage> {
        iced::widget::operation::focus(self.action_id(ACCOUNT_INPUT_ID))
    }

    /// Session-only device listing on Bitcoin mainnet with no wallet loaded:
    /// no pairing persistence, phone discovery or identity creation.
    pub fn set_root(&mut self, datadir: CoincubeDirectory) {
        if self.devices.is_none() {
            self.devices = Some(HardwareWallets::new(datadir, Network::Bitcoin).ephemeral());
        }
    }

    pub fn account(&self) -> Option<&HardwareAccount> {
        match &self.status {
            HardwareStatus::Ready(account) => Some(account),
            _ => None,
        }
    }

    pub fn status(&self) -> &HardwareStatus {
        &self.status
    }

    /// Drops any read or in-flight account. A later `Loaded` is stale.
    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.status = HardwareStatus::Idle;
    }

    pub fn subscription(&self) -> Subscription<HardwareMessage> {
        self.devices
            .as_ref()
            .map(|devices| {
                devices
                    .refresh()
                    .with(self.action_scope)
                    .map(|(action_scope, message)| HardwareMessage::Devices(message, action_scope))
            })
            .unwrap_or_else(Subscription::none)
    }

    fn supported(&self, id: &str) -> Option<(Arc<dyn HWI + Send + Sync>, Fingerprint)> {
        self.devices.as_ref()?.list.iter().find_map(|hw| match hw {
            HardwareWallet::Supported {
                id: candidate,
                device,
                fingerprint,
                ..
            } if candidate == id => Some((device.clone(), *fingerprint)),
            _ => None,
        })
    }

    fn selected_device(&self) -> Option<&str> {
        match &self.status {
            HardwareStatus::Reading(id) => Some(id),
            HardwareStatus::Ready(account) => Some(&account.device_id),
            _ => None,
        }
    }

    /// Returns the follow-up task and whether previously read account
    /// material (and so any scan built from it) was invalidated.
    pub fn update(&mut self, message: HardwareMessage) -> (Task<HardwareMessage>, bool) {
        match message {
            HardwareMessage::Focus(id) => (iced::widget::operation::focus(id), false),
            HardwareMessage::Devices(message, action_scope) => {
                self.apply_device_update(message, action_scope)
            }
            HardwareMessage::PurposeSelected(purpose) => {
                if self.purpose == purpose {
                    return (Task::none(), false);
                }
                self.purpose = purpose;
                self.clear();
                (Task::none(), true)
            }
            HardwareMessage::AccountEdited(value) => {
                self.account = value;
                self.clear();
                (Task::none(), true)
            }
            HardwareMessage::Read(id) => {
                self.clear();
                let account = match parse_account(&self.account) {
                    Ok(account) => account,
                    Err(error) => {
                        self.status = HardwareStatus::Failed(error);
                        return (self.focus_account(), true);
                    }
                };
                let Some((device, listed)) = self.supported(&id) else {
                    self.status = HardwareStatus::Failed(HardwareSourceError::Disconnected);
                    return (self.focus_account(), true);
                };
                let generation = self.generation;
                let action_scope = self.action_scope;
                let purpose = self.purpose;
                self.status = HardwareStatus::Reading(id.clone());
                (
                    Task::batch([
                        Task::perform(
                            read_account(device, id, listed, purpose, account),
                            move |r| {
                                HardwareMessage::Loaded(r.map(Arc::new), generation, action_scope)
                            },
                        ),
                        iced::widget::operation::focus(self.action_id(purpose_action_id(purpose))),
                    ]),
                    true,
                )
            }
            HardwareMessage::Loaded(result, generation, action_scope) => {
                if action_scope != self.action_scope {
                    return (Task::none(), false);
                }
                let HardwareStatus::Reading(device_id) = &self.status else {
                    return (Task::none(), false);
                };
                if generation != self.generation {
                    return (Task::none(), false);
                }
                let device_id = device_id.clone();
                self.status = match result {
                    Ok(account) if self.supported(&account.device_id).is_some() => {
                        HardwareStatus::Ready(account)
                    }
                    Ok(_) => HardwareStatus::Failed(HardwareSourceError::Disconnected),
                    Err(error) => HardwareStatus::Failed(error),
                };
                let focus_id = if self.supported(&device_id).is_some() {
                    self.read_action_id(&device_id)
                } else {
                    self.action_id(ACCOUNT_INPUT_ID)
                };
                (focus_if_unfocused(focus_id), true)
            }
        }
    }

    fn apply_device_update(
        &mut self,
        message: HardwareWalletMessage,
        action_scope: u64,
    ) -> (Task<HardwareMessage>, bool) {
        if action_scope != self.action_scope {
            return (Task::none(), false);
        }
        let Some(devices) = self.devices.as_mut() else {
            return (Task::none(), false);
        };
        let task = devices
            .update(message)
            .map(|task| task.map(move |message| HardwareMessage::Devices(message, action_scope)))
            .unwrap_or_else(|_| Task::none());
        let gone = self
            .selected_device()
            .map(str::to_string)
            .is_some_and(|id| self.supported(&id).is_none());
        if gone {
            self.clear();
            self.status = HardwareStatus::Failed(HardwareSourceError::Disconnected);
        }
        (
            if gone {
                Task::batch([task, focus_if_unfocused(self.action_id(ACCOUNT_INPUT_ID))])
            } else {
                task
            },
            gone,
        )
    }

    #[cfg(test)]
    pub(crate) fn generation_for_test(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub(crate) fn action_scope_for_test(&self) -> u64 {
        self.action_scope
    }
}

pub fn view(source: &HardwareSource) -> Element<'_, HardwareMessage> {
    let reading = matches!(source.status, HardwareStatus::Reading(_));
    let mut devices = Column::new().spacing(8);
    let list = source
        .devices
        .as_ref()
        .map(|devices| devices.list.as_slice())
        .unwrap_or_default();
    if list.is_empty() {
        devices = devices.push(
            p1_regular("Connect and unlock a hardware wallet.").style(theme::text::secondary),
        );
    }
    for hw in list {
        let row = match hw {
            HardwareWallet::Supported {
                id,
                kind,
                fingerprint,
                alias,
                ..
            } => Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(
                    p1_regular(format!(
                        "{kind} {fingerprint}{}",
                        alias
                            .as_ref()
                            .map(|alias| format!(" ({alias})"))
                            .unwrap_or_default()
                    ))
                    .width(Length::Fill),
                )
                .push(
                    ActionButton::new(
                        source.read_action_id(id),
                        "Read account",
                        ActionKind::Secondary,
                        focus,
                    )
                    .compact()
                    .on_press_maybe((!reading).then(|| HardwareMessage::Read(id.clone()))),
                ),
            HardwareWallet::Locked {
                kind, pairing_code, ..
            } => Row::new().push(p1_regular(match pairing_code {
                Some(code) => format!("{kind}: confirm pairing code {code} on the device"),
                None => format!("{kind}: unlock the device"),
            })),
            HardwareWallet::Unsupported { kind, .. } => Row::new().push(
                p1_regular(format!("{kind}: not supported for Split"))
                    .style(theme::text::secondary),
            ),
        };
        devices = devices.push(row);
    }

    let status: Element<HardwareMessage> = match &source.status {
        HardwareStatus::Idle => caption("Reading shares only this account's public key. The device is never asked to sign, and nothing is saved.")
            .style(theme::text::secondary)
            .into(),
        HardwareStatus::Reading(_) => p1_regular("Reading the account from the device…")
            .style(theme::text::secondary)
            .into(),
        HardwareStatus::Ready(account) => p1_bold(format!(
            "Ready to scan account {} of wallet {} ({}).",
            account.account, account.fingerprint, account.purpose
        ))
        .into(),
        HardwareStatus::Failed(error) => p1_regular(error_copy(*error))
            .style(theme::text::warning)
            .into(),
    };

    Column::new()
        .spacing(12)
        .push(p1_bold("Address type"))
        .push(
            PURPOSES
                .iter()
                .copied()
                .fold(Column::new().spacing(6), |choices, purpose| {
                    choices.push(
                        ActionButton::new(
                            source.action_id(purpose_action_id(purpose)),
                            format!(
                                "{}{}",
                                if purpose == source.purpose {
                                    "Selected: "
                                } else {
                                    ""
                                },
                                purpose
                            ),
                            ActionKind::Secondary,
                            focus,
                        )
                        .compact()
                        .on_press(HardwareMessage::PurposeSelected(purpose)),
                    )
                }),
        )
        .push(p1_bold("Account"))
        .push(
            text_input("0", &source.account)
                .id(source.action_id(ACCOUNT_INPUT_ID))
                .on_input(HardwareMessage::AccountEdited)
                .padding(10),
        )
        .push(p1_bold("Hardware wallet"))
        .push(devices)
        .push(status)
        .into()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::{bip32::Xpriv, secp256k1::Secp256k1, Psbt};
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    const SEED: [u8; 32] = [11; 32];

    fn master(network: Network) -> Xpriv {
        Xpriv::new_master(network, &SEED).unwrap()
    }

    fn xpub_at(network: Network, path: &DerivationPath) -> Xpub {
        let secp = Secp256k1::new();
        Xpub::from_priv(&secp, &master(network).derive_priv(&secp, path).unwrap())
    }

    fn fingerprint() -> Fingerprint {
        master(Network::Bitcoin).fingerprint(&Secp256k1::new())
    }

    #[derive(Debug)]
    struct FakeDevice {
        fingerprint: Fingerprint,
        network: Network,
        path_override: Option<DerivationPath>,
    }

    #[async_trait::async_trait]
    impl HWI for FakeDevice {
        fn device_kind(&self) -> async_hwi::DeviceKind {
            async_hwi::DeviceKind::Specter
        }
        async fn get_version(&self) -> Result<async_hwi::Version, async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
        async fn get_master_fingerprint(&self) -> Result<Fingerprint, async_hwi::Error> {
            Ok(self.fingerprint)
        }
        async fn get_extended_pubkey(
            &self,
            path: &DerivationPath,
        ) -> Result<Xpub, async_hwi::Error> {
            Ok(xpub_at(
                self.network,
                self.path_override.as_ref().unwrap_or(path),
            ))
        }
        async fn register_wallet(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<[u8; 32]>, async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
        async fn is_wallet_registered(&self, _: &str, _: &str) -> Result<bool, async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
        async fn display_address(
            &self,
            _: &async_hwi::AddressScript,
        ) -> Result<(), async_hwi::Error> {
            Err(async_hwi::Error::UnimplementedMethod)
        }
        async fn sign_tx(&self, _: &mut Psbt) -> Result<(), async_hwi::Error> {
            panic!("the Split hardware source must never ask a device to sign");
        }
    }

    fn fake(network: Network, path_override: Option<&str>) -> Arc<dyn HWI + Send + Sync> {
        Arc::new(FakeDevice {
            fingerprint: fingerprint(),
            network,
            path_override: path_override.map(|p| p.parse().unwrap()),
        })
    }

    /// A fresh, empty directory. Nanoseconds alone collided when tests ran
    /// in parallel ("File exists"), so the name also carries the process id
    /// and a per-process counter.
    pub(crate) fn unique_temp_root(prefix: &str) -> std::path::PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}-{unique}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        root
    }

    fn temp_root() -> std::path::PathBuf {
        unique_temp_root("coincube-split-hw")
    }

    fn source_with(device: Arc<dyn HWI + Send + Sync>, root: &std::path::Path) -> HardwareSource {
        let mut source = HardwareSource::default();
        add_device(&mut source, device, root);
        source
    }

    pub(crate) fn loaded_for_test(
        generation: u64,
        succeeds: bool,
        action_scope: u64,
    ) -> HardwareMessage {
        let result = if succeeds {
            iced::futures::executor::block_on(read_account(
                fake(Network::Bitcoin, None),
                "dev-1".into(),
                fingerprint(),
                Purpose::Bip84,
                0,
            ))
            .map(Arc::new)
        } else {
            Err(HardwareSourceError::Device)
        };
        HardwareMessage::Loaded(result, generation, action_scope)
    }

    pub(crate) fn assert_focus_task<M: 'static>(
        task: Task<M>,
        element: Element<'_, M>,
        expected: Id,
    ) {
        assert_focus_task_outcome(task, element, None, Some(expected));
    }

    pub(crate) fn assert_focus_task_from<M: 'static>(
        task: Task<M>,
        element: Element<'_, M>,
        initially_focused: Option<Id>,
        expected: Id,
    ) {
        assert_focus_task_outcome(task, element, initially_focused, Some(expected));
    }

    pub(crate) fn assert_focus_task_outcome<M: 'static>(
        task: Task<M>,
        mut element: Element<'_, M>,
        initially_focused: Option<Id>,
        expected: Option<Id>,
    ) {
        use iced::advanced::{
            layout,
            renderer::Headless,
            widget::{operation, operation::focusable, Operation, Tree},
            Layout,
        };
        use iced_runtime::futures::futures::StreamExt;

        for bytes in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, iced::Size::new(760.0, 3000.0)),
        );
        let has_initial_focus = initially_focused.is_some();
        if let Some(id) = initially_focused {
            let mut focus: Box<dyn Operation<()>> = Box::new(focusable::focus(id));
            element.as_widget_mut().operate(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &mut operation::black_box(focus.as_mut()),
            );
        }
        let actions = iced_runtime::task::into_stream(task)
            .map(|stream| iced::futures::executor::block_on(stream.collect::<Vec<_>>()))
            .unwrap_or_default();
        let mut applied = false;
        for action in actions {
            if let iced_runtime::Action::Widget(mut operation) = action {
                applied = true;
                loop {
                    element.as_widget_mut().operate(
                        &mut tree,
                        Layout::new(&node),
                        &renderer,
                        &mut operation::black_box(operation.as_mut()),
                    );
                    match operation.finish() {
                        operation::Outcome::Chain(next) => operation = next,
                        operation::Outcome::None | operation::Outcome::Some(()) => break,
                    }
                }
            }
        }
        assert!(
            applied || has_initial_focus || expected.is_none(),
            "expected a mounted focus operation"
        );
        let mut find: Box<dyn Operation<Id>> = Box::new(focusable::find_focused());
        let found = loop {
            element.as_widget_mut().operate(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &mut operation::black_box(find.as_mut()),
            );
            match find.finish() {
                operation::Outcome::None => break None,
                operation::Outcome::Some(id) => break Some(id),
                operation::Outcome::Chain(next) => find = next,
            }
        };
        assert_eq!(found, expected);
    }

    #[test]
    fn hardware_source_actions_are_keyboard_operable_without_changing_read_semantics() {
        use crate::app::view::shared::landing::capture;
        use iced::keyboard::key::Named;

        let root = temp_root();
        let mut source = source_with(fake(Network::Bitcoin, None), &root);
        let size = (1024, 768);
        let mounted = capture::inspect_full_window(
            "split-hardware-keyboard",
            view(&source),
            size,
            "light",
            theme::Theme::light(),
        );
        let read_id = source.read_action_id("dev-1");
        assert!(mounted.has_focus(read_id.clone()));
        for id in [
            "tenshu.hardware.purpose.bip84",
            "tenshu.hardware.purpose.bip49",
            "tenshu.hardware.purpose.bip44",
            "tenshu.hardware.account",
        ] {
            assert!(
                mounted.has_focus(source.action_id(id)),
                "missing focus: {}",
                id
            );
        }
        for key in [Named::Enter, Named::Space] {
            assert!(matches!(
                capture::activate_with_key(view(&source), read_id.clone(), size, false, key).as_slice(),
                [HardwareMessage::Read(id)] if id == "dev-1"
            ));
            assert!(matches!(
                capture::activate_with_key(
                    view(&source),
                    source.action_id("tenshu.hardware.purpose.bip44"),
                    size,
                    false,
                    key
                )
                .as_slice(),
                [HardwareMessage::PurposeSelected(Purpose::Bip44)]
            ));
        }
        let (_, invalidated) = source.update(HardwareMessage::Focus(read_id));
        assert!(!invalidated, "focus must not clear account material");
        let _ = fs::remove_dir(root);
    }

    #[test]
    fn read_account_focus_returns_after_success_failure_and_disconnect() {
        use crate::app::view::shared::landing::capture;

        let root = temp_root();
        for outcome in ["success", "error", "disconnected"] {
            let mut source = source_with(fake(Network::Bitcoin, None), &root);
            let read_id = source.read_action_id("dev-1");
            let (reading_task, invalidated) = source.update(HardwareMessage::Read("dev-1".into()));
            assert!(invalidated);
            let reading = capture::inspect_full_window(
                &format!("split-hardware-reading-{outcome}"),
                view(&source),
                (1024, 768),
                "dark",
                theme::Theme::dark(),
            );
            assert!(
                !reading.has_focus(read_id.clone()),
                "Read is disabled while reading"
            );
            assert_focus_task(
                reading_task,
                view(&source),
                source.action_id(purpose_action_id(Purpose::Bip84)),
            );
            let generation = source.generation_for_test();
            if outcome == "disconnected" {
                source.devices.as_mut().unwrap().list.clear();
            }
            let (loaded_task, invalidated) = source.update(loaded_for_test(
                generation,
                outcome == "success" || outcome == "disconnected",
                source.action_scope,
            ));
            assert!(invalidated);
            let expected = if outcome == "disconnected" {
                source.action_id(ACCOUNT_INPUT_ID)
            } else {
                read_id.clone()
            };
            assert_focus_task(loaded_task, view(&source), expected);
            assert_eq!(source.account().is_some(), outcome == "success");
        }
        let _ = fs::remove_dir(root);
    }

    #[test]
    fn loaded_focus_preserves_another_tab_and_stale_result_is_inert() {
        let root = temp_root();
        let mut source = source_with(fake(Network::Bitcoin, None), &root);
        let _ = source.update(HardwareMessage::Read("dev-1".into()));
        let generation = source.generation_for_test();
        let (loaded, _) = source.update(loaded_for_test(generation, true, source.action_scope));
        let other = Id::new("other-tab.pin");
        assert_focus_task_from(
            loaded,
            iced::widget::text_input("PIN", "")
                .id(other.clone())
                .on_input(HardwareMessage::AccountEdited)
                .into(),
            Some(other.clone()),
            other.clone(),
        );
        let mut mounted_source = source_with(fake(Network::Bitcoin, None), &root);
        let _ = mounted_source.update(HardwareMessage::Read("dev-1".into()));
        let mounted_generation = mounted_source.generation_for_test();
        let (mounted_loaded, _) = mounted_source.update(loaded_for_test(
            mounted_generation,
            true,
            mounted_source.action_scope,
        ));
        let purpose = mounted_source.action_id(purpose_action_id(Purpose::Bip84));
        assert_focus_task_from(
            mounted_loaded,
            view(&mounted_source),
            Some(purpose.clone()),
            purpose,
        );
        let (stale, invalidated) =
            source.update(loaded_for_test(generation, false, source.action_scope));
        assert!(!invalidated);
        assert_focus_task_from(
            stale,
            iced::widget::text_input("PIN", "")
                .id(other.clone())
                .on_input(HardwareMessage::AccountEdited)
                .into(),
            Some(other.clone()),
            other,
        );
        let _ = fs::remove_dir(root);
    }

    #[test]
    fn choosing_current_purpose_keeps_ready_account() {
        let root = temp_root();
        let mut source = source_with(fake(Network::Bitcoin, None), &root);
        let _ = source.update(HardwareMessage::Read("dev-1".into()));
        let generation = source.generation_for_test();
        let _ = source.update(loaded_for_test(generation, true, source.action_scope));
        assert!(source.account().is_some());
        let (_, invalidated) = source.update(HardwareMessage::PurposeSelected(Purpose::Bip84));
        assert!(!invalidated);
        assert!(source.account().is_some());
        let (_, invalidated) = source.update(HardwareMessage::PurposeSelected(Purpose::Bip49));
        assert!(invalidated);
        assert!(source.account().is_none());
        let _ = fs::remove_dir(root);
    }

    #[test]
    fn device_poll_from_replaced_source_cannot_disconnect_current_account() {
        let root = temp_root();
        let old = source_with(fake(Network::Bitcoin, None), &root);
        let mut current = source_with(fake(Network::Bitcoin, None), &root);
        let _ = current.update(HardwareMessage::Read("dev-1".into()));
        let generation = current.generation;
        let _ = current.update(loaded_for_test(generation, true, current.action_scope));
        assert!(current.account().is_some());
        current.devices.as_mut().unwrap().list.clear();
        let (task, invalidated) = current.update(HardwareMessage::Devices(
            HardwareWalletMessage::Error("old poll".into()),
            old.action_scope,
        ));
        assert!(!invalidated);
        assert_eq!(task.units(), 0);
        assert!(current.account().is_some());
        let _ = fs::remove_dir(root);
    }

    /// Lists a supported fake device as `dev-1` so a `Read` would start a task.
    pub(crate) fn add_fake_device(source: &mut HardwareSource, root: &std::path::Path) {
        add_device(source, fake(Network::Bitcoin, None), root);
    }

    fn add_device(
        source: &mut HardwareSource,
        device: Arc<dyn HWI + Send + Sync>,
        root: &std::path::Path,
    ) {
        source.set_root(CoincubeDirectory::new(root.to_path_buf()));
        source
            .devices
            .as_mut()
            .unwrap()
            .list
            .push(HardwareWallet::Supported {
                id: "dev-1".into(),
                device,
                kind: async_hwi::DeviceKind::Specter,
                fingerprint: fingerprint(),
                version: None,
                registered: None,
                alias: None,
            });
    }

    #[test]
    fn split_hardware_purpose_maps_to_standard_path_and_descriptor() {
        for (purpose, path, prefix) in [
            (Purpose::Bip84, "m/84'/0'/2'", "wpkh("),
            (Purpose::Bip49, "m/49'/0'/2'", "sh(wpkh("),
            (Purpose::Bip44, "m/44'/0'/2'", "pkh("),
        ] {
            assert_eq!(purpose.path(2).unwrap(), path.parse().unwrap());
            let xpub = xpub_at(Network::Bitcoin, &purpose.path(2).unwrap());
            let account =
                account_from_device("dev".into(), purpose, 2, fingerprint(), fingerprint(), xpub)
                    .unwrap();
            let origin = format!("[{}/{}]", fingerprint(), purpose.path(2).unwrap());
            let has_origin =
                |d: &str| d.contains(&origin) || d.contains(&origin.replace('\'', "h"));
            let external = account.external.canonical();
            let internal = account.internal.canonical();
            assert!(external.starts_with(prefix), "{}", external);
            assert!(has_origin(&external), "{}", external);
            assert!(external.contains("/0/*"), "{}", external);
            assert!(has_origin(&internal) && internal.contains("/1/*"));
        }
        // Taproot is not offered.
        assert_eq!(PURPOSES.len(), 3);
        assert!(PURPOSES.iter().all(|p| p.number() != 86));
    }

    #[test]
    fn split_hardware_refuses_wrong_network_depth_and_fingerprint() {
        let path = Purpose::Bip84.path(0).unwrap();
        let good = xpub_at(Network::Bitcoin, &path);
        let check = |listed, reported, xpub| {
            account_from_device("dev".into(), Purpose::Bip84, 0, listed, reported, xpub)
                .map(|_| ())
                .unwrap_err()
        };
        assert_eq!(
            check(
                fingerprint(),
                fingerprint(),
                xpub_at(Network::Testnet, &path)
            ),
            HardwareSourceError::Network
        );
        for wrong in ["m/84'/0'", "m/84'/0'/1'", "m/84'/0'/0'/0"] {
            assert_eq!(
                check(
                    fingerprint(),
                    fingerprint(),
                    xpub_at(Network::Bitcoin, &wrong.parse().unwrap())
                ),
                HardwareSourceError::Depth,
                "{wrong}"
            );
        }
        let other = Fingerprint::from([1, 2, 3, 4]);
        assert_eq!(
            check(other, fingerprint(), good),
            HardwareSourceError::Fingerprint
        );
        let mut relabeled = good;
        relabeled.parent_fingerprint = Fingerprint::default();
        assert_eq!(
            check(fingerprint(), fingerprint(), relabeled),
            HardwareSourceError::ParentFingerprint
        );
        relabeled.parent_fingerprint = fingerprint();
        assert_eq!(
            check(fingerprint(), fingerprint(), relabeled),
            HardwareSourceError::ParentFingerprint
        );
        assert_eq!(
            parse_account("2147483648"),
            Err(HardwareSourceError::Account)
        );
        assert_eq!(parse_account("-1"), Err(HardwareSourceError::Account));
        assert_eq!(parse_account(" 7 "), Ok(7));
    }

    #[tokio::test]
    async fn split_hardware_read_refuses_swapped_device_and_wrong_path() {
        let listed = Fingerprint::from([9, 9, 9, 9]);
        assert_eq!(
            read_account(
                fake(Network::Bitcoin, None),
                "d".into(),
                listed,
                Purpose::Bip84,
                0
            )
            .await
            .unwrap_err(),
            HardwareSourceError::Fingerprint
        );
        assert_eq!(
            read_account(
                fake(Network::Bitcoin, Some("m/84'/0'/5'")),
                "d".into(),
                fingerprint(),
                Purpose::Bip84,
                0
            )
            .await
            .unwrap_err(),
            HardwareSourceError::Depth
        );
        let account = read_account(
            fake(Network::Bitcoin, None),
            "d".into(),
            fingerprint(),
            Purpose::Bip49,
            3,
        )
        .await
        .unwrap();
        assert_eq!((account.account, account.fingerprint), (3, fingerprint()));
        // Redacted: no key material in Debug output.
        let debug = format!("{account:?}");
        assert!(
            !debug.contains("xpub") && !debug.contains("wpkh"),
            "{}",
            debug
        );
    }

    fn read(source: &mut HardwareSource) -> u64 {
        let _ = source.update(HardwareMessage::Read("dev-1".into()));
        assert!(matches!(source.status(), HardwareStatus::Reading(_)));
        source.generation
    }

    async fn loaded(generation: u64, action_scope: u64) -> HardwareMessage {
        let account = read_account(
            fake(Network::Bitcoin, None),
            "dev-1".into(),
            fingerprint(),
            Purpose::Bip84,
            0,
        )
        .await
        .map(Arc::new);
        HardwareMessage::Loaded(account, generation, action_scope)
    }

    /// T2: a result from a read superseded by an edit and a second read must
    /// not satisfy the second read.
    #[tokio::test]
    async fn split_hardware_stale_result_does_not_satisfy_a_newer_read() {
        let root = temp_root();
        let mut source = source_with(fake(Network::Bitcoin, None), &root);
        let scope = source.action_scope;
        let first = read(&mut source);
        let _ = source.update(HardwareMessage::AccountEdited("1".into()));
        let second = read(&mut source);
        assert_ne!(first, second);

        let (_, invalidated) = source.update(loaded(first, scope).await);
        assert!(!invalidated);
        assert!(source.account().is_none());
        assert!(matches!(source.status(), HardwareStatus::Reading(_)));

        let _ = source.update(loaded(second, scope).await);
        assert!(source.account().is_some());
        fs::remove_dir(root).unwrap();
    }

    #[tokio::test]
    async fn split_hardware_cancel_disconnect_and_edits_clear_state_without_datadir_writes() {
        let root = temp_root();
        let mut source = source_with(fake(Network::Bitcoin, None), &root);
        let scope = source.action_scope;

        let generation = read(&mut source);
        let (_, invalidated) = source.update(loaded(generation, scope).await);
        assert!(invalidated && source.account().is_some());

        // Cancel clears; a late result for the cleared generation is dropped.
        source.clear();
        assert!(source.account().is_none());
        let (_, invalidated) = source.update(loaded(generation, scope).await);
        assert!(!invalidated && source.account().is_none());

        // Changing the account type or number clears the read account.
        let generation = read(&mut source);
        let _ = source.update(loaded(generation, scope).await);
        let (_, invalidated) = source.update(HardwareMessage::PurposeSelected(Purpose::Bip44));
        assert!(invalidated && source.account().is_none());
        let generation = read(&mut source);
        let _ = source.update(loaded(generation, scope).await);
        let (_, invalidated) = source.update(HardwareMessage::AccountEdited("1".into()));
        assert!(invalidated && source.account().is_none());

        // Disconnect: the device vanishes from the list.
        let generation = read(&mut source);
        let _ = source.update(loaded(generation, scope).await);
        assert!(source.account().is_some());
        source.devices.as_mut().unwrap().list.clear();
        let (_, invalidated) = source.update(HardwareMessage::Devices(
            HardwareWalletMessage::Error("unplugged".into()),
            source.action_scope,
        ));
        assert!(invalidated);
        assert!(source.account().is_none());
        assert!(matches!(
            source.status(),
            HardwareStatus::Failed(HardwareSourceError::Disconnected)
        ));

        // A result for a device that disconnected mid-read is not accepted.
        let mut source = source_with(fake(Network::Bitcoin, None), &root);
        let scope = source.action_scope;
        let generation = read(&mut source);
        source.devices.as_mut().unwrap().list.clear();
        let _ = source.update(loaded(generation, scope).await);
        assert!(source.account().is_none());

        assert!(!source.devices.as_ref().unwrap().persists_pairing());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir(root).unwrap();
    }
}
