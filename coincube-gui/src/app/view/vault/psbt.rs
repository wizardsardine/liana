use std::collections::{HashMap, HashSet};

use iced::{
    alignment::Horizontal,
    widget::{scrollable, tooltip, Space},
    Alignment, Length,
};

use coincube_core::border_wallet::{CellRef, WordGrid, PATTERN_LENGTH};
use coincube_core::{
    descriptors::{CoincubePolicy, PathInfo, PathSpendInfo},
    miniscript::bitcoin::{
        bip32::Fingerprint, blockdata::transaction::TxOut, Address, Network, OutPoint, Transaction,
        Txid,
    },
};

use coincube_ui::component::amount::BitcoinDisplayUnit;
use coincube_ui::component::modal;
use coincube_ui::{
    color,
    component::{
        amount::*,
        badge, button, card,
        collapse::Collapse,
        form, separation,
        text::{self, *},
    },
    icon, theme,
    widget::{Button, Column, Container, Element, Row, RowExt, TextInput},
};

use crate::{
    app::{
        cache::Cache,
        menu::{Menu, VaultSubMenu},
        state::vault::psbt::{BorderWalletReconstructionState, ReconStep},
        view::{dashboard, message::*, vault::label},
    },
    daemon::model::{Coin, SpendStatus, SpendTx},
    hw::HardwareWallet,
};

#[allow(clippy::too_many_arguments)]
pub fn psbt_view<'a>(
    cache: &'a Cache,
    tx: &'a SpendTx,
    saved: bool,
    desc_info: &'a CoincubePolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    network: Network,
    currently_signing: bool,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    dashboard(
        &Menu::Vault(VaultSubMenu::PSBTs(None)),
        cache,
        Column::new()
            .spacing(20)
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(Container::new(h3("PSBT").bold()).width(Length::Fill))
                    .push(if !tx.sigs.recovery_paths().is_empty() {
                        Some(badge::recovery())
                    } else {
                        None
                    })
                    .push(match tx.status {
                        SpendStatus::Deprecated => Some(badge::deprecated()),
                        SpendStatus::Broadcast => Some(badge::unconfirmed()),
                        SpendStatus::Spent => Some(badge::spent()),
                        _ => None,
                    }),
            )
            .push(spend_header(tx, labels_editing, bitcoin_unit))
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
                        bitcoin_unit,
                    ))
                    .push(outputs_view(
                        &tx.psbt.unsigned_tx,
                        network,
                        &tx.change_indexes,
                        &tx.labels,
                        labels_editing,
                        bitcoin_unit,
                        tx.is_single_payment().is_some(),
                        false,
                    )),
            )
            .push(if saved {
                Row::new()
                    .push(
                        button::secondary(None, "Delete")
                            .width(Length::Fixed(200.0))
                            .on_press_maybe(if currently_signing {
                                None
                            } else {
                                Some(Message::Spend(SpendTxMessage::Delete))
                            }),
                    )
                    .width(Length::Fill)
            } else {
                Row::new()
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::secondary(None, "Save")
                            .width(Length::Fixed(150.0))
                            .on_press_maybe(if currently_signing {
                                None
                            } else {
                                Some(Message::Spend(SpendTxMessage::Save))
                            }),
                    )
                    .width(Length::Fill)
            })
            .push(Space::new().height(10)),
    )
}

pub fn save_action<'a>(saved: bool) -> Element<'a, Message> {
    if saved {
        card::simple(text("Transaction is saved"))
            .width(Length::Fixed(400.0))
            .align_x(iced::alignment::Horizontal::Center)
            .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push(text("Save this transaction"))
                .push(
                    Row::new()
                        .spacing(10)
                        .push(Space::new().width(Length::Fill))
                        .push(button::secondary(None, "Ignore").on_press(Message::Close))
                        .push(
                            button::primary(None, "Save")
                                .on_press(Message::Spend(SpendTxMessage::Confirm)),
                        ),
                ),
        )
        .width(Length::Fixed(400.0))
        .into()
    }
}

/// Return the modal view to broadcast a transaction.
///
/// `conflicting_txids` contains the IDs of any directly conflicting transactions
/// of the transaction to be broadcast.
/// `broadcasting` is `true` while the daemon's `broadcast_spend_tx` RPC is in
/// flight — between the user pressing "Broadcast" and the daemon's reply.
/// In that window the button swaps to a disabled "Broadcasting…" label so the
/// user can see their click registered and isn't tempted to click again.
#[allow(clippy::too_many_arguments)]
pub fn broadcast_action<'a>(
    conflicting_txids: &HashSet<Txid>,
    saved: bool,
    broadcasting: bool,
    error: Option<String>,
    spend_amount_display: &'a str,
    sent_quote: &'a coincube_ui::component::quote_display::Quote,
    sent_image_handle: &'a iced::widget::image::Handle,
    is_self_transfer: bool,
) -> Element<'a, Message> {
    if saved {
        let verb_suffix = if is_self_transfer {
            "has been transferred."
        } else {
            "has been sent successfully."
        };
        coincube_ui::component::sent_celebration_page(
            "bitcoin-send",
            spend_amount_display,
            sent_quote,
            sent_image_handle,
            verb_suffix,
            Message::Spend(super::super::SpendTxMessage::Cancel),
        )
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push(Container::new(h4_bold("Broadcast the transaction")).width(Length::Fill))
                .push(error.map(|err| card::error("Broadcast failed", err)))
                .push(if conflicting_txids.is_empty() {
                    None
                } else {
                    Some(
                        conflicting_txids.iter().fold(
                            Column::new()
                                .spacing(5)
                                .push(Row::new().spacing(10).push(icon::warning_icon()).push(text(
                                    if conflicting_txids.len() > 1 {
                                        "WARNING: Broadcasting this transaction \
                                        will invalidate some pending payments."
                                    } else {
                                        "WARNING: Broadcasting this transaction \
                                        will invalidate a pending payment."
                                    },
                                )))
                                .push(Row::new().padding([0, 30]).push(text(
                                    if conflicting_txids.len() > 1 {
                                        "The following transactions are \
                                        spending one or more inputs \
                                        from the transaction to be \
                                        broadcast and will be \
                                        dropped, along with any other \
                                        transactions that depend on them:"
                                    } else {
                                        "The following transaction is \
                                        spending one or more inputs \
                                        from the transaction to be \
                                        broadcast and will be \
                                        dropped, along with any other \
                                        transactions that depend on it:"
                                    },
                                ))),
                            |col, txid| {
                                col.push(
                                    Row::new()
                                        .padding([0, 30])
                                        .spacing(5)
                                        .align_y(Alignment::Center)
                                        .push(text(txid.to_string()))
                                        .push(
                                            Button::new(
                                                icon::clipboard_icon()
                                                    .style(theme::text::secondary),
                                            )
                                            .on_press(Message::Clipboard(txid.to_string()))
                                            .style(theme::button::transparent_border),
                                        ),
                                )
                            },
                        ),
                    )
                })
                .push(Row::new().push(Column::new().width(Length::Fill)).push(
                    // Disabled "Broadcasting…" state mirrors the
                    // "Processing…" pattern used by the PSBT
                    // update form below — leaving `on_press`
                    // unset is this codebase's idiom for a
                    // visually-disabled button.
                    if broadcasting {
                        button::primary(None, "Broadcasting…")
                    } else {
                        button::primary(None, "Broadcast")
                            .on_press(Message::Spend(SpendTxMessage::Confirm))
                    },
                )),
        )
        .width(Length::Fixed(if conflicting_txids.is_empty() {
            400.0
        } else {
            800.0
        }))
        .into()
    }
}

pub fn delete_action<'a>(deleted: bool) -> Element<'a, Message> {
    if deleted {
        card::simple(
            Column::new()
                .spacing(20)
                .align_x(Alignment::Center)
                .push(text("Successfully deleted this transaction."))
                .push(button::secondary(None, "Go back to PSBTs").on_press(Message::Close)),
        )
        .align_x(iced::alignment::Horizontal::Center)
        .width(Length::Fixed(400.0))
        .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push(text("Delete this PSBT"))
                .push(
                    Row::new()
                        .push(Column::new().width(Length::Fill))
                        .push(
                            button::transparent(None, "Cancel")
                                .on_press(Message::Spend(SpendTxMessage::Cancel)),
                        )
                        .push(
                            button::alert(None, "Delete")
                                .on_press(Message::Spend(SpendTxMessage::Confirm)),
                        ),
                ),
        )
        .width(Length::Fixed(400.0))
        .into()
    }
}

/// Modal shown when the user presses "Sign via Keychain" but Connect
/// isn't ready. Explains the requirement and offers the two ways forward:
/// sign in to Connect (enables Keychain relay signing), or pair a phone
/// over Wi-Fi (local signing, no Connect needed). `signed_in` tweaks the
/// wording — once tokens exist, the blocker is device/stream readiness
/// rather than a missing sign-in.
pub fn keychain_unavailable_action<'a>(signed_in: bool) -> Element<'a, Message> {
    let body = if signed_in {
        "Connect hasn't finished setting up this device for Keychain \
         signing. Set it up now, or pair a phone over Wi-Fi to sign \
         locally without Connect."
    } else {
        "To sign with a Keychain phone through Connect, sign in to \
         COINCUBE Connect. Or pair a phone over Wi-Fi to sign locally — \
         no Connect required."
    };
    // When already signed in, the primary action bootstraps Connect
    // signing on demand ("Sign with Connect"); otherwise it routes to
    // sign-in. Fixed-width buttons keep their labels on one line.
    let (primary_label, primary_msg) = if signed_in {
        ("Sign with Connect", SpendTxMessage::KeychainEnsureConnect)
    } else {
        ("Sign in to Connect", SpendTxMessage::KeychainConnectSignIn)
    };
    card::simple(
        Column::new()
            .spacing(15)
            .push(text("Keychain signing needs Connect").bold())
            .push(text(body).style(theme::text::secondary))
            .push(
                Row::new()
                    .spacing(10)
                    .push(Column::new().width(Length::Fill))
                    .push(
                        button::transparent(None, "Cancel")
                            .on_press(Message::Spend(SpendTxMessage::Cancel)),
                    )
                    .push(
                        button::secondary(None, "Pair a phone")
                            .on_press(Message::Spend(SpendTxMessage::KeychainPairPhone))
                            .width(Length::Fixed(140.0)),
                    )
                    .push(
                        button::primary(None, primary_label)
                            .on_press(Message::Spend(primary_msg))
                            .width(Length::Fixed(190.0)),
                    )
                    .align_y(Alignment::Center),
            ),
    )
    .width(Length::Fixed(600.0))
    .into()
}

pub fn spend_header<'a>(
    tx: &'a SpendTx,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let txid = tx.psbt.unsigned_tx.compute_txid().to_string();
    Column::new()
        .spacing(20)
        .push(if let Some(outpoint) = tx.is_single_payment() {
            let outpoint = outpoint.to_string();
            if let Some(label) = labels_editing.get(&outpoint) {
                label::label_editing(vec![outpoint.clone(), txid.clone()], label, H3_SIZE)
            } else {
                label::label_editable(
                    vec![outpoint.clone(), txid.clone()],
                    tx.labels.get(&outpoint),
                    H3_SIZE,
                )
            }
        } else if let Some(label) = labels_editing.get(&txid) {
            label::label_editing(vec![txid.clone()], label, H3_SIZE)
        } else {
            label::label_editable(vec![txid.clone()], tx.labels.get(&txid), H3_SIZE)
        })
        .push(
            Column::new()
                .push(if tx.is_send_to_self() {
                    Container::new(h1("Self-transfer"))
                } else {
                    Container::new(amount_with_size_and_unit(
                        &tx.spend_amount,
                        H1_SIZE,
                        bitcoin_unit,
                    ))
                })
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .push(h3("Miner fee: ").style(theme::text::secondary))
                        .push(if tx.fee_amount.is_none() {
                            Some(text("Missing information about transaction inputs"))
                        } else {
                            None
                        })
                        .push_maybe(
                            tx.fee_amount
                                .map(|fee| amount_with_size_and_unit(&fee, H3_SIZE, bitcoin_unit)),
                        )
                        .push(text(" ").size(H3_SIZE))
                        .push(tx.min_feerate_vb().map(|rate| {
                            text(format!("(~{} sats/vbyte)", &rate))
                                .size(H4_SIZE)
                                .style(theme::text::secondary)
                        })),
                ),
        )
        .into()
}

pub fn spend_overview_view<'a>(
    tx: &'a SpendTx,
    desc_info: &'a CoincubePolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    currently_signing: bool,
    saved: bool,
) -> Element<'a, Message> {
    // Force the user to save (which commits the derivation-index increment to the
    // database) before exporting, otherwise the exported PSBT can lead to change
    // address reuse.
    let export_button = button::secondary(Some(icon::backup_icon()), "Export").on_press_maybe(
        if currently_signing || !saved {
            None
        } else {
            Some(Message::ExportPsbt)
        },
    );
    Column::new()
        .spacing(20)
        .push(
            Container::new(
                Column::new()
                    .push(
                        Column::new()
                            .padding(15)
                            .spacing(10)
                            .push(
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .push(text("PSBT").bold().width(Length::Fill))
                                    .push(
                                        Row::new()
                                            .spacing(5)
                                            .push(if saved {
                                                Container::new(export_button)
                                            } else {
                                                Container::new(tooltip::Tooltip::new(
                                                    export_button,
                                                    Container::new(p1_regular(
                                                        "Sign or save the transaction first to enable export",
                                                    ))
                                                    .style(theme::card::simple)
                                                    .padding(10),
                                                    tooltip::Position::Top,
                                                ))
                                            })
                                            .push(
                                                button::secondary(
                                                    Some(icon::restore_icon()),
                                                    "Import",
                                                )
                                                .on_press_maybe(if currently_signing || !saved {
                                                    None
                                                } else {
                                                    Some(Message::ImportPsbt)
                                                }),
                                            ),
                                    )
                                    .align_y(Alignment::Center),
                            )
                            .push(
                                Row::new()
                                    .push(p1_bold("Tx ID").width(Length::Fill))
                                    .push(
                                        p2_regular(tx.psbt.unsigned_tx.compute_txid().to_string())
                                            .style(theme::text::secondary),
                                    )
                                    .push(
                                        Button::new(
                                            icon::clipboard_icon().style(theme::text::secondary),
                                        )
                                        .on_press(Message::Clipboard(
                                            tx.psbt.unsigned_tx.compute_txid().to_string(),
                                        ))
                                        .style(theme::button::transparent_border),
                                    )
                                    .align_y(Alignment::Center),
                            ),
                    )
                    .push(signatures(tx, desc_info, key_aliases)),
            )
            .style(theme::card::simple),
        )
        .push(if tx.status == SpendStatus::Pending {
            Some(
                Row::new()
                    .push(Space::new().width(Length::Fill))
                    .push(if tx.path_ready().is_none() {
                        // A single "Sign" entry point opens the unified
                        // picker, which lists every signer — local (HW,
                        // master, border) and Keychain (contacts' phones) —
                        // as its own row.
                        Some(
                            Row::new().push(
                                button::primary(None, "Sign")
                                    .on_press(Message::Spend(SpendTxMessage::Sign))
                                    .width(Length::Fixed(150.0)),
                            ),
                        )
                    } else {
                        Some(
                            Row::new().push(
                                button::primary(None, "Broadcast")
                                    .on_press(Message::Spend(SpendTxMessage::Broadcast))
                                    .width(Length::Fixed(150.0)),
                            ),
                        )
                    })
                    .align_y(Alignment::Center)
                    .spacing(20),
            )
        } else {
            None
        })
        .into()
}

pub fn signatures<'a>(
    tx: &'a SpendTx,
    desc_info: &'a CoincubePolicy,
    keys_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, Message> {
    Column::new()
        .push(if let Some(sigs) = tx.path_ready() {
            Container::new(
                scrollable(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::Center)
                        .spacing(10)
                        .push(p1_bold("Status"))
                        .push(icon::square_check_icon().style(theme::text::success))
                        .push(text("Ready").bold().style(theme::text::success))
                        .push(text("  signed by"))
                        .push(
                            sigs.signed_pubkeys
                                .keys()
                                .fold(Row::new().spacing(5), |row, value| {
                                    row.push(container_from_fg(*value, keys_aliases))
                                }),
                        ),
                )
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(2).scroller_width(2),
                )),
            )
            .padding(15)
        } else {
            // Not broadcastable yet. If some (but not enough) signatures are
            // present, surface a "Partially signed" banner with the signers so
            // the user sees progress instead of a bare "Not ready".
            let signed: Vec<Fingerprint> = {
                let mut v: Vec<_> = tx.signers().into_iter().collect();
                v.sort_by_key(|fg| fg.to_string());
                v
            };
            let is_partial = !signed.is_empty();

            let collapse = Collapse::new(
                move || requirements_header(is_partial, icon::collapse_icon()),
                move || requirements_header(is_partial, icon::collapsed_icon()),
                move || {
                    Into::<Element<'a, Message>>::into(
                        Column::new()
                            .padding(15)
                            .spacing(10)
                            .push(text("Finalizing this transaction requires:"))
                            .push(if tx.sigs.recovery_paths().is_empty() {
                                Some(path_view(
                                    desc_info.primary_path(),
                                    tx.sigs.primary_path(),
                                    keys_aliases,
                                ))
                            } else {
                                tx.sigs.recovery_paths().iter().last().map(|(seq, path)| {
                                    let keys = &desc_info.recovery_paths()[seq];
                                    path_view(keys, path, keys_aliases)
                                })
                            }),
                    )
                },
            );

            let mut col = Column::new();
            if is_partial {
                col = col.push(
                    Container::new(
                        scrollable(
                            Row::new()
                                .spacing(10)
                                .align_y(Alignment::Center)
                                .push(p1_bold("Status"))
                                .push(icon::clock_icon().style(theme::text::warning))
                                .push(text("Partially signed").bold().style(theme::text::warning))
                                .push(text("  signed by"))
                                .push(signed.iter().fold(Row::new().spacing(5), |row, fg| {
                                    row.push(container_from_fg(*fg, keys_aliases))
                                })),
                        )
                        .direction(scrollable::Direction::Horizontal(
                            scrollable::Scrollbar::new().width(2).scroller_width(2),
                        )),
                    )
                    .padding(15),
                );
            }
            Container::new(col.push(collapse))
        })
        .into()
}

/// Header for the collapsible "what's still required" section of a PSBT that
/// can't be broadcast yet. With nothing signed it shows the red "Not ready";
/// once there are signatures the "Partially signed" banner above carries the
/// status, so this becomes a neutral "Remaining requirements" toggle.
fn requirements_header<'a, T: 'a>(
    is_partial: bool,
    collapse_icon: coincube_ui::widget::Text<'a>,
) -> Button<'a, T> {
    // When partial, the "Partially signed" banner above is the status line, so
    // this header is just a neutral "Remaining requirements" toggle (no second
    // "Status" label). With nothing signed it is the primary "Not ready" line.
    let content = if is_partial {
        Row::new()
            .align_y(Alignment::Center)
            .spacing(20)
            .push(
                text("Remaining requirements")
                    .style(theme::text::secondary)
                    .width(Length::Fill),
            )
            .push(collapse_icon)
    } else {
        Row::new()
            .align_y(Alignment::Center)
            .spacing(20)
            .push(p1_bold("Status"))
            .push(
                Row::new()
                    .spacing(5)
                    .align_y(Alignment::Center)
                    .push(icon::square_cross_icon().style(theme::text::error))
                    .push(text("Not ready").style(theme::text::error))
                    .width(Length::Fill),
            )
            .push(collapse_icon)
    };
    Button::new(content)
        .padding(15)
        .width(Length::Fill)
        .style(theme::button::transparent_border)
}

// Display a fingerprint first by its alias if there is any, or in hex otherwise.
fn container_from_fg(
    fg: Fingerprint,
    aliases: &HashMap<Fingerprint, String>,
) -> Container<Message> {
    if let Some(alias) = aliases.get(&fg) {
        Container::new(
            tooltip::Tooltip::new(
                Container::new(text(alias))
                    .padding(10)
                    .style(theme::pill::simple),
                coincube_ui::widget::Text::new(fg.to_string()),
                tooltip::Position::Bottom,
            )
            .style(theme::card::simple),
        )
    } else {
        Container::new(text(fg.to_string()))
            .padding(10)
            .style(theme::pill::simple)
    }
}

pub fn path_view<'a>(
    path: &'a PathInfo,
    sigs: &'a PathSpendInfo,
    key_aliases: &'a HashMap<Fingerprint, String>,
) -> Element<'a, Message> {
    // We get a sorted list of all the fingerprints (which correspond to a signer) from this
    // spending path, and from it get an iterator on those of these fingerprints for which a
    // signature was provided in the PSBT, and those for which there isn't any.
    let mut all_fgs: Vec<Fingerprint> = path.thresh_origins().1.into_keys().collect();
    all_fgs.sort();
    let signed_fgs = sigs.signed_pubkeys.keys();
    let non_signed_fgs = all_fgs
        .into_iter()
        .filter(|fg| !sigs.signed_pubkeys.contains_key(fg));
    let missing_signatures = sigs.threshold.saturating_sub(sigs.sigs_count);

    // From these iterators, create the appropriate rows to be displayed.
    let row_unsigned = non_signed_fgs.into_iter().fold(None, |row, fg| {
        Some(
            row.unwrap_or_else(|| Row::new().spacing(5))
                .push(container_from_fg(fg, key_aliases)),
        )
    });
    let row_signed = signed_fgs
        .into_iter()
        .fold(Row::new().spacing(5), |row, fg| {
            row.push(container_from_fg(*fg, key_aliases))
        });

    scrollable(
        Row::new()
            .align_y(Alignment::Center)
            .push(
                Row::new()
                    .push(if missing_signatures == 0 {
                        icon::square_check_icon().style(theme::text::success)
                    } else {
                        icon::square_cross_icon().style(theme::text::secondary)
                    })
                    .push(Space::new().width(Length::Fixed(20.0))),
            )
            .push(
                p1_regular(format!(
                    "{} more signature{}",
                    missing_signatures,
                    if missing_signatures > 1 {
                        "s from "
                    } else if missing_signatures == 0 {
                        ""
                    } else {
                        " from "
                    }
                ))
                .style(theme::text::secondary),
            )
            .push(row_unsigned)
            .push(
                (!sigs.signed_pubkeys.is_empty())
                    .then_some(p1_regular(", already signed by ").style(theme::text::secondary)),
            )
            .push(row_signed),
    )
    .direction(scrollable::Direction::Horizontal(
        scrollable::Scrollbar::new().width(2).scroller_width(2),
    ))
    .into()
}

pub fn inputs_view<'a>(
    coins: &'a HashMap<OutPoint, Coin>,
    tx: &'a Transaction,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    Container::new(
        Column::new()
            .push(
                Container::new(h4_bold(format!(
                    "{} coin{} spent",
                    tx.input.len(),
                    if tx.input.len() == 1 { "" } else { "s" }
                )))
                .padding(20)
                .width(Length::Fill),
            )
            .push(tx.input.iter().fold(
                Column::new().spacing(10).padding(20),
                |col: Column<'a, Message>, input| {
                    col.push(input_view(
                        &input.previous_output,
                        coins.get(&input.previous_output),
                        labels,
                        labels_editing,
                        bitcoin_unit,
                    ))
                },
            )),
    )
    .style(theme::card::simple)
    .into()
}

#[allow(clippy::too_many_arguments)]
pub fn outputs_view<'a>(
    tx: &'a Transaction,
    network: Network,
    change_indexes: &'a [usize],
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
    is_single_payment: bool,
    is_external: bool,
) -> Element<'a, Message> {
    Column::new()
        .spacing(20)
        .push({
            let count = tx
                .output
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    if is_external {
                        return true;
                    }
                    !change_indexes.contains(i)
                })
                .count();
            if count > 0 {
                Container::new(
                    Column::new()
                        .push(
                            Container::new(h4_bold(format!(
                                "{} payment{}",
                                count,
                                if count == 1 { "" } else { "s" }
                            )))
                            .padding(20)
                            .width(Length::Fill),
                        )
                        .push(
                            tx.output
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| {
                                    if is_external {
                                        return true;
                                    }
                                    !change_indexes.contains(i)
                                })
                                .fold(
                                    Column::new().padding(20),
                                    |col: Column<'a, Message>, (i, output)| {
                                        // External outputs that are not our change are
                                        // not owned by us, so their label is read-only.
                                        let mut is_editable = true;
                                        if is_external && !change_indexes.contains(&i) {
                                            is_editable = false;
                                        }
                                        col.spacing(10).push(payment_view(
                                            i,
                                            tx.compute_txid(),
                                            output,
                                            network,
                                            labels,
                                            labels_editing,
                                            bitcoin_unit,
                                            is_single_payment,
                                            is_editable,
                                        ))
                                    },
                                ),
                        ),
                )
                .style(theme::card::simple)
            } else {
                Container::new(
                    Column::new()
                        .push(h4_bold("Self-transfer").style(|t| {
                            theme::text::custom(t.colors.buttons.transparent_border.active.text)
                        }))
                        .push(
                            p2_regular(
                                "No external recipients — every output returns to this wallet.",
                            )
                            .style(|t| {
                                theme::text::custom(t.colors.buttons.transparent_border.active.text)
                            }),
                        )
                        .spacing(6),
                )
                .padding(20)
                .width(Length::Fill)
                .style(theme::card::simple)
            }
        })
        .push(if !is_external && !change_indexes.is_empty() {
            Some(
                Container::new(Collapse::new(
                    move || {
                        Button::new(
                            Row::new()
                                .align_y(Alignment::Center)
                                .push(h4_bold("Change").width(Length::Fill))
                                .push(icon::collapse_icon()),
                        )
                        .padding(20)
                        .width(Length::Fill)
                        .style(theme::button::transparent_border)
                    },
                    move || {
                        Button::new(
                            Row::new()
                                .align_y(Alignment::Center)
                                .push(h4_bold("Change").width(Length::Fill))
                                .push(icon::collapsed_icon()),
                        )
                        .padding(20)
                        .width(Length::Fill)
                        .style(theme::button::transparent_border)
                    },
                    move || {
                        tx.output
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| change_indexes.contains(i))
                            .fold(
                                Column::new().padding(20),
                                |col: Column<'a, Message>, (_, output)| {
                                    col.spacing(10)
                                        .push(change_view(output, network, bitcoin_unit))
                                },
                            )
                            .into()
                    },
                ))
                .style(theme::card::simple),
            )
        } else {
            None
        })
        .into()
}

fn input_view<'a>(
    outpoint: &'a OutPoint,
    coin: Option<&'a Coin>,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let outpoint = outpoint.to_string();
    Column::new()
        .width(Length::Fill)
        .push(
            Row::new()
                .spacing(5)
                .align_y(Alignment::Center)
                .push(
                    Container::new(if let Some(label) = labels_editing.get(&outpoint) {
                        label::label_editing(vec![outpoint.clone()], label, text::P1_SIZE)
                    } else {
                        label::label_editable(
                            vec![outpoint.clone()],
                            labels.get(&outpoint),
                            text::P1_SIZE,
                        )
                    })
                    .width(Length::Fill),
                )
                .push_maybe(coin.map(|c| amount_with_unit(&c.amount, bitcoin_unit))),
        )
        .push(
            Column::new()
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(5)
                        .push(p1_bold("Outpoint:").style(theme::text::secondary))
                        .push(p2_regular(outpoint.clone()).style(theme::text::secondary))
                        .push(
                            Button::new(icon::clipboard_icon().style(theme::text::secondary))
                                .on_press(Message::Clipboard(outpoint.clone()))
                                .style(theme::button::transparent_border),
                        ),
                )
                .push(coin.map(|c| {
                    let addr = c.address.to_string();
                    Row::new()
                        .align_y(Alignment::Center)
                        .width(Length::Fill)
                        .push(
                            Row::new()
                                .align_y(Alignment::Center)
                                .width(Length::Fill)
                                .spacing(5)
                                .push(p1_bold("Address:").style(theme::text::secondary))
                                .push(p2_regular(addr.clone()).style(theme::text::secondary))
                                .push(
                                    Button::new(
                                        icon::clipboard_icon().style(theme::text::secondary),
                                    )
                                    .on_press(Message::Clipboard(addr))
                                    .style(theme::button::transparent_border),
                                ),
                        )
                }))
                .push(coin.and_then(|c| {
                    labels.get(&c.address.to_string()).map(|label| {
                        Row::new()
                            .align_y(Alignment::Center)
                            .width(Length::Fill)
                            .push(
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .width(Length::Fill)
                                    .spacing(5)
                                    .push(p1_bold("Address label:").style(theme::text::secondary))
                                    .push(p2_regular(label).style(theme::text::secondary)),
                            )
                    })
                })),
        )
        .spacing(5)
        .into()
}

#[allow(clippy::too_many_arguments)]
fn payment_view<'a>(
    i: usize,
    txid: Txid,
    output: &'a TxOut,
    network: Network,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
    is_single_payment: bool,
    is_editable: bool,
) -> Element<'a, Message> {
    let addr = Address::from_script(&output.script_pubkey, network)
        .ok()
        .map(|a| a.to_string());
    let txid = txid.to_string();
    let outpoint = OutPoint {
        txid: txid.parse().expect("txid string is always valid"),
        vout: i as u32,
    }
    .to_string();

    let labelled = if is_single_payment {
        vec![outpoint.clone(), txid.clone()]
    } else {
        vec![outpoint.clone()]
    };
    let label_widget = if is_editable {
        if let Some(label) = labels_editing.get(&outpoint).or_else(|| {
            is_single_payment
                .then(|| labels_editing.get(&txid))
                .flatten()
        }) {
            label::label_editing(labelled, label, text::P1_SIZE)
        } else {
            label::label_editable(labelled, labels.get(&outpoint), text::P1_SIZE)
        }
    } else {
        label::label_non_editable(labelled, labels.get(&outpoint), text::P1_SIZE)
    };
    Column::new()
        .width(Length::Fill)
        .spacing(5)
        .push(
            Row::new()
                .spacing(5)
                .align_y(Alignment::Center)
                .push(Container::new(label_widget).width(Length::Fill))
                .push(amount_with_unit(&output.value, bitcoin_unit)),
        )
        .push(addr.map(|addr| {
            Column::new()
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .width(Length::Fill)
                        .push(
                            Row::new()
                                .align_y(Alignment::Center)
                                .width(Length::Fill)
                                .spacing(5)
                                .push(p1_bold("Address:").style(theme::text::secondary))
                                .push(p2_regular(addr.clone()).style(theme::text::secondary))
                                .push(
                                    Button::new(
                                        icon::clipboard_icon().style(theme::text::secondary),
                                    )
                                    .on_press(Message::Clipboard(addr.clone()))
                                    .style(theme::button::transparent_border),
                                ),
                        ),
                )
                .push(labels.get(&addr).map(|label| {
                    Row::new()
                        .align_y(Alignment::Center)
                        .width(Length::Fill)
                        .push(
                            Row::new()
                                .align_y(Alignment::Center)
                                .width(Length::Fill)
                                .spacing(5)
                                .push(p1_bold("Address label:").style(theme::text::secondary))
                                .push(p2_regular(label).style(theme::text::secondary)),
                        )
                }))
        }))
        .into()
}

fn change_view(
    output: &TxOut,
    network: Network,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<Message> {
    let addr = Address::from_script(&output.script_pubkey, network)
        .unwrap()
        .to_string();
    Column::new()
        .width(Length::Fill)
        .spacing(5)
        .push(
            Row::new()
                .push(Space::new().width(Length::Fill))
                .push(amount_with_unit(&output.value, bitcoin_unit)),
        )
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .width(Length::Fill)
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .width(Length::Fill)
                        .spacing(5)
                        .push(p1_bold("Address:").style(theme::text::secondary))
                        .push(p2_regular(addr.clone()).style(theme::text::secondary))
                        .push(
                            Button::new(icon::clipboard_icon().style(theme::text::secondary))
                                .on_press(Message::Clipboard(addr))
                                .style(theme::button::transparent_border),
                        ),
                ),
        )
        .into()
}

/// A Keychain signer row for the unified signing picker. Built by
/// `SignModal` from the nested Keychain flow once resolved, or derived from
/// the descriptor before/without Connect. `status_label == None` means the
/// signer is idle (clickable to request a signature); `Some` shows live
/// session state.
pub enum SigningKeyKind {
    Master,
    Hardware,
    BorderWallet,
    Keychain,
    /// Not identifiable from local data — an external key whose device isn't
    /// connected and which Connect hasn't resolved. Shown disabled rather than
    /// guessed as hardware or keychain.
    Unknown,
}

/// What clicking an available key row does.
pub enum SigningKeyAction {
    Master,
    Hardware(usize),
    BorderWallet,
    Keychain,
}

pub enum SigningKeyState {
    /// Signature already collected for this key.
    Signed,
    /// A signing operation is under way; carries status text.
    InProgress(String),
    /// Ready to sign — carries the action to fire.
    Available(SigningKeyAction),
    /// Failed keychain session; carries the `pending` index for retry.
    Retry(usize),
    /// Can't sign right now; carries the reason shown on the row.
    Disabled(String),
    /// Unidentified key while Connect is signed out: carries a reason and a
    /// clickable "Sign in to Connect" affordance so the user can authenticate
    /// (which then resolves any Keychain signers among these rows).
    NeedsSignIn(String),
}

pub struct SigningKeyRow {
    pub fingerprint: Fingerprint,
    pub label: String,
    pub kind: SigningKeyKind,
    pub state: SigningKeyState,
}

/// One spending-path card in the signing picker, mirroring the vault-creation
/// "Set keys" layout: a titled, colored card with a subtitle, per-key rows and
/// a "k out of n" threshold badge.
pub struct SigningPath {
    pub title: String,
    pub is_primary: bool,
    /// Recovery timelock (in blocks) for the subtitle; `None` for the primary.
    pub sequence: Option<u16>,
    /// Whether this is the path the transaction actually spends through. Keys
    /// in inactive paths render disabled.
    pub active: bool,
    pub threshold: usize,
    pub total: usize,
    pub collected: usize,
    pub keys: Vec<SigningKeyRow>,
    /// True when the active path has ≥1 idle keychain signer, enabling the
    /// "Request from everyone" batch affordance.
    pub can_request_all: bool,
    /// Whether the card is expanded (keys shown). Active paths are always
    /// expanded; inactive paths default to collapsed and toggle via
    /// `ToggleSpendPath`.
    pub expanded: bool,
}

/// Short human duration for a recovery timelock, e.g. "1y 2m". Mirrors the
/// creation flow's `format_sequence_duration` without the cross-module dep.
fn humanize_timelock(sequence: u16) -> String {
    let mut n_minutes = sequence as u32 * 10;
    let n_years = n_minutes / 525_960;
    n_minutes -= n_years * 525_960;
    let n_months = n_minutes / 43_830;
    n_minutes -= n_months * 43_830;
    let n_days = n_minutes / 1_440;
    n_minutes -= n_days * 1_440;
    let n_hours = n_minutes / 60;
    n_minutes -= n_hours * 60;
    [
        (n_years, "y"),
        (n_months, "m"),
        (n_days, "d"),
        (n_hours, "h"),
        (n_minutes, "mn"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, unit)| format!("{}{}", n, unit))
    .collect::<Vec<_>>()
    .join(" ")
}

fn signing_key_kind_caption(kind: &SigningKeyKind) -> Option<&'static str> {
    match kind {
        SigningKeyKind::Master => Some("This computer"),
        SigningKeyKind::Hardware => Some("Hardware wallet"),
        SigningKeyKind::BorderWallet => Some("Border Wallet"),
        SigningKeyKind::Keychain => Some("Keychain"),
        SigningKeyKind::Unknown => None,
    }
}

/// Render one signer row inside a spending-path card. The key icon is colored
/// with the path color, matching the creation "Set keys" look.
fn signing_key_row(row: &SigningKeyRow, color: iced::Color) -> Element<'static, Message> {
    let key_icon = match row.kind {
        SigningKeyKind::Hardware => icon::usb_drive_icon(),
        _ => icon::round_key_icon(),
    }
    .size(text::H3_SIZE)
    .style(theme::text::adaptive(color));

    let mut info = Column::new()
        .spacing(2)
        .width(Length::Fill)
        .push(p1_bold(row.label.clone()));
    if let Some(caption) = signing_key_kind_caption(&row.kind) {
        info = info.push(text::caption(caption));
    }

    let right: Element<Message> = match &row.state {
        SigningKeyState::Signed => Row::new()
            .spacing(5)
            .align_y(Alignment::Center)
            .push(p1_regular("Signed").color(color::GREEN))
            .push(icon::check_icon().style(theme::text::success))
            .into(),
        SigningKeyState::InProgress(status) => p1_regular(status.clone())
            .style(theme::text::secondary)
            .into(),
        SigningKeyState::Disabled(reason) => p1_regular(reason.clone())
            .style(theme::text::secondary)
            .into(),
        SigningKeyState::NeedsSignIn(reason) => Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(p1_regular(reason.clone()).style(theme::text::secondary))
            .push(
                // Bubbles straight to the tab (which opens/focuses the Home
                // tab for sign-in) without routing through the PSBT panel, so
                // the picker stays open behind the new tab.
                button::secondary(None, "Sign in to Connect").on_press(Message::OpenConnectSignIn),
            )
            .into(),
        SigningKeyState::Retry(idx) => button::secondary(Some(icon::reload_icon()), "Retry")
            .on_press(Message::Spend(SpendTxMessage::RetryKeychainSigner(*idx)))
            .into(),
        SigningKeyState::Available(action) => {
            let (msg, label) = match action {
                SigningKeyAction::Master => {
                    (Message::Spend(SpendTxMessage::SelectMasterSigner), "Sign")
                }
                SigningKeyAction::Hardware(i) => (Message::SelectHardwareWallet(*i), "Sign"),
                SigningKeyAction::BorderWallet => (
                    Message::Spend(SpendTxMessage::SelectBorderWallet(row.fingerprint)),
                    "Sign",
                ),
                SigningKeyAction::Keychain => (
                    Message::Spend(SpendTxMessage::SelectKeychainSigner(row.fingerprint)),
                    "Request",
                ),
            };
            button::primary(None, label).on_press(msg).into()
        }
    };

    card::simple(
        Row::new()
            .spacing(10)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(key_icon)
            .push(info)
            .push(right),
    )
    .into()
}

/// The "k out of n" badge with mini key icons, adapted to show collected
/// signatures. Colored icons = signatures collected; text = collected / threshold.
fn signing_threshold_badge<'a>(
    color: iced::Color,
    collected: usize,
    threshold: usize,
    total: usize,
) -> Element<'a, Message> {
    card::simple(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push((0..total).fold(Row::new(), |row, i| {
                if i < collected {
                    row.push(icon::round_key_icon().style(theme::text::adaptive(color)))
                } else {
                    row.push(icon::round_key_icon())
                }
            }))
            .push(text(format!(
                "{} of {} signature{} collected",
                collected,
                threshold,
                if threshold == 1 { "" } else { "s" }
            ))),
    )
    .padding(10)
    .into()
}

/// Render one spending-path card, mirroring the creation "Set keys" page. The
/// active path is fully expanded; inactive paths default to a collapsed header
/// (title + subtitle + note) with a caret, toggled via `ToggleSpendPath`.
fn signing_path_card(path: SigningPath) -> Element<'static, Message> {
    let SigningPath {
        title,
        is_primary,
        sequence,
        active,
        threshold,
        total,
        collected,
        keys,
        can_request_all,
        expanded,
    } = path;
    let color = if is_primary {
        color::GREEN
    } else {
        color::BLUE
    };
    let subtitle_text = if is_primary {
        "Able to move the funds at any time.".to_string()
    } else {
        format!(
            "Available after inactivity of ~{}",
            sequence.map(humanize_timelock).unwrap_or_default()
        )
    };

    // Header. For an inactive path it's a toggle button (with a caret and the
    // "not available" note); for the active path it's a plain title + subtitle.
    let header: Element<Message> = if active {
        Column::new()
            .spacing(6)
            .push(Row::new().push(Space::new().width(10)).push(p1_bold(title)))
            .push(
                Row::new()
                    .padding(5)
                    .push(p1_regular(subtitle_text).style(theme::text::secondary)),
            )
            .into()
    } else {
        let caret = if expanded {
            icon::collapsed_icon()
        } else {
            icon::collapse_icon()
        };
        Button::new(
            Column::new()
                .spacing(6)
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(20)
                        .push(
                            Column::new()
                                .spacing(2)
                                .width(Length::Fill)
                                .push(p1_bold(title))
                                .push(p1_regular(subtitle_text).style(theme::text::secondary)),
                        )
                        .push(caret),
                )
                .push(
                    p2_regular("This spending path isn't available for this transaction.")
                        .style(theme::text::secondary),
                ),
        )
        .padding(10)
        .width(Length::Fill)
        .style(theme::button::transparent_border)
        .on_press(Message::Spend(SpendTxMessage::ToggleSpendPath(sequence)))
        .into()
    };

    let mut col = Column::new().spacing(10).push(header);

    if expanded {
        let mut keys_col = Column::new().spacing(5);
        for key in &keys {
            keys_col = keys_col.push(signing_key_row(key, color));
        }
        col = col
            .push(keys_col)
            .push(signing_threshold_badge(color, collected, threshold, total));
        if can_request_all {
            col = col.push(
                button::secondary(None, "Request from everyone")
                    .on_press(Message::Spend(SpendTxMessage::RequestFromEveryone))
                    .width(Length::Fill),
            );
        }
    }

    Container::new(col)
        .padding(10)
        .style(theme::card::border)
        .into()
}

/// The unified signing picker: spending-path cards (primary + recovery), each
/// listing its keys with per-key signability, mirroring the vault-creation
/// "Set keys" layout.
pub fn sign_action<'a>(
    paths: Vec<SigningPath>,
    keychain_notices: Vec<String>,
) -> Element<'a, Message> {
    let mut col = Column::new().spacing(15).width(Length::Fill);
    for notice in &keychain_notices {
        col = col.push(p1_regular(notice.clone()).style(theme::text::secondary));
    }
    for path in paths {
        col = col.push(signing_path_card(path));
    }
    card::simple(scrollable(col.padding(5)))
        // Wide enough that long device names, the per-path key rows and their
        // "Sign in to Connect" affordances don't crowd each other.
        .width(Length::Fixed(780.0))
        .max_height(760.0)
        .into()
}

/// Render the Border Wallet reconstruction wizard within the signing modal.
pub fn border_wallet_recon_view(recon: &BorderWalletReconstructionState) -> Element<'_, Message> {
    match recon.step {
        ReconStep::RecoveryPhrase => border_wallet_recon_phrase_view(recon),
        ReconStep::Grid => border_wallet_recon_grid_view(recon),
    }
}

fn border_wallet_recon_phrase_view(
    recon: &BorderWalletReconstructionState,
) -> Element<'_, Message> {
    let header = modal::header(
        Some("Reconstruct Border Wallet".to_string()),
        Some(|| {
            Message::Spend(SpendTxMessage::BorderWalletRecon(
                BorderWalletReconMessage::Cancel,
            ))
        }),
    );

    let mut word_rows = Column::new().spacing(6);
    for chunk_start in (0..12).step_by(3) {
        let mut row_widget = Row::new().spacing(8);
        for i in chunk_start..std::cmp::min(chunk_start + 3, 12) {
            let label = format!("{}.", i + 1);
            let idx = i;
            let input = TextInput::new(&format!("Word {}", i + 1), &recon.phrase_words[i].value)
                .on_input(move |s| {
                    Message::Spend(SpendTxMessage::BorderWalletRecon(
                        BorderWalletReconMessage::PhraseWordEdited(idx, s),
                    ))
                })
                .width(Length::Fill);
            row_widget = row_widget.push(
                Row::new()
                    .push(text::text(label).width(25))
                    .push(input)
                    .spacing(4)
                    .width(Length::Fill),
            );
        }
        word_rows = word_rows.push(row_widget);
    }

    let error_text: Option<Element<Message>> = recon
        .error
        .as_ref()
        .map(|e| p1_regular(e.as_str()).color(color::RED).into());

    let cancel_btn = button::transparent(None, "Cancel").on_press(Message::Spend(
        SpendTxMessage::BorderWalletRecon(BorderWalletReconMessage::Cancel),
    ));

    let next_btn = if recon.phrase_valid {
        button::primary(None, "Next").on_press(Message::Spend(SpendTxMessage::BorderWalletRecon(
            BorderWalletReconMessage::Next,
        )))
    } else {
        button::primary(None, "Next")
    }
    .width(Length::Fill);

    let mut col = Column::new()
        .spacing(12)
        .push(header)
        .push(p1_regular(format!(
            "Enter the 12-word recovery phrase for key #{}",
            recon.target_fingerprint
        )))
        .push(word_rows);

    if let Some(err) = error_text {
        col = col.push(err);
    }

    col = col.push(Row::new().spacing(10).push(cancel_btn).push(next_btn));

    Container::new(col.width(550))
        .padding(20)
        .style(theme::card::modal)
        .into()
}

fn border_wallet_recon_grid_view(recon: &BorderWalletReconstructionState) -> Element<'_, Message> {
    let header = modal::header(
        Some("Select Pattern".to_string()),
        Some(|| {
            Message::Spend(SpendTxMessage::BorderWalletRecon(
                BorderWalletReconMessage::Cancel,
            ))
        }),
    );

    let back_btn =
        button::transparent(Some(icon::previous_icon()), "Back").on_press(Message::Spend(
            SpendTxMessage::BorderWalletRecon(BorderWalletReconMessage::Previous),
        ));

    let status = p1_bold(format!(
        "Selected: {} / {} cells",
        recon.pattern.len(),
        PATTERN_LENGTH
    ));

    let undo_btn = if !recon.pattern.is_empty() {
        button::secondary(None, "Undo").on_press(Message::Spend(SpendTxMessage::BorderWalletRecon(
            BorderWalletReconMessage::UndoLastCell,
        )))
    } else {
        button::secondary(None, "Undo")
    };

    let clear_btn = if !recon.pattern.is_empty() {
        button::secondary(None, "Clear").on_press(Message::Spend(
            SpendTxMessage::BorderWalletRecon(BorderWalletReconMessage::ClearPattern),
        ))
    } else {
        button::secondary(None, "Clear")
    };

    let grid_content: Element<Message> = if let Some(grid) = &recon.grid {
        let mut grid_col = Column::new().spacing(1);

        for row_idx in 0..WordGrid::ROWS {
            let mut row_widget = Row::new().spacing(1);
            for col_idx in 0..WordGrid::COLS {
                let word = grid.word_at(row_idx, col_idx).unwrap_or("???");
                let prefix = &word[..std::cmp::min(4, word.len())];
                let cell = CellRef::new(row_idx as u16, col_idx as u8);
                let is_selected = recon.pattern.cells().contains(&cell);

                let order_num = recon
                    .pattern
                    .cells()
                    .iter()
                    .position(|c| c == &cell)
                    .map(|p| p + 1);

                let label = if let Some(n) = order_num {
                    format!("{}", n)
                } else {
                    prefix.to_string()
                };

                let cell_style = if is_selected {
                    theme::button::primary
                } else {
                    theme::button::secondary
                };

                let r = row_idx as u16;
                let c = col_idx as u8;
                let cell_btn = Button::new(text::text(label).size(11).align_x(Horizontal::Center))
                    .style(cell_style)
                    .on_press(Message::Spend(SpendTxMessage::BorderWalletRecon(
                        BorderWalletReconMessage::ToggleCell(r, c),
                    )))
                    .width(50)
                    .height(28);

                row_widget = row_widget.push(cell_btn);
            }
            grid_col = grid_col.push(row_widget);
        }
        scrollable(grid_col).height(400).into()
    } else {
        p1_regular("No grid available").into()
    };

    let error_text: Option<Element<Message>> = recon
        .error
        .as_ref()
        .map(|e| p1_regular(e.as_str()).color(color::RED).into());

    let next_btn = if recon.pattern.is_complete() {
        button::primary(None, "Sign").on_press(Message::Spend(SpendTxMessage::BorderWalletRecon(
            BorderWalletReconMessage::Next,
        )))
    } else {
        button::primary(None, "Sign")
    }
    .width(Length::Fill);

    let mut col = Column::new()
        .spacing(10)
        .push(header)
        .push(p1_regular(
            "Select 11 cells from the grid in order \u{2014} the same pattern you used during enrollment.",
        ))
        .push(
            Row::new()
                .spacing(10)
                .push(status)
                .push(undo_btn)
                .push(clear_btn),
        )
        .push(grid_content);

    if let Some(checksum) = &recon.checksum_word {
        col = col.push(p1_bold(format!("Checksum word: \"{}\"", checksum)).color(color::GREEN));
    }

    if let Some(err) = error_text {
        col = col.push(err);
    }

    col = col.push(Row::new().spacing(10).push(back_btn).push(next_btn));

    Container::new(col.width(850))
        .padding(20)
        .style(theme::card::modal)
        .into()
}

pub fn sign_action_toasts<'a>(
    hws: &'a [HardwareWallet],
    signing: &HashSet<Fingerprint>,
) -> Vec<Element<'a, Message>> {
    let vec: Vec<Element<'a, Message>> = hws
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
                        coincube_ui::component::notification::processing_hardware_wallet(
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

    vec
}

pub fn update_spend_view<'a>(
    psbt: String,
    updated: &form::Value<String>,
    processing: bool,
) -> Element<'a, Message> {
    Column::new()
        .push(card::simple(
            Column::new()
                .spacing(20)
                .push(
                    Row::new()
                        .push(text("PSBT:").bold().width(Length::Fill))
                        .push(
                            button::secondary(Some(icon::clipboard_icon()), "Copy")
                                .on_press(Message::Clipboard(psbt)),
                        )
                        .align_y(Alignment::Center),
                )
                .push(separation().width(Length::Fill))
                .push(
                    Column::new()
                        .spacing(10)
                        .push(text("Insert updated PSBT:").bold())
                        .push(
                            form::Form::new_trimmed("PSBT", updated, move |msg| {
                                Message::ImportSpend(ImportSpendMessage::PsbtEdited(msg))
                            })
                            .warning("Please enter the correct base64 encoded PSBT")
                            .size(P1_SIZE)
                            .padding(10),
                        )
                        .push(Row::new().push(Space::new().width(Length::Fill)).push(
                            if updated.valid && !updated.value.is_empty() && !processing {
                                button::secondary(None, "Update")
                                    .on_press(Message::ImportSpend(ImportSpendMessage::Confirm))
                            } else if processing {
                                button::secondary(None, "Processing...")
                            } else {
                                button::secondary(None, "Update")
                            },
                        )),
                ),
        ))
        .max_width(400)
        .into()
}

pub fn update_spend_success_view<'a>() -> Element<'a, Message> {
    Column::new()
        .push(
            card::simple(Container::new(
                text("Spend transaction is updated").style(theme::text::secondary),
            ))
            .padding(50),
        )
        .width(Length::Fixed(400.0))
        .align_x(Alignment::Center)
        .into()
}
