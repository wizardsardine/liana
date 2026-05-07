//! Gallery of every `liana_ui::component::hw::*` constructor.
//!
//! All of these widgets render a *signing device* (hardware signer, hot
//! signer, or provider-key entry) — they're variations on the same row, not
//! distinct widget kinds. The list is split across two pages purely for
//! length, not by category.
//!
//! The `supported_hardware_wallet_with_account` constructor requires a
//! message type implementing `From<(Fingerprint, ChildNumber)>`; we use a
//! private [`AccountPick`] newtype and `Element::map` it back to
//! [`DebugMessage`] so the account-picker click is swallowed at the same
//! boundary as every other debug-overlay event.

use iced::{Alignment, Length};
use liana::miniscript::bitcoin::bip32::{ChildNumber, Fingerprint};
use liana_ui::{
    component::{hw, modal, text},
    theme,
    widget::*,
};

use crate::debug::{debug_chrome, DebugMessage, DebugPageEntry};

pub static ENTRY_PAGE_1: DebugPageEntry = DebugPageEntry { view: page_1 };
pub static ENTRY_PAGE_2: DebugPageEntry = DebugPageEntry { view: page_2 };

const ROW_SPACING: f32 = 30.0;

/// Sample fingerprint used for every hw widget in the gallery.
fn fingerprint() -> Fingerprint {
    Fingerprint::from([0xDE, 0xAD, 0xBE, 0xEF])
}

fn account() -> ChildNumber {
    ChildNumber::from_hardened_idx(0).expect("hardcoded")
}

/// Newtype carrying the account-pick callback message from
/// `supported_hardware_wallet_with_account`. The actual message is discarded
/// at the boundary via `Element::map`.
#[derive(Clone, Debug)]
struct AccountPick;
impl From<(Fingerprint, ChildNumber)> for AccountPick {
    fn from(_: (Fingerprint, ChildNumber)) -> Self {
        AccountPick
    }
}

/// One row: a code path paired with its rendered widget. Used as the input
/// shape for [`build_page`] — the chrome (label, bordered card sized to
/// [`modal::BTN_W`], two-column layout) is applied by the helper, not at the
/// call site.
type RowDef = (&'static str, Element<'static, DebugMessage>);

/// Pair a code path with a normal hw widget that emits [`DebugMessage`].
fn row(path: &'static str, widget: impl Into<Element<'static, DebugMessage>>) -> RowDef {
    (path, widget.into())
}

/// Pair a code path with `supported_hardware_wallet_with_account`, mapping
/// its [`AccountPick`] message back to [`DebugMessage`] at the boundary.
fn row_with_account(path: &'static str, widget: Container<'static, AccountPick>) -> RowDef {
    (path, Element::from(widget).map(|_| ()))
}

/// Wrap a single row in the standard chrome: label on top of a bordered card
/// of width [`modal::BTN_W`].
fn entry(
    path: &'static str,
    widget: Element<'static, DebugMessage>,
) -> Column<'static, DebugMessage> {
    Column::new().spacing(8).push(text::p1_regular(path)).push(
        Container::new(widget)
            .width(Length::Fixed(modal::BTN_W as f32))
            .style(theme::card::border),
    )
}

/// Per-page cap on entry count. Splitting at 13 keeps each two-column page
/// comfortable to scan.
const MAX_ENTRIES_PER_PAGE: usize = 13;

/// Build a debug page from a list of rows: applies [`entry`] to each, splits
/// into two side-by-side columns (first half left, rest right), wraps in
/// debug chrome.
fn build_page(title: &'static str, rows: Vec<RowDef>) -> Element<'static, DebugMessage> {
    debug_assert!(rows.len() <= MAX_ENTRIES_PER_PAGE);
    let mid = rows.len().div_ceil(2);
    let mut iter = rows.into_iter().map(|(p, w)| entry(p, w));
    let left = (&mut iter)
        .take(mid)
        .fold(Column::new().spacing(ROW_SPACING), Column::push);
    let right = iter.fold(Column::new().spacing(ROW_SPACING), Column::push);
    let body = Row::new()
        .spacing(40)
        .align_y(Alignment::Start)
        .push(left)
        .push(right);
    debug_chrome(title, body)
}

fn page_1() -> Element<'static, DebugMessage> {
    let alias: Option<&'static str> = Some("My signer");
    let kind = "Ledger";
    let version: Option<&'static str> = Some("v2.1.0");

    let rows = vec![
        row("liana_ui::component::hw::locked_hardware_wallet(<kind>, Some(\"123-456\"))",
            hw::locked_hardware_wallet(kind, Some("123-456"))),
        row("liana_ui::component::hw::supported_hardware_wallet(<kind>, <version>, <fp>, Some(<alias>))",
            hw::supported_hardware_wallet(kind, version, fingerprint(), alias)),
        row_with_account("liana_ui::component::hw::supported_hardware_wallet_with_account(...)",
            hw::supported_hardware_wallet_with_account::<AccountPick, _, _>(
                kind, version, fingerprint(), alias, Some(account()), false)),
        row_with_account("liana_ui::component::hw::supported_hardware_wallet_with_account(..., edit_account=true)",
            hw::supported_hardware_wallet_with_account::<AccountPick, _, _>(
                kind, version, fingerprint(), alias, Some(account()), true)),
        row("liana_ui::component::hw::warning_hardware_wallet(<kind>, <version>, <fp>, Some(<alias>), \"...\")",
            hw::warning_hardware_wallet(kind, version, fingerprint(), alias, "Firmware mismatch")),
        row("liana_ui::component::hw::unimplemented_method_hardware_wallet(<kind>, <version>, <fp>, \"...\")",
            hw::unimplemented_method_hardware_wallet::<DebugMessage, _, _, _>(
                kind, version, fingerprint(), "This action isn't implemented for this device")),
        row("liana_ui::component::hw::disabled_hardware_wallet(<kind>, <version>, <fp>, \"...\")",
            hw::disabled_hardware_wallet::<DebugMessage, _, _, _>(
                kind, version, fingerprint(), "Disabled — already used")),
        row("liana_ui::component::hw::unrelated_hardware_wallet(<kind>, <version>, <fp>)",
            hw::unrelated_hardware_wallet::<DebugMessage, _, _, _>(kind, version, fingerprint())),
        row("liana_ui::component::hw::processing_hardware_wallet(<kind>, <version>, <fp>, Some(<alias>))",
            hw::processing_hardware_wallet(kind, version, fingerprint(), alias)),
        row("liana_ui::component::hw::selected_hardware_wallet(<kind>, <version>, <fp>, Some(<alias>), None, Some(0'), true)",
            hw::selected_hardware_wallet(kind, version, fingerprint(), alias, None, Some(account()), true)),
        row("liana_ui::component::hw::selected_hardware_wallet(..., warning=Some(\"...\"))",
            hw::selected_hardware_wallet(kind, version, fingerprint(), alias, Some("Outdated firmware"), Some(account()), true)),
        row("liana_ui::component::hw::sign_success_hardware_wallet(<kind>, <version>, <fp>, Some(<alias>))",
            hw::sign_success_hardware_wallet(kind, version, fingerprint(), alias)),
        row("liana_ui::component::hw::registration_success_hardware_wallet(<kind>, <version>, <fp>, Some(<alias>))",
            hw::registration_success_hardware_wallet(kind, version, fingerprint(), alias)),
    ];
    build_page("Signing devices (1/2)", rows)
}

fn page_2() -> Element<'static, DebugMessage> {
    let alias: Option<&'static str> = Some("My signer");
    let kind = "Ledger";
    let version: Option<&'static str> = Some("v2.1.0");

    let rows = vec![
        row("liana_ui::component::hw::wrong_network_hardware_wallet(<kind>, <version>)",
            hw::wrong_network_hardware_wallet::<DebugMessage, _, _>(kind, version)),
        row("liana_ui::component::hw::unsupported_hardware_wallet(<kind>, <version>)",
            hw::unsupported_hardware_wallet::<DebugMessage, _, _>(kind, version)),
        row("liana_ui::component::hw::unsupported_version_hardware_wallet(<kind>, <version>, \"v3.0\")",
            hw::unsupported_version_hardware_wallet::<DebugMessage, _, _, _>(kind, version, "v3.0")),
        row("liana_ui::component::hw::taproot_not_supported_device(<kind>)",
            hw::taproot_not_supported_device::<DebugMessage, _>(kind)),
        row("liana_ui::component::hw::sign_success_hot_signer(<fp>, Some(<alias>))",
            hw::sign_success_hot_signer(fingerprint(), alias)),
        row("liana_ui::component::hw::selected_hot_signer(<fp>, Some(<alias>))",
            hw::selected_hot_signer(fingerprint(), alias)),
        row("liana_ui::component::hw::unselected_hot_signer(<fp>, Some(<alias>))",
            hw::unselected_hot_signer(fingerprint(), alias)),
        row("liana_ui::component::hw::hot_signer(<fp>, Some(<alias>), can_sign=true)",
            hw::hot_signer(fingerprint(), alias, true)),
        row("liana_ui::component::hw::hot_signer(<fp>, Some(<alias>), can_sign=false)",
            hw::hot_signer(fingerprint(), alias, false)),
        row("liana_ui::component::hw::selected_provider_key(<fp>, \"alias\", \"key_kind\", \"token\")",
            hw::selected_provider_key(fingerprint(), "Provider", "Cosigner", "TKN42")),
        row("liana_ui::component::hw::unselected_provider_key(<fp>, \"alias\", \"key_kind\", \"token\")",
            hw::unselected_provider_key(fingerprint(), "Provider", "Cosigner", "TKN42")),
        row("liana_ui::component::hw::unsaved_provider_key(<fp>, \"key_kind\", \"token\")",
            hw::unsaved_provider_key(fingerprint(), "Cosigner", "TKN42")),
    ];
    build_page("Signing devices (2/2)", rows)
}
