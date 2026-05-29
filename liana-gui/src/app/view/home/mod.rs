pub mod payment_details;
use iced::widget::column;
pub use payment_details::payment_details_view;

use liana::miniscript::bitcoin;
use liana_ui::{
    component::{
        home::{self, rescan_warning, SyncProgress},
        payment::{self, payment_card, PaymentKind, UIPayment},
        text::new,
    },
    widget::{Column, ColumnExt, Element},
};

use crate::{
    app::{
        menu::{self, Menu},
        view::{coins, message::Message, FiatAmountConverter},
        wallet::SyncStatus,
    },
    daemon::model::Payment,
};

#[allow(clippy::too_many_arguments)]
pub fn home_view<'a>(
    balance: &'a bitcoin::Amount,
    unconfirmed_balance: &'a bitcoin::Amount,
    remaining_sequence: &Option<u32>,
    fiat_converter: Option<FiatAmountConverter>,
    expiring_coins: &[bitcoin::OutPoint],
    events: &'a [Payment],
    is_last_page: bool,
    processing: bool,
    sync_status: &SyncStatus,
    show_rescan_warning: bool,
) -> Element<'a, Message> {
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(*balance));
    let fiat_unconfirmed = fiat_converter.map(|c| c.convert(*unconfirmed_balance));
    let sync = (!sync_status.is_synced()).then_some(match sync_status {
        SyncStatus::BlockchainSync(progress) => SyncProgress::Blockchain(*progress),
        SyncStatus::WalletFullScan => SyncProgress::FullScan,
        _ => SyncProgress::Transactions,
    });
    let balance = Column::new()
        .push(home::balance(
            balance,
            fiat_balance.map(|fiat| fiat.to_display_string()),
            sync.is_some(),
        ))
        .push_maybe(sync.map(home::syncing))
        .push_maybe(
            (unconfirmed_balance.to_sat() != 0 && sync_status.is_synced()).then(|| {
                home::unconfirmed_balance(
                    unconfirmed_balance,
                    fiat_unconfirmed.map(|fiat| fiat.to_display_string()),
                )
            }),
        );

    fn recovery_warning<'a>(expiring_coins: &[bitcoin::OutPoint]) -> Element<'a, Message> {
        home::recovery_warning(
            expiring_coins.len(),
            Message::Menu(Menu::RefreshCoins(expiring_coins.to_owned())),
        )
    }

    let expire_warning = if expiring_coins.is_empty() {
        remaining_sequence
            .map(|sequence| home::recovery_hint(coins::expire_message_units(sequence).join(", ")))
    } else {
        Some(recovery_warning(expiring_coins))
    };

    let rescan_warn = show_rescan_warning.then(|| {
        rescan_warning(
            Message::Menu(Menu::SettingsPreSelected(menu::SettingsOption::Node)),
            Message::HideRescanWarning,
        )
    });

    let history = events.iter().fold(Column::new().spacing(14), |col, event| {
        if event.kind != PaymentKind::SendToSelf {
            col.push(payment_card(
                UIPayment {
                    label: event.label.as_deref().or(event.address_label.as_deref()),
                    kind: event.kind,
                    time: event.time,
                    amount: event.amount,
                    fiat_price: None,
                },
                Some(Message::SelectPayment(event.outpoint)),
            ))
        } else {
            col
        }
    });

    let see_more =
        (!is_last_page && !events.is_empty()).then(|| payment::see_more(processing, Message::Next));

    #[rustfmt::skip]
    let payment_list = column![
        new::d3(crate::t!("home-payment-history")),
        history,
        see_more
    ].spacing(14);

    column![
        column![new::d2(crate::t!("home-balance")), balance].spacing(20),
        rescan_warn,
        expire_warning,
        payment_list
    ]
    .spacing(40)
    .into()
}
