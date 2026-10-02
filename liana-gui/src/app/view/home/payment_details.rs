use std::{collections::HashMap, vec};

use iced::{
    widget::{column, row},
    Alignment,
};

use liana::miniscript::bitcoin::{self, Amount};
use liana_ui::{
    component::{
        amount::{amount_with_fiat_tooltip, AmountSize, FiatAmount},
        button::btn_see_transaction_details,
        form,
        panels::{fees_row, home::payment::kind_icon, transactions},
        section,
    },
    spacing::{HSpacing, VSpacing},
    widget::Element,
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{
            dashboard,
            label::{self, LabelSize},
            message::Message,
        },
    },
    daemon::model::{HistoryTransaction, Payment},
    t,
};

pub fn payment_details_view<'a>(
    cache: &'a Cache,
    tx: &'a HistoryTransaction,
    output_index: usize,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let txid = tx.txid.to_string();
    let outpoint = bitcoin::OutPoint::new(tx.txid, output_index as u32).to_string();
    let title = if tx.wallet_tx.is_outgoing() {
        t!("payment-outgoing")
    } else if tx.wallet_tx.is_incoming() {
        t!("payment-incoming")
    } else {
        t!("payment-title")
    };
    let title = transactions::title(title, tx.time.is_some());
    let payment = Payment::from_tx_output(tx, output_index);
    let payment_label = label::label_field(
        vec![outpoint.clone()],
        labels_editing.get(&outpoint),
        payment
            .as_ref()
            .map(|payment| payment.label())
            .unwrap_or_default(),
        LabelSize::Title,
    );
    let amount = tx.tx.output[output_index].value;
    let amount = amount_with_fiat_tooltip(
        &amount,
        None::<fn(Amount) -> FiatAmount>,
        AmountSize::L,
        true,
        None,
    );
    let kind = payment.map(|payment| kind_icon(payment.kind));
    let amount = row![kind, amount]
        .spacing(HSpacing::S)
        .align_y(Alignment::Center);
    let tx_section = section(t!("transactions-transaction"));
    let feerate = tx
        .feerate()
        .map(|rate| t!("common-feerate-value", rate = rate));
    let fee = tx.wallet_tx.fee().map(|fee| fees_row(Some(fee), feerate));
    let see_details =
        btn_see_transaction_details(Message::Menu(Menu::TransactionPreSelected(tx.txid)));
    let overview = transactions::overview(
        tx.time,
        txid.clone(),
        Message::Clipboard(txid),
        Some(see_details.into()),
    );

    dashboard(
        &Menu::Home,
        cache,
        warning,
        column![title, payment_label, amount, tx_section, fee, overview].spacing(VSpacing::L),
    )
}
