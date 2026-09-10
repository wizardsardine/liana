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
        card, form,
        list::DeviceStatus,
        modal::{self, modal_view, ModalWidth},
        panels::psbts,
        pill,
        text::{self, *},
    },
    icon, theme,
    widget::*,
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
    let delete_msg = if currently_signing {
        None
    } else {
        Some(Message::Spend(SpendTxMessage::Delete))
    };
    dashboard(
        &Menu::PSBTs,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(Container::new(h3("PSBT")).width(Length::Fill))
                    .push_maybe(if !tx.sigs.recovery_paths().is_empty() {
                        Some(pill::recovery())
                    } else {
                        None
                    })
                    .push_maybe(match tx.status {
                        SpendStatus::Deprecated => Some(pill::deprecated()),
                        SpendStatus::Broadcast => Some(pill::unconfirmed()),
                        SpendStatus::Spent => Some(pill::spent()),
                        _ => None,
                    }),
            )
            .push(spend_header(tx, labels_editing))
            .push(spend_overview_view(
                tx,
                desc_info,
                key_aliases,
                currently_signing,
                saved,
            ))
            .push(
                Column::new()
                    .spacing(20)
                    .push(inputs_view(
                        &tx.coins,
                        &tx.psbt.unsigned_tx,
                        &tx.labels,
                        labels_editing,
                    ))
                    .push(outputs_view(
                        &tx.psbt.unsigned_tx,
                        network,
                        &tx.change_indexes,
                        &tx.labels,
                        labels_editing,
                        tx.is_single_payment().is_some(),
                        false,
                    )),
            )
            .push(if saved {
                row![btn_delete(delete_msg)].width(Length::Fill)
            } else {
                Row::new()
                    .push(Space::with_width(Length::Fill))
                    .push(btn_save(
                        (!currently_signing).then_some(Message::Spend(SpendTxMessage::Save)),
                        false,
                    ))
                    .width(Length::Fill)
            })
            .push(Space::with_height(10)),
    )
}

pub fn save_action<'a>(warning: Option<&Error>, saved: bool) -> Element<'a, Message> {
    if saved {
        card::simple(text(t!("psbt-transaction-saved")))
            .width(400)
            .align_x(iced::alignment::Horizontal::Center)
            .into()
    } else {
        let ignore = button::secondary(None, t!("btn-ignore")).on_press(Message::Close);
        let save =
            button::primary(None, t!("btn-save")).on_press(Message::Spend(SpendTxMessage::Confirm));
        let buttons = row![Space::fill_width(), ignore, save].spacing(10);

        let content = column![
            warning.map(|w| warn(Some(w))),
            text(t!("psbt-save-transaction")),
            buttons
        ]
        .spacing(10);

        card::simple(content).width(400).into()
    }
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
        return card::simple(text(t!("psbt-transaction-broadcast")))
            .width(400)
            .align_x(iced::alignment::Horizontal::Center)
            .into();
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

        let warning = row![icon::warning_icon(), text(invalidates)].spacing(10);
        let explanation = row![text(conflicts)].padding([0, 30]);

        conflicting_txids
            .iter()
            .fold(column![warning, explanation].spacing(5), |col, txid| {
                let copy = button::btn_copy(Some(Message::Clipboard(txid.to_string())));
                col.push(
                    row![text(txid.to_string()), copy]
                        .padding([0, 30])
                        .spacing(5)
                        .align_y(Alignment::Center),
                )
            })
    });

    let confirm = row![
        Space::fill_width(),
        btn_broadcast(Some(Message::Spend(SpendTxMessage::Confirm)))
    ];

    let content = column![
        warning.map(|w| warn(Some(w))),
        Container::new(h4_bold(t!("psbt-broadcast-transaction"))).width(Length::Fill),
        conflicts,
        confirm
    ]
    .spacing(10);

    let width = if conflicting_txids.is_empty() {
        400
    } else {
        800
    };

    card::simple(content).width(width).into()
}

pub fn delete_action<'a>(warning: Option<&Error>, deleted: bool) -> Element<'a, Message> {
    if deleted {
        let go_back = button::secondary(None, t!("btn-go-back-to-psbts")).on_press(Message::Close);
        let content = column![text(t!("psbt-delete-success")), go_back]
            .spacing(20)
            .align_x(Alignment::Center);

        return card::simple(content)
            .align_x(iced::alignment::Horizontal::Center)
            .width(400)
            .into();
    }

    let cancel = button::transparent(None, t!("btn-cancel"))
        .on_press(Message::Spend(SpendTxMessage::Cancel));
    let delete =
        button::alert(None, t!("btn-delete")).on_press(Message::Spend(SpendTxMessage::Confirm));
    let buttons = row![Space::fill_width(), cancel, delete];

    let content = column![
        warning.map(|w| warn(Some(w))),
        text(t!("psbt-delete-this")),
        buttons
    ]
    .spacing(10);

    card::simple(content).width(400).into()
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
            label::label_editing(labelled, label, H3_SIZE)
        } else {
            label::label_editable(labelled, tx.labels.get(&outpoint), H3_SIZE)
        }
    } else if let Some(label) = labels_editing.get(&txid) {
        label::label_editing(vec![txid.clone()], label, H3_SIZE)
    } else {
        label::label_editable(vec![txid.clone()], tx.labels.get(&txid), H3_SIZE)
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

    let action = (tx.status == SpendStatus::Broadcastable).then(|| {
        if tx.path_ready().is_none() {
            btn_sign(Some(Message::Spend(SpendTxMessage::Sign)))
        } else {
            btn_broadcast(Some(Message::Spend(SpendTxMessage::Broadcast)))
        }
        .into()
    });

    psbts::spend_overview(
        saved,
        enabled.then_some(Message::ExportPsbt),
        enabled.then_some(Message::ImportPsbt),
        txid.clone(),
        Message::Clipboard(txid),
        signatures(tx, desc_info, key_aliases),
        action,
    )
}

pub fn signatures<'a>(
    tx: &'a SpendTx,
    desc_info: &'a LianaPolicy,
    keys_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, Message> {
    if let Some(sigs) = tx.path_ready() {
        return psbts::signatures_ready(sigs, keys_aliases);
    }

    let requirement = if tx.sigs.recovery_paths().is_empty() {
        Some(psbts::path_row(
            desc_info.primary_path(),
            tx.sigs.primary_path(),
            keys_aliases,
        ))
    } else {
        tx.sigs.recovery_paths().iter().last().map(|(seq, path)| {
            let keys = &desc_info.recovery_paths()[seq];
            psbts::path_row(keys, path, keys_aliases)
        })
    };

    psbts::signatures_missing(requirement)
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

    let payments = if count > 0 {
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
    } else {
        Container::new(
            h4_bold(t!("psbt-no-payment"))
                .style(|t| theme::text::custom(t.colors.buttons.transparent_border.active.text)),
        )
        .padding(20)
        .width(Length::Fill)
        .style(theme::card::button_simple)
        .into()
    };

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

    column![payments, change].spacing(20).into()
}

fn input_view<'a>(
    outpoint: &'a OutPoint,
    coin: Option<&'a Coin>,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let outpoint = outpoint.to_string();

    let label_widget = if let Some(label) = labels_editing.get(&outpoint) {
        label::label_editing(vec![outpoint.clone()], label, text::P1_SIZE)
    } else {
        label::label_editable(vec![outpoint.clone()], labels.get(&outpoint), text::P1_SIZE)
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
            label::label_editing(change_labels, label, text::P1_SIZE)
        } else {
            label::label_editable(change_labels, labels.get(&outpoint), text::P1_SIZE)
        }
    } else {
        label::label_non_editable(change_labels, None, text::P1_SIZE)
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

    let mut signers = vec![];
    if hws.is_empty() {
        signers.push(modal::modal_no_devices_placeholder());
    } else {
        hws.iter().enumerate().for_each(|(i, hw)| {
            let (signed, signing, can_sign) = hw.fingerprint().map_or((false, false, false), |f| {
                (
                    signed.contains(&f),
                    signing.contains(&f),
                    descriptor.contains_fingerprint_in_path(f, recovery_timelock),
                )
            });
            signers.push(device_list_entry(
                hw,
                HwRowMode::Signing {
                    signed,
                    signing,
                    can_sign,
                },
                move || Message::SelectHardwareWallet(i),
            ))
        });
    }

    if let Some(hot_signer) = signer.map(|fingerprint| {
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
    }) {
        signers.push(hot_signer);
    }

    let modal_content = Column::from_vec(signers)
        .align_x(Alignment::Center)
        .spacing(10)
        .width(Length::Fill);

    let width = ModalWidth::L;
    let content = modal_view(Some(title), None, None, width, modal_content);

    let width = width as u32 + 50;
    let warning = warning.map(|w| warn(Some(w)));
    column![warning, content].spacing(10).width(width).into()
}

pub fn sign_action_toasts<'a>(
    error: Option<&Error>,
    hws: &'a [HardwareWallet],
    signing: &HashSet<Fingerprint>,
) -> Vec<Element<'a, Message>> {
    let mut vec: Vec<Element<'a, Message>> = hws
        .iter()
        .filter_map(|hw| {
            if let HardwareWallet::Supported {
                kind,
                fingerprint,
                version,
                alias,
                ..
            } = &hw
            {
                if signing.contains(fingerprint) {
                    Some(
                        liana_ui::component::notification::processing_hardware_wallet(
                            kind,
                            version.as_ref(),
                            fingerprint,
                            alias.as_ref().map(|x| x.as_str()),
                        )
                        .max_width(400.0)
                        .into(),
                    )
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    if let Some(e) = error {
        vec.push(
            liana_ui::component::notification::processing_hardware_wallet_error(
                t!("psbt-device-sign-failed"),
                e.to_string(),
            )
            .max_width(400.0)
            .into(),
        )
    }

    vec
}
