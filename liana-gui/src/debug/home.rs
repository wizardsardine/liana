//! Renders the production [`crate::app::view::home::home_view`] with mock
//! data covering every payment row variant: confirmed/unconfirmed crossed
//! with incoming/outgoing, plus label / address-label / no-label cases.
//!
//! `home_view` returns `Element<'a, Message>` borrowing from balance and
//! events refs, so we hold those in a `OnceLock` to give the returned widget
//! `'static` lifetime, then `.map(|_| ())` to swallow click messages at the
//! debug-overlay boundary.
//!
//! `PaymentKind::SendToSelf` is filtered out by `home_view` itself
//! (`view/home.rs::home_view`), so it isn't represented in the sample.

use std::str::FromStr;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use liana::miniscript::bitcoin::{Amount, OutPoint, Txid};

use liana_ui::{component::payment::PaymentKind, widget::*};

use crate::{
    app::{menu::Menu, view::home, wallet::SyncStatus},
    daemon::model::Payment,
    debug::{dashboard_chrome, DebugMessage, DebugPageEntry},
};

static MENU: Menu = Menu::Home;

pub static ENTRY: DebugPageEntry = DebugPageEntry { view };

struct MockData {
    balance: Amount,
    unconfirmed: Amount,
    events: Vec<Payment>,
}

fn mock_data() -> &'static MockData {
    static DATA: OnceLock<MockData> = OnceLock::new();
    DATA.get_or_init(|| MockData {
        balance: Amount::from_sat(123_456_789),
        unconfirmed: Amount::from_sat(50_000),
        events: sample_events(),
    })
}

fn payment(
    nonce: u8,
    kind: PaymentKind,
    sats: u64,
    time: Option<i64>,
    label: Option<&str>,
    address_label: Option<&str>,
) -> Payment {
    let hex = format!("{nonce}").repeat(32);
    Payment {
        label: label.map(String::from),
        address: None,
        address_label: address_label.map(String::from),
        amount: Amount::from_sat(sats),
        outpoint: OutPoint {
            txid: Txid::from_str(&hex).expect("32-byte hex literal"),
            vout: 0,
        },
        time: time.and_then(|t| DateTime::<Utc>::from_timestamp(t, 0)),
        kind,
    }
}

fn sample_events() -> Vec<Payment> {
    let now: i64 = 1_700_000_000;
    vec![
        // Unconfirmed (Payment::compare puts these first).
        payment(
            0x10,
            PaymentKind::Incoming,
            250_000,
            None,
            Some("salary advance"),
            None,
        ),
        payment(0x11, PaymentKind::Incoming, 8_421, None, None, None),
        payment(
            0x20,
            PaymentKind::Outgoing,
            75_000,
            None,
            Some("groceries"),
            None,
        ),
        payment(0x21, PaymentKind::Outgoing, 1_500, None, None, None),
        // Confirmed.
        payment(
            0x30,
            PaymentKind::Incoming,
            500_000_000,
            Some(now - 3_600),
            Some("invoice #42"),
            None,
        ),
        payment(
            0x31,
            PaymentKind::Incoming,
            21_000,
            Some(now - 7_200),
            None,
            Some("vault deposit"),
        ),
        payment(
            0x32,
            PaymentKind::Incoming,
            137,
            Some(now - 86_400),
            None,
            None,
        ),
        payment(
            0x40,
            PaymentKind::Outgoing,
            1_234_567,
            Some(now - 4_000),
            Some("rent"),
            None,
        ),
        payment(
            0x41,
            PaymentKind::Outgoing,
            999,
            Some(now - 90_000),
            None,
            None,
        ),
    ]
}

fn view() -> Element<'static, DebugMessage> {
    let data = mock_data();
    let body = home::home_view(
        &data.balance,
        &data.unconfirmed,
        &None,
        None,
        &[],
        &data.events,
        true,
        false,
        &SyncStatus::Synced,
        false,
    )
    .map(|_| ());
    dashboard_chrome(&MENU, "Home view — payment variants", body)
}
