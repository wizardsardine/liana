//! Gallery of the list and modal entry constructors of `liana-ui`, with the
//! labels their callers in liana-gui and business-installer pass them, so the
//! translated catalogs can be checked against the entry widths.

use liana::miniscript::bitcoin::{bip32::Fingerprint, Network};
use liana_ui::{
    component::{
        badge::Tile,
        button::EntryWidth,
        form,
        list::{
            self, CollapsibleEntry, DeviceStatus, EntryAccent, EntryKeyKind, EntryPathRole,
            EntryRegisterStatus, EntrySetKeyOwner,
        },
        modal::{self, KeySourceKind},
        pill,
        text::new,
    },
    theme,
    widget::{Container, Element},
};

use crate::{
    debug::{debug_section, layout_page, DebugMessage, Sample},
    t,
};

const EMAIL: &str = "alice@wizardsardine.com";
const WALLET: &str = "Family savings";
const DEVICE: &str = "Ledger Nano S+";
const KEY: &str = "Alice's key";

fn fingerprint() -> Fingerprint {
    Fingerprint::from([0xf2, 0x5b, 0xdf, 0xf6])
}

fn fingerprint_label() -> String {
    format!("#{}", fingerprint())
}

fn sample(entry: Element<'static, DebugMessage>) -> Container<'static, DebugMessage> {
    Container::new(entry)
}

/// A modal entry at the width the key source modal gives it.
fn modal_sample(entry: Element<'static, DebugMessage>) -> Container<'static, DebugMessage> {
    at(modal::BTN_W, entry)
}

/// Sign modal: the entries fill `ModalWidth::XL` minus the modal padding.
const SIGN_MODAL_W: u32 = modal::ModalWidth::XL as u32 - 2 * modal::MODAL_PADDING as u32;
/// Business registration page: the entries fill the 600 content column, 20 of padding each side.
const REGISTRATION_W: u32 = 560;

/// A fill-width entry at the width its caller gives it.
fn at(width: u32, entry: Element<'static, DebugMessage>) -> Container<'static, DebugMessage> {
    Container::new(entry).width(width)
}

fn collapsible(
    tile: Tile,
    title: String,
    subtitle: Option<String>,
) -> Container<'static, DebugMessage> {
    sample(list::entry_collapsible(CollapsibleEntry {
        accent: Some(EntryAccent::Testnet),
        tile,
        title,
        collapsed_subtitle: subtitle.clone(),
        expanded_subtitle: subtitle,
        content: new::caption("content").into(),
        expanded: false,
        on_toggle: (),
    }))
}

#[rustfmt::skip]
fn list_entries_a() -> Sample<8> {
    [
        (sample(list::entry_action(Tile::Import, t!("installer-import-descriptor"), None::<String>, None, EntryWidth::Standard, Some(()))), "liana_ui::component::list::entry_action(Import, installer-import-descriptor)"),
        (sample(list::entry_action_accent(Some(EntryAccent::Testnet), Tile::Import, t!("installer-import-backup"), None::<String>, None, EntryWidth::Standard, Some(()))), "liana_ui::component::list::entry_action_accent(Testnet, Import, installer-import-backup)"),
        (collapsible(Tile::Import, t!("installer-load-shared-wallet"), Some(t!("installer-shared-wallet-help"))), "liana_ui::component::list::entry_collapsible(installer-load-shared-wallet, installer-shared-wallet-help)"),
        (collapsible(Tile::Paste, t!("installer-paste-descriptor"), Some(t!("installer-paste-descriptor-description"))), "liana_ui::component::list::entry_collapsible(installer-paste-descriptor, installer-paste-descriptor-description)"),
        (collapsible(Tile::Import, t!("installer-import-mnemonic"), None), "liana_ui::component::list::entry_collapsible(installer-import-mnemonic)"),
        (sample(list::entry_organization("Wizardsardine", Some(EMAIL), Some(()))), "liana_ui::component::list::entry_organization(_, Some(email))"),
        (sample(list::entry_paste_xpub("", |_| (), ())), "liana_ui::component::list::entry_paste_xpub(\"\")"),
        (sample(list::entry_no_devices(t!("business-no-hardware-wallets"), None::<String>)), "liana_ui::component::list::entry_no_devices(business-no-hardware-wallets)"),
    ]
}

#[rustfmt::skip]
fn list_entries_b() -> Sample<8> {
    let key_pills = vec![pill::key_kind(EntryKeyKind::Internal, KEY).into(), pill::key_kind(EntryKeyKind::External, "Bob").into()];
    [
        (sample(list::entry_wallet(Some(EntryAccent::Testnet), WALLET, Some(new::caption("Liana-kvsc70ze").style(theme::text::secondary).into()), None, None, Some(()))), "liana_ui::component::list::entry_wallet(Testnet, _, Some(subtitle), None, None)"),
        (sample(list::entry_wallet(Some(EntryAccent::Success), WALLET, None, Some(pill::role_manager().into()), Some(pill::to_approve().into()), Some(()))), "liana_ui::component::list::entry_wallet(Success, _, None, role_manager, to_approve)"),
        (sample(list::account_entry(EMAIL, false, Some(()), Some(()))), "liana_ui::component::list::account_entry(email, false)"),
        (sample(list::account_select_entry(EMAIL, true, Some(()))), "liana_ui::component::list::account_select_entry(email, true)"),
        (sample(list::entry_key(EntryKeyKind::Internal, KEY, pill::key_internal().into(), EMAIL, None, Some(()), Some(()))), "liana_ui::component::list::entry_key(Internal, _, key_internal, email, None, _, Some(delete))"),
        (sample(list::entry_path(EntryPathRole::Recovery, t!("business-recovery-path-number", number = 1), t!("business-keys-required-to-spend", threshold = 2, count = 3), pill::path_timelock(t!("business-after-days", count = 30)).into(), key_pills, true, Some(()), Some(()))), "liana_ui::component::list::entry_path(Recovery, business-recovery-path-number, business-keys-required-to-spend, path_timelock(business-after-days), ..)"),
        (sample(list::entry_set_key(EntryKeyKind::External, EntrySetKeyOwner::Own, KEY, Some(EMAIL), Some(pill::xpub_not_set().into()), Some(()))), "liana_ui::component::list::entry_set_key(External, Own, _, Some(email), xpub_not_set)"),
        (sample(list::entry_set_key(EntryKeyKind::SafetyNet, EntrySetKeyOwner::Other, KEY, Some(EMAIL), Some(pill::xpub_set().into()), None)), "liana_ui::component::list::entry_set_key(SafetyNet, Other, _, Some(email), xpub_set)"),
    ]
}

/// The registration entry as `business-installer` builds it: title, fingerprint and a status caption.
fn registration(
    status: EntryRegisterStatus,
    caption: Option<String>,
    enabled: bool,
) -> Container<'static, DebugMessage> {
    let body = iced::widget::column![
        new::b5_medium(DEVICE),
        new::caption(fingerprint_label()).style(theme::text::secondary),
        caption.map(|caption| new::caption(caption).style(theme::text::tertiary)),
    ]
    .spacing(2);
    sample(list::entry_register(
        status,
        body,
        None,
        enabled,
        enabled.then_some(()),
    ))
}

/// A device entry as liana-gui builds it: the key alias as title, kind and fingerprint below.
fn device(status: DeviceStatus) -> Container<'static, DebugMessage> {
    sample(list::entry_device_list(
        KEY,
        Some(format!("ledger {}", fingerprint_label())),
        status,
        EntryWidth::Standard,
        Some(()),
    ))
}

/// A device entry as the business xpub modal builds it: kind and version as title, a reason below.
fn business_device(
    subtitle: Option<String>,
    status: DeviceStatus,
) -> Container<'static, DebugMessage> {
    sample(list::entry_device_list(
        "Ledger 2.2.0",
        subtitle,
        status,
        EntryWidth::Standard,
        Some(()),
    ))
}

#[rustfmt::skip]
fn register_entries() -> Sample<4> {
    [
        (modal_sample(modal::modal_no_devices_placeholder()), "liana_ui::component::modal::modal_no_devices_placeholder()"),
        (registration(EntryRegisterStatus::Registered, Some(t!("business-registered-on-device")), true), "liana_ui::component::list::entry_register(Registered, business-registered-on-device)"),
        (registration(EntryRegisterStatus::Unregistered, Some(t!("business-connect-device-register")), false), "liana_ui::component::list::entry_register(Unregistered, business-connect-device-register)"),
        (registration(EntryRegisterStatus::Unregistered, Some(t!("business-device-unsupported-locked")), false), "liana_ui::component::list::entry_register(Unregistered, business-device-unsupported-locked)"),
    ]
}

#[rustfmt::skip]
fn device_entries_a() -> Sample<9> {
    [
        (device(DeviceStatus::None),                                        "liana_ui::component::list::entry_device_list(None), width 680 (settings register modal)"),
        (device(DeviceStatus::Processing),                                  "liana_ui::component::list::entry_device_list(Processing), width 680"),
        (device(DeviceStatus::Selected),                                    "liana_ui::component::list::entry_device_list(Selected), width 680"),
        (device(DeviceStatus::Registered),                                  "liana_ui::component::list::entry_device_list(Registered), width 680"),
        (device(DeviceStatus::Warning(t!("hw-no-taproot-miniscript"))),     "liana_ui::component::list::entry_device_list(Warning(hw-no-taproot-miniscript)), width 680"),
        (device(DeviceStatus::Unrelated),                                   "liana_ui::component::list::entry_device_list(Unrelated), width 680"),
        (device(DeviceStatus::WrongNetwork),                                "liana_ui::component::list::entry_device_list(WrongNetwork), width 680"),
        (device(DeviceStatus::OutdatedFirmware("2.2.0".to_string())),       "liana_ui::component::list::entry_device_list(OutdatedFirmware), width 680"),
        (device(DeviceStatus::ConnectionError),                             "liana_ui::component::list::entry_device_list(ConnectionError), width 680"),
    ]
}

#[rustfmt::skip]
fn device_entries_b() -> Sample<9> {
    [
        (device(DeviceStatus::Locked(Some("1234".to_string()))),          "liana_ui::component::list::entry_device_list(Locked(Some(code))), width 680"),
        (device(DeviceStatus::Locked(None)),                              "liana_ui::component::list::entry_device_list(Locked(None)), width 680"),
        (device(DeviceStatus::NotInPath),                                     "liana_ui::component::list::entry_device_list(NotInPath), width 680 (sign modal)"),
        (device(DeviceStatus::Signed),                                        "liana_ui::component::list::entry_device_list(Signed), width 680"),
        (device(DeviceStatus::Warning(t!("hw-descriptor-not-registered"))),   "liana_ui::component::list::entry_device_list(Warning(hw-descriptor-not-registered)), width 680"),
        (business_device(None, DeviceStatus::AlreadyUsed),                                  "liana_ui::component::list::entry_device_list(AlreadyUsed), width 680 (business xpub modal)"),
        (business_device(None, DeviceStatus::Selectable(fingerprint())),                    "liana_ui::component::list::entry_device_list(Selectable), width 680"),
        (business_device(Some(t!("decrypt-pairing-code", code = "1234")), DeviceStatus::Fingerprint(fingerprint())), "liana_ui::component::list::entry_device_list(decrypt-pairing-code, Fingerprint), width 680"),
        (business_device(Some(t!("business-not-part-wallet", fingerprint = fingerprint())), DeviceStatus::None), "liana_ui::component::list::entry_device_list(business-not-part-wallet, None), width 680"),
    ]
}

fn key_entry(
    kind: KeySourceKind,
    message: Option<String>,
    error: Option<String>,
) -> Container<'static, DebugMessage> {
    modal_sample(modal::key_entry(
        kind,
        KEY.to_string(),
        Some(fingerprint_label()),
        None::<String>,
        error,
        message,
        Some(()),
    ))
}

#[rustfmt::skip]
fn modal_entries_a() -> Sample<8> {
    [
        (key_entry(KeySourceKind::Device, None, None),                                       "liana_ui::component::modal::key_entry(Device)"),
        (key_entry(KeySourceKind::HotKey, Some(t!("device-hot-key-test-only")), None),       "liana_ui::component::modal::key_entry(HotKey, message: device-hot-key-test-only)"),
        (key_entry(KeySourceKind::Imported, None, Some(t!("installer-key-already-used-in-path"))), "liana_ui::component::modal::key_entry(Imported, error: installer-key-already-used-in-path)"),
        (key_entry(KeySourceKind::Token, None, None),                                        "liana_ui::component::modal::key_entry(Token)"),
        (at(SIGN_MODAL_W, modal::device_entry(Some(fingerprint_label()), Some("Ledger"), Some(DEVICE), DeviceStatus::None, Some(()))), "liana_ui::component::modal::device_entry(Some(fp), Some(kind), Some(alias), None), width 680 (sign modal)"),
        (at(SIGN_MODAL_W, modal::device_entry(Some(fingerprint_label()), None::<String>, Some(t!("installer-hot-signer")), DeviceStatus::Signed, Some(()))), "liana_ui::component::modal::device_entry(Some(fp), None, installer-hot-signer, Signed), width 680"),
        (at(REGISTRATION_W, modal::registration_key_entry(fingerprint_label(), Some("Ledger".to_string()), Some(DEVICE.to_string()), EntryRegisterStatus::Registered, Some(t!("business-registered-on-device")), Some(|| ()))), "liana_ui::component::modal::registration_key_entry(Registered, business-registered-on-device), fill 560 (business registration)"),
        (at(REGISTRATION_W, modal::registration_key_entry(fingerprint_label(), Some("Ledger".to_string()), None, EntryRegisterStatus::Unregistered, Some(t!("business-connect-device-register")), None::<fn() -> DebugMessage>)), "liana_ui::component::modal::registration_key_entry(Unregistered, business-connect-device-register), fill 560"),
    ]
}

#[rustfmt::skip]
fn modal_entries_b() -> Sample<8> {
    let value = form::Value::default();
    [
        (modal_sample(modal::import_xpub_entry(None, Some(()), Some(|| ()))),                             "liana_ui::component::modal::import_xpub_entry(None, Some(taproot note))"),
        (modal_sample(modal::import_xpub_entry(Some(t!("installer-key-already-used-in-path")), None, Some(|| ()))), "liana_ui::component::modal::import_xpub_entry(Some(installer-key-already-used-in-path), None)"),
        (modal_sample(modal::generate_hot_key_entry(Some(|| ()))),                                         "liana_ui::component::modal::generate_hot_key_entry()"),
        (modal_sample(modal::paste_xpub_entry(true, Network::Regtest, &value, Some(()), Some(|_| ()), Some(|| ()), || ())), "liana_ui::component::modal::paste_xpub_entry(true, Some(taproot note))"),
        (modal_sample(modal::paste_xpub_entry(false, Network::Regtest, &value, None, Some(|_| ()), Some(|| ()), || ())), "liana_ui::component::modal::paste_xpub_entry(false, None)"),
        (modal_sample(modal::safety_net_token_entry(true, &value, Some(|_| ()), Some(|| ()), || ())),      "liana_ui::component::modal::safety_net_token_entry(true)"),
        (modal_sample(modal::cosigner_token_entry(false, &value, Some(|_| ()), Some(|| ()), || ())),       "liana_ui::component::modal::cosigner_token_entry(false)"),
        (modal_sample(modal::enter_mnemonic_entry(false, false, &value, |_| (), |_| (), || (), || ())),   "liana_ui::component::modal::enter_mnemonic_entry(false, false)"),
    ]
}

#[rustfmt::skip]
crate::debug_pages!(
    (ENTRY_LIST_1,    layout_page("List entries 1/3",  [vec![debug_section("List entries", list_entries_a())]])),
    (ENTRY_LIST_2,    layout_page("List entries 2/3",  [vec![debug_section("List entries", list_entries_b())]])),
    (ENTRY_LIST_3,    layout_page("List entries 3/3",  [vec![debug_section("Register entries", register_entries())]])),
    (ENTRY_DEVICES_1, layout_page("Device entries 1/2", [vec![debug_section("Device entries", device_entries_a())]])),
    (ENTRY_DEVICES_2, layout_page("Device entries 2/2", [vec![debug_section("Device entries", device_entries_b())]])),
    (ENTRY_MODAL_1,   layout_page("Modal entries 1/2", [vec![debug_section("Modal entries", modal_entries_a())]])),
    (ENTRY_MODAL_2,   layout_page("Modal entries 2/2", [vec![debug_section("Modal entries", modal_entries_b())]])),
);
