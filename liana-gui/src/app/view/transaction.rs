use std::collections::{HashMap, HashSet};

use iced::{
    widget::{column, row, Space},
    Alignment,
};

use liana::{miniscript::bitcoin::Amount, transaction::PaymentKind};
use liana_ui::{
    component::{
        amount::{amount_with_fiat_tooltip, AmountSize, FiatAmount},
        button::{self, btn_bump_fee, btn_cancel_transaction, btn_confirm, btn_go_to_replacement},
        form,
        modal::{modal_view, ModalWidth},
        panels::{fees_row, home::payment::kind_icon, transactions},
        pill,
        text::new,
    },
    icon,
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{Column, Element, SpaceExt},
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{
            self,
            label::{self, LabelSize},
            message::{CreateRbfMessage, Message},
            warning::warn,
            FiatAmountConverter,
        },
    },
    daemon::model::{HistoryTransaction, Txid},
    t,
};

/// Return the modal view for a new RBF transaction.
///
/// `descendant_txids` contains the IDs of any transactions from this wallet that are
/// direct descendants of the transaction to be replaced.
pub fn create_rbf_modal<'a>(
    is_cancel: bool,
    descendant_txids: &HashSet<Txid>,
    feerate: &form::Value<String>,
    replacement_txid: Option<Txid>,
    warning: Option<&'a Error>,
    processing: bool,
) -> Element<'a, Message> {
    let confirm_msg =
        (feerate.valid || is_cancel).then_some(Message::CreateRbf(CreateRbfMessage::Confirm));
    let confirm_button = btn_confirm(confirm_msg);
    let help_text = if is_cancel {
        t!("transactions-rbf-cancel-help")
    } else {
        t!("transactions-rbf-bump-help")
    };
    let descendants = (!descendant_txids.is_empty()).then(|| {
        let invalidates = if descendant_txids.len() > 1 {
            t!("transactions-rbf-invalidates-some")
        } else {
            t!("transactions-rbf-invalidates-one")
        };
        let descendants = if descendant_txids.len() > 1 {
            t!("transactions-rbf-descendants-some")
        } else {
            t!("transactions-rbf-descendants-one")
        };

        let txids =
            descendant_txids
                .iter()
                .fold(Column::new().spacing(VSpacing::XS), |col, txid| {
                    col.push(
                        row![
                            new::caption(txid.to_string()),
                            button::btn_copy(Some(Message::Clipboard(txid.to_string())))
                        ]
                        .spacing(HSpacing::S)
                        .align_y(Alignment::Center),
                    )
                });
        (invalidates, descendants, txids)
    });
    let descendants = descendants.map(|(invalidates, descendants, txids)| {
        column![
            row![icon::warning_icon(), new::caption(invalidates)].spacing(HSpacing::S),
            row![
                Space::with_width(HSpacing::XL),
                column![new::caption(descendants), txids].spacing(VSpacing::XS)
            ],
        ]
        .spacing(VSpacing::XS)
    });
    let feerate_form = (!is_cancel).then(|| {
        if replacement_txid.is_none() {
            form::Form::new_trimmed("", feerate, move |msg| {
                Message::CreateRbf(CreateRbfMessage::FeerateEdited(msg))
            })
            .warning(t!("transactions-rbf-feerate-warning"))
        } else {
            form::Form::new_disabled("", feerate)
        }
    });
    let feerate = feerate_form.map(|form| {
        row![new::caption(t!("common-feerate")), form]
            .spacing(HSpacing::S)
            .align_y(Alignment::Center)
    });
    let status = if replacement_txid.is_none() {
        row![confirm_button]
    } else {
        row![
            icon::circle_check_icon().style(theme::text::secondary),
            new::caption(t!("transactions-rbf-created")).style(theme::text::success)
        ]
        .spacing(HSpacing::S)
        .align_y(Alignment::Center)
    };
    let replacement = replacement_txid
        .map(|id| btn_go_to_replacement(Some(Message::Menu(Menu::PsbtPreSelected(id)))));

    let close = (!processing).then_some(Message::CreateRbf(CreateRbfMessage::Cancel));
    let content = column![
        new::caption(help_text),
        descendants,
        feerate,
        warn(warning),
        status,
        replacement,
    ]
    .spacing(VSpacing::S);

    modal_view(
        Some(t!("transactions-replacement")),
        None,
        close,
        ModalWidth::XL,
        content,
    )
}

pub fn tx_view<'a>(
    cache: &'a Cache,
    tx: &'a HistoryTransaction,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    fiat_converter: Option<FiatAmountConverter>,
) -> Element<'a, Message> {
    let txid = tx.txid.to_string();

    let confirmed = if tx.time.is_some() {
        pill::confirmed()
    } else {
        pill::unconfirmed()
    };
    let label = label::label_field(
        vec![txid.clone()],
        labels_editing.get(&txid),
        tx.label(),
        LabelSize::Display,
    );
    let label_row = row![label, confirmed]
        .spacing(VSpacing::L)
        .align_y(Alignment::Center);
    let kind = tx.wallet_tx.kind().payment_kind();
    let amount: Element<'a, Message> = if kind == PaymentKind::SendToSelf {
        new::d2(t!("common-self-transfer")).into()
    } else {
        amount_with_fiat_tooltip(
            &tx.wallet_tx.amount(),
            None::<fn(Amount) -> FiatAmount>,
            AmountSize::L,
            true,
            None,
        )
    };
    let amount_row = row![kind_icon(kind), amount]
        .spacing(HSpacing::S)
        .align_y(Alignment::Center);
    let feerate = tx
        .feerate()
        .map(|rate| t!("common-feerate-value", rate = rate));
    let to_fiat = fiat_converter.map(|c| move |a| c.convert(a));
    let miner_fee = fees_row(tx.wallet_tx.fee(), feerate, to_fiat);
    // If unconfirmed, give option to use RBF.
    // Check fee amount is some as otherwise we may be missing coins for this transaction.
    let rbf = (tx.time.is_none() && tx.wallet_tx.fee().is_some()).then(|| {
        row![
            btn_bump_fee(Some(Message::CreateRbf(CreateRbfMessage::New(false)))),
            btn_cancel_transaction(Some(Message::CreateRbf(CreateRbfMessage::New(true)))),
        ]
        .spacing(HSpacing::M)
    });
    let summary = column![label_row, amount_row, miner_fee, rbf].spacing(VSpacing::L);
    let overview = transactions::overview(tx.datetime(), txid.clone(), Message::Clipboard(txid));
    let transaction = column![summary, overview].spacing(VSpacing::XL);

    // We do not need to display inputs for external incoming transactions
    let inputs = (!tx.is_incoming())
        .then(|| view::psbt::inputs_view(&tx.coins, &tx.tx, &tx.labels, labels_editing));
    let outputs = view::psbt::outputs_view(
        &tx.tx,
        cache.network,
        &tx.owned_output_indexes(),
        &tx.labels,
        labels_editing,
        tx.is_incoming(),
        Some(&tx.owned_outputs),
    );

    let details = column![
        column![inputs, outputs].spacing(VSpacing::L),
        Space::with_height(VSpacing::S)
    ];

    column![transaction, details].spacing(80).into()
}
