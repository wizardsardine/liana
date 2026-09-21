use std::collections::{HashMap, HashSet};

use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};

use liana::{
    descriptors::{LianaDescriptor, LianaPolicy},
    miniscript::bitcoin::{
        bip32::Fingerprint, blockdata::transaction::TxOut, Address, Network, OutPoint, Transaction,
        Txid,
    },
};

use liana_ui::{
    component::{
        button::{self, btn_broadcast, btn_delete, btn_save, btn_sign},
        form,
        list::DeviceStatus,
        modal::{self, modal_view, ModalWidth},
        notification,
        panels::psbts,
        pill,
        text::new,
    },
    icon,
    spacing::{HSpacing, VSpacing},
    widget::{Column, Container, Element, SpaceExt},
};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        view::{dashboard, label, message::*, warning::warn},
    },
    daemon::model::{Coin, SpendStatus, SpendTx},
    hw::HardwareWallet,
    t,
    view::hw::{device_list_entry, HwRowMode},
};

const LABEL_TITLE_SIZE: u32 = 24;
const LABEL_BODY_SIZE: u32 = 16;
/// Indent of the conflicting transactions listed under the broadcast warning.
const CONFLICT_INDENT: [u16; 2] = [0 /* Top/Bottom */, 30 /* Left/Right */];

#[allow(clippy::too_many_arguments)]
pub fn psbt_view<'a>(
    cache: &'a Cache,
    tx: &'a SpendTx,
    saved: bool,
    desc_info: &'a LianaPolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    network: Network,
    currently_signing: bool,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let recovery = (!tx.sigs.recovery_paths().is_empty()).then_some(pill::recovery());
    let status = psbts::status_pill(tx.status);
    let header = row![
        Container::new(new::h1(t!("psbt-title"))).width(Length::Fill),
        recovery,
        status
    ]
    .align_y(Alignment::Center)
    .spacing(HSpacing::M);

    let inputs = inputs_view(&tx.coins, &tx.psbt.unsigned_tx, &tx.labels, labels_editing);
    let outputs = outputs_view(
        &tx.psbt.unsigned_tx,
        network,
        &tx.change_indexes,
        &tx.labels,
        labels_editing,
        tx.is_single_payment().is_some(),
        false,
    );

    let action = if saved {
        let delete_msg = (!currently_signing).then_some(Message::Spend(SpendTxMessage::Delete));
        row![btn_delete(delete_msg)].width(Length::Fill)
    } else {
        let save_msg = (!currently_signing).then_some(Message::Spend(SpendTxMessage::Save));
        row![Space::fill_width(), btn_save(save_msg, false)].width(Length::Fill)
    };

    let content = column![
        header,
        spend_header(tx, labels_editing),
        spend_overview_view(tx, desc_info, key_aliases, currently_signing, saved),
        column![inputs, outputs].spacing(VSpacing::L),
        action,
        Space::with_height(VSpacing::S)
    ]
    .spacing(VSpacing::L);

    dashboard(&Menu::PSBTs, cache, warning, content)
}

pub fn save_action<'a>(warning: Option<&Error>, saved: bool) -> Element<'a, Message> {
    let content: Element<'a, Message> = if saved {
        Container::new(new::caption(t!("psbt-transaction-saved")))
            .align_x(iced::alignment::Horizontal::Center)
            .into()
    } else {
        let ignore = button::btn_ignore(Some(Message::Close));
        let save = button::btn_save(Some(Message::Spend(SpendTxMessage::Confirm)), true);
        let buttons = row![Space::fill_width(), ignore, save].spacing(HSpacing::M);

        column![
            warning.map(|w| warn(Some(w))),
            new::caption(t!("psbt-save-transaction")),
            buttons
        ]
        .spacing(VSpacing::S)
        .into()
    };

    modal_view(None::<String>, None, None, ModalWidth::S, content)
}

/// Return the modal view to broadcast a transaction.
///
/// `conflicting_txids` contains the IDs of any directly conflicting transactions
/// of the transaction to be broadcast.
pub fn broadcast_action<'a>(
    conflicting_txids: &HashSet<Txid>,
    warning: Option<&Error>,
    saved: bool,
) -> Element<'a, Message> {
    if saved {
        let content = Container::new(new::caption(t!("psbt-transaction-broadcast")))
            .align_x(iced::alignment::Horizontal::Center);
        return modal_view(None::<String>, None, None, ModalWidth::S, content);
    }

    let conflicts = (!conflicting_txids.is_empty()).then(|| {
        let (invalidates, conflicts) = if conflicting_txids.len() > 1 {
            (
                t!("psbt-broadcast-invalidates-some"),
                t!("psbt-broadcast-conflicts-some"),
            )
        } else {
            (
                t!("psbt-broadcast-invalidates-one"),
                t!("psbt-broadcast-conflicts-one"),
            )
        };

        let warning = row![icon::warning_icon(), new::caption(invalidates)].spacing(HSpacing::M);
        let explanation = row![new::caption(conflicts)].padding(CONFLICT_INDENT);

        conflicting_txids.iter().fold(
            column![warning, explanation].spacing(VSpacing::XS),
            |col, txid| {
                let copy = button::btn_copy(Some(Message::Clipboard(txid.to_string())));
                col.push(
                    row![new::caption(txid.to_string()), copy]
                        .padding(CONFLICT_INDENT)
                        .spacing(HSpacing::S)
                        .align_y(Alignment::Center),
                )
            },
        )
    });

    let confirm = row![
        Space::fill_width(),
        btn_broadcast(Some(Message::Spend(SpendTxMessage::Confirm)))
    ];

    let content = column![
        warning.map(|w| warn(Some(w))),
        Container::new(new::h3_semi(t!("psbt-broadcast-transaction"))).width(Length::Fill),
        conflicts,
        confirm
    ]
    .spacing(VSpacing::S);

    let width = if conflicting_txids.is_empty() {
        ModalWidth::S
    } else {
        ModalWidth::XL
    };

    modal_view(None::<String>, None, None, width, content)
}

pub fn delete_action<'a>(warning: Option<&Error>, deleted: bool) -> Element<'a, Message> {
    let content = if deleted {
        let go_back = button::btn_go_back_to_psbts(Some(Message::Close));
        row![Space::fill_width(), go_back, Space::fill_width(),]
    } else {
        let cancel = button::btn_cancel(Some(Message::Spend(SpendTxMessage::Cancel)));
        let delete = button::btn_delete(Some(Message::Spend(SpendTxMessage::Confirm)));
        row![
            Space::fill_width(),
            cancel,
            Space::with_width(HSpacing::XL),
            delete,
            Space::fill_width(),
        ]
    };

    let warning = warning.map(|w| warn(Some(w)));
    let message = if deleted {
        t!("psbt-delete-success")
    } else {
        t!("common-are-you-sure")
    };
    let text = row![
        Space::fill_width(),
        new::caption(message),
        Space::fill_width()
    ];
    let content = column![warning, text, Space::fill_height(), content].height(100);

    modal_view(
        Some(t!("psbt-delete-this")),
        None,
        None,
        ModalWidth::S,
        content,
    )
}

pub fn spend_header<'a>(
    tx: &'a SpendTx,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let txid = tx.psbt.unsigned_tx.compute_txid().to_string();

    let label = if let Some(outpoint) = tx.is_single_payment() {
        let outpoint = outpoint.to_string();
        let labelled = vec![outpoint.clone(), txid.clone()];
        if let Some(label) = labels_editing.get(&outpoint) {
            label::label_editing(labelled, label, LABEL_TITLE_SIZE)
        } else {
            label::label_editable(labelled, tx.labels.get(&outpoint), LABEL_TITLE_SIZE)
        }
    } else if let Some(label) = labels_editing.get(&txid) {
        label::label_editing(vec![txid.clone()], label, LABEL_TITLE_SIZE)
    } else {
        label::label_editable(vec![txid.clone()], tx.labels.get(&txid), LABEL_TITLE_SIZE)
    };

    psbts::spend_header(
        label,
        tx.is_send_to_self(),
        tx.spend_amount,
        tx.fee_amount,
        tx.min_feerate_vb(),
    )
}

pub fn spend_overview_view<'a>(
    tx: &'a SpendTx,
    desc_info: &'a LianaPolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    currently_signing: bool,
    saved: bool,
) -> Element<'a, Message> {
    let enabled = saved && !currently_signing;
    let txid = tx.psbt.unsigned_tx.compute_txid().to_string();

    let action = match tx.status {
        SpendStatus::Unsigned => Some(btn_sign(Some(Message::Spend(SpendTxMessage::Sign))).into()),
        SpendStatus::Broadcastable => {
            Some(btn_broadcast(Some(Message::Spend(SpendTxMessage::Broadcast))).into())
        }
        SpendStatus::Timelocked
        | SpendStatus::Broadcast
        | SpendStatus::Confirmed
        | SpendStatus::Deprecated
        | SpendStatus::Unknown => None,
    };

    let (status, details) = match tx.sigs.signed_path() {
        Some(sigs) => (psbts::signatures_ready(sigs, key_aliases), None),
        None => {
            let requirement = if tx.sigs.recovery_paths().is_empty() {
                Some(psbts::path_row(
                    desc_info.primary_path(),
                    tx.sigs.primary_path(),
                    key_aliases,
                ))
            } else {
                tx.sigs.recovery_paths().iter().last().map(|(seq, path)| {
                    let keys = &desc_info.recovery_paths()[seq];
                    psbts::path_row(keys, path, key_aliases)
                })
            };
            (
                psbts::signatures_missing(),
                Some(psbts::signatures_requirement(requirement)),
            )
        }
    };

    psbts::spend_overview(
        saved,
        enabled.then_some(Message::ExportPsbt),
        enabled.then_some(Message::ImportPsbt),
        txid.clone(),
        Message::Clipboard(txid),
        status,
        details,
        action,
    )
}

pub fn inputs_view<'a>(
    coins: &'a HashMap<OutPoint, Coin>,
    tx: &'a Transaction,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let title = t!("psbt-coins-spent", count = tx.input.len());

    let inputs = tx
        .input
        .iter()
        .map(|input| {
            input_view(
                &input.previous_output,
                coins.get(&input.previous_output),
                labels,
                labels_editing,
            )
        })
        .collect();

    psbts::collapsible_section(title, inputs)
}

pub fn outputs_view<'a>(
    tx: &'a Transaction,
    network: Network,
    change_indexes: &'a [usize],
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    is_single_payment: bool,
    is_external: bool,
) -> Element<'a, Message> {
    let is_payment = |i: &usize| is_external || !change_indexes.contains(i);
    let count = tx
        .output
        .iter()
        .enumerate()
        .filter(|(i, _)| is_payment(i))
        .count();

    let payments = (count > 0).then_some({
        let title = t!("psbt-payments", count = count);
        let rows = tx
            .output
            .iter()
            .enumerate()
            .filter(|(i, _)| is_payment(i))
            .map(|(i, output)| {
                payment_view(
                    i,
                    tx.compute_txid(),
                    output,
                    network,
                    labels,
                    labels_editing,
                    is_single_payment,
                    !is_external || change_indexes.contains(&i),
                )
            })
            .collect();

        psbts::collapsible_section(title, rows)
    });

    let change = (!is_external && !change_indexes.is_empty()).then(|| {
        let rows = tx
            .output
            .iter()
            .enumerate()
            .filter(|(i, _)| change_indexes.contains(i))
            .map(|(_, output)| change_view(output, network))
            .collect();

        psbts::collapsible_section(t!("psbt-change"), rows)
    });

    column![payments, change].spacing(VSpacing::L).into()
}

fn input_view<'a>(
    outpoint: &'a OutPoint,
    coin: Option<&'a Coin>,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let outpoint = outpoint.to_string();

    let label_widget = if let Some(label) = labels_editing.get(&outpoint) {
        label::label_editing(vec![outpoint.clone()], label, LABEL_BODY_SIZE)
    } else {
        label::label_editable(
            vec![outpoint.clone()],
            labels.get(&outpoint),
            LABEL_BODY_SIZE,
        )
    };

    let address = coin.map(|c| c.address.to_string());
    let address_label = coin
        .and_then(|c| labels.get(&c.address.to_string()))
        .map(String::as_str);

    psbts::input_row(
        label_widget,
        coin.map(|c| c.amount),
        outpoint.clone(),
        Message::Clipboard(outpoint),
        address.clone(),
        address_label,
        address.map(Message::Clipboard),
    )
}

#[allow(clippy::too_many_arguments)]
fn payment_view<'a>(
    i: usize,
    txid: Txid,
    output: &'a TxOut,
    network: Network,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    is_single: bool,
    is_editable: bool,
) -> Element<'a, Message> {
    let addr = Address::from_script(&output.script_pubkey, network)
        .ok()
        .map(|a| a.to_string());
    let outpoint = OutPoint {
        txid,
        vout: i as u32,
    }
    .to_string();
    // if the payment is single in the transaction, then the label of the txid
    // is attached to the label of the payment.
    let change_labels = if is_single {
        vec![outpoint.clone(), txid.to_string()]
    } else {
        vec![outpoint.clone()]
    };

    let label_widget = if is_editable {
        if let Some(label) = labels_editing.get(&outpoint) {
            label::label_editing(change_labels, label, LABEL_BODY_SIZE)
        } else {
            label::label_editable(change_labels, labels.get(&outpoint), LABEL_BODY_SIZE)
        }
    } else {
        label::label_non_editable(change_labels, None, LABEL_BODY_SIZE)
    };

    let address_label = addr
        .as_ref()
        .and_then(|addr| labels.get(addr))
        .map(String::as_str);

    psbts::payment_row(
        label_widget,
        output.value,
        addr.clone(),
        address_label,
        addr.map(Message::Clipboard),
    )
}

fn change_view(output: &TxOut, network: Network) -> Element<'_, Message> {
    let addr = Address::from_script(&output.script_pubkey, network)
        .unwrap()
        .to_string();

    psbts::change_row(output.value, addr.clone(), Message::Clipboard(addr))
}

#[allow(clippy::too_many_arguments)]
pub fn sign_action<'a>(
    warning: Option<&Error>,
    hws: &'a [HardwareWallet],
    descriptor: &LianaDescriptor,
    signer: Option<Fingerprint>,
    signer_alias: Option<&'a String>,
    signed: &HashSet<Fingerprint>,
    signing: &HashSet<Fingerprint>,
    recovery_timelock: Option<u16>,
) -> Element<'a, Message> {
    let title = t!("psbt-select-signing-device");

    let mut signers: Vec<Element<'a, Message>> = if hws.is_empty() {
        vec![modal::modal_no_devices_placeholder()]
    } else {
        hws.iter()
            .enumerate()
            .map(|(i, hw)| {
                let (signed, signing, can_sign) =
                    hw.fingerprint().map_or((false, false, false), |f| {
                        (
                            signed.contains(&f),
                            signing.contains(&f),
                            descriptor.contains_fingerprint_in_path(f, recovery_timelock),
                        )
                    });
                device_list_entry(
                    hw,
                    HwRowMode::Signing {
                        signed,
                        signing,
                        can_sign,
                    },
                    move || Message::SelectHardwareWallet(i),
                )
            })
            .collect()
    };

    signers.extend(signer.map(|fingerprint| {
        let can_sign = descriptor.contains_fingerprint_in_path(fingerprint, recovery_timelock);
        let select_msg = can_sign.then_some(Message::Spend(SpendTxMessage::SelectHotSigner));
        let fp = Some(format!("#{fingerprint}"));
        let alias = signer_alias;
        if signed.contains(&fingerprint) {
            modal::device_entry(fp, None::<&str>, alias, DeviceStatus::Signed, None)
        } else if !can_sign {
            modal::device_entry(fp, None::<&str>, alias, DeviceStatus::NotInPath, None)
        } else {
            modal::device_entry(fp, None::<&str>, alias, DeviceStatus::None, select_msg)
        }
    }));

    let signers = Column::from_vec(signers)
        .align_x(Alignment::Center)
        .spacing(VSpacing::S)
        .width(Length::Fill);

    let modal_width = ModalWidth::L;
    let content = modal_view(Some(title), None, None, modal_width, signers);
    let warning = warning.map(|w| warn(Some(w)));

    column![warning, content]
        .spacing(VSpacing::S)
        .width(modal_width as u32 + 50)
        .into()
}

pub fn sign_action_toasts<'a>(
    error: Option<&Error>,
    hws: &'a [HardwareWallet],
    signing: &HashSet<Fingerprint>,
) -> Vec<Element<'a, Message>> {
    let mut toasts: Vec<Element<'a, Message>> = hws
        .iter()
        .filter_map(|hw| match hw {
            HardwareWallet::Supported {
                kind,
                fingerprint,
                version,
                alias,
                ..
            } if signing.contains(fingerprint) => Some(
                notification::processing_hardware_wallet(
                    kind,
                    version.as_ref(),
                    fingerprint,
                    alias.as_deref(),
                )
                .max_width(400.0)
                .into(),
            ),
            _ => None,
        })
        .collect();

    toasts.extend(error.map(|e| {
        notification::processing_hardware_wallet_error(t!("psbt-device-sign-failed"), e.to_string())
            .max_width(400.0)
            .into()
    }));

    toasts
}
