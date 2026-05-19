//! Gallery of [`crate::app::view::home::home_view`] states, rendered inside the
//! production dashboard chrome with mock data.
//!
//! The payments list reuses the four variants from the payment-card gallery
//! (`debug/payment_cards.rs`); fiat price is always `None` in home for now, so
//! the two outgoing entries that differ only by fiat source render the same
//! until fiat support lands. `home_view` filters `PaymentKind::SendToSelf` out,
//! so that variant is carried for parity but does not appear in the list.
//!
//! `home_view` borrows balance / unconfirmed / events with the returned
//! widget's lifetime, so those live in a `OnceLock`; the remaining knobs are
//! call-scoped. Click messages are swallowed with `.map(|_| ())`.

use std::str::FromStr;
use std::sync::OnceLock;

use liana::miniscript::bitcoin::{Amount, OutPoint, Txid};
use liana_ui::widget::*;

use crate::{
    app::{
        cache::FiatPriceRequest,
        menu::Menu,
        view::{home, FiatAmountConverter},
        wallet::SyncStatus,
    },
    daemon::model::Payment,
    debug::{
        dashboard_chrome, dashboard_chrome_with_cache, payment_cards, rescanning_cache,
        DebugMessage, DebugPageEntry,
    },
    services::fiat::{Currency, PriceSource},
};

static MENU: Menu = Menu::Home;

pub static ENTRY_PAYMENTS: DebugPageEntry = DebugPageEntry {
    view: payments_view,
};
pub static ENTRY_EMPTY: DebugPageEntry = DebugPageEntry { view: empty_view };
pub static ENTRY_SYNCING: DebugPageEntry = DebugPageEntry { view: syncing_view };
pub static ENTRY_UNCONFIRMED: DebugPageEntry = DebugPageEntry {
    view: unconfirmed_view,
};
pub static ENTRY_RESCAN_WARNING: DebugPageEntry = DebugPageEntry {
    view: rescan_warning_view,
};
pub static ENTRY_EXPIRING: DebugPageEntry = DebugPageEntry {
    view: expiring_view,
};
pub static ENTRY_SEQUENCE_HINT: DebugPageEntry = DebugPageEntry {
    view: sequence_hint_view,
};
pub static ENTRY_PAGINATION: DebugPageEntry = DebugPageEntry {
    view: pagination_view,
};
pub static ENTRY_SIDEBAR_RESCAN: DebugPageEntry = DebugPageEntry {
    view: sidebar_rescan_view,
};
pub static ENTRY_FIAT: DebugPageEntry = DebugPageEntry { view: fiat_view };

struct Fixtures {
    balance: Amount,
    zero: Amount,
    unconfirmed: Amount,
    events: Vec<Payment>,
}

fn fixtures() -> &'static Fixtures {
    static F: OnceLock<Fixtures> = OnceLock::new();
    F.get_or_init(|| Fixtures {
        balance: Amount::from_sat(200_000),
        zero: Amount::from_sat(0),
        unconfirmed: Amount::from_sat(50_000),
        events: events(),
    })
}

/// The exact four payment-card gallery variants ([`payment_cards::variants`]),
/// as `Payment` rows. `Payment` has no fiat or status field: status is derived
/// from `time` by `home_view`, and fiat is dropped until home gains fiat support.
fn events() -> Vec<Payment> {
    payment_cards::variants()
        .into_iter()
        .enumerate()
        .map(|(i, v)| Payment {
            label: v.label.map(String::from),
            address: None,
            address_label: None,
            amount: Amount::from_sat(v.sats),
            outpoint: OutPoint {
                txid: Txid::from_str(&format!("{:02x}", i + 1).repeat(32))
                    .expect("64-char hex literal"),
                vout: 0,
            },
            time: v.time,
            kind: v.kind,
        })
        .collect()
}

fn expiring_coins() -> Vec<OutPoint> {
    vec![
        OutPoint {
            txid: Txid::from_str(&"ab".repeat(32)).expect("64-char hex literal"),
            vout: 0,
        },
        OutPoint {
            txid: Txid::from_str(&"cd".repeat(32)).expect("64-char hex literal"),
            vout: 1,
        },
    ]
}

fn fiat_converter() -> FiatAmountConverter {
    FiatAmountConverter::new(
        98_765.0,
        Some(1_700_000_000),
        FiatPriceRequest::new(PriceSource::MempoolSpace, Currency::USD),
    )
    .expect("positive price")
}

#[allow(clippy::too_many_arguments)]
fn home_body(
    unconfirmed_balance: &'static Amount,
    fiat: Option<FiatAmountConverter>,
    events: &'static [Payment],
    remaining_sequence: Option<u32>,
    expiring_coins: &[OutPoint],
    is_last_page: bool,
    processing: bool,
    sync_status: SyncStatus,
    show_rescan_warning: bool,
) -> Element<'static, DebugMessage> {
    let fx = fixtures();
    home::home_view(
        &fx.balance,
        unconfirmed_balance,
        &remaining_sequence,
        fiat,
        expiring_coins,
        events,
        is_last_page,
        processing,
        &sync_status,
        show_rescan_warning,
    )
    .map(|_| ())
}

fn payments_view() -> Element<'static, DebugMessage> {
    let fx = fixtures();
    let body = home_body(
        &fx.zero,
        None,
        &fx.events,
        None,
        &[],
        true,
        false,
        SyncStatus::Synced,
        false,
    );
    dashboard_chrome(&MENU, "Home: payments", body)
}

fn empty_view() -> Element<'static, DebugMessage> {
    let fx = fixtures();
    let body = home_body(
        &fx.zero,
        None,
        &[],
        None,
        &[],
        true,
        false,
        SyncStatus::Synced,
        false,
    );
    dashboard_chrome(&MENU, "Home: empty history", body)
}

fn syncing_view() -> Element<'static, DebugMessage> {
    let fx = fixtures();
    let body = home_body(
        &fx.zero,
        None,
        &fx.events,
        None,
        &[],
        true,
        false,
        SyncStatus::BlockchainSync(0.42),
        false,
    );
    dashboard_chrome(&MENU, "Home: syncing (blinking balance)", body)
}

fn unconfirmed_view() -> Element<'static, DebugMessage> {
    let fx = fixtures();
    let body = home_body(
        &fx.unconfirmed,
        None,
        &fx.events,
        None,
        &[],
        true,
        false,
        SyncStatus::Synced,
        false,
    );
    dashboard_chrome(&MENU, "Home: unconfirmed balance", body)
}

fn rescan_warning_view() -> Element<'static, DebugMessage> {
    let fx = fixtures();
    let body = home_body(
        &fx.zero,
        None,
        &fx.events,
        None,
        &[],
        true,
        false,
        SyncStatus::Synced,
        true,
    );
    dashboard_chrome(&MENU, "Home: rescan warning", body)
}

fn expiring_view() -> Element<'static, DebugMessage> {
    let fx = fixtures();
    let expiring = expiring_coins();
    let body = home_body(
        &fx.zero,
        None,
        &fx.events,
        None,
        &expiring,
        true,
        false,
        SyncStatus::Synced,
        false,
    );
    dashboard_chrome(&MENU, "Home: recovery available", body)
}

fn sequence_hint_view() -> Element<'static, DebugMessage> {
    let fx = fixtures();
    let body = home_body(
        &fx.zero,
        None,
        &fx.events,
        Some(65_000),
        &[],
        true,
        false,
        SyncStatus::Synced,
        false,
    );
    dashboard_chrome(&MENU, "Home: recovery sequence hint", body)
}

fn pagination_view() -> Element<'static, DebugMessage> {
    let fx = fixtures();
    let body = home_body(
        &fx.zero,
        None,
        &fx.events,
        None,
        &[],
        false,
        false,
        SyncStatus::Synced,
        false,
    );
    dashboard_chrome(&MENU, "Home: see more", body)
}

fn sidebar_rescan_view() -> Element<'static, DebugMessage> {
    let fx = fixtures();
    let body = home_body(
        &fx.zero,
        None,
        &fx.events,
        None,
        &[],
        true,
        false,
        SyncStatus::Synced,
        false,
    );
    dashboard_chrome_with_cache(
        &MENU,
        "Home: sidebar rescan progress",
        rescanning_cache(),
        body,
    )
}

fn fiat_view() -> Element<'static, DebugMessage> {
    let fx = fixtures();
    let body = home_body(
        &fx.unconfirmed,
        Some(fiat_converter()),
        &fx.events,
        None,
        &[],
        true,
        false,
        SyncStatus::Synced,
        false,
    );
    dashboard_chrome(&MENU, "Home: fiat balance", body)
}
