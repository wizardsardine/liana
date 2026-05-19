//! Renders the installer's "select key source" and "edit key alias"
//! modals (`installer::SelectKeySource` and `installer::EditKeyAlias`)
//! in their `Step::Select` initial state, with assorted public-API setups.
//!
//! Each page constructs the modal via its public `new()` constructor,
//! together with a mocked [`HardwareWallets`], then calls the production
//! `DescriptorEditModal::view` to render. Visual states reachable only by
//! mutating private fields (`Step::Details`, in-flight processing,
//! errors, internal export modal) are out of scope — see the discussion
//! on the parent task. To exercise more of those, expose `pub(crate)`
//! test hooks on `SelectKeySource`.
//!
//! `HardwareWallet::Supported` requires an `Arc<dyn HWI + Send + Sync>`;
//! we satisfy it with a [`MockHwi`] whose async methods return
//! `UnimplementedMethod` — never invoked in the rendering path.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

use async_hwi::{AddressScript, DeviceKind, Error as HwiError, Version, HWI};
use async_trait::async_trait;
use liana::miniscript::{
    bitcoin::{
        bip32::{DerivationPath, Fingerprint, Xpub},
        psbt::Psbt,
        Network,
    },
    descriptor::DescriptorPublicKey,
};
use liana_connect::keys::api::KeyKind;

use liana_ui::widget::*;

use crate::{
    debug::{installer_with_modal, DebugMessage, DebugPageEntry},
    dir::LianaDirectory,
    hw::{HardwareWallet, HardwareWallets, UnsupportedReason},
    installer::{
        DescriptorEditModal, EditKeyAlias, Key, KeySource, Message as InstallerMessage, PathData,
        PathKind, SelectKeySource, SelectKeySourceMessage,
    },
    signer::Signer,
};

pub static ENTRY_EMPTY: DebugPageEntry = DebugPageEntry { view: empty_view };
pub static ENTRY_OPTIONS_OPEN: DebugPageEntry = DebugPageEntry {
    view: options_open_view,
};
pub static ENTRY_WITH_HWS: DebugPageEntry = DebugPageEntry {
    view: with_hws_view,
};
pub static ENTRY_TAPROOT_PATH: DebugPageEntry = DebugPageEntry {
    view: taproot_path_view,
};
pub static ENTRY_SAFETY_NET: DebugPageEntry = DebugPageEntry {
    view: safety_net_view,
};
pub static ENTRY_EDIT_ALIAS: DebugPageEntry = DebugPageEntry {
    view: edit_alias_view,
};

/// Sample xpub used to construct mock `Key` values. The xpub itself is
/// inert — nothing in the rendering path validates it on-chain.
const SAMPLE_XPUB: &str = "[f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<0;1>/*";

/// SAFETY: iced renders on the main thread; debug-overlay state is only
/// read during rendering, so satisfying `OnceLock`'s `Sync` bound with an
/// unconditional `unsafe impl Sync` is sound here.
struct StateCell<T>(T);
unsafe impl<T> Sync for StateCell<T> {}

/// Stand-in `HWI` implementation. None of the async methods are reached
/// by `hw_list_view*` rendering — they exist only so we can construct
/// `Arc<dyn HWI + Send + Sync>`.
#[derive(Debug)]
struct MockHwi(DeviceKind);

#[async_trait]
impl HWI for MockHwi {
    fn device_kind(&self) -> DeviceKind {
        self.0
    }
    async fn get_version(&self) -> Result<Version, HwiError> {
        Err(HwiError::UnimplementedMethod)
    }
    async fn get_master_fingerprint(&self) -> Result<Fingerprint, HwiError> {
        Err(HwiError::UnimplementedMethod)
    }
    async fn get_extended_pubkey(&self, _path: &DerivationPath) -> Result<Xpub, HwiError> {
        Err(HwiError::UnimplementedMethod)
    }
    async fn register_wallet(
        &self,
        _name: &str,
        _policy: &str,
    ) -> Result<Option<[u8; 32]>, HwiError> {
        Err(HwiError::UnimplementedMethod)
    }
    async fn is_wallet_registered(&self, _name: &str, _policy: &str) -> Result<bool, HwiError> {
        Err(HwiError::UnimplementedMethod)
    }
    async fn display_address(&self, _script: &AddressScript) -> Result<(), HwiError> {
        Err(HwiError::UnimplementedMethod)
    }
    async fn sign_tx(&self, _tx: &mut Psbt) -> Result<(), HwiError> {
        Err(HwiError::UnimplementedMethod)
    }
}

fn fp(b: u8) -> Fingerprint {
    Fingerprint::from([b; 4])
}

fn ver(major: u32, minor: u32, patch: u32) -> Version {
    Version {
        major,
        minor,
        patch,
        prerelease: None,
    }
}

fn supported_hw(
    kind: DeviceKind,
    version: Option<Version>,
    fingerprint: Fingerprint,
    alias: Option<&'static str>,
) -> HardwareWallet {
    HardwareWallet::Supported {
        id: format!("dbg-{kind:?}-{fingerprint}"),
        device: Arc::new(MockHwi(kind)),
        kind,
        fingerprint,
        version,
        registered: None,
        alias: alias.map(String::from),
    }
}

fn unsupported_hw(
    kind: DeviceKind,
    version: Option<Version>,
    reason: UnsupportedReason,
) -> HardwareWallet {
    HardwareWallet::Unsupported {
        id: format!("dbg-unsup-{kind:?}"),
        kind,
        version,
        reason,
    }
}

fn locked_hw(kind: DeviceKind, pairing_code: Option<&'static str>) -> HardwareWallet {
    HardwareWallet::Locked {
        id: format!("dbg-lock-{kind:?}"),
        device: Arc::new(Mutex::new(None)),
        pairing_code: pairing_code.map(String::from),
        kind,
    }
}

fn empty_hws() -> HardwareWallets {
    HardwareWallets::new(
        LianaDirectory::new(std::path::PathBuf::new()),
        Network::Bitcoin,
    )
}

fn hws_with(list: Vec<HardwareWallet>) -> HardwareWallets {
    let mut hws = empty_hws();
    hws.list = list;
    hws
}

fn fresh_signer() -> Arc<Mutex<Signer>> {
    Arc::new(Mutex::new(
        Signer::generate(Network::Bitcoin).expect("hot signer generation"),
    ))
}

/// A primary path with one slot to fill at coordinates `(0, 0)`.
fn primary_path() -> PathData {
    PathData {
        coordinates: vec![(0, 0)],
        keys: vec![],
        token_kind: vec![],
    }
}

/// A safety-net path with one slot, restricted to `KeyKind::SafetyNet`
/// tokens — this is what unlocks the safety-net token entry in the
/// "other options" section of `SelectKeySource`.
fn safety_net_path() -> PathData {
    PathData {
        coordinates: vec![(0, 0)],
        keys: vec![],
        token_kind: vec![KeyKind::SafetyNet],
    }
}

fn empty_state() -> StateCell<(SelectKeySource, HardwareWallets)> {
    let modal = SelectKeySource::new(
        Network::Bitcoin,
        false,
        primary_path(),
        HashMap::new(),
        HashMap::new(),
        fresh_signer(),
    );
    StateCell((modal, empty_hws()))
}

fn empty_view() -> Element<'static, DebugMessage> {
    static STATE: OnceLock<StateCell<(SelectKeySource, HardwareWallets)>> = OnceLock::new();
    let s = STATE.get_or_init(empty_state);
    let body = s.0 .0.view(&s.0 .1).map(|_| ());
    installer_with_modal(
        "Select key source — empty primary path",
        "liana_gui::installer::step::descriptor::editor::key::SelectKeySource::view",
        body,
    )
}

/// Same setup as [`empty_state`], but driven through the production
/// `update()` path with a `Collapse(true)` message so the "Other options"
/// section is expanded — surfacing the load-key / paste-xpub / generate
/// hot-key entries that are otherwise hidden behind the collapsible
/// header.
fn options_open_state() -> StateCell<(SelectKeySource, HardwareWallets)> {
    let mut modal = SelectKeySource::new(
        Network::Bitcoin,
        false,
        primary_path(),
        HashMap::new(),
        HashMap::new(),
        fresh_signer(),
    );
    let mut hws = empty_hws();
    let _ = modal.update(
        &mut hws,
        InstallerMessage::SelectKeySource(SelectKeySourceMessage::Collapse(true)),
    );
    StateCell((modal, hws))
}

fn options_open_view() -> Element<'static, DebugMessage> {
    static STATE: OnceLock<StateCell<(SelectKeySource, HardwareWallets)>> = OnceLock::new();
    let s = STATE.get_or_init(options_open_state);
    let body = s.0 .0.view(&s.0 .1).map(|_| ());
    installer_with_modal(
        "Select key source — no devices, options open",
        "liana_gui::installer::step::descriptor::editor::key::SelectKeySource::view",
        body,
    )
}

fn with_hws_state() -> StateCell<(SelectKeySource, HardwareWallets)> {
    // Cover every visual branch reachable from `widget_signing_device` with
    // `taproot = false`: a clickable Supported row, both Locked sub-cases
    // (with / without pairing code), and every `UnsupportedReason`.
    let hws = hws_with(vec![
        supported_hw(
            DeviceKind::Ledger,
            Some(ver(2, 1, 0)),
            fp(0xAA),
            Some("Vault key"),
        ),
        locked_hw(DeviceKind::Jade, Some("123-456")),
        locked_hw(DeviceKind::BitBox02, None),
        unsupported_hw(
            DeviceKind::Coldcard,
            Some(ver(5, 1, 0)),
            UnsupportedReason::Version {
                minimal_supported_version: "6.0.0".to_string(),
            },
        ),
        unsupported_hw(
            DeviceKind::Specter,
            Some(ver(2, 0, 0)),
            UnsupportedReason::Method("display_address"),
        ),
        unsupported_hw(
            DeviceKind::Ledger,
            Some(ver(2, 1, 0)),
            UnsupportedReason::WrongNetwork,
        ),
        unsupported_hw(
            DeviceKind::BitBox02,
            Some(ver(9, 13, 0)),
            UnsupportedReason::AppIsNotOpen,
        ),
        unsupported_hw(
            DeviceKind::Coldcard,
            None,
            UnsupportedReason::NotPartOfWallet(fp(0xFF)),
        ),
    ]);
    let modal = SelectKeySource::new(
        Network::Bitcoin,
        false,
        primary_path(),
        HashMap::new(),
        HashMap::new(),
        fresh_signer(),
    );
    StateCell((modal, hws))
}

fn with_hws_view() -> Element<'static, DebugMessage> {
    static STATE: OnceLock<StateCell<(SelectKeySource, HardwareWallets)>> = OnceLock::new();
    let s = STATE.get_or_init(with_hws_state);
    let body = s.0 .0.view(&s.0 .1).map(|_| ());
    installer_with_modal(
        "Select key source — with detected devices",
        "liana_gui::installer::step::descriptor::editor::key::SelectKeySource::view",
        body,
    )
}

/// Detected devices for a taproot-required path (`taproot = true`). Each
/// supported device here is below its tap-miniscript minimum (or absent
/// from `DEVICES_COMPATIBLE_WITH_TAPMINISCRIPT` entirely, like Jade), so
/// `widget_signing_device`'s `(_, false, true)` arm fires and shows
/// "This device doesn't support taproot miniscript". One Specter row is
/// included as a control — Specter has no minimum, so it stays
/// selectable.
fn taproot_path_state() -> StateCell<(SelectKeySource, HardwareWallets)> {
    let hws = hws_with(vec![
        supported_hw(
            DeviceKind::Ledger,
            Some(ver(2, 1, 0)), // < 2.2.0
            fp(0xAA),
            Some("Old Ledger"),
        ),
        supported_hw(
            DeviceKind::Coldcard,
            Some(ver(5, 0, 0)), // < 6.3.3
            fp(0xBB),
            Some("Old Coldcard"),
        ),
        supported_hw(
            DeviceKind::BitBox02,
            Some(ver(9, 13, 0)), // < 9.21.0
            fp(0xCC),
            Some("Old BitBox02"),
        ),
        supported_hw(
            DeviceKind::Jade,
            Some(ver(1, 0, 24)), // Jade not in the table at all
            fp(0xDD),
            Some("Jade"),
        ),
        supported_hw(
            DeviceKind::Specter,
            Some(ver(2, 0, 0)), // Specter has no minimum -> compatible
            fp(0xEE),
            Some("Specter"),
        ),
    ]);
    let modal = SelectKeySource::new(
        Network::Bitcoin,
        true, // taproot path
        primary_path(),
        HashMap::new(),
        HashMap::new(),
        fresh_signer(),
    );
    StateCell((modal, hws))
}

fn taproot_path_view() -> Element<'static, DebugMessage> {
    static STATE: OnceLock<StateCell<(SelectKeySource, HardwareWallets)>> = OnceLock::new();
    let s = STATE.get_or_init(taproot_path_state);
    let body = s.0 .0.view(&s.0 .1).map(|_| ());
    installer_with_modal(
        "Select key source — taproot path",
        "liana_gui::installer::step::descriptor::editor::key::SelectKeySource::view",
        body,
    )
}

fn safety_net_state() -> StateCell<(SelectKeySource, HardwareWallets)> {
    let modal = SelectKeySource::new(
        Network::Bitcoin,
        false,
        safety_net_path(),
        HashMap::new(),
        HashMap::new(),
        fresh_signer(),
    );
    StateCell((modal, empty_hws()))
}

fn safety_net_view() -> Element<'static, DebugMessage> {
    static STATE: OnceLock<StateCell<(SelectKeySource, HardwareWallets)>> = OnceLock::new();
    let s = STATE.get_or_init(safety_net_state);
    let body = s.0 .0.view(&s.0 .1).map(|_| ());
    installer_with_modal(
        "Select key source — safety-net path",
        "liana_gui::installer::step::descriptor::editor::key::SelectKeySource::view",
        body,
    )
}

// ---- Edit key alias --------------------------------------------------------

fn sample_key(name: &str, fingerprint: Fingerprint) -> Key {
    Key {
        source: KeySource::Manual,
        name: name.to_string(),
        fingerprint,
        key: DescriptorPublicKey::from_str(SAMPLE_XPUB).expect("sample xpub parses"),
        account: None,
    }
}

fn edit_alias_state() -> StateCell<(EditKeyAlias, HardwareWallets)> {
    // Two existing keys so the "alias already used" check has something
    // to bump against.
    let mut keys: HashMap<Fingerprint, (Vec<(usize, usize)>, Key)> = HashMap::new();
    let other_fp = fp(0xCC);
    keys.insert(other_fp, (vec![(0, 1)], sample_key("Backup key", other_fp)));
    let target_fp = fp(0xAA);
    keys.insert(
        target_fp,
        (vec![(0, 0)], sample_key("Vault key", target_fp)),
    );
    let modal = EditKeyAlias::new(
        keys,
        target_fp,
        "Vault key".to_string(),
        PathKind::Primary,
        vec![(0, 0)],
    );
    StateCell((modal, empty_hws()))
}

fn edit_alias_view() -> Element<'static, DebugMessage> {
    static STATE: OnceLock<StateCell<(EditKeyAlias, HardwareWallets)>> = OnceLock::new();
    let s = STATE.get_or_init(edit_alias_state);
    let body = s.0 .0.view(&s.0 .1).map(|_| ());
    installer_with_modal(
        "Edit key alias — default",
        "liana_gui::installer::step::descriptor::editor::key::EditKeyAlias::view",
        body,
    )
}
