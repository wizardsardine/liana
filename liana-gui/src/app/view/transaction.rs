use std::collections::{HashMap, HashSet};

use iced::{
    widget::{column, row, Space},
    Alignment, Length, Pixels,
};

use liana::descriptors::LianaPolicy;
use liana_ui::{
    component::{
        button::{
            self, btn_bump_fee, btn_cancel_transaction, btn_confirm, btn_delete,
            btn_go_to_replacement, btn_previous, btn_save,
        },
        form,
        modal::{modal_view, ModalWidth},
        panels::{self, fees_row, psbts, transactions},
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
            message::{CreateRbfMessage, Message, SpendTxMessage},
            warning::warn,
            FiatAmountConverter,
        },
    },
    daemon::model::{Fingerprint, HistoryTransaction, SpendStatus, SpendTx, Txid},
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

pub enum TxDetail<'a> {
    Transaction(&'a HistoryTransaction),
    Psbt {
        tx: &'a SpendTx,
        desc_info: &'a LianaPolicy,
        key_aliases: &'a HashMap<Fingerprint, String>,
        saved: bool,
        currently_signing: bool,
        previous: bool,
    },
}

pub fn tx_view<'a>(
    cache: &'a Cache,
    detail: TxDetail<'a>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    fiat_converter: Option<FiatAmountConverter>,
) -> Element<'a, Message> {
    let (txid, unsigned_tx, wallet_tx, labels, coins, label) = match &detail {
        TxDetail::Transaction(tx) => (
            tx.txid,
            &tx.tx,
            &tx.wallet_tx,
            &tx.labels,
            &tx.coins,
            tx.label(),
        ),
        TxDetail::Psbt { tx, .. } => (
            tx.psbt.unsigned_tx.compute_txid(),
            &tx.psbt.unsigned_tx,
            &tx.wallet_tx,
            &tx.labels,
            &tx.coins,
            tx.label(),
        ),
    };
    let txid = txid.to_string();

    let label = label::label_field(
        vec![txid.clone()],
        labels_editing.get(&txid),
        label,
        LabelSize::Display,
    );
    let pills = match &detail {
        TxDetail::Transaction(tx) if tx.time.is_some() => row![pill::confirmed()],
        TxDetail::Transaction(_) => row![pill::unconfirmed()],
        TxDetail::Psbt { tx, .. } => {
            let recovery = (!tx.sigs.recovery_paths().is_empty()).then_some(pill::recovery());
            row![recovery, psbts::status_pill(tx.status)]
        }
    };
    let label_row = row![label, pills.spacing(VSpacing::L).align_y(Alignment::Center)]
        .spacing(VSpacing::L)
        .align_y(Alignment::Center);
    let amount_row = transactions::amount_row(wallet_tx.kind().payment_kind(), wallet_tx.amount());
    let feerate = match &detail {
        TxDetail::Transaction(tx) => tx
            .feerate()
            .map(|rate| t!("common-feerate-value", rate = rate)),
        TxDetail::Psbt { tx, .. } => tx
            .min_feerate_vb()
            .map(|rate| t!("common-approx-feerate-value", rate = rate)),
    };
    if matches!(detail, TxDetail::Psbt { .. }) && wallet_tx.fee().is_none() {
        log::error!("Spend {} has an unknown fee", txid);
    }
    let to_fiat = fiat_converter.map(|c| move |a| c.convert(a));
    let miner_fee = fees_row(wallet_tx.fee(), feerate, to_fiat);
    // If unconfirmed, give option to use RBF.
    // Check fee amount is some as otherwise we may be missing coins for this transaction.
    let rbf = match &detail {
        TxDetail::Transaction(tx) => {
            (tx.time.is_none() && tx.wallet_tx.fee().is_some()).then(|| {
                row![
                    btn_bump_fee(Some(Message::CreateRbf(CreateRbfMessage::New(false)))),
                    btn_cancel_transaction(Some(Message::CreateRbf(CreateRbfMessage::New(true)))),
                ]
                .spacing(HSpacing::M)
            })
        }
        TxDetail::Psbt { .. } => None,
    };
    let summary = column![label_row, amount_row, miner_fee, rbf].spacing(VSpacing::L);

    let overview = overview(&detail, txid);
    let transaction = column![summary, overview].spacing(VSpacing::XL);

    let (is_incoming, owned_indexes, owned_default_labels) = match &detail {
        TxDetail::Transaction(tx) => (
            tx.is_incoming(),
            tx.owned_output_indexes(),
            Some(&tx.owned_outputs),
        ),
        TxDetail::Psbt { tx, .. } => (false, tx.change_indexes.clone(), None),
    };
    // We do not need to display inputs for external incoming transactions
    let inputs =
        (!is_incoming).then(|| view::psbt::inputs_view(coins, unsigned_tx, labels, labels_editing));
    let outputs = view::psbt::outputs_view(
        unsigned_tx,
        cache.network,
        &owned_indexes,
        labels,
        labels_editing,
        is_incoming,
        owned_default_labels,
    );
    let footer = match detail {
        TxDetail::Transaction(_) => None,
        TxDetail::Psbt {
            saved,
            currently_signing,
            previous,
            ..
        } => {
            let msg = |msg| (!currently_signing).then_some(msg);
            Some(
                if saved {
                    row![btn_delete(msg(Message::Spend(SpendTxMessage::Delete)))]
                } else {
                    let previous = previous.then(|| btn_previous(msg(Message::Previous)));
                    let save = btn_save(msg(Message::Spend(SpendTxMessage::Save)), false);
                    row![previous, Space::fill_width(), save]
                }
                .width(Length::Fill),
            )
        }
    };
    let details = column![
        column![inputs, outputs, footer].spacing(VSpacing::L),
        Space::with_height(VSpacing::S)
    ];

    let spacing: Pixels = match detail {
        TxDetail::Transaction(_) => 80.into(),
        TxDetail::Psbt { .. } => VSpacing::XXL.into(),
    };

    column![transaction, details].spacing(spacing).into()
}

fn overview<'a>(detail: &TxDetail<'a>, txid: String) -> Element<'a, Message> {
    let date = match detail {
        TxDetail::Transaction(tx) => Some(transactions::date_row(tx.datetime())),
        TxDetail::Psbt { .. } => None,
    };
    let ids = column![
        date,
        panels::txid_row(txid.clone(), Message::Clipboard(txid))
    ]
    .spacing(VSpacing::SM);
    let signatures = match detail {
        TxDetail::Transaction(_) => None,
        TxDetail::Psbt {
            tx,
            desc_info,
            key_aliases,
            saved,
            currently_signing,
            ..
        } => {
            let signatures = match tx.sigs.signed_path() {
                Some(sigs) => psbts::Signatures::Ready(sigs),
                None if tx.sigs.recovery_paths().is_empty() => {
                    psbts::Signatures::Missing(desc_info.primary_path(), tx.sigs.primary_path())
                }
                None => {
                    let (seq, sigs) = tx
                        .sigs
                        .recovery_paths()
                        .last_key_value()
                        .expect("not empty");
                    psbts::Signatures::Missing(&desc_info.recovery_paths()[seq], sigs)
                }
            };
            let enabled = *saved && !currently_signing;
            Some(psbts::signatures_card(
                signatures,
                key_aliases,
                *saved,
                enabled.then_some(Message::ExportPsbt),
                enabled.then_some(Message::ImportPsbt),
                (tx.status == SpendStatus::Unsigned)
                    .then_some(Message::Spend(SpendTxMessage::Sign)),
                (tx.status == SpendStatus::Broadcastable)
                    .then_some(Message::Spend(SpendTxMessage::Broadcast)),
            ))
        }
    };
    column![ids, signatures].spacing(VSpacing::L).into()
}
