//! Coin control's split surfaces: the banner counting coins still on both
//! chains, the selection bar, and the Split coins overlay (Tenshu canvas
//! v157, `P-Advanced`).

use coincube_core::miniscript::bitcoin::{Amount, OutPoint};
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::locale::Text as CopyText;
use coincube_ui::localized_format;
use coincube_ui::{
    component::{amount::*, button, text::*},
    theme,
    widget::{Column, Container, Element, Row},
};
use iced::{
    widget::{checkbox, Space},
    Alignment, Length,
};

use crate::{
    app::view::message::{CoinControlMessage, Message},
    chain::ChainId,
    services::{chain_status::short_name, split_coins::Method},
};

/// The smallest split key worth sending; anything smaller is mostly fee.
pub const MIN_KEY_SATS: u64 = 10_000;

/// One row of the overlay's input table.
pub struct InputRow {
    pub outpoint: OutPoint,
    pub amount: Amount,
    pub label: String,
    pub included: bool,
    pub key: bool,
}

/// Everything the overlay shows. Built by the Coin control panel.
pub struct SplitOverlay {
    pub here: ChainId,
    pub method: Method,
    pub signature_available: bool,
    pub inputs: Vec<InputRow>,
    pub key_needed: bool,
    pub address: Option<String>,
    pub feerate: Option<u64>,
    pub fee_failed: bool,
    /// Estimated virtual size of the split, for the fee line.
    pub vbytes: Option<u64>,
    pub working: bool,
    pub error: Option<String>,
}

fn chain_name(chain: ChainId) -> &'static str {
    if chain.is_blake2b() {
        "Bitcoin Blake2b"
    } else {
        "Bitcoin"
    }
}

fn twin(chain: ChainId) -> ChainId {
    crate::services::entangled::twin_chain(chain).unwrap_or(chain)
}

fn sats(amount: Amount) -> String {
    format!("{} sats", format_u64_as_string(amount.to_sat(), ","))
}

fn coins_count(n: usize) -> String {
    if n == 1 {
        localized_copy("1 coin").to_string()
    } else {
        localized_format!("{} coins", "{} monedas", n)
    }
}

/// The banner above the coin table while coins are still on both chains.
pub fn both_chains_banner<'a>(
    here: ChainId,
    count: usize,
    total: Amount,
    can_split: bool,
    mask_amounts: bool,
) -> Element<'a, Message> {
    let amount = if mask_amounts {
        "•••••• sats".to_string()
    } else {
        sats(total)
    };
    let text = if count == 1 {
        localized_format!(
            "{} · {} also exists on {}. Until split, a transaction on one chain can be copied to the other.",
            "{} · {} también existe en {}. Hasta separarla, una transacción en una cadena se puede copiar a la otra.",
            coins_count(count),
            amount,
            chain_name(twin(here)),
        )
    } else {
        localized_format!(
            "{} · {} also exist on {}. Until split, a transaction on one chain can be copied to the other.",
            "{} · {} también existen en {}. Hasta separarlas, una transacción en una cadena se puede copiar a la otra.",
            coins_count(count),
            amount,
            chain_name(twin(here)),
        )
    };
    Container::new(
        Row::new()
            .spacing(16)
            .align_y(Alignment::Center)
            .push(
                Container::new(p2_bold(localized_copy("Both chains")))
                    .padding([4, 10])
                    .style(theme::pill::warning),
            )
            .push(Container::new(p2_regular(text)).width(Length::Fill))
            .push(if can_split {
                button::primary_compact(None, coincube_ui::locale::copy("Split coins"))
                    .on_press(Message::CoinControl(CoinControlMessage::OpenSplit))
            } else {
                button::primary_compact(None, coincube_ui::locale::copy("Split coins"))
            }),
    )
    .padding(16)
    .style(theme::card::simple)
    .width(Length::Fill)
    .into()
}

/// The bar under the coin table while coins are chosen.
pub fn selection_bar<'a>(count: usize, total: Amount, mask_amounts: bool) -> Element<'a, Message> {
    let summary = if count == 0 {
        "No coins selected".to_string()
    } else {
        format!(
            "{} selected · {}",
            count,
            if mask_amounts {
                "•••••• sats".to_string()
            } else {
                sats(total)
            }
        )
    };
    Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .push(Container::new(p1_regular(summary)).width(Length::Fill))
        .push(if count > 0 {
            button::secondary_compact(None, "Clear")
                .on_press(Message::CoinControl(CoinControlMessage::ClearChosen))
        } else {
            button::secondary_compact(None, "Clear")
        })
        .push(if count > 0 {
            button::primary_compact(None, coincube_ui::locale::copy("Send selected"))
                .on_press(Message::CoinControl(CoinControlMessage::SendSelected))
        } else {
            button::primary_compact(None, coincube_ui::locale::copy("Send selected"))
        })
        .into()
}

/// The warning before Send selected spends coins that are on both chains.
/// On Bitcoin it can be acknowledged; on Bitcoin Blake2b (a Vault that can't
/// make the replay-protected signature) the only way on is to split first.
pub fn send_warning<'a>(here: ChainId, count: usize, ack: bool) -> Element<'a, Message> {
    let other = chain_name(twin(here));
    let question = if count == 1 {
        format!("1 selected coin also exists on {}", other)
    } else {
        format!("{} selected coins also exist on {}", count, other)
    };
    let body = if here.is_blake2b() {
        localized_copy("Your hardware signer can't make a replay-protected signature, so this transaction could be copied to Bitcoin and move your bitcoin too. Split these coins first.")
    } else {
        localized_copy("A Bitcoin transaction that spends them can be copied to Bitcoin Blake2b and move those coins too, to the same recipient. Split them first, or continue if that doesn't matter to you.")
    };
    let mut column = Column::new()
        .spacing(16)
        .push(h4_bold(question))
        .push(p1_regular(crate::chain::canonical_network_name(here)))
        .push(p1_regular(CopyText::ReplaySiblingNetwork.get().replacen(
            "{network}",
            crate::chain::canonical_network_name(twin(here)),
            1,
        )))
        .push(p1_regular(body).style(theme::text::secondary));
    if !here.is_blake2b() {
        column = column.push(
            checkbox(ack)
                .label(localized_copy(
                    "I understand the Bitcoin Blake2b copies may move too",
                ))
                .on_toggle(|ack| {
                    Message::CoinControl(CoinControlMessage::AcknowledgeBothChains(ack))
                }),
        );
    }
    let mut footer = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(Space::new().width(Length::Fill))
        .push(
            button::secondary(None, CopyText::Cancel.get())
                .on_press(Message::CoinControl(CoinControlMessage::CloseWarning)),
        );
    if !here.is_blake2b() {
        footer = footer.push(if ack {
            button::secondary(None, localized_copy("Send anyway"))
                .on_press(Message::CoinControl(CoinControlMessage::SendAnyway))
        } else {
            button::secondary(None, localized_copy("Send anyway"))
        });
    }
    footer = footer.push(
        button::primary(None, localized_copy("Split first"))
            .on_press(Message::CoinControl(CoinControlMessage::OpenSplit)),
    );
    Container::new(column.push(footer))
        .padding(24)
        .max_width(560)
        .style(theme::card::modal)
        .into()
}

impl SplitOverlay {
    pub fn spent(&self) -> (usize, Amount) {
        let included: Vec<_> = self.inputs.iter().filter(|row| row.included).collect();
        (
            included.iter().filter(|row| !row.key).count(),
            included.iter().map(|row| row.amount).sum(),
        )
    }

    fn has_key(&self) -> bool {
        self.inputs.iter().any(|row| row.key)
    }

    pub fn fee(&self) -> Option<Amount> {
        Some(Amount::from_sat(self.feerate?.checked_mul(self.vbytes?)?))
    }

    /// Why **Sign and send** is unavailable, if it is.
    pub fn blocked(&self) -> Option<&'static str> {
        let (coins, _) = self.spent();
        if self.working {
            Some("Building the transaction…")
        } else if self.inputs.iter().all(|row| row.key) {
            // Opened only to get a split key: nothing is on both chains yet.
            Some("No coins to split yet")
        } else if coins == 0 {
            Some("Select at least one coin")
        } else if self.method == Method::SplitKey && !self.has_key() {
            Some("Waiting for a split key")
        } else if self.method == Method::Signature && !self.signature_available {
            Some("This Vault can't make a replay-protected signature")
        } else if self.feerate.is_none() {
            Some(if self.fee_failed {
                "Fee rate unavailable. Try again later."
            } else {
                "Getting the fee rate…"
            })
        } else {
            None
        }
    }
}

fn input_row<'a>(row: &InputRow, here: ChainId) -> Element<'a, Message> {
    let outpoint = row.outpoint;
    let check: Element<'a, Message> = if row.key {
        checkbox(true).into()
    } else {
        checkbox(row.included)
            .on_toggle(move |_| Message::CoinControl(CoinControlMessage::ToggleSplitCoin(outpoint)))
            .into()
    };
    Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(check)
        .push(Container::new(amount(&row.amount)).width(Length::Fixed(150.0)))
        .push(
            Container::new(p2_regular(row.label.clone()).style(theme::text::secondary))
                .width(Length::Fill),
        )
        .push(
            Container::new(p2_regular(if row.key {
                format!("{} only", short_name(here))
            } else {
                "Both chains".to_string()
            }))
            .width(Length::Fixed(130.0)),
        )
        .push(
            Container::new(
                Container::new(p2_bold(if row.key {
                    localized_copy("Split key")
                } else {
                    localized_copy("Splitting")
                }))
                .padding([2, 8])
                .style(if row.key {
                    theme::pill::success
                } else {
                    theme::pill::warning
                }),
            )
            .width(Length::Fixed(100.0)),
        )
        .into()
}

fn method_choice<'a>(
    selected: bool,
    enabled: bool,
    title: &'static str,
    detail: String,
    method: Method,
) -> Element<'a, Message> {
    let body = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(p1_bold(if selected { "◉" } else { "○" }))
        .push(
            Column::new()
                .spacing(2)
                .push(p1_bold(title))
                .push(p2_regular(detail).style(theme::text::secondary)),
        );
    let choice = iced::widget::button(body)
        .padding(12)
        .width(Length::Fill)
        .style(theme::button::transparent_border);
    if enabled {
        choice
            .on_press(Message::CoinControl(CoinControlMessage::SplitMethod(
                method,
            )))
            .into()
    } else {
        choice.into()
    }
}

/// The Split coins dialog.
pub fn split_overlay<'a>(overlay: &SplitOverlay) -> Element<'a, Message> {
    let here = overlay.here;
    let other = chain_name(twin(here));
    let (coins, spent) = overlay.spent();
    let intro = if here.is_blake2b() {
        format!(
            "Sends these coins back to this Vault in one {} transaction that {} rejects. Nothing moves on {}.",
            chain_name(here),
            other,
            other
        )
    } else {
        format!(
            "Sends these coins back to this Vault in one {} transaction that also spends a coin found only on {}. {} rejects it. Nothing moves on {}.",
            chain_name(here),
            chain_name(here),
            other,
            other
        )
    };
    let mut body = Column::new()
        .spacing(20)
        .push(h4_bold(localized_format!(
            "Split coins on {}",
            "Monedas divididas en {}",
            chain_name(here)
        )))
        .push(p1_regular(crate::chain::canonical_network_name(here)))
        .push(p1_regular(intro).style(theme::text::secondary));

    if here.is_blake2b() {
        body = body.push(
            Column::new()
                .spacing(8)
                .push(p1_bold(localized_copy("How to split")))
                .push(method_choice(
                    overlay.method == Method::Signature,
                    overlay.signature_available,
                    "Replay-protected signature",
                    if overlay.signature_available {
                        "Signed so that Bitcoin rejects it. No extra coin needed.".to_string()
                    } else {
                        "Not available: this Vault has a hardware signer that can't make this signature."
                            .to_string()
                    },
                    Method::Signature,
                ))
                .push(method_choice(
                    overlay.method == Method::SplitKey,
                    true,
                    "Split key",
                    format!("Spends a coin found only on {} with these coins.", chain_name(here)),
                    Method::SplitKey,
                )),
        );
    }

    let table = overlay.inputs.iter().fold(
        Column::new()
            .spacing(8)
            .push(p1_bold(localized_copy("Coins in this transaction"))),
        |table, row| table.push(input_row(row, here)),
    );
    body = body.push(table);

    if overlay.method == Method::SplitKey {
        body = body.push(if overlay.key_needed {
            key_needed_box(overlay)
        } else {
            p2_regular(localized_format!(
                "The split key is a coin found only on {}. It comes back with the split, so you can use it again.", "La llave dividida es una moneda que se encuentra sólo en {}. Regresa con la división, por lo que se puede utilizar de nuevo.",
                chain_name(here)
            ))
            .style(theme::text::secondary)
            .into()
        });
    }

    body = body.push(split_result(overlay, coins, spent));
    body = body
        .push(
            p2_regular(localized_format!(
                "After 6 confirmations these coins exist on each chain separately. Their {} copies stay where they are and can be spent on their own.", "Después de 6 confirmaciones estas monedas existen en cada cadena por separado. Sus copias {} se quedan donde están y pueden ser gastadas por su cuenta.",
                other
            ))
            .style(theme::text::secondary),
        )
        .push(
            p2_regular(localized_copy("Spending these coins together links them to each other on the blockchain."))
                .style(theme::text::secondary),
        );
    if let Some(error) = &overlay.error {
        body = body
            .push(p2_regular(coincube_ui::locale::notice(error.clone())).style(theme::text::error));
    }

    let blocked = overlay.blocked();
    let footer = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(
            Container::new(p2_regular(blocked.unwrap_or("")).style(theme::text::secondary))
                .width(Length::Fill),
        )
        .push(if overlay.working {
            button::secondary(None, CopyText::Cancel.get())
        } else {
            button::secondary(None, CopyText::Cancel.get())
                .on_press(Message::CoinControl(CoinControlMessage::CancelSplit))
        })
        .push(if blocked.is_none() {
            button::primary(None, localized_copy("Sign and send"))
                .on_press(Message::CoinControl(CoinControlMessage::SignSplit))
        } else {
            button::primary(None, localized_copy("Sign and send"))
        });

    Container::new(
        Column::new()
            .spacing(20)
            .push(iced::widget::scrollable(body).height(Length::Shrink))
            .push(iced::widget::rule::horizontal(1))
            .push(footer),
    )
    .padding(24)
    .max_width(760)
    .max_height(720)
    .style(theme::card::modal)
    .into()
}

fn split_result<'a>(overlay: &SplitOverlay, coins: usize, spent: Amount) -> Column<'a, Message> {
    let fee = match (overlay.fee(), overlay.feerate) {
        (Some(fee), Some(rate)) => {
            localized_format!(
                "Fee ≈ {} at {} sat/vB",
                "Comisión ≈ {} a {} sat/vB",
                sats(fee),
                rate
            )
        }
        _ if overlay.fee_failed => localized_copy("Fee rate unavailable").to_string(),
        _ => localized_copy("Getting the fee rate…").to_string(),
    };
    let back = overlay
        .fee()
        .map(|fee| spent.checked_sub(fee).unwrap_or(Amount::ZERO));
    Column::new()
        .spacing(6)
        .push(p1_bold(localized_copy("Result")))
        .push(p2_regular(localized_format!(
            "Spends {} · {}{}",
            "Gasta {} · {}{}",
            coins_count(coins),
            sats(spent),
            if overlay.method == Method::SplitKey && overlay.has_key() {
                localized_copy(" including the split key")
            } else {
                ""
            }
        )))
        .push(p2_regular(match back {
            Some(back) => localized_format!(
                "Back to this Vault · fresh address · ≈ {}",
                "Volver a este Vault · dirección nueva · ≈ {}",
                sats(back)
            ),
            None => localized_copy("Back to this Vault · fresh address").to_string(),
        }))
        .push(p2_regular(fee).style(theme::text::secondary))
}

fn key_needed_box<'a>(overlay: &SplitOverlay) -> Element<'a, Message> {
    let here = overlay.here;
    let from = if here.is_blake2b() {
        "A Bitcoin Blake2b exchange, a mining pool payout, or another Bitcoin Blake2b wallet"
    } else {
        "An exchange withdrawal, or another wallet you own"
    };
    Container::new(
        Column::new()
            .spacing(8)
            .push(p1_bold(localized_copy("This needs a split key")))
            .push(p2_regular(localized_format!(
                "Send one small coin that exists only on {} to this Vault from somewhere else. At least {}.", "Envíe una pequeña moneda que sólo existe en {} a este Vault de algún otro lugar. Al menos {}.",
                chain_name(here),
                sats(Amount::from_sat(MIN_KEY_SATS))
            )))
            .push(match &overlay.address {
                Some(address) => p1_regular(address.clone()),
                None => p2_regular(localized_copy("Getting an address…")).style(theme::text::secondary),
            })
            .push(p2_regular(localized_format!("Where from: {}", "Dónde de: {}", from)).style(theme::text::secondary))
            .push(
                p2_regular(localized_format!(
                    "A coin counts only once its history proves it exists only on {}, for example coins mined after the fork. If it can't be proven, it shows Couldn't verify and you can try another source.", "Una moneda cuenta sólo una vez que su historia demuestra que existe sólo en {}, por ejemplo monedas extraídas después del tenedor. Si no se puede probar, muestra No se pudo verificar y se puede probar otra fuente.",
                    chain_name(here)
                ))
                .style(theme::text::secondary),
            )
            .push(Space::new().height(Length::Fixed(4.0))),
    )
    .padding(16)
    .style(theme::card::simple)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn row(n: u8, sats: u64, key: bool) -> InputRow {
        InputRow {
            outpoint: OutPoint::from_str(&format!("{}:0", format!("{:02x}", n).repeat(32)))
                .unwrap(),
            amount: Amount::from_sat(sats),
            label: "No label".into(),
            included: true,
            key,
        }
    }

    fn overlay() -> SplitOverlay {
        SplitOverlay {
            here: ChainId::Bitcoin,
            method: Method::SplitKey,
            signature_available: false,
            inputs: vec![
                row(1, 100_000, false),
                row(2, 100_000, false),
                row(3, 30_000, true),
            ],
            key_needed: false,
            address: None,
            feerate: Some(5),
            fee_failed: false,
            vbytes: Some(300),
            working: false,
            error: None,
        }
    }

    #[test]
    fn spends_the_included_coins_and_the_key() {
        let o = overlay();
        assert_eq!(o.spent(), (2, Amount::from_sat(230_000)));
        assert_eq!(o.fee(), Some(Amount::from_sat(1_500)));
        assert_eq!(o.blocked(), None);
    }

    #[test]
    fn blocks_without_coins_key_or_fee() {
        let mut o = overlay();
        o.inputs
            .iter_mut()
            .filter(|r| !r.key)
            .for_each(|r| r.included = false);
        assert_eq!(o.blocked(), Some("Select at least one coin"));
        let mut o = overlay();
        o.inputs.retain(|r| !r.key);
        o.key_needed = true;
        assert_eq!(o.blocked(), Some("Waiting for a split key"));
        let mut o = overlay();
        o.feerate = None;
        o.fee_failed = true;
        assert_eq!(o.blocked(), Some("Fee rate unavailable. Try again later."));
        let mut o = overlay();
        o.method = Method::Signature;
        assert_eq!(
            o.blocked(),
            Some("This Vault can't make a replay-protected signature")
        );
    }

    #[test]
    fn overlay_lists_the_key_as_a_fixed_input_and_names_the_result() {
        use crate::app::view::shared::landing::capture;
        let mounted = capture::inspect(
            "vault-split-coins-overlay",
            split_overlay(&overlay()),
            (1280, 900),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        assert!(mounted.contains("Split coins on Bitcoin"));
        assert!(mounted.contains("Coins in this transaction"));
        assert!(mounted.contains("Splitting"));
        assert!(mounted.contains("Split key"));
        assert!(mounted.contains("Spends 2 coins · 230,000 sats including the split key"));
        assert!(mounted.contains("≈ 228,500 sats"));
        assert!(mounted.contains("Sign and send"));
        assert!(!mounted.contains("How to split"), "Bitcoin has one method");

        let mut blake2b = overlay();
        blake2b.here = ChainId::BitcoinBlake2b;
        let mounted = capture::inspect(
            "vault-split-coins-overlay-blake2b",
            split_overlay(&blake2b),
            (1280, 900),
            "light",
            coincube_ui::theme::Theme::light(),
        );
        assert!(mounted.contains("How to split"));
        assert!(mounted.contains("Not available: this Vault has a hardware signer"));
    }
}
