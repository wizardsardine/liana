//! Renders each `app::view::hw::hw_list_view*` function inside its
//! production modal context. Three pages, one per function:
//!
//! - `hw_list_view` — signing flow (used by PSBT view).
//! - `hw_list_view_for_registration` — wallet registration (used by
//!   Settings).
//! - `hw_list_view_verify_address` — address verification (used by
//!   Receive).
//!
//! Each page mocks a list of `HardwareWallet`s exercising every visual
//! variant of that function, wraps the list in a card-style modal body,
//! and overlays it on top of the production dashboard chrome via
//! [`liana_ui::widget::modal::Modal`]. The dashboard's sidebar shows the
//! menu under which the modal appears in production. All clicks are
//! swallowed at the debug-overlay boundary.
//!
//! `HardwareWallet::Supported` requires an `Arc<dyn HWI + Send + Sync>`;
//! we satisfy it with a [`MockHwi`] whose async methods all return
//! `UnimplementedMethod` — the rendering path never invokes them, only
//! the struct's data fields (`kind`, `version`, `fingerprint`, `alias`,
//! `registered`).

use std::sync::{Arc, Mutex, OnceLock};

use async_hwi::{AddressScript, DeviceKind, Error as HwiError, Version, HWI};
use async_trait::async_trait;
use iced::Length;
use liana::miniscript::bitcoin::{
    bip32::{DerivationPath, Fingerprint, Xpub},
    psbt::Psbt,
};

use liana_ui::{
    component::{card, text::*},
    widget::{modal::Modal, *},
};

use crate::{
    app::{
        menu::Menu,
        view::{self, hw as view_hw, Message as ViewMessage},
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
                fp(0xAA),
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

fn signing_view() -> Element<'static, DebugMessage> {
    let hws = signing_hws();
    // (signed, signing, can_sign) per row, picked to exercise each visual
    // branch of `hw_list_view`.
    let states: &[(bool, bool, bool)] = &[
        (false, false, true),  // supported, idle
        (false, false, true),  // registered = false → warning
        (false, false, false), // !can_sign → disabled
        (false, true, true),   // signing → processing
        (true, false, true),   // signed → success
        (false, false, true),  // unsupported version
        (false, false, true),  // wrong network
        (false, false, true),  // not part of wallet
        (false, false, true),  // unsupported (other)
        (false, false, true),  // locked
    ];
    let list = hws
        .iter()
        .enumerate()
        .fold(Column::new().spacing(10), |col, (i, hw)| {
            let (signed, signing, can_sign) = states[i];
            col.push(view_hw::hw_list_view(i, hw, signed, signing, can_sign))
        });

    let modal_body = card::simple(
        Column::new()
            .spacing(20)
            .push(p1_bold("Select signing device to sign with:"))
            .push(list)
            .width(Length::Fill),
    )
    .max_width(500);

    dashboard_with_modal(
        &super::PSBTS_MENU,
        "HW modal — signing flow",
        p1_regular("(production: PSBT details visible behind the modal)"),
        modal_body,
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

fn registration_view() -> Element<'static, DebugMessage> {
    let hws = registration_hws();
    // (chosen, processing, registered) per row.
    let states: &[(bool, bool, bool)] = &[
        (false, false, false), // idle, selectable
        (true, true, false),   // chosen + processing → processing
        (false, false, true),  // registered → success
        (false, false, false), // unsupported
        (false, false, false), // locked
    ];
    let list = hws
        .iter()
        .enumerate()
        .fold(Column::new().spacing(10), |col, (i, hw)| {
            let (chosen, processing, registered) = states[i];
            col.push(view_hw::hw_list_view_for_registration(
                i, hw, chosen, processing, registered,
            ))
        });

    let modal_body = card::simple(
        Column::new()
            .spacing(20)
            .push(p1_bold("Register wallet on signing device:"))
            .push(list)
            .width(Length::Fill),
    )
    .max_width(500);

    dashboard_with_modal(
        &super::SETTINGS_MENU,
        "HW modal — registration flow",
        p1_regular("(production: settings page visible behind the modal)"),
        modal_body,
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

fn verify_address_view() -> Element<'static, DebugMessage> {
    let hws = verify_address_hws();
    // `chosen` per row.
    let states: &[bool] = &[
        false, // selectable
        true,  // chosen → processing
        false, // Specter → unimplemented method
        false, // unsupported
        false, // locked
    ];
    let list = hws
        .iter()
        .enumerate()
        .fold(Column::new().spacing(10), |col, (i, hw)| {
            col.push(view_hw::hw_list_view_verify_address(i, hw, states[i]))
        });

    let modal_body = card::simple(
        Column::new()
            .spacing(20)
            .push(p1_bold("Select device to verify address on:"))
            .push(list)
            .width(Length::Fill),
    )
    .max_width(500);

    dashboard_with_modal(
        &super::RECEIVE_MENU,
        "HW modal — verify address flow",
        p1_regular("(production: receive panel visible behind the modal)"),
        modal_body,
    )
}
