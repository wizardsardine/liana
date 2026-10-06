use std::{collections::HashMap, vec};

use iced::{
    widget::{column, row, Space},
    Alignment,
};

use liana::miniscript::bitcoin::{self, Amount};
use liana_ui::{
    component::{
        amount::{amount_with_fiat_tooltip, AmountSize, FiatAmount},
        button::btn_see_more_details,
        form,
        panels::{fees_row, home::payment::kind_icon, transactions},
        pill, section,
    },
    spacing::{HSpacing, VSpacing},
    widget::{Element, SpaceExt},
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
            FiatAmountConverter,
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
    fiat_converter: Option<FiatAmountConverter>,
) -> Element<'a, Message> {
    let txid = tx.txid.to_string();
    let outpoint = bitcoin::OutPoint::new(tx.txid, output_index as u32).to_string();

    let confirmed = if tx.time.is_some() {
        pill::confirmed()
    } else {
        pill::unconfirmed()
    };
    let payment = Payment::from_tx_output(tx, output_index);
    let label = payment
        .as_ref()
        .map(|payment| payment.label())
        .unwrap_or_default();
    let payment_label = label::label_field(
        vec![outpoint.clone()],
        labels_editing.get(&outpoint),
        &label,
        LabelSize::Display,
    );
    let label_row = row![payment_label, confirmed]
        .spacing(VSpacing::L)
        .align_y(Alignment::Center);
    let amount = tx.tx.output[output_index].value;
    let amount = amount_with_fiat_tooltip(
        &amount,
        None::<fn(Amount) -> FiatAmount>,
        AmountSize::L,
        true,
        None,
    );
    let kind = payment.map(|payment| kind_icon(payment.kind));
    let amount_row = row![kind, amount]
        .spacing(HSpacing::S)
        .align_y(Alignment::Center);
    let payment = column![label_row, amount_row].spacing(VSpacing::L);

    let tx_section = section(t!("transactions-transaction"));
    let overview = transactions::overview(tx.datetime(), txid.clone(), Message::Clipboard(txid));
    let transaction = column![tx_section, overview].spacing(VSpacing::SM);

    let feerate = tx
        .feerate()
        .map(|rate| t!("common-feerate-value", rate = rate));
    let to_fiat = fiat_converter.map(|c| move |a| c.convert(a));
    let miner_fee = fees_row(tx.wallet_tx.fee(), feerate, to_fiat);

    let see_details = btn_see_more_details(Message::Menu(Menu::TransactionPreSelected(tx.txid)));
    let btn_row = row![Space::fill_width(), see_details];
    let transaction = column![transaction, miner_fee, btn_row].spacing(VSpacing::XL);

    let content = column![payment, transaction].spacing(80);

    dashboard(&Menu::Home, cache, warning, content)
}
