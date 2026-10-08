use std::collections::{BTreeMap, HashMap, HashSet};

use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};

use liana::{
    descriptors::{LianaDescriptor, LianaPolicy},
    label::Label,
    miniscript::bitcoin::{
        bip32::Fingerprint, blockdata::transaction::TxOut, Address, Network, OutPoint, Transaction,
        Txid,
    },
};

use liana_ui::{
    component::{
        button::{self, btn_broadcast},
        form,
        list::DeviceStatus,
        modal::{self, modal_view, ModalWidth},
        notification,
        panels::psbts,
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
        view::{
            dashboard,
            label::{self, LabelSize},
            message::*,
            transaction::{tx_view, TxDetail},
            warning::warn,
            FiatAmountConverter,
        },
    },
    daemon::model::{Coin, SpendTx},
    hw::HardwareWallet,
    t,
    view::hw::{device_list_entry, HwRowMode},
};

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
    currently_signing: bool,
    warning: Option<&'a Error>,
    fiat_converter: Option<FiatAmountConverter>,
) -> Element<'a, Message> {
    let detail = TxDetail::Psbt {
        tx,
        desc_info,
        key_aliases,
        saved,
        currently_signing,
        previous: false,
        warnings: Vec::new(),
    };
    let content = tx_view(cache, detail, labels_editing, fiat_converter);
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
    change_indexes: &[usize],
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    is_external: bool,
    owned_default_labels: Option<&BTreeMap<usize, Label>>,
) -> Element<'a, Message> {
    let txid = tx.compute_txid();
    let label_field = |i: usize| {
        let outpoint = OutPoint::new(txid, i as u32).to_string();
        let default_label = owned_default_labels
            .and_then(|default_labels| default_labels.get(&i))
            .unwrap_or(&Label::None);
        let label = liana::label::resolve(labels.get(&outpoint).map(String::as_str), default_label);
        label::label_field(
            vec![outpoint.clone()],
            labels_editing.get(&outpoint),
            &label,
            LabelSize::Body,
        )
    };
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
                let label = if !is_external || change_indexes.contains(&i) {
                    label_field(i)
                } else {
                    let outpoint = OutPoint::new(txid, i as u32).to_string();
                    label::label_non_editable(vec![outpoint], None, LabelSize::Body)
                };
                payment_view(label, output, network, labels)
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
            .map(|(i, output)| {
                let label = owned_default_labels.is_some().then(|| label_field(i));
                change_view(label, output, network)
            })
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

    let default_label = coin.map_or(&Label::None, |c| &c.default_label);
    let label = liana::label::resolve(labels.get(&outpoint).map(String::as_str), default_label);
    let label_widget = label::label_field(
        vec![outpoint.clone()],
        labels_editing.get(&outpoint),
        &label,
        LabelSize::Body,
    );

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

fn payment_view<'a>(
    label: Element<'a, Message>,
    output: &'a TxOut,
    network: Network,
    labels: &'a HashMap<String, String>,
) -> Element<'a, Message> {
    let addr = Address::from_script(&output.script_pubkey, network)
        .ok()
        .map(|a| a.to_string());

    let address_label = addr
        .as_ref()
        .and_then(|addr| labels.get(addr))
        .map(String::as_str);

    psbts::payment_row(
        label,
        output.value,
        addr.clone(),
        address_label,
        addr.map(Message::Clipboard),
    )
}

fn change_view<'a>(
    label: Option<Element<'a, Message>>,
    output: &TxOut,
    network: Network,
) -> Element<'a, Message> {
    let addr = Address::from_script(&output.script_pubkey, network)
        .unwrap()
        .to_string();

    psbts::change_row(label, output.value, addr.clone(), Message::Clipboard(addr))
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
