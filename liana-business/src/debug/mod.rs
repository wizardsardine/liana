//! Business-specific debug stack.
//!
//! Surfaces the views in [`crate::settings::views`] in the debug overlay
//! provided by `liana-gui`. Aggregated into [`EXTRA_STACKS`], which the
//! binary's `main.rs` hands to `liana_gui::gui::GUI::new` so that the
//! overlay's stack list contains every business panel after liana-gui's
//! built-in stacks.
//!
//! The whole module is gated by the `debugger` cargo feature.

use std::str::FromStr;
use std::sync::{Arc, OnceLock};

use liana::descriptors::LianaDescriptor;
use liana_gui::{
    app::wallet::Wallet,
    debug::{dashboard_chrome, DebugMessage, DebugPageEntry, DebugStack, SETTINGS_MENU},
};
use liana_ui::widget::Element;

use crate::settings::{ui::BusinessSettingsUI, views, BackendCurrency};

/// Sample descriptor used for `wallet_view`. Simple two-key Liana
/// descriptor — inert; nothing in the rendering path validates it.
const SAMPLE_DESCRIPTOR: &str = "wsh(or_d(pk([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*),and_v(v:pkh([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<2;3>/*),older(52596))))#x6u6lmej";

pub static ENTRY_LIST: DebugPageEntry = DebugPageEntry { view: render_list };
pub static ENTRY_WALLET: DebugPageEntry = DebugPageEntry {
    view: render_wallet,
};
pub static ENTRY_GENERAL_OFF: DebugPageEntry = DebugPageEntry {
    view: render_general_off,
};
pub static ENTRY_GENERAL_ON: DebugPageEntry = DebugPageEntry {
    view: render_general_on,
};
pub static ENTRY_ABOUT: DebugPageEntry = DebugPageEntry { view: render_about };

pub const BUSINESS_SETTINGS: DebugStack = DebugStack {
    name: "Business settings",
    menu: Some(&SETTINGS_MENU),
    pages: &[
        &ENTRY_LIST,
        &ENTRY_WALLET,
        &ENTRY_GENERAL_OFF,
        &ENTRY_GENERAL_ON,
        &ENTRY_ABOUT,
    ],
};

/// Slice handed to `liana_gui::gui::GUI::new` from `main.rs`. Append new
/// business / business-installer stacks here as they appear.
pub const EXTRA_STACKS: &[&DebugStack] = &[&BUSINESS_SETTINGS];

/// SAFETY: iced renders on the main thread; debug-overlay state is only
/// read during rendering, so satisfying `OnceLock`'s `Sync` bound with an
/// unconditional `unsafe impl Sync` is sound here. Mirrors the wrapper
/// used by `liana_gui::debug::installer_modals::StateCell`.
struct StateCell<T>(T);
unsafe impl<T> Sync for StateCell<T> {}

fn debug_settings_state() -> &'static BusinessSettingsUI {
    static STATE: OnceLock<StateCell<BusinessSettingsUI>> = OnceLock::new();
    &STATE
        .get_or_init(|| {
            let descriptor =
                LianaDescriptor::from_str(SAMPLE_DESCRIPTOR).expect("sample descriptor parses");
            let wallet = Arc::new(Wallet::new(descriptor));
            StateCell(BusinessSettingsUI::for_debug(wallet))
        })
        .0
}

fn render_list() -> Element<'static, DebugMessage> {
    let body = views::list_view().map(|_| ());
    dashboard_chrome(&SETTINGS_MENU, "Business settings — sections", body)
}

fn render_wallet() -> Element<'static, DebugMessage> {
    let body = views::wallet_view(debug_settings_state()).map(|_| ());
    dashboard_chrome(&SETTINGS_MENU, "Business settings — wallet", body)
}

fn render_general_off() -> Element<'static, DebugMessage> {
    let body = views::general_view(false, BackendCurrency::USD).map(|_| ());
    dashboard_chrome(
        &SETTINGS_MENU,
        "Business settings — general (fiat off)",
        body,
    )
}

fn render_general_on() -> Element<'static, DebugMessage> {
    let body = views::general_view(true, BackendCurrency::EUR).map(|_| ());
    dashboard_chrome(
        &SETTINGS_MENU,
        "Business settings — general (fiat on, EUR)",
        body,
    )
}

fn render_about() -> Element<'static, DebugMessage> {
    let body = views::about_view().map(|_| ());
    dashboard_chrome(&SETTINGS_MENU, "Business settings — about", body)
}
