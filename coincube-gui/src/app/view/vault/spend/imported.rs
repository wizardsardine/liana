//! Review and signing for an imported wallet's PSBT, laid out like the
//! Vault's Send review. The wallet's own signers sign it: a connected
//! hardware wallet, or another signer through a PSBT file or base64.

use std::collections::HashMap;

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use iced::{
    widget::{rule, text_input, Column, Container, Row, Space},
    Alignment, Length,
};

use coincube_core::miniscript::bitcoin::{psbt::Psbt, Amount, Network, OutPoint, Txid};
use coincube_ui::{
    component::{amount::*, button, form, text::*},
    icon, theme,
    widget::Element,
};

use super::send_head;
use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        state::imported_bitcoin::ImportMessage,
        view::{dashboard_headed, message::Message, vault::psbt},
    },
    daemon::model::Coin,
};

/// A hardware wallet shown on the review. `id` is set when the device can
/// sign.
pub struct ImportedSigner {
    pub label: String,
    pub id: Option<String>,
}

/// Everything the imported review shows. The state checks the signatures;
/// this only renders them.
pub struct ImportedReview<'a> {
    pub psbt: &'a Psbt,
    pub network: Network,
    pub change_indexes: &'a [usize],
    pub coins: &'a HashMap<OutPoint, Coin>,
    pub labels: &'a HashMap<String, String>,
    pub labels_editing: &'a HashMap<String, form::Value<String>>,
    /// Shown messages: construction warnings, then file or broadcast notices.
    pub warnings: Vec<String>,
    pub notices: Vec<String>,
    pub error: Option<String>,
    pub signatures_per_input: &'a [usize],
    pub required_signatures: usize,
    /// The verified final transaction, once every input is satisfied.
    pub final_txid: Option<Txid>,
    pub broadcast_txid: Option<Txid>,
    pub signers: Vec<ImportedSigner>,
    /// `None` until the hardware wallet list loads.
    pub signers_loaded: bool,
    pub busy: bool,
    pub can_broadcast: bool,
    pub signed_psbt_text: &'a str,
}

fn imported(message: ImportMessage) -> Message {
    Message::ImportedBitcoin(message)
}

/// The Send head's step for this PSBT: it is saved when it opens, so Sign
/// until it is broadcast.
fn step(review: &ImportedReview) -> usize {
    if review.broadcast_txid.is_some() {
        3
    } else {
        2
    }
}

fn header<'a>(review: &ImportedReview<'a>, cache: &Cache) -> Element<'a, Message> {
    let tx = &review.psbt.unsigned_tx;
    let payments: Vec<_> = tx
        .output
        .iter()
        .enumerate()
        .filter(|(index, _)| !review.change_indexes.contains(index))
        .collect();
    let amount: Element<'a, Message> = if payments.is_empty() {
        h1(localized_copy("Self-transfer")).into()
    } else {
        let total = payments
            .iter()
            .fold(Amount::ZERO, |sum, (_, output)| sum + output.value);
        amount_with_size_and_unit(&total, H1_SIZE, cache.bitcoin_unit).into()
    };
    let fee: Element<'a, Message> = match review.psbt.fee() {
        Ok(fee) => amount_with_size_and_unit(&fee, H3_SIZE, cache.bitcoin_unit).into(),
        Err(_) => text(localized_copy(
            "Missing information about transaction inputs",
        ))
        .into(),
    };
    Column::new()
        .push(amount)
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(h3(localized_copy("Miner fee: ")).style(theme::text::secondary))
                .push(fee),
        )
        .into()
}

fn status_row<'a>(review: &ImportedReview<'a>) -> Element<'a, Message> {
    let fewest = review
        .signatures_per_input
        .iter()
        .copied()
        .min()
        .unwrap_or(0);
    let required = review.required_signatures.max(1);
    let (status_icon, status, detail): (_, &'static str, String) =
        if review.broadcast_txid.is_some() {
            (
                icon::square_check_icon().style(theme::text::success),
                localized_copy("Broadcast"),
                String::new(),
            )
        } else if review.final_txid.is_some() {
            (
                icon::square_check_icon().style(theme::text::success),
                localized_copy("Ready"),
                localized_copy("  every input is signed").to_string(),
            )
        } else if review.signatures_per_input.iter().any(|count| *count > 0) {
            (
                icon::clock_icon().style(theme::text::warning),
                localized_copy("Partially signed"),
                localized_format!(
                    "  {} of {} signatures on each input",
                    "  {} de {} firmas en cada entrada",
                    fewest,
                    required
                ),
            )
        } else {
            (
                icon::clock_icon().style(theme::text::warning),
                localized_copy("Not signed yet"),
                if required == 1 {
                    localized_copy("  needs 1 signature").to_string()
                } else {
                    localized_format!("  needs {} signatures", "  necesita {} firmas", required)
                },
            )
        };
    let ready = review.final_txid.is_some() || review.broadcast_txid.is_some();
    Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(p1_bold(localized_copy("Status")))
        .push(status_icon)
        .push(text(status).bold().style(if ready {
            theme::text::success
        } else {
            theme::text::warning
        }))
        .push(text(detail))
        .into()
}

fn overview_card<'a>(review: &ImportedReview<'a>) -> Element<'a, Message> {
    let txid = review.psbt.unsigned_tx.compute_txid().to_string();
    Container::new(
        Column::new()
            .padding(15)
            .spacing(15)
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(p1_bold("Tx ID").width(Length::Fill))
                    .push(p2_regular(txid.clone()).style(theme::text::secondary))
                    .push(
                        iced::widget::button(icon::clipboard_icon().style(theme::text::secondary))
                            .on_press(Message::CopyWithToast(
                                txid,
                                "Copied transaction ID to clipboard".to_string(),
                            ))
                            .style(theme::button::transparent_border),
                    ),
            )
            .push(status_row(review)),
    )
    .width(Length::Fill)
    .style(theme::card::simple)
    .into()
}

fn sign_card<'a>(review: &ImportedReview<'a>) -> Element<'a, Message> {
    let idle = !review.busy;
    let mut devices = Column::new().spacing(10);
    if review.signers_loaded && review.signers.is_empty() {
        devices = devices.push(
            p2_regular(localized_copy(
                "Connect and unlock a hardware wallet to sign here.",
            ))
            .style(theme::text::secondary),
        );
    }
    for signer in &review.signers {
        devices = devices.push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(p1_regular(signer.label.clone()).width(Length::Fill))
                .push(signer.id.clone().map(|id| {
                    button::secondary(None, localized_copy("Sign"))
                        .width(Length::Fixed(150.0))
                        .on_press_maybe(idle.then(|| imported(ImportMessage::SignWithDevice(id))))
                })),
        );
    }
    let can_import = idle && !review.signed_psbt_text.trim().is_empty();
    Container::new(
        Column::new()
            .padding(20)
            .spacing(15)
            .push(h4_bold(localized_copy("Sign")))
            .push(
                p2_regular(localized_copy(
                    "Sign with this wallet's own signers. Your Cube's keys can't sign for an imported wallet.",
                ))
                .style(theme::text::secondary),
            )
            .push(p1_bold(localized_copy("Hardware wallet")))
            .push(devices)
            .push(rule::horizontal(1))
            .push(p1_bold(localized_copy("Another signer")))
            .push(
                p2_regular(localized_copy(
                    "Save or copy the PSBT for the signer, then load or paste the signed PSBT. For multisig, repeat with each signer.",
                ))
                .style(theme::text::secondary),
            )
            .push(
                Row::new()
                    .spacing(10)
                    .push(
                        button::secondary(
                            Some(icon::backup_icon()),
                            localized_copy("Save PSBT file"),
                        )
                        .on_press_maybe(idle.then(|| {
                            imported(ImportMessage::SavePsbt(coincube_ui::locale::language()))
                        })),
                    )
                    .push(
                        button::secondary(Some(icon::clipboard_icon()), localized_copy("Copy PSBT"))
                            .on_press(imported(ImportMessage::CopyPsbt)),
                    )
                    .push(
                        button::secondary(
                            Some(icon::restore_icon()),
                            localized_copy("Load signed PSBT"),
                        )
                        .on_press_maybe(idle.then(|| {
                            imported(ImportMessage::LoadSignedPsbt(
                                coincube_ui::locale::language(),
                            ))
                        })),
                    ),
            )
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(
                        text_input(
                            localized_copy("Paste signed PSBT (base64)"),
                            review.signed_psbt_text,
                        )
                        .on_input(|value| imported(ImportMessage::SignedPsbtEdited(value)))
                        .padding(10)
                        .width(Length::Fill),
                    )
                    .push(
                        button::secondary(None, localized_copy("Import"))
                            .width(Length::Fixed(150.0))
                            .on_press_maybe(
                                can_import.then(|| imported(ImportMessage::ImportSignedPsbt)),
                            ),
                    ),
            ),
    )
    .width(Length::Fill)
    .style(theme::card::simple)
    .into()
}

pub fn imported_review_view<'a>(
    menu: &'a Menu,
    cache: &'a Cache,
    review: ImportedReview<'a>,
) -> Element<'a, Message> {
    let tx = &review.psbt.unsigned_tx;
    let payments = tx.output.len() - review.change_indexes.len().min(tx.output.len());
    let signing = review.final_txid.is_none() && review.broadcast_txid.is_none();
    let broadcast = (review.final_txid.is_some() && review.broadcast_txid.is_none()).then(|| {
        Row::new().push(Space::new().width(Length::Fill)).push(
            button::primary(
                None,
                if review.busy {
                    localized_copy("Broadcasting…")
                } else {
                    localized_copy("Broadcast")
                },
            )
            .width(Length::Fixed(150.0))
            .on_press_maybe(
                (review.can_broadcast && !review.busy).then(|| imported(ImportMessage::Broadcast)),
            ),
        )
    });
    let content = Column::new()
        .spacing(20)
        .push(p1_regular(crate::chain::canonical_network_name(
            cache.chain(),
        )))
        .push(header(&review, cache))
        .push((!review.warnings.is_empty()).then(|| {
            review
                .warnings
                .iter()
                .fold(Column::new().spacing(5), |col, warning| {
                    col.push(
                        Row::new()
                            .spacing(5)
                            .push(icon::warning_icon().style(theme::text::warning))
                            .push(text(warning.clone()).style(theme::text::warning)),
                    )
                })
        }))
        .push(
            review
                .notices
                .iter()
                .fold(Column::new().spacing(5), |col, notice| {
                    col.push(p2_regular(notice.clone()).style(theme::text::secondary))
                }),
        )
        .push(
            review
                .error
                .clone()
                .map(|error| p1_regular(error).style(theme::text::error)),
        )
        .push(overview_card(&review))
        .push(signing.then(|| sign_card(&review)))
        .push(broadcast)
        .push(psbt::outputs_view(
            tx,
            review.network,
            review.change_indexes,
            review.labels,
            review.labels_editing,
            cache.bitcoin_unit,
            payments == 1,
            false,
        ))
        .push(psbt::inputs_view(
            review.coins,
            tx,
            review.labels,
            review.labels_editing,
            cache.bitcoin_unit,
        ));
    dashboard_headed(
        menu,
        cache,
        send_head(cache, "Send", step(&review)),
        content,
    )
}
