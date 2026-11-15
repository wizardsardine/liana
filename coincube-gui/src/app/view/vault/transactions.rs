use coincube_ui::locale::copy as localized_copy;
use coincube_ui::locale::Text as CopyText;
use coincube_ui::localized_format;
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;

use chrono::{DateTime, Local, Utc};
use iced::{
    advanced::widget::Id,
    widget::{scrollable, text::Wrapping, tooltip, Space},
    Alignment, Length,
};

use coincube_ui::{
    component::{
        amount::*, button, card, form, pagination::pagination_controls, text::*,
        transaction::TransactionDirection,
    },
    icon::{self, receipt_icon},
    theme,
    widget::{action::Action, Button, Column, Container, Element, Row},
};

use crate::{
    app::{
        cache::Cache,
        menu::{Menu, VaultSubMenu},
        view::{
            dashboard_headed, loading_placeholder,
            message::{CreateRbfMessage, Message},
            placeholder,
            vault::label,
            vault_page_head, FiatAmountConverter,
        },
    },
    daemon::model::{HistoryTransaction, Txid},
    export::ImportExportMessage,
};

pub const RBF_DESCENDANTS_SCROLL_ID: &str = "vault-rbf-descendants";
pub const RBF_BUMP_TRIGGER_ID: &str = "vault-rbf-bump-trigger";
pub const RBF_CANCEL_TRIGGER_ID: &str = "vault-rbf-cancel-trigger";

fn rbf_trigger(is_cancel: bool) -> Action<'static, Message> {
    button::secondary_action(
        Id::new(if is_cancel {
            RBF_CANCEL_TRIGGER_ID
        } else {
            RBF_BUMP_TRIGGER_ID
        }),
        None,
        if is_cancel {
            localized_copy("Cancel transaction")
        } else {
            localized_copy("Bump fee")
        },
        Message::FocusAction,
    )
    .width(Length::Fixed(200.0))
    .on_press(Message::CreateRbf(CreateRbfMessage::New(is_cancel)))
}

pub fn transactions_view<'a>(
    menu: &'a Menu,
    cache: &'a Cache,
    txs: &'a [HistoryTransaction],
    current_page: u32,
    is_last_page: bool,
    processing: bool,
) -> Element<'a, Message> {
    let fiat_converter = cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

    // While the initial page-0 fetch is in flight, `txs` is empty but the
    // wallet may well have history — show a loading state instead of the
    // "No transactions" empty state, which would otherwise flash for a few
    // seconds before the data lands.
    let is_initial_empty = txs.is_empty() && current_page == 0;
    let show_loading = is_initial_empty && processing;
    let show_empty_state = is_initial_empty && !processing;

    let list = txs
        .iter()
        .enumerate()
        .fold(Column::new().spacing(2), |col, (i, tx)| {
            col.push(tx_list_view(
                i,
                tx,
                cache.bitcoin_unit,
                fiat_converter,
                cache.show_direction_badges,
                cache.balance_amounts_masked(),
            ))
        });

    let body: Element<'a, Message> = if show_loading {
        loading_placeholder(receipt_icon().size(80), "Loading transactions")
    } else if show_empty_state {
        placeholder(
            receipt_icon().size(80),
            coincube_ui::locale::copy("No transactions yet"),
            coincube_ui::locale::copy(
                "Your transaction history will appear here once you send or receive coins.",
            ),
        )
    } else {
        // The dashboard already wraps the whole panel in a page-level
        // scrollable, so the list is pushed directly — nesting another
        // scrollable here would make its `Fill` height resolve against an
        // unbounded parent and break layout. Pagination keeps each page
        // bounded (PAGE_SIZE rows), so the page scroll is enough.
        Column::new()
            .spacing(12)
            .push(
                Row::new()
                    .spacing(12)
                    .push(
                        Container::new(text(localized_copy("Time")).size(CAPTION_SIZE))
                            .width(Length::Fixed(104.0)),
                    )
                    .push(
                        Container::new(text(localized_copy("Details")).size(CAPTION_SIZE))
                            .width(Length::Fill),
                    )
                    .push(
                        Container::new(text(localized_copy("Amount")).size(CAPTION_SIZE))
                            .width(Length::Fixed(190.0)),
                    )
                    .push(
                        Container::new(text(localized_copy("Status")).size(CAPTION_SIZE))
                            .width(Length::Fixed(90.0)),
                    ),
            )
            .push(list)
            .push(pagination_controls(
                Message::VaultPrevPage,
                Message::VaultNextPage,
                current_page > 0,
                !is_last_page,
                processing,
                current_page,
            ))
            .push(Space::new().height(Length::Fixed(30.0)))
            .into()
    };

    dashboard_headed(
        menu,
        cache,
        vault_page_head(cache, localized_copy("Activity")),
        Column::new()
            .push(
                Row::new().push(Space::new().width(Length::Fill)).push(
                    button::secondary(Some(icon::backup_icon()), localized_copy("Export"))
                        .on_press(ImportExportMessage::Open.into()),
                ),
            )
            .push(body)
            .align_x(Alignment::Center)
            .spacing(16),
    )
}

fn tx_list_view(
    i: usize,
    tx: &HistoryTransaction,
    bitcoin_unit: BitcoinDisplayUnit,
    fiat_converter: Option<FiatAmountConverter>,
    show_direction_badges: bool,
    mask_amounts: bool,
) -> Element<'_, Message> {
    let direction = if tx.is_external() {
        TransactionDirection::Incoming
    } else if tx.is_send_to_self() {
        TransactionDirection::SelfTransfer
    } else {
        TransactionDirection::Outgoing
    };

    // A self-transfer has no outgoing amount by definition — everything it
    // sends lands back in this wallet — so show what it moved (inputs less
    // the fee) rather than a flat zero.
    let amount = if tx.is_external() || tx.is_send_to_self() {
        &tx.incoming_amount
    } else {
        &tx.outgoing_amount
    };

    let label = if let Some(outpoint) = tx.is_single_payment() {
        tx.labels.get(&outpoint.to_string())
    } else {
        tx.labels.get(&tx.tx.compute_txid().to_string())
    };
    let title = label.filter(|label| !label.is_empty()).map_or_else(
        || match direction {
            TransactionDirection::Incoming => localized_copy("Received Bitcoin"),
            TransactionDirection::Outgoing => localized_copy("Sent Bitcoin"),
            TransactionDirection::SelfTransfer => localized_copy("Self-transfer"),
        },
        String::as_str,
    );
    let date = tx
        .time
        .and_then(|t| DateTime::<Utc>::from_timestamp(t as i64, 0))
        .map(|t| {
            let local = t.with_timezone(&Local);
            local
                .format(&coincube_ui::locale::date_pattern("%b %d, %H:%M", &local))
                .to_string()
        })
        .unwrap_or_else(|| localized_copy("Pending").to_owned());
    let status = if tx.time.is_none() {
        localized_copy("Unconfirmed")
    } else {
        CopyText::Confirmed.get()
    };
    let sign = match direction {
        TransactionDirection::Incoming => "+",
        TransactionDirection::Outgoing => "−",
        TransactionDirection::SelfTransfer => "",
    };
    let details = list_details(tx, title, direction, show_direction_badges);
    let amount_column = list_amount(amount, sign, bitcoin_unit, fiat_converter, mask_amounts);
    Container::new(
        Button::new(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(Container::new(text(date).size(CAPTION_SIZE)).width(Length::Fixed(104.0)))
                .push(details.width(Length::Fill))
                .push(Container::new(amount_column).width(Length::Fixed(190.0)))
                .push(Container::new(text(status).size(CAPTION_SIZE)).width(Length::Fixed(90.0))),
        )
        .padding(10)
        .width(Length::Fill)
        .on_press(Message::Select(i))
        .style(theme::button::transparent_border),
    )
    .style(theme::card::simple)
    .into()
}

/// Return the modal view for a new RBF transaction.
///
/// `descendant_txids` contains the IDs of any transactions from this wallet that are
/// direct descendants of the transaction to be replaced.
pub fn create_rbf_modal<'a>(
    chain: crate::chain::ChainId,
    is_cancel: bool,
    descendant_txids: &HashSet<Txid>,
    feerate: &form::Value<String>,
    replacement_txid: Option<Txid>,
    processing: bool,
) -> Element<'a, Message> {
    let created = replacement_txid.is_some();
    let mut confirm_button = button::primary_action(
        Id::new("vault-rbf-confirm"),
        None,
        localized_copy("Confirm"),
        Message::FocusAction,
    );
    if !created && !processing && (feerate.valid || is_cancel) {
        confirm_button = confirm_button.on_press(Message::CreateRbf(CreateRbfMessage::Confirm));
    }
    let help_text = if is_cancel {
        localized_copy(
            "Replace the transaction with one paying a higher feerate \
        that sends the coins back to your wallet. There is no guarantee \
        the original transaction won't get mined first. New inputs may \
        be used for the replacement transaction.",
        )
    } else {
        localized_copy(
            "Replace the transaction with one paying a higher feerate \
        to incentivize faster confirmation. New inputs may be used \
        for the replacement transaction.",
        )
    };
    let mut body = Column::new()
        .spacing(16)
        .width(Length::Fill)
        .push(h4_bold(CopyText::TransactionReplacement.get()))
        .push(p2_regular(crate::chain::canonical_network_name(chain)));

    if created {
        body = body
            .push(card::simple(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(icon::square_check_icon().style(theme::text::success))
                    .push(
                        text(localized_copy(
                            "Replacement PSBT created successfully and ready to be signed",
                        ))
                        .width(Length::Fill)
                        .wrapping(Wrapping::WordOrGlyph)
                        .style(theme::text::success),
                    ),
            ))
            .push(
                Row::new()
                    .spacing(10)
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::secondary_action(
                            Id::new("vault-rbf-close"),
                            None,
                            CopyText::Close.get(),
                            Message::FocusAction,
                        )
                        .on_press(Message::CreateRbf(CreateRbfMessage::Cancel)),
                    )
                    .push(
                        button::primary_action(
                            Id::new("vault-rbf-go-to-replacement"),
                            None,
                            localized_copy("Go to replacement"),
                            Message::FocusAction,
                        )
                        .on_press(Message::Menu(Menu::Vault(
                            VaultSubMenu::PSBTs(replacement_txid),
                        ))),
                    ),
            );
    } else {
        body = body
            .push(p1_bold(if is_cancel {
                localized_copy("Cancel transaction")
            } else {
                localized_copy("Bump fee")
            }))
            .push(
                text(help_text)
                    .width(Length::Fill)
                    .wrapping(Wrapping::WordOrGlyph),
            );

        body = append_descendants(body, descendant_txids);

        body = body.push(replacement_fee_entry(is_cancel, feerate, processing));
        if processing {
            body = body.push(
                text(localized_copy("Creating replacement PSBT…")).style(theme::text::secondary),
            );
        }
        body = body.push(
            Row::new()
                .spacing(10)
                .push(Space::new().width(Length::Fill))
                .push(
                    button::secondary_action(
                        Id::new("vault-rbf-back"),
                        None,
                        CopyText::Back.get(),
                        Message::FocusAction,
                    )
                    .on_press_maybe(
                        (!processing).then_some(Message::CreateRbf(CreateRbfMessage::Cancel)),
                    ),
                )
                .push(confirm_button),
        );
    }

    card::modal(body).width(Length::Fill).max_width(720).into()
}

pub fn transaction_detail_view<'a>(
    menu: &'a Menu,
    cache: &'a Cache,
    tx: &'a HistoryTransaction,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
    supports_vault_rbf: bool,
) -> Element<'a, Message> {
    let txid = tx.tx.compute_txid().to_string();
    let display_amount = if tx.is_external() || tx.is_send_to_self() {
        &tx.incoming_amount
    } else {
        &tx.outgoing_amount
    };
    let status = if tx.time.is_some() {
        CopyText::Confirmed.get()
    } else {
        localized_copy("Unconfirmed · awaiting a Bitcoin confirmation")
    };
    let back_btn: Element<'_, Message> =
        button::secondary(Some(icon::previous_icon()), CopyText::Back.get())
            .width(Length::Fixed(150.0))
            .on_press(Message::Close)
            .into();

    let title = if tx.is_send_to_self() {
        localized_copy("Self-transfer")
    } else if tx.is_external() {
        localized_copy("Received Bitcoin")
    } else {
        localized_copy("Sent Bitcoin")
    };

    // The title and the Cube line are in the page head; Back stays a body
    // action for now (plan L3 leaves screen actions to the W PRs).
    dashboard_headed(
        menu,
        cache,
        vault_page_head(cache, title),
        Column::new()
            .push(back_btn)
            .push(editable_history_label(tx, labels_editing, &txid))
            .push(
                history_amount_summary(cache, tx, display_amount, status, bitcoin_unit),
            )
            // If unconfirmed, give option to use RBF.
            // Check fee amount is some as otherwise we may be missing coins for this transaction.
            .push(if supports_vault_rbf && tx.time.is_none() && tx.fee_amount.is_some() {
                Some(
                    Row::new()
                        .push(
                            rbf_trigger(false),
                        )
                        .push(
                            tooltip::Tooltip::new(
                                rbf_trigger(true),
                                "Best effort attempt at double spending an unconfirmed outgoing transaction",
                                tooltip::Position::Top,
                            )
                        )
                        .spacing(10),
                )
            } else {
                None
            })
            .push(card::simple(
                Column::new()
                    .push(tx.time.and_then(|t| DateTime::<Utc>::from_timestamp(t as i64, 0)).map(|t| {
                        let local = t.with_timezone(&Local);
                        let date = local
                            .format(&coincube_ui::locale::date_pattern("%b. %d, %Y - %T %Z", &local))
                            .to_string();
                        Row::new()
                            .width(Length::Fill)
                            .push(Container::new(text(localized_copy("Date:")).bold()).width(Length::Fill))
                            .push(Container::new(text(date)).width(Length::Shrink))
                    }))
                    .push(
                        Row::new()
                            .width(Length::Fill)
                            .align_y(Alignment::Center)
                            .push(Container::new(text("Txid:").bold()).width(Length::Fill))
                            .push(
                                Row::new()
                                    .align_y(Alignment::Center)
                                    .push(Container::new(text(txid.clone()).small()))
                                    .push(
                                        Button::new(icon::clipboard_icon())
                                            .on_press(Message::Clipboard(txid.clone()))
                                            .style(theme::button::transparent_border),
                                    )
                                    .width(Length::Shrink),
                            ),
                    )
                    .spacing(5),
            ))
            .push(
                history_io(tx, cache, labels_editing, bitcoin_unit),
            )
            .spacing(20),
    )
}

fn list_details<'a>(
    tx: &HistoryTransaction,
    title: &'a str,
    direction: TransactionDirection,
    show_direction_badges: bool,
) -> Row<'a, Message> {
    let mut details = Row::new().spacing(8).align_y(Alignment::Center).push(
        coincube_ui::image::asset_network_logo("btc", "bitcoin", 24.0),
    );
    if show_direction_badges {
        details = details.push(text(match direction {
            TransactionDirection::Incoming => "↓",
            TransactionDirection::Outgoing => "↑",
            TransactionDirection::SelfTransfer => "↺",
        }));
    }
    details = details.push(
        Column::new().spacing(2).push(text(title).bold()).push(
            text(if tx.is_batch() {
                localized_copy("Batch transaction")
            } else {
                localized_copy("Bitcoin on-chain")
            })
            .size(CAPTION_SIZE)
            .style(theme::text::secondary),
        ),
    );

    details
}

fn list_amount<'a>(
    amount: &coincube_core::miniscript::bitcoin::Amount,
    sign: &'static str,
    bitcoin_unit: BitcoinDisplayUnit,
    fiat_converter: Option<FiatAmountConverter>,
    mask_amounts: bool,
) -> Column<'a, Message> {
    let mut amount_column = Column::new().align_x(Alignment::End).spacing(2).push(
        Row::new()
            .spacing(3)
            .align_y(Alignment::Center)
            .push(text(if mask_amounts { "" } else { sign }))
            .push(if mask_amounts {
                Row::new().push(text("••••••").size(P2_SIZE))
            } else {
                amount_with_size_and_unit(amount, P2_SIZE, bitcoin_unit)
            }),
    );
    if let Some(converter) = fiat_converter.filter(|_| !mask_amounts) {
        let fiat = converter.convert(*amount);
        amount_column = amount_column.push(
            text(localized_format!(
                "≈ {} {} · current rate",
                "♥ {} {} · tasa actual",
                fiat.to_formatted_string(),
                fiat.currency()
            ))
            .size(CAPTION_SIZE)
            .style(theme::text::secondary),
        );
    }

    amount_column
}

fn append_descendants<'a>(
    mut body: Column<'a, Message>,
    descendant_txids: &HashSet<Txid>,
) -> Column<'a, Message> {
    if !descendant_txids.is_empty() {
        let mut descendants: Vec<_> = descendant_txids.iter().collect();
        descendants.sort();
        let visible_rows = descendant_txids.len().min(3);
        let list =
            descendants
                .into_iter()
                .fold(Column::new().spacing(6).padding([4, 4]), |col, txid| {
                    let id = txid.to_string();
                    col.push(
                        Row::new()
                            .spacing(8)
                            .align_y(Alignment::Center)
                            .push(
                                text(id.clone())
                                    .size(CAPTION_SIZE)
                                    .width(Length::Fill)
                                    .wrapping(Wrapping::Glyph),
                            )
                            .push(
                                Action::new(
                                    Id::from(format!("vault-rbf-copy-{id}")),
                                    icon::clipboard_icon().style(theme::text::secondary),
                                    Message::FocusAction,
                                )
                                .on_press(Message::Clipboard(id))
                                .style(theme::button::transparent_border),
                            ),
                    )
                });
        body = body.push(Container::new(Row::new()
                .spacing(10)
                .push(icon::warning_icon())
                .push(text(localized_format!(
                    "Replacing this transaction will invalidate {} later {} when the replacement is broadcast. Transactions depending on them may also be dropped.",
                    "Al publicar el reemplazo, se invalidarán {} {} posteriores. Las transacciones que dependan de esos pagos también podrían quedar descartadas.",
                    descendant_txids.len(),
                    localized_copy(if descendant_txids.len() == 1 { "payment" } else { "payments" }),
                )).width(Length::Fill).wrapping(Wrapping::WordOrGlyph).style(theme::text::warning)))
                .padding(12)
                .width(Length::Fill)
                .style(theme::card::warning))
                .push(scrollable(list)
                    .id(Id::new(RBF_DESCENDANTS_SCROLL_ID))
                    .height(Length::Fixed((visible_rows * 31 + (visible_rows - 1) * 6 + 8) as f32)));
    }

    body
}

fn replacement_fee_entry<'a>(
    is_cancel: bool,
    feerate: &'a form::Value<String>,
    processing: bool,
) -> Column<'a, Message> {
    if is_cancel {
        Column::new()
            .spacing(5)
            .push(p1_bold(localized_copy("Feerate")))
            .push(
                text(localized_copy(
                    "Set automatically when the cancellation PSBT is created.",
                ))
                .style(theme::text::secondary),
            )
    } else {
        Column::new()
            .spacing(5)
            .push(p1_bold(localized_copy("Feerate (sats/vbyte)")))
            .push(
                if processing {
                    form::Form::new_disabled("", feerate)
                } else {
                    form::Form::new_trimmed("", feerate, |msg| {
                        Message::CreateRbf(CreateRbfMessage::FeerateEdited(msg))
                    })
                }
                .warning(localized_copy(
                    "Feerate must be greater than the previous value and at most 1000 sats/vbyte",
                ))
                .size(P1_SIZE)
                .padding(10),
            )
    }
}

fn editable_history_label<'a>(
    tx: &'a HistoryTransaction,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    txid: &str,
) -> Element<'a, Message> {
    if let Some(outpoint) = tx.is_single_payment() {
        // if the payment is a payment of a single payment transaction then
        // the label of the transaction is attached to the label of the payment outpoint
        let outpoint = outpoint.to_string();
        if let Some(label) = labels_editing.get(&outpoint) {
            label::label_editing(vec![outpoint.clone(), txid.to_owned()], label, H3_SIZE)
        } else {
            label::label_editable(
                vec![outpoint.clone(), txid.to_owned()],
                tx.labels.get(&outpoint),
                H3_SIZE,
            )
        }
    } else if let Some(label) = labels_editing.get(txid) {
        label::label_editing(vec![txid.to_owned()], label, H3_SIZE)
    } else {
        label::label_editable(vec![txid.to_owned()], tx.labels.get(txid), H1_SIZE)
    }
}

fn history_amount_summary<'a>(
    cache: &Cache,
    tx: &HistoryTransaction,
    display_amount: &coincube_core::miniscript::bitcoin::Amount,
    status: &'static str,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Column<'a, Message> {
    Column::new().spacing(20).push(
        Column::new()
            .push(Container::new(if cache.balance_amounts_masked() {
                Row::new().push(text("••••••").size(H1_SIZE))
            } else {
                amount_with_size_and_unit(display_amount, H1_SIZE, bitcoin_unit)
            }))
            .push(
                cache
                    .fiat_price
                    .as_ref()
                    .and_then(|price| price.try_into().ok())
                    .filter(|_| !cache.balance_amounts_masked())
                    .map(|converter: FiatAmountConverter| {
                        let fiat = converter.convert(*display_amount);
                        text(localized_format!(
                            "≈ {} {} · current rate",
                            "♥ {} {} · tasa actual",
                            fiat.to_formatted_string(),
                            fiat.currency()
                        ))
                        .size(P2_SIZE)
                        .style(theme::text::secondary)
                    }),
            )
            .push(text(status).size(P2_SIZE))
            .push(tx.fee_amount.map(|fee_amount| {
                Row::new()
                    .align_y(Alignment::Center)
                    .push(h3(localized_copy("Miner fee: ")).style(theme::text::secondary))
                    .push(if cache.balance_amounts_masked() {
                        Row::new().push(text("••••••").size(H3_SIZE))
                    } else {
                        amount_with_size_and_unit(&fee_amount, H3_SIZE, bitcoin_unit)
                    })
                    .push(text(" ").size(H3_SIZE))
                    .push(
                        text(if cache.balance_amounts_masked() {
                            "••••••".to_string()
                        } else {
                            format!(
                                "({} sats/vbyte)",
                                fee_amount.to_sat() / tx.tx.vsize() as u64
                            )
                        })
                        .size(H4_SIZE)
                        .style(theme::text::secondary),
                    )
            })),
    )
}

fn history_io<'a>(
    tx: &'a HistoryTransaction,
    cache: &Cache,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Column<'a, Message> {
    Column::new()
        .spacing(20)
        // We do not need to display inputs for external incoming transactions
        .push(if tx.is_external() {
            None
        } else {
            Some(super::psbt::history_inputs_view(
                &tx.coins,
                &tx.tx,
                &tx.labels,
                labels_editing,
                bitcoin_unit,
                cache.balance_amounts_masked(),
            ))
        })
        .push(super::psbt::history_outputs_view(
            &tx.tx,
            cache.network,
            &tx.change_indexes,
            &tx.labels,
            labels_editing,
            bitcoin_unit,
            tx.is_single_payment().is_some(),
            tx.is_external(),
            cache.balance_amounts_masked(),
        ))
}

#[cfg(test)]
mod rbf_view_tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use std::str::FromStr;

    fn txid(hex: char) -> Txid {
        Txid::from_str(&hex.to_string().repeat(64)).unwrap()
    }

    fn rate(value: &str, valid: bool) -> form::Value<String> {
        form::Value {
            value: value.into(),
            valid,
            warning: None,
        }
    }

    #[test]
    fn rbf_warning_text_meets_light_theme_body_contrast() {
        let light = theme::Theme::light();
        let card = theme::card::warning(&light);
        let text = theme::text::warning(&light)
            .color
            .expect("warning text color");
        let iced::Background::Color(background) = card.background.expect("warning background")
        else {
            panic!("warning background must be a solid color");
        };
        assert!(theme::tenshu::contrast(text, background) >= 4.5);
        assert!(card.border.width > 0.0, "warning border remains visible");
    }

    #[test]
    fn rbf_detail_triggers_activate_with_keyboard() {
        for (is_cancel, trigger) in [(false, RBF_BUMP_TRIGGER_ID), (true, RBF_CANCEL_TRIGGER_ID)] {
            let messages = capture::activate(
                rbf_trigger(is_cancel).into(),
                Id::new(trigger),
                (1024, 768),
                false,
            );
            assert!(messages.iter().any(|message| matches!(
                message,
                Message::CreateRbf(CreateRbfMessage::New(value)) if *value == is_cancel
            )));
        }
    }

    #[test]
    fn rbf_descendant_list_tracks_row_height_and_caps_visible_rows() {
        let inspect = |count: usize| {
            let descendants: HashSet<_> = "abcdef".chars().take(count).map(txid).collect();
            capture::inspect_full_window(
                "vault-rbf-descendant-height",
                create_rbf_modal(
                    crate::chain::ChainId::Bitcoin,
                    false,
                    &descendants,
                    &rate("12", true),
                    None,
                    false,
                ),
                (1024, 768),
                "light",
                theme::Theme::light(),
            )
        };
        let feerate_top = |count| {
            inspect(count)
                .text
                .iter()
                .find_map(|(value, bounds)| (value == "Feerate (sats/vbyte)").then_some(bounds.y))
                .expect("feerate label")
        };
        let row_delta = feerate_top(2) - feerate_top(1);
        assert!(
            (32.0..=48.0).contains(&row_delta),
            "one extra row: {row_delta}",
            row_delta = row_delta
        );
        assert!((feerate_top(5) - feerate_top(3)).abs() <= 1.0);
        let five: HashSet<_> = "abcde".chars().map(txid).collect();
        let last = txid('e');
        assert!(inspect(5).has_focus(Id::from(format!("vault-rbf-copy-{last}"))));
        assert!(capture::activate(
            create_rbf_modal(
                crate::chain::ChainId::Bitcoin,
                false,
                &five,
                &rate("12", true),
                None,
                false
            ),
            Id::from(format!("vault-rbf-copy-{last}")),
            (1024, 768),
            false,
        )
        .iter()
        .any(|message| matches!(message, Message::Clipboard(id) if id == &last.to_string())));
    }

    #[test]
    fn rbf_review_states_fit_compact_and_wide_in_both_themes() {
        let descendants = HashSet::from([txid('a'), txid('b')]);
        let none = HashSet::new();
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                let inspect = |scenario,
                               cancel,
                               ids: &HashSet<Txid>,
                               rate: &form::Value<String>,
                               created,
                               processing| {
                    capture::inspect_full_window(
                        scenario,
                        create_rbf_modal(
                            crate::chain::ChainId::Bitcoin,
                            cancel,
                            ids,
                            rate,
                            created,
                            processing,
                        ),
                        outer,
                        appearance,
                        theme,
                    )
                };
                let bump = inspect(
                    "vault-rbf-bump",
                    false,
                    &none,
                    &rate("12", true),
                    None,
                    false,
                );
                assert!(bump.contains("Bump fee"));
                assert!(bump.contains("Feerate (sats/vbyte)"));
                assert!(bump.has_focus(Id::new("vault-rbf-confirm")));

                let invalid = inspect(
                    "vault-rbf-invalid",
                    false,
                    &none,
                    &rate("3", false),
                    None,
                    false,
                );
                assert!(invalid.contains("Feerate must be greater"));
                assert!(!invalid.has_focus(Id::new("vault-rbf-confirm")));
                assert!(invalid.has_focus(Id::new("vault-rbf-back")));

                let cancel = inspect(
                    "vault-rbf-cancel",
                    true,
                    &none,
                    &rate("5", true),
                    None,
                    false,
                );
                assert!(cancel.contains("Cancel transaction"));
                assert!(cancel.contains("Set automatically"));
                assert!(!cancel.contains("Feerate (sats/vbyte)"));
                assert!(cancel.has_focus(Id::new("vault-rbf-confirm")));

                let warning = inspect(
                    "vault-rbf-descendants",
                    false,
                    &descendants,
                    &rate("12", true),
                    None,
                    false,
                );
                assert!(warning.contains("invalidate 2 later payments"));
                assert!(warning.contains(&txid('a').to_string()));
                assert!(warning.contains(&txid('b').to_string()));
                assert!(warning.has_focus(Id::from(format!("vault-rbf-copy-{}", txid('a')))));

                let created = inspect(
                    "vault-rbf-created",
                    false,
                    &none,
                    &rate("12", true),
                    Some(txid('c')),
                    false,
                );
                assert!(created.contains("Replacement PSBT created successfully"));
                assert!(created.has_focus(Id::new("vault-rbf-go-to-replacement")));
                assert!(!created.has_focus(Id::new("vault-rbf-confirm")));

                for is_cancel in [false, true] {
                    let pending = inspect(
                        "vault-rbf-pending",
                        is_cancel,
                        &none,
                        &rate("12", true),
                        None,
                        true,
                    );
                    assert!(pending.contains("Creating replacement PSBT"));
                    assert!(!pending.has_focus(Id::new("vault-rbf-back")));
                    assert!(!pending.has_focus(Id::new("vault-rbf-confirm")));
                }
            }
        }
    }

    #[test]
    fn rbf_review_keyboard_actions_keep_existing_routes() {
        let none = HashSet::new();
        let valid = rate("12", true);
        let activate = |cancel, rate: &form::Value<String>, created, target: &str| {
            capture::activate(
                create_rbf_modal(
                    crate::chain::ChainId::Bitcoin,
                    cancel,
                    &none,
                    rate,
                    created,
                    false,
                ),
                Id::from(target.to_owned()),
                (1440, 900),
                false,
            )
        };
        assert!(activate(false, &valid, None, "vault-rbf-confirm")
            .iter()
            .any(|m| matches!(m, Message::CreateRbf(CreateRbfMessage::Confirm))));
        assert!(activate(true, &valid, None, "vault-rbf-confirm")
            .iter()
            .any(|m| matches!(m, Message::CreateRbf(CreateRbfMessage::Confirm))));
        assert!(activate(false, &valid, None, "vault-rbf-back")
            .iter()
            .any(|m| matches!(m, Message::CreateRbf(CreateRbfMessage::Cancel))));
        let descendants = HashSet::from([txid('a')]);
        assert!(capture::activate(
            create_rbf_modal(
                crate::chain::ChainId::Bitcoin,
                false,
                &descendants,
                &valid,
                None,
                false
            ),
            Id::from(format!("vault-rbf-copy-{}", txid('a'))),
            (1440, 900),
            false,
        )
        .iter()
        .any(|m| matches!(m, Message::Clipboard(id) if id == &txid('a').to_string())));
        assert!(activate(false, &valid, Some(txid('c')), "vault-rbf-go-to-replacement")
            .iter().any(|m| matches!(m, Message::Menu(Menu::Vault(VaultSubMenu::PSBTs(Some(id)))) if *id == txid('c'))));
    }
    fn synthetic_history(
        incoming: bool,
        self_transfer: bool,
        confirmed: bool,
    ) -> HistoryTransaction {
        use coincube_core::miniscript::bitcoin::{
            absolute, bip32::ChildNumber, hashes::Hash, transaction, Address, Amount, Network,
            OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, WPubkeyHash,
        };
        let script = ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([1; 20]));
        let previous_output = OutPoint::new(Txid::from_byte_array([2; 32]), 0);
        let transaction = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output,
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                ..TxIn::default()
            }],
            output: vec![TxOut {
                value: Amount::from_sat(42_000),
                script_pubkey: script.clone(),
            }],
        };
        let coins = if incoming {
            Vec::new()
        } else {
            vec![crate::daemon::model::Coin {
                amount: Amount::from_sat(50_000),
                outpoint: previous_output,
                address: Address::from_script(&script, Network::Bitcoin).unwrap(),
                block_height: Some(840_000),
                derivation_index: ChildNumber::from(0),
                spend_info: None,
                is_immature: false,
                is_change: false,
                is_from_self: false,
            }]
        };
        HistoryTransaction::new(
            transaction,
            confirmed.then_some(840_001),
            confirmed.then_some(1_728_000_000),
            coins,
            if incoming || self_transfer {
                vec![0]
            } else {
                vec![]
            },
            Network::Bitcoin,
        )
    }

    fn assert_history_presenter_case(
        tx: &HistoryTransaction,
        privacy: crate::app::state::preferences::BalancePrivacy,
        title: &str,
        status: &str,
        expect_rbf: bool,
        has_fee: bool,
    ) {
        use crate::app::cache::{FiatPrice, FiatPriceRequest};
        use crate::app::state::preferences::{BalancePrivacy, BalancePrivacyChrome};
        use crate::services::fiat::{api::GetPriceResult, Currency, PriceSource};
        let cache = Cache {
            has_vault: true,
            cube_name: "Synthetic Cube".into(),
            balance_privacy: Some(BalancePrivacyChrome {
                privacy,
                busy: false,
                interactive: false,
            }),
            fiat_price: Some(FiatPrice {
                res: Ok(GetPriceResult {
                    value: 100_000.0,
                    updated_at: None,
                }),
                request: FiatPriceRequest::new(PriceSource::Coincube, Currency::USD),
                btcb2_valid_until: None,
            }),
            ..Cache::default()
        };
        let converter = cache
            .fiat_price
            .as_ref()
            .and_then(|price| price.try_into().ok());
        let row = capture::inspect_full_window(
            "vault-ordinary-row",
            tx_list_view(
                7,
                tx,
                BitcoinDisplayUnit::Sats,
                converter,
                true,
                cache.balance_amounts_masked(),
            ),
            (1440, 1000),
            "dark",
            theme::Theme::dark(),
        );
        let menu = Menu::Vault(VaultSubMenu::Transactions(None));
        let editing = HashMap::new();
        let detail = capture::inspect_full_window(
            "vault-ordinary-detail",
            transaction_detail_view(&menu, &cache, tx, &editing, BitcoinDisplayUnit::Sats, true),
            (1440, 1000),
            "dark",
            theme::Theme::dark(),
        );
        assert!(row.contains(title));
        assert!(row.contains(status));
        assert!(detail.contains(status));
        assert_eq!(detail.contains("Miner fee:"), has_fee);
        assert_eq!(detail.has_focus(Id::new(RBF_BUMP_TRIGGER_ID)), expect_rbf);
        assert_eq!(detail.has_focus(Id::new(RBF_CANCEL_TRIGGER_ID)), expect_rbf);
        if privacy == BalancePrivacy::Shown {
            assert!(row.contains("42,000"));
            assert!(detail.contains("42,000"));
            assert!(row.contains("current rate"));
            assert!(detail.contains("current rate"));
            assert_eq!(detail.contains("8,000"), has_fee);
        } else {
            assert!(row.contains("••••••"));
            assert!(detail.contains("••••••"));
            for hidden in ["42,000", "8,000", "current rate"] {
                assert!(!row.contains(hidden));
                assert!(!detail.contains(hidden));
            }
        }
        let selection = capture::click_text(
            tx_list_view(
                7,
                tx,
                BitcoinDisplayUnit::Sats,
                converter,
                true,
                cache.balance_amounts_masked(),
            ),
            title,
            0,
            (1440, 1000),
        );
        assert!(selection
            .iter()
            .any(|message| matches!(message, Message::Select(7))));
        let back = capture::click_text(
            transaction_detail_view(&menu, &cache, tx, &editing, BitcoinDisplayUnit::Sats, true),
            "Back",
            0,
            (1440, 1000),
        );
        assert!(back.iter().any(|message| matches!(message, Message::Close)));
    }

    #[test]
    fn ordinary_history_presenters_preserve_amounts_privacy_and_rbf_admission() {
        use crate::app::state::preferences::BalancePrivacy;
        for (incoming, self_transfer, confirmed, title, status, rbf, fee) in [
            (
                true,
                false,
                true,
                "Received Bitcoin",
                "Confirmed",
                false,
                false,
            ),
            (
                false,
                false,
                false,
                "Sent Bitcoin",
                "Unconfirmed",
                true,
                true,
            ),
            (false, true, true, "Self-transfer", "Confirmed", false, true),
        ] {
            let tx = synthetic_history(incoming, self_transfer, confirmed);
            for privacy in [
                BalancePrivacy::Shown,
                BalancePrivacy::Hidden,
                BalancePrivacy::Unresolved,
            ] {
                assert_history_presenter_case(&tx, privacy, title, status, rbf, fee);
            }
        }
    }
}
