//! Renders the three production HW-device modals (signing,
//! registration, verify-address) by calling the same view functions
//! the real app calls (`view::psbt::sign_action`,
//! `view::settings::register_wallet_modal`,
//! `view::receive::verify_address_modal`). One page per modal; each
//! constructs mock state (a list of `HardwareWallet`s plus the
//! `signed`/`signing`/`registered`/`chosen` `HashSet`s the production
//! builder expects) and hands it to the production fn.
//!
//! Each rendered modal body is overlaid on top of the production
//! dashboard chrome via [`liana_ui::widget::modal::Modal`]; the
//! dashboard's sidebar shows the menu under which the modal appears in
//! production. All clicks are swallowed at the debug-overlay boundary.
//!
//! `HardwareWallet::Supported` requires an `Arc<dyn HWI + Send + Sync>`;
//! we satisfy it with a [`MockHwi`] whose async methods all return
//! `UnimplementedMethod`: the rendering path never invokes them, only
//! the struct's data fields (`kind`, `version`, `fingerprint`, `alias`,
//! `registered`).

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

use async_hwi::{AddressScript, DeviceKind, Error as HwiError, Version, HWI};
use async_trait::async_trait;
use liana::descriptors::LianaDescriptor;
use liana::miniscript::bitcoin::{
    address::Address,
    bip32::{ChildNumber, DerivationPath, Fingerprint, Xpub},
    psbt::Psbt,
    Network,
};

use liana_ui::component::text::{h2, p1_regular};
use liana_ui::widget::{modal::Modal, *};

use crate::{
    app::{
        menu::Menu,
        view::{self, Message as ViewMessage},
    },
    debug::{static_cache, DebugMessage, DebugPageEntry, NAV_HINT},
    hw::{HardwareWallet, UnsupportedReason},
};

pub static ENTRY_SIGNING: DebugPageEntry = DebugPageEntry { view: signing_view };
pub static ENTRY_REGISTRATION: DebugPageEntry = DebugPageEntry {
    view: registration_view,
};
pub static ENTRY_VERIFY_ADDRESS: DebugPageEntry = DebugPageEntry {
    view: verify_address_view,
};

/// Stand-in `HWI` implementation for mocked `HardwareWallet::Supported`
/// values. Every async method returns `UnimplementedMethod`; nothing in
/// the rendering path actually invokes them.
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

pub(super) fn fp(b: u8) -> Fingerprint {
    Fingerprint::from([b; 4])
}

pub(super) fn ver(major: u32, minor: u32, patch: u32) -> Version {
    Version {
        major,
        minor,
        patch,
        prerelease: None,
    }
}

pub(super) fn supported(
    kind: DeviceKind,
    version: Option<Version>,
    fingerprint: Fingerprint,
    alias: Option<&'static str>,
    registered: Option<bool>,
) -> HardwareWallet {
    HardwareWallet::Supported {
        id: format!("dbg-{kind:?}-{fingerprint}"),
        device: Arc::new(MockHwi(kind)),
        kind,
        fingerprint,
        version,
        registered,
        alias: alias.map(String::from),
    }
}

pub(super) fn unsupported(
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

pub(super) fn locked(kind: DeviceKind, pairing_code: Option<&'static str>) -> HardwareWallet {
    HardwareWallet::Locked {
        id: format!("dbg-lock-{kind:?}"),
        device: Arc::new(Mutex::new(None)),
        pairing_code: pairing_code.map(String::from),
        kind,
    }
}

/// Sample Liana descriptor used to drive `sign_action`'s
/// `descriptor.contains_fingerprint_in_path` check. Fingerprint
/// `19608592` is reused as the first signing-mock's fp so that one row
/// exercises the `supported_device` (clickable) branch through
/// production logic.
const SAMPLE_DESCRIPTOR: &str = "wsh(or_d(pk([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*),and_v(v:pkh([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<2;3>/*),older(52596))))#x6u6lmej";

fn sample_descriptor() -> &'static LianaDescriptor {
    static D: OnceLock<LianaDescriptor> = OnceLock::new();
    D.get_or_init(|| {
        LianaDescriptor::from_str(SAMPLE_DESCRIPTOR).expect("sample descriptor parses")
    })
}

/// Wrap a debug page body in the production dashboard, then overlay a
/// modal body on top using [`liana_ui::widget::modal::Modal`]. Both base
/// and overlay are mapped through the production `Message` type so iced
/// can join them into one widget tree.
fn dashboard_with_modal<B, M>(
    menu: &'static Menu,
    title: &'static str,
    base_body: B,
    modal_body: M,
) -> Element<'static, DebugMessage>
where
    B: Into<Element<'static, ViewMessage>>,
    M: Into<Element<'static, ViewMessage>>,
{
    let dash_content: Column<'static, ViewMessage> = Column::new()
        .spacing(30)
        .push(h2(title))
        .push(p1_regular(NAV_HINT))
        .push(base_body);
    let dashboard_elem = view::dashboard(menu, static_cache(), None, dash_content);
    let elem: Element<'static, ViewMessage> = Modal::new(dashboard_elem, modal_body).into();
    elem.map(|_| ())
}

// ---- signing flow ----------------------------------------------------------

fn signing_hws() -> &'static [HardwareWallet] {
    static HWS: OnceLock<Vec<HardwareWallet>> = OnceLock::new();
    HWS.get_or_init(|| {
        vec![
            supported(
                DeviceKind::Ledger,
                Some(ver(2, 1, 0)),
                Fingerprint::from([0x19, 0x60, 0x85, 0x92]),
                Some("Vault key"),
                Some(true),
            ),
            supported(
                DeviceKind::BitBox02,
                Some(ver(9, 13, 0)),
                fp(0xBB),
                Some("Backup key"),
                Some(false),
            ),
            supported(
                DeviceKind::Coldcard,
                Some(ver(5, 1, 0)),
                fp(0xCC),
                Some("Cosigner"),
                Some(true),
            ),
            supported(
                DeviceKind::Jade,
                Some(ver(1, 0, 24)),
                fp(0xDD),
                Some("Jade"),
                Some(true),
            ),
            supported(
                DeviceKind::Specter,
                Some(ver(2, 0, 0)),
                fp(0xEE),
                Some("Specter"),
                Some(true),
            ),
            unsupported(
                DeviceKind::Ledger,
                Some(ver(1, 0, 0)),
                UnsupportedReason::Version {
                    minimal_supported_version: "2.0.0".to_string(),
                },
            ),
            unsupported(
                DeviceKind::BitBox02,
                Some(ver(9, 13, 0)),
                UnsupportedReason::WrongNetwork,
            ),
            unsupported(
                DeviceKind::Coldcard,
                None,
                UnsupportedReason::NotPartOfWallet(fp(0xFF)),
            ),
            unsupported(
                DeviceKind::Jade,
                Some(ver(1, 0, 0)),
                UnsupportedReason::AppIsNotOpen,
            ),
            locked(DeviceKind::BitBox02, Some("123-456")),
        ]
    })
}

fn signing_signed() -> &'static HashSet<Fingerprint> {
    static S: OnceLock<HashSet<Fingerprint>> = OnceLock::new();
    S.get_or_init(|| HashSet::from([fp(0xEE)]))
}

fn signing_signing() -> &'static HashSet<Fingerprint> {
    static S: OnceLock<HashSet<Fingerprint>> = OnceLock::new();
    S.get_or_init(|| HashSet::from([fp(0xDD)]))
}

fn signing_view() -> Element<'static, DebugMessage> {
    let body = view::psbt::sign_action(
        None,
        signing_hws(),
        sample_descriptor(),
        None,
        None,
        signing_signed(),
        signing_signing(),
        None,
    );
    dashboard_with_modal(
        &super::PSBTS_MENU,
        "HW modal:signing flow",
        p1_regular("(production: PSBT details visible behind the modal)"),
        body,
    )
}

// ---- registration flow -----------------------------------------------------

fn registration_hws() -> &'static [HardwareWallet] {
    static HWS: OnceLock<Vec<HardwareWallet>> = OnceLock::new();
    HWS.get_or_init(|| {
        vec![
            supported(
                DeviceKind::Ledger,
                Some(ver(2, 1, 0)),
                fp(0xAA),
                Some("Vault key"),
                None,
            ),
            supported(
                DeviceKind::BitBox02,
                Some(ver(9, 13, 0)),
                fp(0xBB),
                Some("Backup key"),
                None,
            ),
            supported(
                DeviceKind::Coldcard,
                Some(ver(5, 1, 0)),
                fp(0xCC),
                Some("Cosigner"),
                Some(true),
            ),
            unsupported(
                DeviceKind::Jade,
                Some(ver(1, 0, 0)),
                UnsupportedReason::WrongNetwork,
            ),
            locked(DeviceKind::BitBox02, Some("789-012")),
        ]
    })
}

fn registration_registered() -> &'static HashSet<Fingerprint> {
    static S: OnceLock<HashSet<Fingerprint>> = OnceLock::new();
    S.get_or_init(|| HashSet::from([fp(0xCC)]))
}

fn registration_view() -> Element<'static, DebugMessage> {
    let body = view::settings::register_wallet_modal(
        None,
        registration_hws(),
        false,
        None,
        registration_registered(),
    );
    dashboard_with_modal(
        &super::SETTINGS_MENU,
        "HW modal:registration flow",
        p1_regular("(production: settings page visible behind the modal)"),
        body,
    )
}

// ---- verify-address flow ---------------------------------------------------

fn verify_address_hws() -> &'static [HardwareWallet] {
    static HWS: OnceLock<Vec<HardwareWallet>> = OnceLock::new();
    HWS.get_or_init(|| {
        vec![
            supported(
                DeviceKind::Ledger,
                Some(ver(2, 1, 0)),
                fp(0xAA),
                Some("Vault key"),
                Some(true),
            ),
            supported(
                DeviceKind::BitBox02,
                Some(ver(9, 13, 0)),
                fp(0xBB),
                Some("Backup key"),
                Some(true),
            ),
            supported(
                DeviceKind::Specter,
                Some(ver(2, 0, 0)),
                fp(0xEE),
                Some("Specter"),
                Some(true),
            ),
            unsupported(
                DeviceKind::Coldcard,
                Some(ver(5, 1, 0)),
                UnsupportedReason::Method("display_address"),
            ),
            locked(DeviceKind::Jade, None),
        ]
    })
}

fn verify_address_chosen() -> &'static HashSet<Fingerprint> {
    static S: OnceLock<HashSet<Fingerprint>> = OnceLock::new();
    S.get_or_init(|| HashSet::from([fp(0xBB)]))
}

fn sample_address() -> &'static Address {
    static A: OnceLock<Address> = OnceLock::new();
    A.get_or_init(|| {
        Address::from_str("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq")
            .expect("hardcoded sample")
            .require_network(Network::Bitcoin)
            .expect("mainnet sample")
    })
}

fn sample_child_number() -> &'static ChildNumber {
    static C: OnceLock<ChildNumber> = OnceLock::new();
    C.get_or_init(|| ChildNumber::Normal { index: 0 })
}

fn verify_address_view() -> Element<'static, DebugMessage> {
    let body = view::receive::verify_address_modal(
        None,
        verify_address_hws(),
        verify_address_chosen(),
        sample_address(),
        sample_child_number(),
    );
    dashboard_with_modal(
        &super::RECEIVE_MENU,
        "HW modal:verify address flow",
        p1_regular("(production: receive panel visible behind the modal)"),
        body,
    )
}
