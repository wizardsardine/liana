use coincube_ui::locale::copy as localized_copy;
use coincube_ui::locale::notice;
use coincube_ui::localized_format;
use std::collections::{HashMap, HashSet};

use coincube_ui::locale::Text as CopyText;
use iced::advanced::widget::Id;
use iced::{
    alignment::Horizontal,
    widget::{checkbox, scrollable, text::Wrapping, tooltip, Space},
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
    widget::{Button, Column, ColumnExt, Container, Element, Row, RowExt, TextInput},
};

pub(crate) fn action_id(scope: u64, name: &str) -> Id {
    Id::from(format!("tenshu.psbt.{scope}.{name}"))
}

fn primary_action<'a>(
    id: Id,
    label: &'static str,
) -> coincube_ui::widget::action::Action<'a, Message> {
    button::primary_action(id, None, label, Message::FocusAction)
}

fn secondary_action<'a>(
    id: Id,
    label: &'static str,
) -> coincube_ui::widget::action::Action<'a, Message> {
    button::secondary_action(id, None, label, Message::FocusAction)
}

use crate::{
    app::{
        cache::Cache,
        menu::{Menu, VaultSubMenu},
        state::vault::{
            psbt::{BorderWalletReconstructionState, ReconStep},
            replay::{self, PillTone, ReplayReview},
        },
        view::{dashboard, message::*, vault::label},
    },
    daemon::model::{Coin, SpendStatus, SpendTx},
    hw::HardwareWallet,
    services::entangled::Entanglement,
};

/// What the spend screen needs to render the replay-protection pill of a
/// Bitcoin Blake2b spend. `None` on every other chain: nothing is added to
/// the screen and Broadcast gates on the path threshold as before.
#[derive(Debug, Clone)]
pub struct ReplayPill<'a> {
    /// The review as of the current cache
    /// ([`crate::app::state::vault::psbt::PsbtState::current_review`]).
    pub review: std::borrow::Cow<'a, ReplayReview>,
    /// [`replay::entangled_inputs`] for this PSBT, from the cache.
    pub entangled: Vec<(usize, Entanglement)>,
    /// [`crate::app::state::vault::psbt::PsbtState::broadcast_ready`].
    pub broadcast_ready: bool,
    /// The spend screen is re-checking the inputs against the twin chain
    /// right now; Broadcast waits for the answer.
    pub checking: bool,
    /// Inputs (by index) the re-check could not get an answer for.
    pub unresolved: Vec<usize>,
}

/// The status pill plus, for a replayable spend, either the acknowledgement
/// the user must give before Broadcast is enabled or — when a replayable
/// input is known to also exist on Bitcoin (`#276` I13) — the requirement
/// that cannot be acknowledged away.
fn replay_status_view<'a>(pill: &ReplayPill<'a>) -> Element<'a, Message> {
    let (label, tone) = replay::pill_copy(&pill.review.status(), &pill.entangled);
    let blocked = replay::blocked_entangled_inputs(&pill.review.status(), &pill.entangled);
    let style: fn(&theme::Theme) -> iced::widget::container::Style = match tone {
        PillTone::Success => theme::pill::success,
        PillTone::Warning => theme::pill::warning,
        PillTone::Neutral => theme::pill::simple,
    };
    let mut column = Column::new().spacing(10).padding(15).push(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(p1_bold(localized_copy("Replay")))
            .push(
                Container::new(p2_regular(label))
                    .padding(10)
                    .center_x(Length::Shrink)
                    .style(style),
            ),
    );
    if let Some(required) = replay::blocked_entangled_copy(&blocked) {
        // No checkbox: the acknowledgement does not apply to a required
        // signature, and showing one would suggest it does.
        column = column
            .push(p2_regular(required).style(theme::text::warning))
            .push(
                coincube_ui::component::button::secondary(None, localized_copy("Split first"))
                    .on_press(Message::SplitFirst),
            );
    } else if pill.review.status().needs_acknowledgement() {
        if pill.checking {
            column = column.push(
                p2_regular(localized_copy(replay::CHECKING_COPY)).style(theme::text::secondary),
            );
        } else if !pill.unresolved.is_empty() {
            let (noun, verb) = if pill.unresolved.len() == 1 {
                (localized_copy("input"), localized_copy("exists"))
            } else {
                (localized_copy("inputs"), localized_copy("exist"))
            };
            column = column.push(
                p2_regular(localized_format!(
                    "Could not check whether {} {} {} also on Bitcoin (Connect did not answer). \
                     You can still send after acknowledging below; the check is retried after \
                     the next sync.",
                    "No se pudo comprobar si {} {} {} también en Bitcoin porque Connect no respondió. Puedes enviar después de confirmar abajo. La comprobación se repetirá tras la próxima sincronización.",
                    noun,
                    pill.unresolved
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    verb
                ))
                .style(theme::text::warning),
            );
        }
        let acknowledged = pill.review.acknowledged();
        column = column.push(
            checkbox(acknowledged)
                .label(localized_copy(replay::REPLAYABLE_ACKNOWLEDGEMENT))
                .on_toggle(|checked| Message::Spend(SpendTxMessage::AcknowledgeReplay(checked))),
        );
    } else if matches!(pill.review.status(), replay::ReplayStatus::Protected) {
        column = column.push(
            p2_regular(localized_copy(replay::PROTECTED_LIMITATION)).style(theme::text::secondary),
        );
    }
    let unchecked: Vec<usize> = pill
        .entangled
        .iter()
        .filter(|(_, status)| matches!(status, Entanglement::Unknown))
        .map(|(index, _)| *index)
        .collect();
    if !unchecked.is_empty() {
        column = column.push(
            p2_regular(localized_format!(
                "Not yet checked whether {} {} also on Bitcoin (the lookup will run after the next sync).",
                "Aún falta comprobar si {} {} también existen en Bitcoin. La comprobación se hará después de la próxima sincronización.",
                localized_copy(if unchecked.len() == 1 { "input" } else { "inputs" }),
                unchecked
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .style(theme::text::secondary),
        );
    }
    column.into()
}

#[allow(clippy::too_many_arguments)]
pub fn psbt_view<'a>(
    action_scope: u64,
    cache: &'a Cache,
    tx: &'a SpendTx,
    saved: bool,
    desc_info: &'a CoincubePolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    network: Network,
    currently_signing: bool,
    bitcoin_unit: BitcoinDisplayUnit,
    replay: Option<ReplayPill<'a>>,
    broadcast_ready: bool,
    recipient_identities: &'a [(usize, crate::services::branta::LookupResult)],
) -> Element<'a, Message> {
    // A pending PSBT is an authorization screen: Sign and Broadcast must
    // always show the exact amounts the user is approving. Once the proposal
    // is terminal, its detail is passive and follows the global privacy eye.
    let mask_amounts = mask_passive_psbt_detail(cache.balance_amounts_masked(), &tx.status);
    let identity_summary = recipient_identity_summary(
        tx,
        network,
        bitcoin_unit,
        broadcast_ready,
        recipient_identities,
        cache.theme_mode,
        mask_amounts,
    );
    let output_identities = if identity_summary.is_some() {
        &[][..]
    } else {
        recipient_identities
    };
    dashboard(
        &Menu::Vault(VaultSubMenu::PSBTs(None)),
        cache,
        Column::new()
            .spacing(20)
            .push(
                secondary_action(
                    action_id(action_scope, "overview.back"),
                    localized_copy("‹ All PSBTs"),
                )
                .on_press(Message::Close),
            )
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .spacing(10)
                    .push(
                        Container::new(h3(localized_copy("PSBT proposal")).bold())
                            .width(Length::Fill),
                    )
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
            .push(p1_regular(crate::chain::canonical_network_name(
                cache.chain(),
            )))
            .push(spend_header_masked(
                tx,
                labels_editing,
                bitcoin_unit,
                mask_amounts,
            ))
            .push_maybe(identity_summary)
            .push(spend_overview_view(
                action_scope,
                tx,
                desc_info,
                key_aliases,
                currently_signing,
                saved,
                replay,
                broadcast_ready,
                true,
            ))
            .push(
                Column::new()
                    .spacing(20)
                    .push(history_inputs_view(
                        &tx.coins,
                        &tx.psbt.unsigned_tx,
                        &tx.labels,
                        labels_editing,
                        bitcoin_unit,
                        mask_amounts,
                    ))
                    .push(outputs_view_with_identities_masked(
                        &tx.psbt.unsigned_tx,
                        network,
                        &tx.change_indexes,
                        &tx.labels,
                        labels_editing,
                        bitcoin_unit,
                        tx.is_single_payment().is_some(),
                        false,
                        output_identities,
                        cache.theme_mode,
                        mask_amounts,
                    )),
            )
            .push(if saved {
                Row::new()
                    .push(
                        secondary_action(
                            action_id(action_scope, "overview.delete"),
                            localized_copy("Delete"),
                        )
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
                        secondary_action(
                            action_id(action_scope, "overview.save"),
                            localized_copy("Save"),
                        )
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

fn mask_passive_psbt_detail(balance_amounts_masked: bool, status: &SpendStatus) -> bool {
    balance_amounts_masked && *status != SpendStatus::Pending
}

/// The detail screen shows a current Branta association for a single external
/// recipient beside the signing controls. Readiness comes from the spend state;
/// Branta only identifies the recipient and never asserts that a PSBT is safe.
fn recipient_identity_summary<'a>(
    tx: &'a SpendTx,
    network: Network,
    bitcoin_unit: BitcoinDisplayUnit,
    broadcast_ready: bool,
    identities: &'a [(usize, crate::services::branta::LookupResult)],
    theme_mode: coincube_ui::theme::palette::ThemeMode,
    mask_amounts: bool,
) -> Option<Element<'a, Message>> {
    if !broadcast_ready || tx.status != SpendStatus::Pending || tx.is_send_to_self() {
        return None;
    }
    // Keep the per-output cards for multi-recipient PSBTs, so the identities
    // remain next to their respective outputs rather than one appearing to
    // apply to the whole transaction.
    if tx
        .psbt
        .unsigned_tx
        .output
        .iter()
        .enumerate()
        .filter(|(index, _)| !tx.change_indexes.contains(index))
        .count()
        != 1
    {
        return None;
    }
    let (vout, output) = tx
        .psbt
        .unsigned_tx
        .output
        .iter()
        .enumerate()
        .find(|(index, _)| !tx.change_indexes.contains(index))?;
    let (_, crate::services::branta::LookupResult::Identified(matches)) =
        identities.iter().find(|(index, _)| *index == vout)?
    else {
        return None;
    };
    let address = Address::from_script(&output.script_pubkey, network).ok()?;
    let current: Vec<_> = matches
        .iter()
        .enumerate()
        .filter(|(_, identity)| identity.is_current())
        .collect();
    if current.is_empty() {
        return None;
    }
    let cards = current
        .into_iter()
        .fold(Column::new().spacing(10), |column, (index, identity)| {
            column.push(crate::app::view::shared::recipient_identity::card(
                identity,
                Message::OpenVaultRecipientIdentity(vout, index),
                theme_mode,
            ))
        });
    Some(
        Column::new()
            .spacing(10)
            .push(h4_bold(localized_copy("Recipient identity")))
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Start)
                    .push(
                        p2_regular(address.to_string())
                            .style(theme::text::secondary)
                            .wrapping(Wrapping::WordOrGlyph)
                            .width(Length::Fill),
                    )
                    .push({
                        let amount: Element<'a, Message> = if mask_amounts {
                            p2_regular("••••").into()
                        } else {
                            amount_with_unit(&output.value, bitcoin_unit)
                                .width(Length::Shrink)
                                .into()
                        };
                        amount
                    }),
            )
            .push(cards)
            .into(),
    )
}

pub fn save_action<'a>(saved: bool) -> Element<'a, Message> {
    if saved {
        card::simple(text(localized_copy("Transaction is saved")))
            .width(Length::Fixed(400.0))
            .align_x(iced::alignment::Horizontal::Center)
            .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push(text(localized_copy("Save this transaction")))
                .push(
                    Row::new()
                        .spacing(10)
                        .push(Space::new().width(Length::Fill))
                        .push(
                            button::secondary(None, localized_copy("Ignore"))
                                .on_press(Message::Close),
                        )
                        .push(
                            button::primary(None, localized_copy("Save"))
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
#[cfg(test)]
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
    broadcast_action_with_identity_review(
        crate::chain::ChainId::Bitcoin,
        conflicting_txids,
        saved,
        broadcasting,
        error,
        spend_amount_display,
        sent_quote,
        sent_image_handle,
        is_self_transfer,
        None,
        BroadcastReceipt::default(),
    )
}

/// The receipt's status line for a broadcast.
///
/// A self-transfer that replaces a pending transaction is a cancel: the coins
/// that transaction spent come back to the wallet, minus the new fee, so saying
/// they were "sent" reads as if they had left it. They only come back if the
/// cancellation confirms, though: the original can still be mined first.
fn broadcast_headline(is_self_transfer: bool, replaces_pending: bool) -> Option<&'static str> {
    match (is_self_transfer, replaces_pending) {
        (true, true) => Some(CopyText::CancellationSent.get()),
        (true, false) => Some(CopyText::TransferSent.get()),
        (false, _) => None,
    }
}

/// What the broadcast receipt lists beside the amount.
#[derive(Debug, Default, Clone)]
pub struct BroadcastReceipt<'a> {
    /// Network fee with its unit, when the spend recorded one.
    pub fee: Option<&'a str>,
    pub txid: Option<Txid>,
}

#[allow(clippy::too_many_arguments)]
pub fn broadcast_action_with_identity_review<'a>(
    chain: crate::chain::ChainId,
    conflicting_txids: &HashSet<Txid>,
    saved: bool,
    broadcasting: bool,
    error: Option<String>,
    spend_amount_display: &'a str,
    sent_quote: &'a coincube_ui::component::quote_display::Quote,
    sent_image_handle: &'a iced::widget::image::Handle,
    is_self_transfer: bool,
    identity_review: Option<Element<'a, Message>>,
    receipt: BroadcastReceipt<'a>,
) -> Element<'a, Message> {
    if saved {
        use coincube_ui::component::payment_result::{
            payment_result, short_id, Outcome, ReceiptRow,
        };
        let replaces_pending = !conflicting_txids.is_empty();
        let detail = if is_self_transfer && replaces_pending {
            localized_copy("The coins return to this wallet once the cancellation confirms.")
        } else {
            localized_copy("You can close this. Activity updates when it confirms.")
        };
        let mut page = payment_result(
            Outcome::Confirming,
            spend_amount_display,
            "bitcoin-send",
            sent_quote,
            sent_image_handle,
            Message::Spend(super::super::SpendTxMessage::Cancel),
        )
        .detail(detail)
        .row(ReceiptRow::new(localized_copy("Paid from"), "Vault"))
        .voice(if is_self_transfer {
            None
        } else {
            coincube_ui::worldview_copy::VoiceLine::PaymentSent.get()
        });
        if let Some(headline) = broadcast_headline(is_self_transfer, replaces_pending) {
            page = page.headline(headline);
        }
        if let Some(fee) = receipt.fee {
            page = page.row(ReceiptRow::new(localized_copy("Network fee"), fee));
        }
        if let Some(txid) = receipt.txid {
            let txid = txid.to_string();
            page = page.row(
                ReceiptRow::new(localized_copy("Transaction ID"), short_id(&txid))
                    .mono()
                    .on_copy(Message::CopyWithToast(
                        txid,
                        "Transaction ID copied to clipboard".to_string(),
                    )),
            );
        }
        // The modal draws no surface of its own.
        Container::new(page)
            .padding(32)
            .max_width(1000)
            .style(theme::card::modal)
            .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push(
                    Container::new(h4_bold(CopyText::BroadcastTheTransaction.get()))
                        .width(Length::Fill),
                )
                .push(p1_regular(crate::chain::canonical_network_name(chain)))
                .push(error.map(|err| card::error(CopyText::BroadcastFailed.get(), notice(err))))
                .push(if conflicting_txids.is_empty() {
                    None
                } else {
                    Some(
                        conflicting_txids.iter().fold(
                            Column::new()
                                .spacing(5)
                                .push(Row::new().spacing(10).push(icon::warning_icon()).push(text(
                                    if conflicting_txids.len() > 1 {
                                        localized_copy(
                                            "WARNING: Broadcasting this transaction \
                                        will invalidate some pending payments.",
                                        )
                                    } else {
                                        localized_copy(
                                            "WARNING: Broadcasting this transaction \
                                        will invalidate a pending payment.",
                                        )
                                    },
                                )))
                                .push(Row::new().padding([0, 30]).push(text(
                                    if conflicting_txids.len() > 1 {
                                        localized_copy(
                                            "The following transactions are \
                                        spending one or more inputs \
                                        from the transaction to be \
                                        broadcast and will be \
                                        dropped, along with any other \
                                        transactions that depend on them:",
                                        )
                                    } else {
                                        localized_copy(
                                            "The following transaction is \
                                        spending one or more inputs \
                                        from the transaction to be \
                                        broadcast and will be \
                                        dropped, along with any other \
                                        transactions that depend on it:",
                                        )
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
                                            .on_press(Message::CopyWithToast(
                                                txid.to_string(),
                                                "Copied transaction ID to clipboard".to_string(),
                                            ))
                                            .style(theme::button::transparent_border),
                                        ),
                                )
                            },
                        ),
                    )
                })
                .push_maybe(identity_review)
                .push(Row::new().push(Column::new().width(Length::Fill)).push(
                    // Disabled "Broadcasting…" state mirrors the
                    // "Processing…" pattern used by the PSBT
                    // update form below — leaving `on_press`
                    // unset is this codebase's idiom for a
                    // visually-disabled button.
                    if broadcasting {
                        button::primary(None, localized_copy("Broadcasting…"))
                    } else {
                        button::primary(None, localized_copy("Broadcast"))
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

/// Repeat the output association next to the irreversible broadcast action.
/// The ordinary PSBT review remains visible behind the modal.
pub fn broadcast_recipient_identities<'a>(
    identities: &'a [(usize, crate::services::branta::LookupResult)],
    outputs: &'a [TxOut],
    network: Network,
    bitcoin_unit: BitcoinDisplayUnit,
    theme_mode: coincube_ui::theme::palette::ThemeMode,
) -> Element<'a, Message> {
    let content = identities
        .iter()
        .fold(Column::new().spacing(10), |column, (vout, result)| {
            let crate::services::branta::LookupResult::Identified(matches) = result else {
                return column;
            };
            let Some(output) = outputs.get(*vout) else {
                return column;
            };
            let Ok(address) = Address::from_script(&output.script_pubkey, network) else {
                return column;
            };
            let column = column
                .push(text(address.to_string()).size(12))
                .push(amount_with_unit(&output.value, bitcoin_unit));
            matches
                .iter()
                .enumerate()
                .fold(column, |column, (index, identity)| {
                    column.push(crate::app::view::shared::recipient_identity::card(
                        identity,
                        Message::OpenVaultRecipientIdentity(*vout, index),
                        theme_mode,
                    ))
                })
        });
    Container::new(scrollable(content).height(Length::Shrink))
        .max_height(280)
        .into()
}

pub fn delete_action<'a>(deleted: bool) -> Element<'a, Message> {
    if deleted {
        card::simple(
            Column::new()
                .spacing(20)
                .align_x(Alignment::Center)
                .push(text(localized_copy(
                    "Successfully deleted this transaction.",
                )))
                .push(
                    button::secondary(None, localized_copy("Go back to PSBTs"))
                        .on_press(Message::Close),
                ),
        )
        .align_x(iced::alignment::Horizontal::Center)
        .width(Length::Fixed(400.0))
        .into()
    } else {
        card::simple(
            Column::new()
                .spacing(10)
                .push(text(localized_copy("Delete this PSBT")))
                .push(
                    Row::new()
                        .push(Column::new().width(Length::Fill))
                        .push(
                            button::transparent(None, CopyText::Cancel.get())
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
pub fn keychain_unavailable_action<'a>(action_scope: u64, signed_in: bool) -> Element<'a, Message> {
    let body = if signed_in {
        localized_copy(
            "Connect hasn't finished setting up this device for COINCUBE Mobile \
         signing. Set it up now, or pair a phone over Wi-Fi to sign \
         locally without Connect.",
        )
    } else {
        localized_copy(
            "To sign with a phone through Connect, sign in to \
         COINCUBE Connect. Or pair a phone over Wi-Fi to sign locally — \
         no Connect required.",
        )
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
            .push(text(localized_copy("Mobile signing needs Connect")).bold())
            .push(text(body).style(theme::text::secondary))
            .push(
                Row::new()
                    .spacing(10)
                    .push(Column::new().width(Length::Fill))
                    .push(
                        button::secondary_action(
                            action_id(action_scope, "keychain-unavailable.cancel"),
                            None,
                            CopyText::Cancel.get(),
                            Message::FocusAction,
                        )
                        .style(theme::button::container)
                        .on_press(Message::Spend(SpendTxMessage::Cancel)),
                    )
                    .push(
                        secondary_action(
                            action_id(action_scope, "keychain-unavailable.pair"),
                            localized_copy("Pair a phone"),
                        )
                        .on_press(Message::Spend(SpendTxMessage::KeychainPairPhone))
                        .width(Length::Fixed(140.0)),
                    )
                    .push(
                        primary_action(
                            action_id(action_scope, "keychain-unavailable.connect"),
                            primary_label,
                        )
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
    spend_header_masked(tx, labels_editing, bitcoin_unit, false)
}

fn spend_header_masked<'a>(
    tx: &'a SpendTx,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'a, Message> {
    let txid = tx.psbt.unsigned_tx.compute_txid().to_string();
    let amount_title: Element<'a, Message> = if tx.is_send_to_self() {
        h1(localized_copy("Self-transfer")).into()
    } else if mask_amounts {
        h1("••••").into()
    } else {
        amount_with_size_and_unit(&tx.spend_amount, H1_SIZE, bitcoin_unit).into()
    };
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
            Column::new().push(amount_title).push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(h3(localized_copy("Miner fee: ")).style(theme::text::secondary))
                    .push(if tx.fee_amount.is_none() {
                        Some(text(localized_copy(
                            "Missing information about transaction inputs",
                        )))
                    } else {
                        None
                    })
                    .push_maybe(tx.fee_amount.map(|fee| -> Element<'a, Message> {
                        if mask_amounts {
                            h3("••••").into()
                        } else {
                            amount_with_size_and_unit(&fee, H3_SIZE, bitcoin_unit).into()
                        }
                    }))
                    .push(text(" ").size(H3_SIZE))
                    .push(tx.min_feerate_vb().map(|rate| {
                        text(format!("(~{} sats/vbyte)", rate))
                            .size(H4_SIZE)
                            .style(theme::text::secondary)
                    })),
            ),
        )
        .into()
}

#[allow(clippy::too_many_arguments)]
pub fn spend_overview_view<'a>(
    action_scope: u64,
    tx: &'a SpendTx,
    desc_info: &'a CoincubePolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    currently_signing: bool,
    saved: bool,
    replay: Option<ReplayPill<'a>>,
    broadcast_ready: bool,
    allow_signing: bool,
) -> Element<'a, Message> {
    spend_overview_with_cancel(
        action_scope,
        tx,
        desc_info,
        key_aliases,
        currently_signing,
        saved,
        replay,
        broadcast_ready,
        None,
        allow_signing,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn spend_overview_with_cancel<'a>(
    action_scope: u64,
    tx: &'a SpendTx,
    desc_info: &'a CoincubePolicy,
    key_aliases: &'a HashMap<Fingerprint, String>,
    currently_signing: bool,
    saved: bool,
    replay: Option<ReplayPill<'a>>,
    broadcast_ready: bool,
    cancel: Option<Message>,
    allow_signing: bool,
) -> Element<'a, Message> {
    // The state supplies cryptographically verified readiness. Claimed
    // signature counts below are progress hints only.
    let sign_or_broadcast = match &replay {
        None => !broadcast_ready,
        Some(pill) => !pill.review.signatures_complete(&pill.entangled),
    };
    // Force the user to save (which commits the derivation-index increment to the
    // database) before exporting, otherwise the exported PSBT can lead to change
    // address reuse.
    let export_button = button::secondary_action(
        action_id(action_scope, "overview.export"),
        Some(icon::backup_icon()),
        coincube_ui::locale::copy("Export"),
        Message::FocusAction,
    )
    .on_press_maybe(if currently_signing || !saved {
        None
    } else {
        Some(Message::ExportPsbt)
    });
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
                                                        localized_copy("Sign or save the transaction first to enable export"),
                                                    ))
                                                    .style(theme::card::simple)
                                                    .padding(10),
                                                    tooltip::Position::Top,
                                                ))
                                            })
                                            .push(
                                                button::secondary_action(
                                                    action_id(action_scope, "overview.import"),
                                                    Some(icon::restore_icon()),
                                                    coincube_ui::locale::copy("Import"),
                                                    Message::FocusAction,
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
                                        coincube_ui::widget::action::Action::new(
                                            action_id(action_scope, "overview.copy-txid"),
                                            icon::clipboard_icon().style(theme::text::secondary),
                                            Message::FocusAction,
                                        )
                                        .on_press(Message::CopyWithToast(
                                            tx.psbt.unsigned_tx.compute_txid().to_string(),
                                            "Copied transaction ID to clipboard".to_string(),
                                        ))
                                        .style(theme::button::transparent_border),
                                    )
                                    .align_y(Alignment::Center),
                            ),
                    )
                    .push(signatures(tx, desc_info, key_aliases, match &replay {
                        None => broadcast_ready,
                        Some(pill) => pill.review.signatures_complete(&pill.entangled),
                    }))
                    .push_maybe(replay.as_ref().map(replay_status_view)),
            )
            .style(theme::card::simple),
        )
        .push(if tx.status == SpendStatus::Pending || cancel.is_some() {
            Some(
                Row::new()
                    .push(Space::new().width(Length::Fill))
                    .push_maybe(cancel.map(|message| secondary_action(action_id(action_scope, "overview.cancel"), CopyText::Cancel.get())
                        .on_press(message).width(Length::Fixed(150.0))))
                    .push(if tx.status != SpendStatus::Pending {
                        None
                    } else if sign_or_broadcast && allow_signing {
                        // A single "Sign" entry point opens the unified
                        // picker, which lists every signer — local (HW,
                        // master, border) and Keychain (contacts' phones) —
                        // as its own row.
                        Some(
                            Row::new().push(
                                primary_action(action_id(action_scope, "overview.sign"), localized_copy("Sign"))
                                    .on_press(Message::Spend(SpendTxMessage::Sign))
                                    .width(Length::Fixed(150.0)),
                            ),
                        )
                    } else if !allow_signing {
                        None
                    } else {
                        // Disabled (never hidden) while a replayable BTCB2
                        // spend awaits its acknowledgement.
                        Some(
                            Row::new().push(
                                primary_action(action_id(action_scope, "overview.broadcast"), localized_copy("Broadcast"))
                                    .on_press_maybe(
                                        broadcast_ready
                                            .then_some(Message::Spend(SpendTxMessage::Broadcast)),
                                    )
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
    signatures_verified: bool,
) -> Element<'a, Message> {
    Column::new()
        .push(
            if let Some(sigs) = tx.path_ready().filter(|_| signatures_verified) {
                Container::new(
                    scrollable(
                        Row::new()
                            .spacing(5)
                            .align_y(Alignment::Center)
                            .spacing(10)
                            .push(p1_bold(localized_copy("Status")))
                            .push(icon::square_check_icon().style(theme::text::success))
                            .push(
                                text(CopyText::Ready.get())
                                    .bold()
                                    .style(theme::text::success),
                            )
                            .push(text(localized_copy("  signed by")))
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
                                .push(text(localized_copy(
                                    "Finalizing this transaction requires:",
                                )))
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
                                    .push(p1_bold(localized_copy("Status")))
                                    .push(icon::clock_icon().style(theme::text::warning))
                                    .push(
                                        text(localized_copy("Partially signed"))
                                            .bold()
                                            .style(theme::text::warning),
                                    )
                                    .push(text(localized_copy("  signed by")))
                                    .push(signed.iter().fold(Row::new().spacing(5), |row, fg| {
                                        row.push(container_from_fg(*fg, keys_aliases))
                                    })),
                            )
                            .direction(
                                scrollable::Direction::Horizontal(
                                    scrollable::Scrollbar::new().width(2).scroller_width(2),
                                ),
                            ),
                        )
                        .padding(15),
                    );
                }
                Container::new(col.push(collapse))
            },
        )
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
                text(localized_copy("Remaining requirements"))
                    .style(theme::text::secondary)
                    .width(Length::Fill),
            )
            .push(collapse_icon)
    } else {
        Row::new()
            .align_y(Alignment::Center)
            .spacing(20)
            .push(p1_bold(localized_copy("Status")))
            .push(
                Row::new()
                    .spacing(5)
                    .align_y(Alignment::Center)
                    .push(icon::square_cross_icon().style(theme::text::error))
                    .push(text(localized_copy("Not ready")).style(theme::text::error))
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
                p1_regular(match missing_signatures {
                    0 => localized_copy("0 more signature").to_owned(),
                    1 => localized_format!("{} more signature from ", "Falta {} firma de ", 1),
                    n => localized_format!("{} more signatures from ", "Faltan {} firmas de ", n),
                })
                .style(theme::text::secondary),
            )
            .push(row_unsigned)
            .push((!sigs.signed_pubkeys.is_empty()).then_some(
                p1_regular(localized_copy(", already signed by ")).style(theme::text::secondary),
            ))
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
    history_inputs_view(coins, tx, labels, labels_editing, bitcoin_unit, false)
}

/// History detail can hide passive coin values without changing the PSBT
/// signing/review presentation, which always shows exact authorization values.
pub fn history_inputs_view<'a>(
    coins: &'a HashMap<OutPoint, Coin>,
    tx: &'a Transaction,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'a, Message> {
    Container::new(
        Column::new()
            .push(
                Container::new(h4_bold(localized_format!(
                    "{} coin{} spent",
                    "Monedas gastadas: {} moneda{}",
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
                        mask_amounts,
                    ))
                },
            )),
    )
    .style(theme::card::simple)
    .into()
}

fn history_amount_redaction(mask_amounts: bool) -> Option<&'static str> {
    mask_amounts.then_some("••••")
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
    history_outputs_view(
        tx,
        network,
        change_indexes,
        labels,
        labels_editing,
        bitcoin_unit,
        is_single_payment,
        is_external,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn history_outputs_view<'a>(
    tx: &'a Transaction,
    network: Network,
    change_indexes: &'a [usize],
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
    is_single_payment: bool,
    is_external: bool,
    mask_amounts: bool,
) -> Element<'a, Message> {
    outputs_view_with_identities_masked(
        tx,
        network,
        change_indexes,
        labels,
        labels_editing,
        bitcoin_unit,
        is_single_payment,
        is_external,
        &[],
        coincube_ui::theme::palette::ThemeMode::default(),
        mask_amounts,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn outputs_view_with_identities<'a>(
    tx: &'a Transaction,
    network: Network,
    change_indexes: &'a [usize],
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
    is_single_payment: bool,
    is_external: bool,
    identities: &'a [(usize, crate::services::branta::LookupResult)],
    theme_mode: coincube_ui::theme::palette::ThemeMode,
) -> Element<'a, Message> {
    outputs_view_with_identities_masked(
        tx,
        network,
        change_indexes,
        labels,
        labels_editing,
        bitcoin_unit,
        is_single_payment,
        is_external,
        identities,
        theme_mode,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn outputs_view_with_identities_masked<'a>(
    tx: &'a Transaction,
    network: Network,
    change_indexes: &'a [usize],
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
    is_single_payment: bool,
    is_external: bool,
    identities: &'a [(usize, crate::services::branta::LookupResult)],
    theme_mode: coincube_ui::theme::palette::ThemeMode,
    mask_amounts: bool,
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
                                        let col = col.spacing(10).push(payment_view(
                                            i,
                                            tx.compute_txid(),
                                            output,
                                            network,
                                            labels,
                                            labels_editing,
                                            bitcoin_unit,
                                            is_single_payment,
                                            is_editable,
                                            mask_amounts,
                                        ));
                                        if let Some((_, crate::services::branta::LookupResult::Identified(matches))) = identities.iter().find(|(vout, _)| *vout == i) {
                                            matches.iter().enumerate().fold(col, |col, (identity_index, identity)| {
                                                col.push(crate::app::view::shared::recipient_identity::card(identity,
                                                    Message::OpenVaultRecipientIdentity(i, identity_index), theme_mode))
                                            })
                                        } else { col }
                                    },
                                ),
                        ),
                )
                .style(theme::card::simple)
            } else {
                Container::new(
                    Column::new()
                        .push(h4_bold(localized_copy("Self-transfer")).style(|t| {
                            theme::text::custom(t.colors.buttons.transparent_border.active.text)
                        }))
                        .push(
                            p2_regular(
                                localized_copy("No external recipients — every output returns to this wallet."),
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
                                .push(h4_bold(localized_copy("Change")).width(Length::Fill))
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
                                .push(h4_bold(localized_copy("Change")).width(Length::Fill))
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
                                        .push(change_view(output, network, bitcoin_unit, mask_amounts))
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
    mask_amounts: bool,
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
                .push_maybe(coin.map(|c| {
                    let value: Element<'a, Message> =
                        if let Some(redaction) = history_amount_redaction(mask_amounts) {
                            text(redaction).into()
                        } else {
                            amount_with_unit(&c.amount, bitcoin_unit).into()
                        };
                    value
                })),
        )
        .push(
            Column::new()
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(5)
                        .push(p1_bold(localized_copy("Outpoint:")).style(theme::text::secondary))
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
                                .push(
                                    p1_bold(localized_copy("Address:"))
                                        .style(theme::text::secondary),
                                )
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
                                    .push(
                                        p1_bold(localized_copy("Address label:"))
                                            .style(theme::text::secondary),
                                    )
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
    mask_amounts: bool,
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
    let amount_value: Element<'a, Message> =
        if let Some(redaction) = history_amount_redaction(mask_amounts) {
            text(redaction).into()
        } else {
            amount_with_unit(&output.value, bitcoin_unit).into()
        };
    Column::new()
        .width(Length::Fill)
        .spacing(5)
        .push(
            Row::new()
                .spacing(5)
                .align_y(Alignment::Center)
                .push(Container::new(label_widget).width(Length::Fill))
                .push(amount_value),
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
                                .push(
                                    p1_bold(localized_copy("Address:"))
                                        .style(theme::text::secondary),
                                )
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
                                .push(
                                    p1_bold(localized_copy("Address label:"))
                                        .style(theme::text::secondary),
                                )
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
    mask_amounts: bool,
) -> Element<Message> {
    let addr = Address::from_script(&output.script_pubkey, network)
        .unwrap()
        .to_string();
    let amount_value: Element<'_, Message> =
        if let Some(redaction) = history_amount_redaction(mask_amounts) {
            text(redaction).into()
        } else {
            amount_with_unit(&output.value, bitcoin_unit).into()
        };
    Column::new()
        .width(Length::Fill)
        .spacing(5)
        .push(
            Row::new()
                .push(Space::new().width(Length::Fill))
                .push(amount_value),
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
                        .push(p1_bold(localized_copy("Address:")).style(theme::text::secondary))
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
    /// A Keychain phone paired over the local network. It signs through the
    /// device list like a USB wallet, but it is the same Keychain key as
    /// [`Self::Keychain`] reached without Connect.
    LanKeychain,
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
    /// Unidentified key while Connect is signed in, but Keychain signing never
    /// came up for this Cube — the session is most likely for a different
    /// account than the one that created it. Carries the explanation and a
    /// "Switch account" affordance that leads to the Connect overview.
    NeedsAccountSwitch(String),
    /// A Keychain phone's last attempt over the local network failed. Offers
    /// Connect instead when it has resolved this key, and another local
    /// attempt unless the phone asked to be paired again.
    LanFailed {
        reason: String,
        /// Device-list index for another local attempt; `None` when retrying
        /// the local network can't succeed.
        retry: Option<usize>,
        /// Whether Connect can deliver the same request instead.
        connect: bool,
    },
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
        SigningKeyKind::Keychain => Some("Mobile"),
        SigningKeyKind::LanKeychain => Some("Mobile · local network"),
        SigningKeyKind::Unknown => None,
    }
}

/// Render one signer row inside a spending-path card. The key icon is colored
/// with the path color, matching the creation "Set keys" look.
fn signing_key_row(
    row: &SigningKeyRow,
    color: iced::Color,
    action_scope: u64,
    sequence: Option<u16>,
) -> Element<'static, Message> {
    let row_id = |action: &str| {
        action_id(
            action_scope,
            &format!(
                "picker.path.{}.key.{}.{}",
                sequence.map_or_else(|| "primary".to_string(), |seq| seq.to_string()),
                row.fingerprint,
                action
            ),
        )
    };
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
        info = info.push(text::caption(localized_copy(caption)));
    }

    let right: Element<Message> = match &row.state {
        SigningKeyState::Signed => Row::new()
            .spacing(5)
            .align_y(Alignment::Center)
            .push(p1_regular(localized_copy("Signed")).color(color::GREEN))
            .push(icon::check_icon().style(theme::text::success))
            .into(),
        SigningKeyState::InProgress(status) => p1_regular(notice(status.clone()))
            .style(theme::text::secondary)
            .into(),
        SigningKeyState::Disabled(reason) => p1_regular(notice(reason.clone()))
            .style(theme::text::secondary)
            .into(),
        // Stack the button *under* the reason rather than beside it: the sibling
        // `info` column is `Fill`, so a side-by-side Row starves this slot and the
        // button's label wraps into a cramped oval. A Column gives it a full line
        // at its natural width.
        SigningKeyState::NeedsSignIn(reason) => Column::new()
            .spacing(6)
            .align_x(Alignment::End)
            .push(p1_regular(notice(reason.clone())).style(theme::text::secondary))
            .push(
                // Bubbles straight to the tab (which opens/focuses the Home
                // tab for sign-in) without routing through the PSBT panel, so
                // the picker stays open behind the new tab.
                secondary_action(row_id("sign-in"), localized_copy("Sign in to Connect"))
                    .on_press(Message::OpenConnectSignIn),
            )
            .into(),
        SigningKeyState::NeedsAccountSwitch(reason) => Column::new()
            .spacing(6)
            .align_x(Alignment::End)
            .push(p1_regular(notice(reason.clone())).style(theme::text::secondary))
            .push(
                secondary_action(row_id("switch-account"), localized_copy("Switch account"))
                    .on_press(Message::OpenConnectSwitchAccount),
            )
            .into(),
        SigningKeyState::Retry(idx) => button::secondary_action(
            row_id("retry"),
            Some(icon::reload_icon()),
            CopyText::Retry.get(),
            Message::FocusAction,
        )
        .on_press(Message::Spend(SpendTxMessage::RetryKeychainSigner(*idx)))
        .into(),
        // Stacked like `NeedsSignIn`, for the same reason: beside the `Fill`
        // info column the buttons' labels would wrap.
        SigningKeyState::LanFailed {
            reason,
            retry,
            connect,
        } => {
            let mut col = Column::new()
                .spacing(6)
                .align_x(Alignment::End)
                .push(p1_regular(notice(reason.clone())).style(theme::text::secondary));
            if *connect {
                col = col.push(
                    primary_action(
                        row_id("send-connect"),
                        localized_copy("Send through Connect"),
                    )
                    .on_press(Message::Spend(
                        SpendTxMessage::SelectKeychainSigner(row.fingerprint),
                    )),
                );
            }
            if retry.is_some() {
                col = col.push(
                    button::secondary_action(
                        row_id("retry-wifi"),
                        Some(icon::reload_icon()),
                        coincube_ui::locale::copy("Try Wi-Fi again"),
                        Message::FocusAction,
                    )
                    .on_press(Message::SelectPsbtHardwareWallet(row.fingerprint)),
                );
            }
            col.into()
        }
        SigningKeyState::Available(action) => {
            let (msg, label) = match action {
                SigningKeyAction::Master => {
                    (Message::Spend(SpendTxMessage::SelectMasterSigner), "Sign")
                }
                SigningKeyAction::Hardware(_) => {
                    (Message::SelectPsbtHardwareWallet(row.fingerprint), "Sign")
                }
                SigningKeyAction::BorderWallet => (
                    Message::Spend(SpendTxMessage::SelectBorderWallet(row.fingerprint)),
                    "Sign",
                ),
                SigningKeyAction::Keychain => (
                    Message::Spend(SpendTxMessage::SelectKeychainSigner(row.fingerprint)),
                    "Request",
                ),
            };
            primary_action(row_id("activate"), localized_copy(label))
                .on_press(msg)
                .into()
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
            .push(text(localized_format!(
                "{} of {} signature{} collected",
                "Firmas recopiladas: {} de {} firma{}",
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
fn signing_path_card(path: SigningPath, action_scope: u64) -> Element<'static, Message> {
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
        localized_copy("Able to move the funds at any time.").to_string()
    } else {
        localized_format!(
            "Available after inactivity of ~{}",
            "Disponible tras ~{} de inactividad",
            sequence.map(humanize_timelock).unwrap_or_default()
        )
    };
    // Built in English by the signing state; shown in the window's language.
    let title = localized_copy(&title).to_string();

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
        coincube_ui::widget::action::Action::new(
            action_id(
                action_scope,
                &format!(
                    "picker.path.{}.toggle",
                    sequence.map_or_else(|| "primary".to_string(), |seq| seq.to_string())
                ),
            ),
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
                    p2_regular(localized_copy(
                        "This spending path isn't available for this transaction.",
                    ))
                    .style(theme::text::secondary),
                ),
            Message::FocusAction,
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
            keys_col = keys_col.push(signing_key_row(key, color, action_scope, sequence));
        }
        col = col
            .push(keys_col)
            .push(signing_threshold_badge(color, collected, threshold, total));
        if can_request_all {
            col = col.push(
                secondary_action(
                    action_id(
                        action_scope,
                        &format!(
                            "picker.path.{}.request-all",
                            sequence.map_or_else(|| "primary".to_string(), |seq| seq.to_string())
                        ),
                    ),
                    localized_copy("Request from everyone"),
                )
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
    chain: crate::chain::ChainId,
    action_scope: u64,
    paths: Vec<SigningPath>,
    keychain_notices: Vec<String>,
    error: Option<String>,
) -> Element<'a, Message> {
    let mut col = Column::new().spacing(15).width(Length::Fill).push(
        Row::new()
            .align_y(Alignment::Center)
            .push(p1_regular(crate::chain::canonical_network_name(chain)).width(Length::Fill))
            .push(
                secondary_action(
                    action_id(action_scope, "picker.close"),
                    CopyText::Close.get(),
                )
                .on_press(Message::Spend(SpendTxMessage::Cancel))
                .width(Length::Fixed(100.0)),
            ),
    );
    if let Some(error) = error {
        col = col.push(card::warning(notice(error)));
    }
    for notice in &keychain_notices {
        col = col.push(
            p1_regular(coincube_ui::locale::notice(notice.clone())).style(theme::text::secondary),
        );
    }
    for path in paths {
        col = col.push(signing_path_card(path, action_scope));
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
            let input = TextInput::new(
                &localized_format!("Word {}", "Palabra {}", i + 1),
                &recon.phrase_words[i].value,
            )
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
        .map(|e| p1_regular(notice(e.clone())).color(color::RED).into());

    let cancel_btn = button::transparent(None, CopyText::Cancel.get()).on_press(Message::Spend(
        SpendTxMessage::BorderWalletRecon(BorderWalletReconMessage::Cancel),
    ));

    let next_btn = if recon.phrase_valid {
        button::primary(None, localized_copy("Next")).on_press(Message::Spend(
            SpendTxMessage::BorderWalletRecon(BorderWalletReconMessage::Next),
        ))
    } else {
        button::primary(None, localized_copy("Next"))
    }
    .width(Length::Fill);

    let prompt = if recon.phrase_prefilled {
        // Not necessarily the Cube's *master* seed: the installer derives a
        // grid phrase from whichever signer that session held, which is the
        // master seed only in developer mode.
        format!(
            "Recovery phrase for key #{}, re-derived from the seed it was created with",
            recon.target_fingerprint
        )
    } else {
        format!(
            "Enter the 12-word recovery phrase for key #{}",
            recon.target_fingerprint
        )
    };

    let mut col = Column::new()
        .spacing(12)
        .push(header)
        .push(p1_regular(prompt))
        .push(p1_regular(localized_copy("Reconstruction needs the recovery phrase and your exact 11-cell pattern in order. The checksum word is recalculated from your pattern."))
            .style(theme::text::secondary))
        .push(word_rows);

    if recon.phrase_prefilled {
        col = col.push(
            p1_regular(localized_copy(
                "Filled in for you. Edit any word if this key was enrolled from a \
                 different phrase.",
            ))
            .style(theme::text::secondary),
        );
    }

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

    let back_btn = button::transparent(Some(icon::previous_icon()), CopyText::Back.get()).on_press(
        Message::Spend(SpendTxMessage::BorderWalletRecon(
            BorderWalletReconMessage::Previous,
        )),
    );

    let status = p1_bold(localized_format!(
        "Selected: {} / {} cells",
        "Seleccionado: {} / células {}",
        recon.pattern.len(),
        PATTERN_LENGTH
    ));

    let undo_btn = if !recon.pattern.is_empty() {
        button::secondary(None, localized_copy("Undo")).on_press(Message::Spend(
            SpendTxMessage::BorderWalletRecon(BorderWalletReconMessage::UndoLastCell),
        ))
    } else {
        button::secondary(None, localized_copy("Undo"))
    };

    let clear_btn = if !recon.pattern.is_empty() {
        button::secondary(None, localized_copy("Clear")).on_press(Message::Spend(
            SpendTxMessage::BorderWalletRecon(BorderWalletReconMessage::ClearPattern),
        ))
    } else {
        button::secondary(None, localized_copy("Clear"))
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
        p1_regular(localized_copy("No grid available")).into()
    };

    let error_text: Option<Element<Message>> = recon
        .error
        .as_ref()
        .map(|e| p1_regular(notice(e.clone())).color(color::RED).into());

    let next_btn = if recon.pattern.is_complete() {
        button::primary(None, localized_copy("Sign")).on_press(Message::Spend(
            SpendTxMessage::BorderWalletRecon(BorderWalletReconMessage::Next),
        ))
    } else {
        button::primary(None, localized_copy("Sign"))
    }
    .width(Length::Fill);

    let mut col = Column::new()
        .spacing(10)
        .push(header)
        .push(p1_regular(
            "Select 11 cells from the grid in order \u{2014} the same pattern you used during enrollment.",
        ))
        .push(p1_regular(localized_copy("The pattern must rebuild the enrolled key before any signature is added."))
            .style(theme::text::secondary))
        .push(
            Row::new()
                .spacing(10)
                .push(status)
                .push(undo_btn)
                .push(clear_btn),
        )
        .push(grid_content);

    if let Some(checksum) = &recon.checksum_word {
        col = col.push(
            p1_bold(localized_format!(
                "Checksum word: \"{}\"",
                "Ver resumen: \"{}\"",
                checksum
            ))
            .color(color::GREEN),
        );
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
                        .push(text(localized_copy("Insert updated PSBT:")).bold())
                        .push(
                            form::Form::new_trimmed("PSBT", updated, move |msg| {
                                Message::ImportSpend(ImportSpendMessage::PsbtEdited(msg))
                            })
                            .warning(localized_copy(
                                "Please enter the correct base64 encoded PSBT",
                            ))
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
                text(localized_copy("Spend transaction is updated")).style(theme::text::secondary),
            ))
            .padding(50),
        )
        .width(Length::Fixed(400.0))
        .align_x(Alignment::Center)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use coincube_core::{
        border_wallet::OrderedPattern,
        descriptors::CoincubeDescriptor,
        miniscript::bitcoin::{absolute, bip32::ChildNumber, transaction, Amount, Sequence, TxIn},
    };
    use coincubed::commands::LCSpendInfo;
    use iced::keyboard::key::Named;
    use std::str::FromStr;

    const DESC: &str = "wsh(or_d(multi(2,[f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<0;1>/*,[2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<0;1>/*),and_v(v:thresh(1,pkh([f714c228/48'/1'/0'/2']tpubDEwJnTwfKoMvu8AXXBPydBVWDpzNP5tatjjZ56q4TQioGL7iL9xzTbMoCCQ3tfGihtff7vtR4xsjcRuhZ7HWARVAkGZ1HZcpBhVdou76k7j/<2;3>/*),a:pkh([2522f23c/48'/1'/0'/2']tpubDEoTU4bDW1EXN1rnLXnRfue1a7DeqjJcs39PkEeLcVXhVKzCnFo9yQX2EeeXJ6kh4hgbz5o9v7YAc1EE97AEJpJbKNmDxE3ZQo4msGPSp2J/<2;3>/*)),older(65535))))#9s8ekrce";
    const MAINNET_ADDR: &str = "bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg";
    const GRID_PHRASE: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn fingerprint(hex: &str) -> Fingerprint {
        Fingerprint::from_str(hex).unwrap()
    }

    fn address() -> Address {
        Address::from_str(MAINNET_ADDR).unwrap().assume_checked()
    }

    fn outpoint(n: u32) -> OutPoint {
        OutPoint::new(Txid::from_str(&format!("{n:064x}")).unwrap(), n)
    }

    fn coin(n: u32, sats: u64, spent: bool) -> Coin {
        Coin {
            amount: Amount::from_sat(sats),
            outpoint: outpoint(n),
            address: address(),
            block_height: Some(840_000),
            derivation_index: ChildNumber::from(n),
            spend_info: spent.then(|| LCSpendInfo {
                txid: Txid::from_str(&format!("{:064x}", n + 100)).unwrap(),
                height: None,
            }),
            is_immature: false,
            is_change: false,
            is_from_self: false,
        }
    }

    fn tx_with_outputs() -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
            input: vec![
                TxIn {
                    previous_output: outpoint(1),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    ..TxIn::default()
                },
                TxIn {
                    previous_output: outpoint(2),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    ..TxIn::default()
                },
            ],
            output: vec![
                TxOut {
                    value: Amount::from_sat(40_000),
                    script_pubkey: address().script_pubkey(),
                },
                TxOut {
                    value: Amount::from_sat(10_000),
                    script_pubkey: address().script_pubkey(),
                },
            ],
        }
    }

    fn form_value(value: &str, valid: bool) -> form::Value<String> {
        form::Value {
            value: value.to_string(),
            valid,
            warning: None,
        }
    }

    fn quote() -> coincube_ui::component::quote_display::Quote {
        coincube_ui::component::quote_display::Quote {
            id: "test".to_string(),
            text: "Steady hands.".to_string(),
            author: "Coincube".to_string(),
            source: "tests".to_string(),
            category: "bitcoin-send".to_string(),
            length: "short".to_string(),
            contexts: vec!["bitcoin-send".to_string()],
        }
    }

    fn image_handle() -> iced::widget::image::Handle {
        iced::widget::image::Handle::from_rgba(1, 1, vec![255, 255, 255, 255])
    }

    fn signing_row(
        fingerprint: Fingerprint,
        label: &str,
        kind: SigningKeyKind,
        state: SigningKeyState,
    ) -> SigningKeyRow {
        SigningKeyRow {
            fingerprint,
            label: label.to_string(),
            kind,
            state,
        }
    }

    #[test]
    fn pending_psbt_review_keeps_authorization_amounts_visible() {
        assert!(!mask_passive_psbt_detail(true, &SpendStatus::Pending));
        for status in [
            SpendStatus::Broadcast,
            SpendStatus::Spent,
            SpendStatus::Deprecated,
        ] {
            assert!(mask_passive_psbt_detail(true, &status));
            assert!(!mask_passive_psbt_detail(false, &status));
        }
    }

    #[test]
    fn signer_picker_mounted_keyboard_actions_and_replacement() {
        let fp = fingerprint("f714c228");
        let make_path = |state| SigningPath {
            title: "Primary spending option:".into(),
            is_primary: true,
            sequence: None,
            active: true,
            threshold: 1,
            total: 1,
            collected: 0,
            keys: vec![signing_row(fp, "This Cube", SigningKeyKind::Master, state)],
            can_request_all: false,
            expanded: true,
        };
        let close = action_id(91, "picker.close");
        let sign = action_id(91, &format!("picker.path.primary.key.{fp}.activate"));
        let mounted = capture::inspect_full_window(
            "psbt-picker-keyboard",
            sign_action(
                crate::chain::ChainId::Bitcoin,
                91,
                vec![make_path(SigningKeyState::Available(
                    SigningKeyAction::Master,
                ))],
                vec![],
                None,
            ),
            (1200, 800),
            "dark",
            theme::Theme::dark(),
        );
        assert!(mounted.has_focus(close.clone()));
        assert!(mounted.has_focus(sign.clone()));
        assert!(
            mounted.focus.iter().position(|(id, _)| id == &close)
                < mounted.focus.iter().position(|(id, _)| id == &sign)
        );
        for key in [Named::Enter, Named::Space] {
            assert!(matches!(
                capture::activate_with_key(
                    sign_action(
                        crate::chain::ChainId::Bitcoin,
                        91,
                        vec![make_path(SigningKeyState::Available(
                            SigningKeyAction::Master
                        ))],
                        vec![],
                        None
                    ),
                    sign.clone(),
                    (1200, 800),
                    false,
                    key
                )
                .as_slice(),
                [Message::Spend(SpendTxMessage::SelectMasterSigner)]
            ));
        }
        let busy = capture::inspect_full_window(
            "psbt-picker-busy",
            sign_action(
                crate::chain::ChainId::Bitcoin,
                91,
                vec![make_path(SigningKeyState::InProgress("Signing…".into()))],
                vec![],
                None,
            ),
            (1024, 768),
            "light",
            theme::Theme::light(),
        );
        assert!(!busy.has_focus(sign));
        assert!(busy.has_focus(close));

        let inactive = || SigningPath {
            title: "Recovery spending option:".into(),
            is_primary: false,
            sequence: Some(42),
            active: false,
            threshold: 1,
            total: 1,
            collected: 0,
            keys: vec![signing_row(
                fp,
                "Unavailable",
                SigningKeyKind::Hardware,
                SigningKeyState::Disabled("This path is unavailable".into()),
            )],
            can_request_all: false,
            expanded: false,
        };
        let toggle = action_id(91, "picker.path.42.toggle");
        let collapsed = capture::inspect_full_window(
            "psbt-picker-inactive-path",
            sign_action(
                crate::chain::ChainId::Bitcoin,
                91,
                vec![inactive()],
                vec![],
                None,
            ),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(collapsed.has_focus(toggle.clone()));
        assert!(!collapsed.has_focus(action_id(91, &format!("picker.path.42.key.{fp}.activate"))));
        for key in [Named::Enter, Named::Space] {
            assert!(matches!(
                capture::activate_with_key(
                    sign_action(
                        crate::chain::ChainId::Bitcoin,
                        91,
                        vec![inactive()],
                        vec![],
                        None
                    ),
                    toggle.clone(),
                    (1024, 768),
                    false,
                    key,
                )
                .as_slice(),
                [Message::Spend(SpendTxMessage::ToggleSpendPath(Some(42)))]
            ));
        }
    }

    #[test]
    fn keychain_unavailable_has_distinct_keyboard_handoff_actions() {
        for (size, appearance, theme) in [
            ((1024, 768), "dark", theme::Theme::dark()),
            ((1440, 900), "light", theme::Theme::light()),
        ] {
            let mounted = capture::inspect_full_window(
                "psbt-keychain-unavailable",
                keychain_unavailable_action(92, false),
                size,
                appearance,
                theme,
            );
            for name in ["cancel", "pair", "connect"] {
                assert!(mounted.has_focus(action_id(92, &format!("keychain-unavailable.{name}"))));
            }
            assert!(matches!(
                capture::activate_with_key(
                    keychain_unavailable_action(92, false),
                    action_id(92, "keychain-unavailable.connect"),
                    size,
                    false,
                    Named::Enter,
                )
                .as_slice(),
                [Message::Spend(SpendTxMessage::KeychainConnectSignIn)]
            ));
            assert!(matches!(
                capture::activate_with_key(
                    keychain_unavailable_action(92, true),
                    action_id(92, "keychain-unavailable.pair"),
                    size,
                    false,
                    Named::Space,
                )
                .as_slice(),
                [Message::Spend(SpendTxMessage::KeychainPairPhone)]
            ));
        }
        assert_ne!(action_id(92, "picker.close"), action_id(93, "picker.close"));
    }

    #[test]
    fn recipient_identity_summary_is_visible_only_for_a_current_ready_proposal() {
        let fixture = crate::app::state::vault::test_support::unified::fixture();
        let mut input_coin = coin(1, 50_000, false);
        input_coin.outpoint = fixture.psbt.unsigned_tx.input[0].previous_output;
        let tx = SpendTx::new(
            None,
            fixture.psbt,
            vec![input_coin],
            &fixture.descriptor,
            &coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::verification_only(),
            Network::Signet,
        );
        let ticket = crate::services::branta::test_ticket(true);
        let identity = crate::services::branta::test_identity(&ticket, "Synthetic Merchant");
        let identities = [(
            0,
            crate::services::branta::LookupResult::Identified(vec![identity]),
        )];

        for (appearance, theme, mode) in [
            (
                "dark",
                theme::Theme::dark(),
                coincube_ui::theme::palette::ThemeMode::Dark,
            ),
            (
                "light",
                theme::Theme::light(),
                coincube_ui::theme::palette::ThemeMode::Light,
            ),
        ] {
            let summary = recipient_identity_summary(
                &tx,
                Network::Signet,
                BitcoinDisplayUnit::Sats,
                true,
                &identities,
                mode,
                false,
            )
            .expect("current identity on a ready single-recipient proposal");
            let mounted = capture::inspect(
                "psbt-detail-recipient-identity",
                summary,
                (1024, 768),
                appearance,
                theme,
            );
            assert!(mounted.contains("Recipient identity"));
            assert!(mounted.contains("Synthetic Merchant"));
            assert!(mounted.contains("View with Branta"));
            let hidden = capture::inspect(
                "psbt-detail-recipient-identity-hidden",
                recipient_identity_summary(
                    &tx,
                    Network::Signet,
                    BitcoinDisplayUnit::Sats,
                    true,
                    &identities,
                    mode,
                    true,
                )
                .expect("masked current identity"),
                (1024, 768),
                appearance,
                theme,
            );
            assert!(hidden.contains("••••"));
            assert!(hidden.contains("Synthetic Merchant"));
        }
        assert!(recipient_identity_summary(
            &tx,
            Network::Signet,
            BitcoinDisplayUnit::Sats,
            false,
            &identities,
            coincube_ui::theme::palette::ThemeMode::Dark,
            false,
        )
        .is_none());
        ticket.set_test_enabled(false);
        assert!(recipient_identity_summary(
            &tx,
            Network::Signet,
            BitcoinDisplayUnit::Sats,
            true,
            &identities,
            coincube_ui::theme::palette::ThemeMode::Dark,
            false,
        )
        .is_none());
    }

    #[test]
    fn overview_preserves_disabled_gates_and_modal_hides_base_from_tab_order() {
        let fixture = crate::app::state::vault::test_support::unified::fixture();
        let policy = fixture.descriptor.policy();
        let mut input_coin = coin(1, 50_000, false);
        input_coin.outpoint = fixture.psbt.unsigned_tx.input[0].previous_output;
        let tx = SpendTx::new(
            None,
            fixture.psbt.clone(),
            vec![input_coin],
            &fixture.descriptor,
            &coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::verification_only(),
            Network::Signet,
        );
        let aliases = HashMap::new();
        let overview = |currently_signing| {
            spend_overview_with_cancel(
                94,
                &tx,
                &policy,
                &aliases,
                currently_signing,
                true,
                None,
                false,
                Some(Message::Spend(SpendTxMessage::Cancel)),
                true,
            )
        };
        let enabled = capture::inspect_full_window(
            "psbt-overview-enabled",
            overview(false),
            (1200, 800),
            "dark",
            theme::Theme::dark(),
        );
        for name in ["export", "import", "copy-txid", "cancel", "sign"] {
            assert!(
                enabled.has_focus(action_id(94, &format!("overview.{name}"))),
                "missing {name}: {:?}",
                enabled.focus
            );
        }
        for key in [Named::Enter, Named::Space] {
            assert!(matches!(
                capture::activate_with_key(
                    overview(false),
                    action_id(94, "overview.sign"),
                    (1200, 800),
                    false,
                    key,
                )
                .as_slice(),
                [Message::Spend(SpendTxMessage::Sign)]
            ));
        }
        let busy = capture::inspect_full_window(
            "psbt-overview-busy",
            overview(true),
            (1024, 768),
            "light",
            theme::Theme::light(),
        );
        for name in ["export", "import"] {
            assert!(!busy.has_focus(action_id(94, &format!("overview.{name}"))));
        }
        // Inspect the actual overlay operation too. The generic content-only
        // capture intentionally probes just the root, not overlay widgets.
        use iced::advanced::{
            layout,
            renderer::Headless,
            widget::{Operation, Tree},
            Layout,
        };
        struct FocusProbe(Vec<Id>);
        impl Operation for FocusProbe {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn focusable(
                &mut self,
                id: Option<&Id>,
                _bounds: iced::Rectangle,
                _: &mut dyn iced::advanced::widget::operation::Focusable,
            ) {
                if let Some(id) = id {
                    self.0.push(id.clone());
                }
            }
        }
        let size = iced::Size::new(1200.0, 800.0);
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut element: Element<'_, Message> = coincube_ui::widget::modal::Modal::new(
            overview(false),
            sign_action(crate::chain::ChainId::Bitcoin, 95, vec![], vec![], None),
        )
        .into();
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, size),
        );
        let mut probe = FocusProbe(Vec::new());
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut probe);
        let viewport = iced::Rectangle::with_size(size);
        let mut overlay = element
            .as_widget_mut()
            .overlay(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &viewport,
                iced::Vector::ZERO,
            )
            .unwrap();
        let overlay_node = overlay.as_overlay_mut().layout(&renderer, size);
        overlay
            .as_overlay_mut()
            .operate(Layout::new(&overlay_node), &renderer, &mut probe);
        assert!(probe.0.contains(&action_id(95, "picker.close")));
        assert!(!probe.0.contains(&action_id(94, "overview.sign")));
    }

    #[test]
    fn worldview_signer_and_broadcast_confirmations_show_the_exact_chain() {
        use crate::chain::ChainId;

        let quote = quote();
        let image = image_handle();
        for (chain, expected, other) in [
            (
                ChainId::Bitcoin,
                "Bitcoin SHA-256 · Mainnet",
                "Bitcoin BLAKE2b · Mainnet",
            ),
            (
                ChainId::BitcoinBlake2b,
                "Bitcoin BLAKE2b · Mainnet",
                "Bitcoin SHA-256 · Mainnet",
            ),
        ] {
            for dialog in [
                sign_action(chain, 1, vec![], vec![], None),
                broadcast_action_with_identity_review(
                    chain,
                    &HashSet::new(),
                    false,
                    false,
                    None,
                    "40,000 sats",
                    &quote,
                    &image,
                    false,
                    None,
                    BroadcastReceipt::default(),
                ),
            ] {
                let mounted = capture::inspect(
                    "psbt-canonical-network",
                    dialog,
                    (1024, 768),
                    "dark",
                    theme::Theme::dark(),
                );
                assert!(mounted.contains(expected), "missing {}", expected);
                for wrong in [other, "Spamcoin", "LukeCoin", "Bitcoin (XBT)"] {
                    assert!(!mounted.contains(wrong), "unexpected {}", wrong);
                }
            }
        }
    }

    #[test]
    fn action_modals_build_for_success_pending_and_error_states() {
        let mut conflicts = HashSet::new();
        conflicts.insert(Txid::from_str(&format!("{:064x}", 1)).unwrap());
        conflicts.insert(Txid::from_str(&format!("{:064x}", 2)).unwrap());
        let quote = quote();
        let image = image_handle();

        let _ = save_action(false);
        let _ = save_action(true);
        let _ = delete_action(false);
        let _ = delete_action(true);
        let _ = keychain_unavailable_action(1, false);
        let _ = keychain_unavailable_action(1, true);
        let _ = broadcast_action(
            &HashSet::new(),
            false,
            false,
            None,
            "40,000 sats",
            &quote,
            &image,
            false,
        );
        let _ = broadcast_action(
            &conflicts,
            false,
            true,
            Some("mempool rejected".to_string()),
            "40,000 sats",
            &quote,
            &image,
            false,
        );
        let _ = broadcast_action(
            &HashSet::new(),
            true,
            false,
            None,
            "40,000 sats",
            &quote,
            &image,
            true,
        );
    }

    #[test]
    fn inputs_and_outputs_views_build_with_labels_missing_coins_change_and_external_rows() {
        let tx = tx_with_outputs();
        let mut coins = HashMap::new();
        coins.insert(outpoint(1), coin(1, 60_000, false));
        let mut labels = HashMap::new();
        labels.insert(outpoint(1).to_string(), "input coin".to_string());
        labels.insert(address().to_string(), "destination".to_string());
        labels.insert(
            OutPoint::new(tx.compute_txid(), 0).to_string(),
            "payment output".to_string(),
        );
        let mut labels_editing = HashMap::new();
        labels_editing.insert(outpoint(1).to_string(), form_value("editing", true));

        let _ = inputs_view(
            &coins,
            &tx,
            &labels,
            &labels_editing,
            BitcoinDisplayUnit::Sats,
        );
        let _ = outputs_view(
            &tx,
            Network::Bitcoin,
            &[1],
            &labels,
            &labels_editing,
            BitcoinDisplayUnit::Sats,
            true,
            false,
        );
        let _ = outputs_view(
            &tx,
            Network::Bitcoin,
            &[0, 1],
            &labels,
            &labels_editing,
            BitcoinDisplayUnit::BTC,
            false,
            false,
        );
        let _ = outputs_view(
            &tx,
            Network::Bitcoin,
            &[1],
            &labels,
            &labels_editing,
            BitcoinDisplayUnit::Sats,
            false,
            true,
        );

        assert_eq!(history_amount_redaction(true), Some("••••"));
        assert_eq!(history_amount_redaction(false), None);
        let _ = history_inputs_view(
            &coins,
            &tx,
            &labels,
            &labels_editing,
            BitcoinDisplayUnit::Sats,
            true,
        );
        let _ = history_outputs_view(
            &tx,
            Network::Bitcoin,
            &[1],
            &labels,
            &labels_editing,
            BitcoinDisplayUnit::Sats,
            true,
            false,
            true,
        );
    }

    #[test]
    fn signing_picker_builds_all_key_states_and_path_shapes() {
        let fg1 = fingerprint("f714c228");
        let fg2 = fingerprint("2522f23c");
        let fg3 = fingerprint("8a64f2a9");

        assert_eq!(humanize_timelock(0), "");
        assert_eq!(humanize_timelock(6), "1h");
        assert_eq!(humanize_timelock(144), "1d");
        assert_eq!(
            signing_key_kind_caption(&SigningKeyKind::Master),
            Some("This computer")
        );
        assert_eq!(signing_key_kind_caption(&SigningKeyKind::Unknown), None);
        assert_eq!(
            signing_key_kind_caption(&SigningKeyKind::LanKeychain),
            Some("Mobile · local network")
        );

        let active = SigningPath {
            title: "Primary path".to_string(),
            is_primary: true,
            sequence: None,
            active: true,
            threshold: 2,
            total: 9,
            collected: 3,
            can_request_all: true,
            expanded: true,
            keys: vec![
                signing_row(
                    fg1,
                    "Already signed",
                    SigningKeyKind::Master,
                    SigningKeyState::Signed,
                ),
                signing_row(
                    fg2,
                    "Hardware signing",
                    SigningKeyKind::Hardware,
                    SigningKeyState::InProgress("Waiting for device".to_string()),
                ),
                signing_row(
                    fg3,
                    "Cannot sign",
                    SigningKeyKind::Unknown,
                    SigningKeyState::Disabled("Unavailable".to_string()),
                ),
                signing_row(
                    fg1,
                    "Connect prompt",
                    SigningKeyKind::Keychain,
                    SigningKeyState::NeedsSignIn("Sign in to resolve".to_string()),
                ),
                signing_row(
                    fg2,
                    "Retry keychain",
                    SigningKeyKind::Keychain,
                    SigningKeyState::Retry(0),
                ),
                signing_row(
                    fg3,
                    "Master signer",
                    SigningKeyKind::Master,
                    SigningKeyState::Available(SigningKeyAction::Master),
                ),
                signing_row(
                    fg1,
                    "Hardware signer",
                    SigningKeyKind::Hardware,
                    SigningKeyState::Available(SigningKeyAction::Hardware(0)),
                ),
                signing_row(
                    fg2,
                    "Border signer",
                    SigningKeyKind::BorderWallet,
                    SigningKeyState::Available(SigningKeyAction::BorderWallet),
                ),
                signing_row(
                    fg3,
                    "Mobile signer",
                    SigningKeyKind::Keychain,
                    SigningKeyState::Available(SigningKeyAction::Keychain),
                ),
                signing_row(
                    fg1,
                    "Mobile over Wi-Fi",
                    SigningKeyKind::LanKeychain,
                    SigningKeyState::Available(SigningKeyAction::Hardware(1)),
                ),
                signing_row(
                    fg2,
                    "Wi-Fi failed, both roads",
                    SigningKeyKind::LanKeychain,
                    SigningKeyState::LanFailed {
                        reason: "Couldn't sign over Wi-Fi".to_string(),
                        retry: Some(1),
                        connect: true,
                    },
                ),
                signing_row(
                    fg3,
                    "Wi-Fi failed, nothing to offer",
                    SigningKeyKind::LanKeychain,
                    SigningKeyState::LanFailed {
                        reason: "Pair this phone again".to_string(),
                        retry: None,
                        connect: false,
                    },
                ),
            ],
        };

        let inactive_collapsed = SigningPath {
            title: "Recovery path".to_string(),
            is_primary: false,
            sequence: Some(144),
            active: false,
            threshold: 1,
            total: 1,
            collected: 0,
            can_request_all: false,
            expanded: false,
            keys: Vec::new(),
        };

        let inactive_expanded = SigningPath {
            title: "Expanded recovery path".to_string(),
            is_primary: false,
            sequence: Some(65535),
            active: false,
            threshold: 1,
            total: 1,
            collected: 0,
            can_request_all: false,
            expanded: true,
            keys: vec![signing_row(
                fg1,
                "Unavailable key",
                SigningKeyKind::Unknown,
                SigningKeyState::Disabled("Inactive path".to_string()),
            )],
        };

        let _ = sign_action(
            crate::chain::ChainId::Bitcoin,
            1,
            vec![active, inactive_collapsed, inactive_expanded],
            vec!["Connect is not ready yet.".to_string()],
            Some("Signing device unavailable.".to_string()),
        );
    }

    #[test]
    fn path_and_border_wallet_reconstruction_views_build_each_step() {
        let policy = CoincubeDescriptor::from_str(DESC).unwrap().policy();
        let mut signed_pubkeys = HashMap::new();
        signed_pubkeys.insert(fingerprint("f714c228"), 1);
        let sigs = PathSpendInfo {
            threshold: 2,
            sigs_count: 1,
            signed_pubkeys,
        };
        let aliases = HashMap::from([(fingerprint("f714c228"), "Laptop key".to_string())]);
        let _ = path_view(policy.primary_path(), &sigs, &aliases);

        let mut phrase_words = vec![form_value("", false); 12];
        phrase_words[0].value = "abandon".to_string();
        let phrase_recon = BorderWalletReconstructionState {
            target_fingerprint: fingerprint("f714c228"),
            network: Network::Bitcoin,
            step: ReconStep::RecoveryPhrase,
            phrase_words,
            phrase_valid: false,
            grid: None,
            pattern: OrderedPattern::new(),
            checksum_word: None,
            phrase_prefilled: false,
            error: Some("Check the phrase".to_string()),
        };
        let _ = border_wallet_recon_view(&phrase_recon);

        // The prefilled variant renders a different prompt and an extra note,
        // so it is a distinct branch of the phrase view.
        let prefilled_phrase_recon = BorderWalletReconstructionState {
            target_fingerprint: fingerprint("f714c228"),
            network: Network::Bitcoin,
            step: ReconStep::RecoveryPhrase,
            phrase_words: vec![form_value("abandon", true); 12],
            phrase_valid: true,
            grid: None,
            pattern: OrderedPattern::new(),
            checksum_word: None,
            phrase_prefilled: true,
            error: None,
        };
        let _ = border_wallet_recon_view(&prefilled_phrase_recon);

        let mut pattern = OrderedPattern::new();
        pattern.add(CellRef::new(0, 0)).unwrap();
        let grid_recon = BorderWalletReconstructionState {
            target_fingerprint: fingerprint("2522f23c"),
            network: Network::Bitcoin,
            step: ReconStep::Grid,
            phrase_words: vec![form_value("abandon", true); 12],
            phrase_valid: true,
            grid: Some(WordGrid::from_recovery_phrase(GRID_PHRASE).unwrap()),
            pattern,
            checksum_word: Some("about".to_string()),
            phrase_prefilled: false,
            error: Some("Pick eleven cells".to_string()),
        };
        let _ = border_wallet_recon_view(&grid_recon);

        let mut complete_pattern = OrderedPattern::new();
        for col in 0..PATTERN_LENGTH as u8 {
            complete_pattern.add(CellRef::new(0, col)).unwrap();
        }
        let complete_grid_recon = BorderWalletReconstructionState {
            target_fingerprint: fingerprint("8a64f2a9"),
            network: Network::Bitcoin,
            step: ReconStep::Grid,
            phrase_words: vec![form_value("abandon", true); 12],
            phrase_valid: true,
            grid: Some(WordGrid::from_recovery_phrase(GRID_PHRASE).unwrap()),
            pattern: complete_pattern,
            checksum_word: None,
            phrase_prefilled: false,
            error: None,
        };
        let _ = border_wallet_recon_view(&complete_grid_recon);
    }

    #[test]
    fn update_spend_views_build_valid_invalid_processing_and_success_states() {
        let valid = form_value("cHNidP8=", true);
        let invalid = form_value("not psbt", false);

        let _ = update_spend_view("original-psbt".to_string(), &valid, false);
        let _ = update_spend_view("original-psbt".to_string(), &valid, true);
        let _ = update_spend_view("original-psbt".to_string(), &invalid, false);
        let _ = update_spend_success_view();
    }

    /// The broadcast receipt leads with status and amount, lists the fee and
    /// the transaction a user may need later, and says it is safe to close.
    #[test]
    fn broadcast_receipt_lists_fee_and_transaction() {
        use crate::app::view::shared::landing::capture;
        use std::str::FromStr;

        let quote = quote();
        let image = image_handle();
        let txid =
            Txid::from_str("4e0ca17b9c2d6f1a8b3e5d7c9f0a1b2c3d4e5f60718293a4b5c6d7e866d20f19")
                .unwrap();
        let mounted = capture::inspect(
            "vault-broadcast-receipt",
            broadcast_action_with_identity_review(
                crate::chain::ChainId::Bitcoin,
                &HashSet::new(),
                true,
                false,
                None,
                "150,000 SATS",
                &quote,
                &image,
                false,
                None,
                BroadcastReceipt {
                    fee: Some("1,240 SATS"),
                    txid: Some(txid),
                },
            ),
            (1440, 900),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        for text in [
            "Sent · waiting for confirmation",
            "150,000",
            "You can close this. Activity updates when it confirms.",
            "Paid from",
            "Network fee",
            "1,240 SATS",
            "Transaction ID",
            "4e0ca17b … 66d20f19",
            "Done",
        ] {
            assert!(mounted.contains(text), "missing {}", text);
        }
    }

    /// A cancel must not read as if the coins left the wallet, and a
    /// self-transfer is not a payment to someone else.
    #[test]
    fn a_cancel_and_a_transfer_get_their_own_status_line() {
        assert_eq!(broadcast_headline(true, true), Some("Cancellation sent"));
        assert_eq!(broadcast_headline(true, false), Some("Transfer sent"));
        assert_eq!(broadcast_headline(false, true), None);
        assert_eq!(broadcast_headline(false, false), None);
    }
}
