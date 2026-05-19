//! Gallery of every `liana_ui::component::modal::legacy::*` constructor.
//!
//! All of these widgets render a *signing device* (hardware signer, hot
//! signer, or provider-key entry): they're variations on the same row, not
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
    component::{modal, text},
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
/// shape for [`build_page`]: the chrome (label, bordered card sized to
/// [`modal::BTN_W`], two-column layout) is applied by the helper, not at the
/// call site.
type RowDef = (&'static str, Element<'static, DebugMessage>);

/// Pair a code path with a normal hw widget that emits [`DebugMessage`].
fn row(path: &'static str, widget: impl Into<Element<'static, DebugMessage>>) -> RowDef {
    (path, widget.into())
}

/// Pair a code path with `supported_device_with_account`, mapping its
/// [`AccountPick`] message back to [`DebugMessage`] at the boundary.
fn row_with_account(path: &'static str, widget: Element<'static, AccountPick>) -> RowDef {
    (path, widget.map(|_| ()))
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

    use modal::legacy as hw;
    let rows = vec![
        row("liana_ui::component::modal::legacy::locked_device(<kind>, Some(\"123-456\"), None)",
            hw::locked_device::<DebugMessage, _>(kind, Some("123-456"), None)),
        row("liana_ui::component::modal::legacy::supported_device(<kind>, <version>, <fp>, Some(<alias>), None)",
            hw::supported_device::<DebugMessage, _, _, _>(kind, version, fingerprint(), alias, None)),
        row_with_account("liana_ui::component::modal::legacy::supported_device_with_account(...)",
            hw::supported_device_with_account::<AccountPick, _, _>(
                kind, version, fingerprint(), alias, Some(account()), false, None)),
        row_with_account("liana_ui::component::modal::legacy::supported_device_with_account(..., edit_account=true)",
            hw::supported_device_with_account::<AccountPick, _, _>(
                kind, version, fingerprint(), alias, Some(account()), true, None)),
        row("liana_ui::component::modal::legacy::warning_device(<kind>, <version>, <fp>, Some(<alias>), \"...\", None)",
            hw::warning_device::<DebugMessage, _, _, _>(kind, version, fingerprint(), alias, "Firmware mismatch", None)),
        row("liana_ui::component::modal::legacy::unimplemented_method_device(<kind>, <version>, <fp>, \"...\", None)",
            hw::unimplemented_method_device::<DebugMessage, _, _, _>(
                kind, version, fingerprint(), "This action isn't implemented for this device", None)),
        row("liana_ui::component::modal::legacy::disabled_device(<kind>, <version>, <fp>, \"...\", None)",
            hw::disabled_device::<DebugMessage, _, _, _>(
                kind, version, fingerprint(), "Disabled, already used", None)),
        row("liana_ui::component::modal::legacy::unrelated_device(<kind>, <version>, <fp>, None)",
            hw::unrelated_device::<DebugMessage, _, _, _>(kind, version, fingerprint(), None)),
        row("liana_ui::component::modal::legacy::processing_device(<kind>, <version>, <fp>, Some(<alias>), None)",
            hw::processing_device::<DebugMessage, _, _, _>(kind, version, fingerprint(), alias, None)),
        row("liana_ui::component::modal::legacy::selected_device(<kind>, <version>, <fp>, Some(<alias>), None, Some(0'), true, None)",
            hw::selected_device::<DebugMessage, _, _, _>(kind, version, fingerprint(), alias, None, Some(account()), true, None)),
        row("liana_ui::component::modal::legacy::selected_device(..., warning=Some(\"...\"))",
            hw::selected_device::<DebugMessage, _, _, _>(kind, version, fingerprint(), alias, Some("Outdated firmware"), Some(account()), true, None)),
        row("liana_ui::component::modal::legacy::signed_device(<kind>, <version>, <fp>, Some(<alias>), None)",
            hw::signed_device::<DebugMessage, _, _, _>(kind, version, fingerprint(), alias, None)),
        row("liana_ui::component::modal::legacy::registered_device(<kind>, <version>, <fp>, Some(<alias>), None)",
            hw::registered_device::<DebugMessage, _, _, _>(kind, version, fingerprint(), alias, None)),
    ];
    build_page("Signing devices (1/2)", rows)
}

fn page_2() -> Element<'static, DebugMessage> {
    let alias: Option<&'static str> = Some("My signer");
    let kind = "Ledger";
    let version: Option<&'static str> = Some("v2.1.0");

    use modal::legacy as hw;
    let rows = vec![
        row("liana_ui::component::modal::legacy::wrong_network_device(<kind>, <version>, None)",
            hw::wrong_network_device::<DebugMessage, _, _>(kind, version, None)),
        row("liana_ui::component::modal::legacy::unsupported_device(<kind>, <version>, None)",
            hw::unsupported_device::<DebugMessage, _, _>(kind, version, None)),
        row("liana_ui::component::modal::legacy::unsupported_version_device(<kind>, <version>, \"v3.0\", None)",
            hw::unsupported_version_device::<DebugMessage, _, _, _>(kind, version, "v3.0", None)),
        row("liana_ui::component::modal::legacy::taproot_unsupported_device(<kind>, None)",
            hw::taproot_unsupported_device::<DebugMessage, _>(kind, None)),
        row("liana_ui::component::modal::legacy::signed_hot_key(<fp>, Some(<alias>), None)",
            hw::signed_hot_key::<DebugMessage, _>(fingerprint(), alias, None)),
        row("liana_ui::component::modal::legacy::selected_hot_key(<fp>, Some(<alias>), None)",
            hw::selected_hot_key::<DebugMessage, _>(fingerprint(), alias, None)),
        row("liana_ui::component::modal::legacy::unselected_hot_key(<fp>, Some(<alias>), None)",
            hw::unselected_hot_key::<DebugMessage, _>(fingerprint(), alias, None)),
        row("liana_ui::component::modal::legacy::hot_key(<fp>, Some(<alias>), can_sign=true, None)",
            hw::hot_key::<DebugMessage, _>(fingerprint(), alias, true, None)),
        row("liana_ui::component::modal::legacy::hot_key(<fp>, Some(<alias>), can_sign=false, None)",
            hw::hot_key::<DebugMessage, _>(fingerprint(), alias, false, None)),
        row("liana_ui::component::modal::legacy::selected_provider(<fp>, \"alias\", \"key_kind\", \"token\", None)",
            hw::selected_provider::<DebugMessage, _>(fingerprint(), "Provider", "Cosigner", "TKN42", None)),
        row("liana_ui::component::modal::legacy::unselected_provider(<fp>, \"alias\", \"key_kind\", \"token\", None)",
            hw::unselected_provider::<DebugMessage, _>(fingerprint(), "Provider", "Cosigner", "TKN42", None)),
        row("liana_ui::component::modal::legacy::unsaved_provider(<fp>, \"key_kind\", \"token\", None)",
            hw::unsaved_provider::<DebugMessage, _>(fingerprint(), "Cosigner", "TKN42", None)),
    ];
    build_page("Signing devices (2/2)", rows)
}
