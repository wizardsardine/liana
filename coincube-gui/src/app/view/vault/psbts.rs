//! Vault › PSBT proposals: a signing queue.
//!
//! Open proposals (not signed, partially signed, ready, broadcast) sit on
//! top; confirmed and can't-be-spent ones fold into a Closed group. Each row
//! names the payment, shows signature progress as a small meter and says
//! what's next in tinted text.

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use iced::{
    widget::{text::Wrapping, Space},
    Alignment, Background, Border, Length,
};

use coincube_core::{descriptors::PathSpendInfo, miniscript::bitcoin::Address};
use coincube_ui::component::amount::{BitcoinDisplayUnit, DisplayAmount};
use coincube_ui::{
    component::{
        amount::amount_with_size_and_unit,
        tenshu::{disclosure::disclosure, text as tt},
    },
    icon,
    theme::{
        self,
        tenshu::{button::ActionKind, text as tone_text, Tone},
        Theme,
    },
    widget::{Button, Column, Container, Element, Row, TextInput},
};

use crate::{
    app::menu::{Menu, VaultSubMenu},
    daemon::model::{SpendStatus, SpendTx},
};

use super::super::message::*;

/// Base64 PSBTs always start with this (the `psbt\xff` magic).
pub const PSBT_PREFIX: &str = "cHNidP8";

/// Error shown when the pasted text does not parse as a PSBT.
pub const NOT_A_PSBT: &str =
    "This isn't a PSBT. Check that you copied all of it, starting with cHNidP8.";

/// The list page's own state, owned by the PSBTs panel.
#[derive(Debug, Default, Clone, Copy)]
pub struct ListView<'a> {
    pub closed_expanded: bool,
    pub note: Option<ListNote<'a>>,
    pub import: Option<ImportPanel<'a>>,
}

/// The status line under the heading.
#[derive(Debug, Clone, Copy)]
pub enum ListNote<'a> {
    /// A proposal was deleted from its detail page.
    Deleted(&'a str),
    /// A pasted or chosen PSBT was stored.
    Imported,
}

/// The inline import panel.
#[derive(Debug, Clone, Copy)]
pub struct ImportPanel<'a> {
    pub text: &'a str,
    pub error: Option<&'a str>,
    pub busy: bool,
}

/// Which group a proposal belongs to.
pub fn is_open(status: &SpendStatus) -> bool {
    matches!(status, SpendStatus::Pending | SpendStatus::Broadcast)
}

pub fn psbts_view<'a>(
    spend_txs: &'a [SpendTx],
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
    list: ListView<'a>,
) -> Element<'a, Message> {
    let open: Vec<(usize, &SpendTx)> = spend_txs
        .iter()
        .enumerate()
        .filter(|(_, tx)| is_open(&tx.status))
        .collect();
    let closed: Vec<(usize, &SpendTx)> = spend_txs
        .iter()
        .enumerate()
        .filter(|(_, tx)| !is_open(&tx.status))
        .collect();

    let mut page = Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(list_heading());
    if let Some(note) = list.note {
        page = page.push(note_view(note));
    }
    if let Some(import) = list.import {
        page = page.push(import_view(import));
    }
    if spend_txs.is_empty() {
        page = page.push(empty_view());
    }
    if !open.is_empty() {
        page = page.push(
            Column::new()
                .spacing(8)
                .push(group_title(localized_copy("In progress"), open.len()))
                .push(rows(&open, bitcoin_unit, mask_amounts)),
        );
    }
    if !closed.is_empty() {
        page = page.push(
            disclosure(
                "tenshu.psbts.closed",
                localized_copy("Closed"),
                list.closed_expanded,
                Message::FocusAction,
            )
            .caption(closed.len().to_string())
            .on_toggle(|_| Message::Psbts(PsbtsMessage::ToggleClosed))
            .content(rows(&closed, bitcoin_unit, mask_amounts)),
        );
    }
    page.into()
}

fn list_heading<'a>() -> Element<'a, Message> {
    Row::new()
        .spacing(16)
        .align_y(Alignment::Start)
        .push(
            Column::new()
                .spacing(4)
                .width(Length::Fill)
                .push(tt::heading(localized_copy("PSBT proposals")))
                .push(
                    tt::body(localized_copy(
                        "Transactions waiting for signatures from this Vault's keys. Share one with another signer, or bring their signatures back.",
                    ))
                    .style(tone_text::muted),
                ),
        )
        .push(
            Row::new()
                .spacing(8)
                .push(
                    coincube_ui::component::tenshu::action::ActionButton::new(
                        "tenshu.psbts.import",
                        localized_copy("Import PSBT"),
                        ActionKind::Secondary,
                        Message::FocusAction,
                    )
                    .icon(icon::import_icon())
                    .on_press(Message::Psbts(PsbtsMessage::OpenImport)),
                )
                .push(
                    coincube_ui::component::tenshu::action::ActionButton::new(
                        "tenshu.psbts.create",
                        localized_copy("Create PSBT"),
                        ActionKind::Primary,
                        Message::FocusAction,
                    )
                    .icon(icon::plus_icon())
                    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Send))),
                ),
        )
        .into()
}

fn note_view<'a>(note: ListNote<'a>) -> Element<'a, Message> {
    let (title, sub) = match note {
        ListNote::Deleted(name) => (
            localized_format!("Deleted {}", "Se eliminó {}", name),
            localized_copy(
                "Nothing was broadcast. Its coins are free to spend in another transaction.",
            ),
        ),
        ListNote::Imported => (
            localized_copy("PSBT imported").to_string(),
            localized_copy("Its signatures were checked against this Vault's keys."),
        ),
    };
    Container::new(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(
                icon::check_icon()
                    .size(18)
                    .style(|t: &Theme| tone_text::tone(t, Tone::Good)),
            )
            .push(
                Column::new()
                    .spacing(2)
                    .width(Length::Fill)
                    .push(tt::label(title))
                    .push(tt::caption(sub)),
            )
            .push(
                Button::new(icon::cross_icon().size(16).style(tone_text::muted))
                    .padding(6)
                    .style(theme::button::transparent)
                    .on_press(Message::Psbts(PsbtsMessage::DismissNote)),
            ),
    )
    .padding([10, 14])
    .width(Length::Fill)
    .style(theme::tenshu::surface::card)
    .into()
}

fn import_view<'a>(import: ImportPanel<'a>) -> Element<'a, Message> {
    let ready = !import.text.trim().is_empty() && !import.busy;
    let mut panel = Column::new()
        .spacing(12)
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(tt::section(localized_copy("Import a PSBT")).width(Length::Fill))
                .push(
                    Button::new(icon::cross_icon().size(16).style(tone_text::muted))
                        .padding(6)
                        .style(theme::button::transparent)
                        .on_press(Message::Psbts(PsbtsMessage::CloseImport)),
                ),
        )
        .push(
            tt::body(localized_copy(
                "Paste a base64 PSBT from another signer or wallet. If it updates a proposal already here, its new signatures are merged in.",
            ))
            .style(tone_text::muted),
        )
        .push(
            TextInput::new("cHNidP8BAH0CAAAAA…", import.text)
                .id("tenshu.psbts.import.input")
                .on_input(|text| Message::Psbts(PsbtsMessage::ImportEdited(text)))
                .on_submit_maybe(ready.then_some(Message::Psbts(PsbtsMessage::SubmitImport)))
                .padding(12),
        );
    if let Some(error) = import.error {
        panel = panel.push(
            tt::body(error)
                .style(|t: &Theme| tone_text::tone(t, Tone::Bad))
                .wrapping(Wrapping::WordOrGlyph),
        );
    }
    panel = panel.push(
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(
                coincube_ui::component::tenshu::action::ActionButton::new(
                    "tenshu.psbts.import.submit",
                    localized_copy(if import.busy {
                        "Importing…"
                    } else {
                        "Import"
                    }),
                    ActionKind::Primary,
                    Message::FocusAction,
                )
                .on_press_maybe(ready.then_some(Message::Psbts(PsbtsMessage::SubmitImport))),
            )
            .push(
                coincube_ui::component::tenshu::action::ActionButton::new(
                    "tenshu.psbts.import.cancel",
                    coincube_ui::locale::Text::Cancel.get(),
                    ActionKind::Secondary,
                    Message::FocusAction,
                )
                .on_press(Message::Psbts(PsbtsMessage::CloseImport)),
            )
            .push(Space::new().width(Length::Fill))
            .push(
                coincube_ui::component::tenshu::action::ActionButton::new(
                    "tenshu.psbts.import.file",
                    localized_copy("Choose a .psbt file"),
                    ActionKind::Quiet,
                    Message::FocusAction,
                )
                .icon(icon::restore_icon())
                .on_press_maybe(
                    (!import.busy).then_some(Message::Psbts(PsbtsMessage::ImportFromFile)),
                ),
            ),
    );
    Container::new(panel)
        .padding([16, 18])
        .width(Length::Fill)
        .style(theme::tenshu::surface::card)
        .into()
}

fn empty_view<'a>() -> Element<'a, Message> {
    let step = |n: &'static str, title: &'static str, body: &'static str| {
        Row::new()
            .spacing(10)
            .width(Length::Fixed(210.0))
            .push(
                Container::new(tt::caption(n).style(tone_text::ink))
                    .center_x(Length::Fixed(22.0))
                    .center_y(Length::Fixed(22.0))
                    .style(|t: &Theme| iced::widget::container::Style {
                        border: Border {
                            color: t.colors.shell.line,
                            width: 1.0,
                            radius: 11.0.into(),
                        },
                        ..Default::default()
                    }),
            )
            .push(
                Column::new()
                    .push(tt::caption(localized_copy(title)).style(tone_text::ink))
                    .push(tt::caption(localized_copy(body)).wrapping(Wrapping::Word)),
            )
    };
    Container::new(
        Column::new()
            .spacing(16)
            .align_x(Alignment::Center)
            .push(
                icon::receipt_icon()
                    .size(28)
                    .style(|t: &Theme| tone_text::tone(t, Tone::Accent)),
            )
            .push(tt::section(localized_copy("No proposals yet")))
            .push(
                Container::new(
                    tt::body(localized_copy(
                        "A PSBT is a transaction that still needs signatures. Create one here, or import one a co-signer sent you. Signatures are checked before anything can be broadcast.",
                    ))
                    .style(tone_text::muted)
                    .align_x(iced::alignment::Horizontal::Center),
                )
                .max_width(520),
            )
            .push(
                Row::new()
                    .spacing(8)
                    .push(
                        coincube_ui::component::tenshu::action::ActionButton::new(
                            "tenshu.psbts.empty.create",
                            localized_copy("Create PSBT"),
                            ActionKind::Primary,
                            Message::FocusAction,
                        )
                        .icon(icon::plus_icon())
                        .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Send))),
                    )
                    .push(
                        coincube_ui::component::tenshu::action::ActionButton::new(
                            "tenshu.psbts.empty.import",
                            localized_copy("Import PSBT"),
                            ActionKind::Secondary,
                            Message::FocusAction,
                        )
                        .on_press(Message::Psbts(PsbtsMessage::OpenImport)),
                    ),
            )
            // The steps sit apart from the actions above (canvas review).
            .push(Space::new().height(16))
            .push(Container::new(rule()).max_width(690))
            .push(
                Row::new()
                    .spacing(28)
                    .push(step("1", "Create", "Pick coins and recipients."))
                    .push(step(
                        "2",
                        "Collect signatures",
                        "Sign here, on a device, or send it to a co-signer.",
                    ))
                    .push(step("3", "Broadcast", "Once enough keys have signed.")),
            ),
    )
    .padding([48, 32])
    .center_x(Length::Fill)
    .style(theme::tenshu::surface::card)
    .into()
}

fn group_title<'a>(title: &'static str, count: usize) -> Element<'a, Message> {
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(tt::section(title))
        .push(tt::caption(count.to_string()))
        .into()
}

fn rows<'a>(
    txs: &[(usize, &'a SpendTx)],
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'a, Message> {
    txs.iter()
        .enumerate()
        .fold(Column::new(), |col, (n, (i, tx))| {
            let col = if n == 0 { col } else { col.push(rule()) };
            col.push(spend_tx_row(*i, tx, bitcoin_unit, mask_amounts))
        })
        .into()
}

fn rule<'a>() -> Element<'a, Message> {
    Container::new(Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(theme::tenshu::control::divider)
        .into()
}

fn most_signed_path<'a>(
    primary: &'a PathSpendInfo,
    recovery: impl Iterator<Item = &'a PathSpendInfo>,
) -> &'a PathSpendInfo {
    recovery.fold(primary, |best, path| {
        // A recovery path can be further along with the same raw count when
        // its threshold is lower. Keep primary only when progress really ties.
        if path.sigs_count.saturating_mul(best.threshold)
            > best.sigs_count.saturating_mul(path.threshold)
        {
            path
        } else {
            best
        }
    })
}

/// The spending path a row reports: the ready one, else the furthest along.
fn reported_path(tx: &SpendTx) -> (&PathSpendInfo, bool) {
    let path = tx.path_ready().unwrap_or_else(|| {
        most_signed_path(tx.sigs.primary_path(), tx.sigs.recovery_paths().values())
    });
    (path, std::ptr::eq(path, tx.sigs.primary_path()))
}

/// What a row says is next, and its tone.
pub fn row_status(tx: &SpendTx) -> (String, Tone) {
    match tx.status {
        SpendStatus::Pending if tx.path_ready().is_some() => {
            (localized_copy("Ready to broadcast").to_string(), Tone::Good)
        }
        SpendStatus::Pending if !tx.signers().is_empty() => {
            let (path, _) = reported_path(tx);
            let missing = path.threshold.saturating_sub(path.sigs_count).max(1);
            (
                if missing == 1 {
                    localized_copy("Needs 1 more signature").to_string()
                } else {
                    localized_format!("Needs {} more signatures", "Faltan {} firmas", missing)
                },
                Tone::Warn,
            )
        }
        SpendStatus::Pending => (localized_copy("Not signed").to_string(), Tone::Muted),
        SpendStatus::Broadcast => (
            localized_copy("Broadcast · unconfirmed").to_string(),
            Tone::Accent,
        ),
        SpendStatus::Spent => (localized_copy("Confirmed").to_string(), Tone::Muted),
        SpendStatus::Deprecated => (localized_copy("Can't be spent").to_string(), Tone::Muted),
    }
}

fn short_address(address: &str) -> String {
    if address.chars().count() <= 16 {
        address.to_string()
    } else {
        let head: String = address.chars().take(8).collect();
        let tail: String = address
            .chars()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{head}…{tail}")
    }
}

/// The row title: the proposal's label, else who it pays, else its txid.
pub fn proposal_title(tx: &SpendTx) -> String {
    let txid = tx.psbt.unsigned_tx.compute_txid();
    let txid_str = txid.to_string();
    if let Some(label) = tx
        .is_single_payment()
        .and_then(|outpoint| tx.labels.get(&outpoint.to_string()))
        .filter(|label| !label.is_empty())
        .or_else(|| tx.labels.get(&txid_str).filter(|label| !label.is_empty()))
    {
        return label.clone();
    }
    if tx.is_send_to_self() {
        return localized_copy("Self-transfer").to_string();
    }
    let recipients: Vec<String> = tx
        .psbt
        .unsigned_tx
        .output
        .iter()
        .enumerate()
        .filter(|(i, _)| !tx.change_indexes.contains(i))
        .map(|(i, output)| {
            tx.labels
                .get(&format!("{txid}:{i}"))
                .filter(|label| !label.is_empty())
                .cloned()
                .or_else(|| {
                    Address::from_script(&output.script_pubkey, tx.network)
                        .ok()
                        .map(|address| short_address(&address.to_string()))
                })
                .unwrap_or_else(|| short_address(&output.script_pubkey.to_hex_string()))
        })
        .collect();
    match recipients.as_slice() {
        [] => format!("PSBT {}…", &txid_str[..8]),
        [one] => localized_format!("To {}", "Para {}", one),
        [first, rest @ ..] => {
            localized_format!("To {} and {} more", "Para {} y {} más", first, rest.len())
        }
    }
}

fn recipient_count(tx: &SpendTx) -> usize {
    tx.psbt
        .unsigned_tx
        .output
        .len()
        .saturating_sub(tx.change_indexes.len())
}

/// The row's second line: txid prefix, recipient count, fee.
pub fn proposal_subline(tx: &SpendTx, bitcoin_unit: BitcoinDisplayUnit, mask: bool) -> String {
    let txid = tx.psbt.unsigned_tx.compute_txid().to_string();
    let n = recipient_count(tx);
    let mut parts = vec![format!("PSBT {}", &txid[..8])];
    if !tx.is_send_to_self() {
        parts.push(if n == 1 {
            localized_copy("1 recipient").to_string()
        } else {
            localized_format!("{} recipients", "{} destinatarios", n)
        });
    }
    if let Some(fee) = tx.fee_amount {
        parts.push(localized_format!(
            "fee {}",
            "comisión {}",
            if mask {
                "••••".to_string()
            } else {
                fee.to_formatted_string_with_unit(bitcoin_unit)
            }
        ));
    }
    parts.join(" · ")
}

/// `threshold` short bars, `signed` of them filled.
pub fn signature_pips<'a>(signed: usize, threshold: usize) -> Element<'a, Message> {
    sized_signature_pips(signed, threshold, 22.0, 6.0)
}

/// [`signature_pips`] at a given pip size; the detail's meter uses 32×8.
pub fn sized_signature_pips<'a>(
    signed: usize,
    threshold: usize,
    width: f32,
    height: f32,
) -> Element<'a, Message> {
    (0..threshold.max(1))
        .fold(Row::new().spacing(4), |row, i| {
            let on = i < signed;
            row.push(
                Container::new(Space::new())
                    .width(Length::Fixed(width))
                    .height(Length::Fixed(height))
                    .style(move |t: &Theme| iced::widget::container::Style {
                        background: Some(Background::Color(if on {
                            t.colors.shell.good
                        } else {
                            t.colors.shell.hover
                        })),
                        border: Border {
                            color: t.colors.shell.line,
                            width: if on { 0.0 } else { 1.0 },
                            radius: (height / 2.0).into(),
                        },
                        ..Default::default()
                    }),
            )
        })
        .into()
}

fn spend_tx_row(
    i: usize,
    tx: &SpendTx,
    bitcoin_unit: BitcoinDisplayUnit,
    mask_amounts: bool,
) -> Element<'_, Message> {
    let closed = !is_open(&tx.status);
    let (status, tone) = row_status(tx);
    let (path, primary) = reported_path(tx);
    let signed = path.sigs_count.min(path.threshold);
    let amount: Element<'_, Message> = if tx.is_send_to_self() {
        tt::label(localized_copy("Self-transfer")).into()
    } else if mask_amounts {
        tt::label("••••••").into()
    } else {
        amount_with_size_and_unit(&tx.spend_amount, 15, bitcoin_unit).into()
    };
    let title = tt::label(proposal_title(tx)).wrapping(Wrapping::WordOrGlyph);
    let title = if closed {
        title.style(tone_text::muted)
    } else {
        title
    };
    Button::new(
        Row::new()
            .spacing(24)
            .align_y(Alignment::Center)
            .push(
                Column::new()
                    .spacing(2)
                    .width(Length::FillPortion(5))
                    .push(title)
                    .push(
                        tt::caption(proposal_subline(tx, bitcoin_unit, mask_amounts))
                            .wrapping(Wrapping::WordOrGlyph),
                    ),
            )
            .push(
                Column::new()
                    .spacing(5)
                    .width(Length::FillPortion(3))
                    .push(signature_pips(signed, path.threshold))
                    .push(tt::caption(format!(
                        "{} of {} · {}",
                        signed,
                        path.threshold,
                        localized_copy(if primary { "Primary" } else { "Recovery" })
                    ))),
            )
            .push(
                Column::new()
                    .spacing(2)
                    .width(Length::FillPortion(3))
                    .align_x(Alignment::End)
                    .push(amount)
                    .push(
                        tt::caption(status)
                            .style(move |t: &Theme| tone_text::tone(t, tone))
                            .wrapping(Wrapping::WordOrGlyph),
                    ),
            )
            .push(icon::chevron_right().size(16).style(tone_text::muted)),
    )
    .padding([14, 12])
    .width(Length::Fill)
    .on_press(Message::Select(i))
    .style(theme::button::transparent_border)
    .into()
}

#[cfg(test)]
mod tests {
    use super::{most_signed_path, short_address};
    use coincube_core::descriptors::PathSpendInfo;
    use std::collections::HashMap;

    fn path(sigs_count: usize, threshold: usize) -> PathSpendInfo {
        PathSpendInfo {
            threshold,
            sigs_count,
            signed_pubkeys: HashMap::new(),
        }
    }

    #[test]
    fn partial_recovery_signatures_are_visible_when_primary_has_none() {
        let primary = path(0, 2);
        let recovery = path(1, 2);
        let displayed = most_signed_path(&primary, std::iter::once(&recovery));
        assert_eq!((displayed.sigs_count, displayed.threshold), (1, 2));
    }

    #[test]
    fn path_with_more_progress_wins_equal_signature_counts() {
        let primary = path(1, 3);
        let recovery = path(1, 2);
        let displayed = most_signed_path(&primary, std::iter::once(&recovery));
        assert!(std::ptr::eq(displayed, &recovery));
    }

    #[test]
    fn long_addresses_are_shortened_in_the_middle() {
        assert_eq!(short_address("bc1qshort"), "bc1qshort");
        assert_eq!(
            short_address("bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"),
            "bc1qxy2k…hx0wlh"
        );
    }
}
