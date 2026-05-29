//! Gallery of `liana_ui::component::payment::payment_card` variants.
//!
//! The four [`variants`] are the single source of truth, also reused by the
//! home-panel debug pages (`debug/home.rs`) to populate the payments list.

use liana::miniscript::bitcoin::Amount;
use liana_ui::{
    component::payment::{payment_card, FiatPrice, FiatSource, PaymentKind, UIPayment},
    widget::*,
};

use crate::debug::{debug_chrome, DebugMessage, DebugPageEntry};

pub static ENTRY: DebugPageEntry = DebugPageEntry {
    view: payments_view,
};

pub struct Variant {
    pub label: Option<&'static str>,
    pub kind: PaymentKind,
    pub time: Option<chrono::DateTime<chrono::Utc>>,
    pub sats: u64,
    pub fiat: Option<FiatPrice>,
}

fn time() -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::TimeZone;
    Some(chrono::Utc.with_ymd_and_hms(2025, 10, 24, 0, 0, 0).unwrap())
}

fn fiat(amount: &str, source: FiatSource) -> Option<FiatPrice> {
    Some(FiatPrice {
        amount: amount.to_string(),
        currency: "USD".to_string(),
        source,
    })
}

/// The four payment-card variants shown in this gallery.
pub fn variants() -> Vec<Variant> {
    vec![
        Variant {
            label: None,
            kind: PaymentKind::SendToSelf,
            time: time(),
            sats: 123_456_000_000,
            fiat: None,
        },
        Variant {
            label: Some("Manu May Salary"),
            kind: PaymentKind::Outgoing,
            time: time(),
            sats: 75_000,
            fiat: fiat("1,234", FiatSource::User),
        },
        Variant {
            label: Some("Sofia September Salary"),
            kind: PaymentKind::Outgoing,
            time: time(),
            sats: 75_000,
            fiat: fiat("1,234", FiatSource::Wizardsardine),
        },
        Variant {
            label: Some("Veeeeeeeeeeeeeeeeeeeeeeeeery long laaaaaaaaaaaaaaaabel!"),
            kind: PaymentKind::Incoming,
            time: time(),
            sats: 123_000_000,
            fiat: fiat("1,234", FiatSource::Timestamp),
        },
        Variant {
            label: Some("Pending invoice"),
            kind: PaymentKind::Outgoing,
            time: None,
            sats: 42_000,
            fiat: fiat("12", FiatSource::User),
        },
        Variant {
            label: None,
            kind: PaymentKind::Incoming,
            time: time(),
            sats: 2_100_000,
            fiat: fiat("34", FiatSource::Timestamp),
        },
    ]
}

fn card(variant: Variant) -> Element<'static, DebugMessage> {
    let Variant {
        label,
        kind,
        time,
        sats,
        fiat,
    } = variant;
    let payment = UIPayment {
        label,
        kind,
        time,
        amount: Amount::from_sat(sats),
        fiat_price: fiat,
    };
    payment_card(payment, Some(()))
}

fn payments_view() -> Element<'static, DebugMessage> {
    let body = variants()
        .into_iter()
        .fold(Column::new().spacing(20), |col, variant| {
            col.push(card(variant))
        });
    debug_chrome("Payment card: outgoing", body)
}
