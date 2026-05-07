//! Renders rows that mirror [`crate::app::view::transactions::tx_list_view`]
//! (`liana-gui/src/app/view/transactions.rs:95`), with mock data covering
//! every visual variant the user encounters in production: incoming /
//! outgoing / send-to-self crossed with confirmed / unconfirmed and
//! single-payment / batch.
//!
//! Construction is inlined rather than calling `tx_list_view` directly,
//! because that helper is private and `transactions_view` wraps the list in
//! the dashboard chrome (sidebar + cache). This mirrors the
//! "wrapped" pattern used by `debug::cards`.

use chrono::{DateTime, Local, Utc};
use iced::{Alignment, Length};
use liana::miniscript::bitcoin::Amount;

use liana_ui::{
    component::{amount::amount, badge, text::*},
    theme,
    widget::*,
};

use crate::{
    app::menu::Menu,
    debug::{dashboard_chrome, DebugMessage, DebugPageEntry},
};

static MENU: Menu = Menu::Transactions;

pub static ENTRY: DebugPageEntry = DebugPageEntry { view };

#[derive(Clone, Copy)]
enum Direction {
    Incoming,
    Outgoing,
    SelfTransfer,
}

struct TxRow {
    direction: Direction,
    label: Option<&'static str>,
    time: Option<i64>,
    is_batch: bool,
    sats: u64,
}

fn tx_row(row: TxRow) -> Element<'static, DebugMessage> {
    let badge_widget: Container<'static, DebugMessage> = match row.direction {
        Direction::Incoming => badge::receive(),
        Direction::Outgoing => badge::spend(),
        Direction::SelfTransfer => badge::cycle(),
    };

    let date_widget = row.time.map(|t| {
        Container::new(
            text(
                DateTime::<Utc>::from_timestamp(t, 0)
                    .expect("valid unix timestamp")
                    .with_timezone(&Local)
                    .format("%b. %d, %Y - %T")
                    .to_string(),
            )
            .style(theme::text::secondary)
            .small(),
        )
    });

    let amount_value = Amount::from_sat(row.sats);
    let amount_widget: Row<'static, DebugMessage> = match row.direction {
        Direction::Incoming => Row::new()
            .spacing(5)
            .align_y(Alignment::Center)
            .push(text("+"))
            .push(amount(&amount_value)),
        Direction::Outgoing => Row::new()
            .spacing(5)
            .align_y(Alignment::Center)
            .push(text("-"))
            .push(amount(&amount_value)),
        Direction::SelfTransfer => Row::new().push(text("Self-transfer")),
    };

    let body = Row::new()
        .spacing(20)
        .align_y(Alignment::Center)
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(badge_widget)
                .push(
                    Column::new()
                        .push_maybe(row.label.map(p1_regular))
                        .push_maybe(date_widget),
                )
                .width(Length::Fill),
        )
        .push_maybe(row.time.is_none().then(badge::unconfirmed))
        .push_maybe(row.is_batch.then(badge::batch))
        .push(amount_widget);

    Container::new(
        Button::new(body)
            .padding(10)
            .on_press(())
            .style(theme::button::transparent_border),
    )
    .style(theme::card::button_simple)
    .into()
}

fn view() -> Element<'static, DebugMessage> {
    let now: i64 = 1_700_000_000;

    let rows: Vec<Element<'static, DebugMessage>> = vec![
        tx_row(TxRow {
            direction: Direction::Outgoing,
            label: Some("rent"),
            time: None,
            is_batch: false,
            sats: 1_500_000,
        }),
        tx_row(TxRow {
            direction: Direction::Outgoing,
            label: None,
            time: None,
            is_batch: true,
            sats: 4_200_000,
        }),
        tx_row(TxRow {
            direction: Direction::Incoming,
            label: Some("invoice batch"),
            time: None,
            is_batch: true,
            sats: 750_000,
        }),
        tx_row(TxRow {
            direction: Direction::Incoming,
            label: None,
            time: None,
            is_batch: false,
            sats: 12_345,
        }),
        tx_row(TxRow {
            direction: Direction::Outgoing,
            label: Some("supplier"),
            time: Some(now - 3_600),
            is_batch: false,
            sats: 250_000,
        }),
        tx_row(TxRow {
            direction: Direction::Outgoing,
            label: None,
            time: Some(now - 7_200),
            is_batch: true,
            sats: 6_543_210,
        }),
        tx_row(TxRow {
            direction: Direction::Incoming,
            label: Some("payroll"),
            time: Some(now - 86_400),
            is_batch: false,
            sats: 25_000_000,
        }),
        tx_row(TxRow {
            direction: Direction::Incoming,
            label: None,
            time: Some(now - 172_800),
            is_batch: true,
            sats: 9_999,
        }),
        tx_row(TxRow {
            direction: Direction::SelfTransfer,
            label: Some("vault rotation"),
            time: Some(now - 4_000),
            is_batch: false,
            sats: 0,
        }),
        tx_row(TxRow {
            direction: Direction::SelfTransfer,
            label: None,
            time: None,
            is_batch: false,
            sats: 0,
        }),
    ];

    let body = rows
        .into_iter()
        .fold(Column::new().spacing(10), Column::push);
    dashboard_chrome(&MENU, "Transactions list — variants", body)
}
