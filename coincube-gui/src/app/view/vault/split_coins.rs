//! Split coins surfaces. Coin control keeps the banner counting coins still on
//! both chains, the selection bar and the send warning; their Split buttons go
//! to Tools › Split coins, which is the whole flow: get a split coin, review
//! and sign, then confirmations (Tenshu canvas, `splitflow` page).

use coincube_core::miniscript::bitcoin::{Amount, OutPoint, Txid};
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::locale::Text as CopyText;
use coincube_ui::localized_format;
use coincube_ui::{
    component::{amount::*, button, text::*},
    icon, theme,
    widget::{Column, Container, Element, Row},
};
use iced::{
    widget::{checkbox, qr_code, Space},
    Alignment, Length,
};

use crate::{
    app::{
        menu::{Menu, VaultSubMenu},
        view::message::{CoinControlMessage, Message, SplitMessage},
    },
    chain::ChainId,
    services::{chain_status::SPLIT_CONFIRMATIONS, split_coins::Method},
};

/// The smallest split coin worth sending; anything smaller is mostly fee.
pub const MIN_KEY_SATS: u64 = 10_000;

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
                button::primary_compact(None, localized_copy("Split coins"))
                    .on_press(Message::CoinControl(CoinControlMessage::OpenSplit))
            } else {
                button::primary_compact(None, localized_copy("Split coins"))
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
        localized_copy("No coins selected").to_string()
    } else {
        localized_format!(
            "{} selected · {}",
            "{} seleccionadas · {}",
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
            button::secondary_compact(None, localized_copy("Clear"))
                .on_press(Message::CoinControl(CoinControlMessage::ClearChosen))
        } else {
            button::secondary_compact(None, localized_copy("Clear"))
        })
        .push(if count > 0 {
            button::primary_compact(None, localized_copy("Send selected"))
                .on_press(Message::CoinControl(CoinControlMessage::SendSelected))
        } else {
            button::primary_compact(None, localized_copy("Send selected"))
        })
        .into()
}

/// The warning before Send selected spends coins that are on both chains.
/// Either chain can send anyway after the acknowledgement; replay is the
/// user's choice, never a block. `imported` names an imported Vault's reason:
/// its signers make only ordinary signatures.
pub fn send_warning<'a>(
    here: ChainId,
    count: usize,
    ack: bool,
    imported: bool,
) -> Element<'a, Message> {
    let other = chain_name(twin(here));
    let question = if count == 1 {
        localized_format!(
            "1 selected coin also exists on {}",
            "1 moneda seleccionada también existe en {}",
            other
        )
    } else {
        localized_format!(
            "{} selected coins also exist on {}",
            "{} monedas seleccionadas también existen en {}",
            count,
            other
        )
    };
    let body = if here.is_blake2b() && imported {
        localized_copy("This imported wallet signs the standard Bitcoin way, so this transaction could be copied to Bitcoin and move your bitcoin too. Split these coins first, or continue if that doesn't matter to you.")
    } else if here.is_blake2b() {
        localized_copy("A Bitcoin Blake2b transaction that spends them can be copied to Bitcoin and move those coins too, to the same recipient. Split them first, or continue if that doesn't matter to you.")
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
    let ack_label = if here.is_blake2b() {
        localized_copy("I understand the Bitcoin copies may move too")
    } else {
        localized_copy("I understand the Bitcoin Blake2b copies may move too")
    };
    column = column.push(
        checkbox(ack)
            .label(ack_label)
            .on_toggle(|ack| Message::CoinControl(CoinControlMessage::AcknowledgeBothChains(ack))),
    );
    let mut footer = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(Space::new().width(Length::Fill))
        .push(
            button::secondary(None, CopyText::Cancel.get())
                .on_press(Message::CoinControl(CoinControlMessage::CloseWarning)),
        );
    footer = footer.push(if ack {
        button::secondary(None, localized_copy("Send anyway"))
            .on_press(Message::CoinControl(CoinControlMessage::SendAnyway))
    } else {
        button::secondary(None, localized_copy("Send anyway"))
    });
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

// ---------------------------------------------------------------------------
// Tools › Split coins
// ---------------------------------------------------------------------------

/// Where the split coin stands while the flow waits for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStatus {
    /// Nothing has arrived yet.
    Waiting,
    /// A coin arrived and has no confirmation yet.
    Confirming { amount: Amount },
    /// A confirmed coin whose history is being checked.
    Checking { amount: Amount, confirmations: u32 },
    /// Its history shows it exists on both chains: it can't be a split coin.
    BothChains { amount: Amount },
    /// Its history couldn't be checked.
    CouldntVerify { amount: Amount },
}

/// The step the flow is on, derived from the Vault each time it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Coins to split, and the split needs a split coin first.
    Key(KeyStatus),
    /// Ready to review and sign.
    Review,
    /// A split was built and is waiting to be signed and sent.
    Unsigned { txid: Txid },
    /// A split was sent and has this many confirmations.
    Sent { confirmations: u32 },
    /// An earlier split confirmed, and coins are still on both chains.
    Rest { split: usize },
    /// Nothing is on both chains, but some coins have no answer yet.
    Verifying { count: usize },
    /// Nothing is on both chains.
    Done,
}

/// One both-chain coin in the review.
#[derive(Debug, Clone)]
pub struct ReviewCoin {
    pub outpoint: OutPoint,
    pub amount: Amount,
    pub label: String,
    pub included: bool,
}

/// What the side card about the twin Cube offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyCard {
    /// On Bitcoin, no copy yet: Create copy.
    Create,
    /// On Bitcoin, the copy exists.
    Exists,
    /// On Bitcoin Blake2b: this Cube is the copy.
    ThisIsTheCopy,
    /// Nothing to offer.
    None,
}

/// Everything Tools › Split coins shows. Built by the Split coins panel.
#[derive(Debug, Clone)]
pub struct SplitFlow {
    pub here: ChainId,
    pub step: Step,
    /// Coins still on both chains, with the user's choices.
    pub coins: Vec<ReviewCoin>,
    /// The split coin the split spends, when the method uses one.
    pub key: Option<Amount>,
    pub method: Method,
    pub signature_available: bool,
    /// The address to send a split coin to.
    pub address: Option<String>,
    pub address_error: Option<String>,
    pub feerate: Option<u64>,
    pub fee_failed: bool,
    /// Estimated virtual size of the split, for the fee line.
    pub vbytes: Option<u64>,
    pub working: bool,
    pub error: Option<String>,
    pub copy: CopyCard,
    pub mask_amounts: bool,
    pub unit: BitcoinDisplayUnit,
}

impl SplitFlow {
    fn other(&self) -> &'static str {
        chain_name(twin(self.here))
    }

    /// The included coins: their count and total, without the split coin.
    pub fn spent(&self) -> (usize, Amount) {
        let included: Vec<_> = self.coins.iter().filter(|coin| coin.included).collect();
        (
            included.len(),
            included.iter().map(|coin| coin.amount).sum(),
        )
    }

    /// Everything the transaction spends, the split coin included.
    pub fn total_in(&self) -> Amount {
        self.spent().1 + self.key_spent().unwrap_or(Amount::ZERO)
    }

    fn key_spent(&self) -> Option<Amount> {
        (self.method == Method::SplitKey)
            .then_some(self.key)
            .flatten()
    }

    pub fn fee(&self) -> Option<Amount> {
        Some(Amount::from_sat(self.feerate?.checked_mul(self.vbytes?)?))
    }

    /// What comes back to this Vault after the fee.
    pub fn back(&self) -> Option<Amount> {
        self.fee()
            .map(|fee| self.total_in().checked_sub(fee).unwrap_or(Amount::ZERO))
    }

    /// Why **Sign and send** is unavailable, if it is. English: the view
    /// localizes it.
    pub fn blocked(&self) -> Option<&'static str> {
        let (coins, _) = self.spent();
        if self.working {
            Some("Building the split…")
        } else if coins == 0 {
            Some("Select at least one coin.")
        } else if self.method == Method::SplitKey && self.key.is_none() {
            Some("Waiting for a split coin.")
        } else if self.method == Method::Signature && !self.signature_available {
            Some("This Vault can't make a replay-protected signature.")
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

    fn money(&self, amount: Amount) -> String {
        if self.mask_amounts {
            "•••••• sats".to_string()
        } else {
            sats(amount)
        }
    }

    fn amount_cell<'a>(&self, amount: Amount) -> Element<'a, Message> {
        if self.mask_amounts {
            p1_bold("••••••").into()
        } else {
            amount_with_unit(&amount, self.unit).into()
        }
    }
}

/// The flow's step indices: `None` hides the stepper.
fn stepper_position(flow: &SplitFlow) -> Option<usize> {
    match flow.step {
        Step::Key(_) => Some(0),
        Step::Review | Step::Unsigned { .. } => Some(1),
        Step::Sent { .. } => Some(2),
        Step::Rest { .. } | Step::Verifying { .. } | Step::Done => None,
    }
}

fn stepper<'a>(flow: &SplitFlow) -> Option<Element<'a, Message>> {
    let at = stepper_position(flow)?;
    // A signature split needs no split coin: two steps.
    let first = if flow.method == Method::Signature {
        1
    } else {
        0
    };
    let labels = [
        localized_copy("Get a split coin"),
        localized_copy("Review and sign"),
        localized_copy("Confirmations"),
    ];
    let mut row = Row::new().spacing(12).align_y(Alignment::Center);
    for (i, label) in labels.iter().enumerate().skip(first) {
        let number = i + 1 - first;
        let mark: Element<'a, Message> = if i < at {
            Container::new(icon::check_icon().style(theme::text::success))
                .padding([2, 8])
                .style(theme::pill::success)
                .into()
        } else {
            Container::new(p2_bold(number.to_string()))
                .padding([2, 8])
                .style(if i == at {
                    theme::pill::primary
                } else {
                    theme::pill::simple
                })
                .into()
        };
        let text = if i == at {
            p1_bold(*label)
        } else {
            p1_regular(*label).style(theme::text::secondary)
        };
        row = row.push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(mark)
                .push(text),
        );
        if i < labels.len() - 1 {
            row = row
                .push(Container::new(iced::widget::rule::horizontal(1)).width(Length::Fixed(40.0)));
        }
    }
    Some(row.into())
}

fn panel<'a>(content: Column<'a, Message>) -> Element<'a, Message> {
    Container::new(content.spacing(16))
        .padding(24)
        .style(theme::card::simple)
        .width(Length::Fill)
        .into()
}

fn status_box<'a>(title: String, detail: String, tone: Tone) -> Element<'a, Message> {
    let mark = match tone {
        Tone::Busy => p1_bold("…").style(theme::text::warning),
        Tone::Bad => icon::warning_icon().style(theme::text::error),
        Tone::Good => icon::check_icon().style(theme::text::success),
    };
    Container::new(
        Row::new()
            .spacing(12)
            .align_y(Alignment::Start)
            .push(mark)
            .push(
                Column::new()
                    .spacing(4)
                    .width(Length::Fill)
                    .push(p1_bold(title))
                    .push(p2_regular(detail).style(theme::text::secondary)),
            ),
    )
    .padding(16)
    .style(match tone {
        Tone::Bad => theme::card::error,
        Tone::Busy => theme::card::warning,
        Tone::Good => theme::card::border,
    })
    .width(Length::Fill)
    .into()
}

#[derive(Clone, Copy)]
enum Tone {
    Busy,
    Bad,
    Good,
}

fn key_step<'a>(
    flow: &SplitFlow,
    status: KeyStatus,
    qr: Option<&'a qr_code::Data>,
) -> Element<'a, Message> {
    let here = chain_name(flow.here);
    let other = flow.other();
    let address: Element<'a, Message> = match (&flow.address, &flow.address_error) {
        (Some(address), _) => Container::new(
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(Container::new(p1_regular(address.clone())).width(Length::Fill))
                .push(
                    button::secondary_compact(Some(icon::copy_icon()), localized_copy("Copy"))
                        .on_press(Message::CopyWithToast(
                            address.clone(),
                            localized_copy("Address copied").to_string(),
                        )),
                ),
        )
        .padding(12)
        .style(theme::card::border)
        .width(Length::Fill)
        .into(),
        (None, Some(error)) => p2_regular(coincube_ui::locale::notice(error.clone()))
            .style(theme::text::error)
            .into(),
        (None, None) => p2_regular(localized_copy("Getting an address…"))
            .style(theme::text::secondary)
            .into(),
    };
    let where_from = if flow.here.is_blake2b() {
        localized_copy(
            "a Bitcoin Blake2b mining pool payout, or an exchange or wallet that has already split its coins.",
        )
    } else {
        localized_copy(
            "a mining pool payout, or an exchange or wallet that has already split its coins.",
        )
    };
    let details = Column::new()
        .spacing(10)
        .width(Length::Fill)
        .push(p2_bold(localized_format!(
            "Send at least {} to",
            "Envía al menos {} a",
            sats(Amount::from_sat(MIN_KEY_SATS))
        )))
        .push(address)
        .push(
            p2_regular(localized_format!(
                "Where from: {}",
                "De dónde: {}",
                where_from
            ))
            .style(theme::text::secondary),
        )
        .push(
            p2_regular(localized_copy(
                "Not from: coins you held before the fork, anything sent from this Vault, or a wallet or exchange that hasn't split its coins. Those can still exist on both chains.",
            ))
            .style(theme::text::secondary),
        );
    let body: Element<'a, Message> = match qr {
        Some(qr) => Row::new()
            .spacing(24)
            .align_y(Alignment::Start)
            .push(
                Container::new(
                    iced::widget::QRCode::<coincube_ui::theme::Theme>::new(qr).total_size(148),
                )
                .padding(8)
                .style(theme::card::border),
            )
            .push(details)
            .into(),
        None => details.into(),
    };
    let status = match status {
        KeyStatus::Waiting => status_box(
            localized_copy("Waiting for a coin").to_string(),
            localized_copy(
                "You can leave this page. The coin shows up here, and in Coin control, when it arrives.",
            )
            .to_string(),
            Tone::Busy,
        ),
        KeyStatus::Confirming { amount } => status_box(
            localized_format!(
                "{} received · waiting for its first confirmation",
                "{} recibidos · esperando su primera confirmación",
                flow.money(amount)
            ),
            localized_copy("Its history is checked once it confirms.").to_string(),
            Tone::Busy,
        ),
        KeyStatus::Checking {
            amount,
            confirmations,
        } => status_box(
            localized_format!(
                "Checking it exists only on {}",
                "Comprobando que solo existe en {}",
                here
            ),
            if confirmations == 1 {
                localized_format!(
                    "{} · 1 confirmation. This usually takes under a minute.",
                    "{} · 1 confirmación. Suele tardar menos de un minuto.",
                    flow.money(amount)
                )
            } else {
                localized_format!(
                    "{} · {} confirmations. This usually takes under a minute.",
                    "{} · {} confirmaciones. Suele tardar menos de un minuto.",
                    flow.money(amount),
                    confirmations
                )
            },
            Tone::Busy,
        ),
        KeyStatus::BothChains { .. } => status_box(
            localized_copy("This coin exists on both chains").to_string(),
            localized_format!(
                "Nothing in its history exists only on {}, so it also exists on {}. It stays in your Vault as an ordinary coin. Send one from another source.",
                "Nada en su historial existe solo en {}, así que también existe en {}. Se queda en tu Vault como una moneda normal. Envía una desde otro origen.",
                here,
                other
            ),
            Tone::Bad,
        ),
        KeyStatus::CouldntVerify { .. } => status_box(
            localized_format!(
                "Couldn't confirm this coin exists only on {}",
                "No se pudo confirmar que esta moneda solo existe en {}",
                here
            ),
            localized_copy(
                "Its history couldn't be checked. It stays in your Vault as an ordinary coin. Send one from another source.",
            )
            .to_string(),
            Tone::Bad,
        ),
    };
    panel(
        Column::new()
            .push(h4_bold(localized_copy("Send a split coin to this Vault")))
            .push(
                p1_regular(localized_format!(
                    "This is a small coin that exists only on {}: newly mined, or from coins that were already split. This Vault checks its history before using it. The split spends it with your coins and it comes straight back, so you only need one.",
                    "Es una moneda pequeña que solo existe en {}: recién minada, o procedente de monedas que ya se separaron. Este Vault comprueba su historial antes de usarla. La separación la gasta junto con tus monedas y vuelve enseguida, así que solo necesitas una.",
                    here
                ))
                .style(theme::text::secondary),
            )
            .push(body)
            .push(status),
    )
}

fn method_choice<'a>(
    selected: bool,
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
    iced::widget::button(body)
        .padding(12)
        .width(Length::Fill)
        .style(theme::button::transparent_border)
        .on_press(Message::Split(SplitMessage::Method(method)))
        .into()
}

/// What a coin row starts with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowMark {
    /// A checkbox to leave the coin out.
    Check,
    /// The split coin: always included.
    Lock,
    /// A plain list.
    None,
}

fn coin_row<'a>(flow: &SplitFlow, coin: &ReviewCoin, mark: RowMark) -> Element<'a, Message> {
    let outpoint = coin.outpoint;
    let locked = mark == RowMark::Lock;
    let check: Element<'a, Message> = if mark == RowMark::None {
        Space::new().width(Length::Fixed(0.0)).into()
    } else if locked {
        Container::new(icon::lock_icon().style(theme::text::secondary))
            .width(Length::Fixed(20.0))
            .into()
    } else if flow.working {
        checkbox(coin.included).into()
    } else {
        checkbox(coin.included)
            .on_toggle(move |_| Message::Split(SplitMessage::ToggleCoin(outpoint)))
            .into()
    };
    let short = {
        let txid = outpoint.txid.to_string();
        format!(
            "{}…{}:{}",
            &txid[..4],
            &txid[txid.len() - 4..],
            outpoint.vout
        )
    };
    let (name, sub, chains) = if locked {
        (
            localized_copy("Split coin").to_string(),
            localized_copy("Comes back with the split").to_string(),
            localized_format!("Only on {}", "Solo en {}", chain_name(flow.here)),
        )
    } else {
        (
            coin.label.clone(),
            short,
            localized_copy("On both chains").to_string(),
        )
    };
    let amount = flow.amount_cell(coin.amount);
    Container::new(
        Row::new()
            .spacing(16)
            .align_y(Alignment::Center)
            .push(check)
            .push(
                Column::new()
                    .spacing(2)
                    .width(Length::Fill)
                    .push(p1_regular(name))
                    .push(p2_regular(sub).style(theme::text::secondary)),
            )
            .push(
                Container::new(p2_regular(chains).style(theme::text::secondary))
                    .width(Length::Fixed(170.0)),
            )
            .push(
                Container::new(amount)
                    .width(Length::Fixed(170.0))
                    .align_x(Alignment::End),
            ),
    )
    .padding([10, 4])
    .width(Length::Fill)
    .into()
}

fn ledger_line<'a>(
    label: String,
    detail: Option<String>,
    value: Element<'a, Message>,
    total: bool,
) -> Element<'a, Message> {
    let mut left = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(if total {
            p1_bold(label)
        } else {
            p1_regular(label)
        });
    if let Some(detail) = detail {
        left = left.push(p2_regular(detail).style(theme::text::secondary));
    }
    Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .padding([0, 4])
        .push(Container::new(left).width(Length::Fill))
        .push(Container::new(value).align_x(Alignment::End))
        .into()
}

fn review_step<'a>(flow: &SplitFlow) -> Element<'a, Message> {
    let here = chain_name(flow.here);
    let other = flow.other();
    let lead = if flow.method == Method::Signature {
        localized_format!(
            "One {} transaction sends these coins back to this Vault, signed so that {} rejects it. Their copies there don't move.",
            "Una transacción de {} devuelve estas monedas a este Vault, firmada para que {} la rechace. Sus copias allí no se mueven.",
            here,
            other
        )
    } else {
        localized_format!(
            "One {} transaction sends these coins back to this Vault. {} rejects it, so their copies there don't move.",
            "Una transacción de {} devuelve estas monedas a este Vault. {} la rechaza, así que sus copias allí no se mueven.",
            here,
            other
        )
    };
    let mut column = Column::new()
        .push(h4_bold(localized_copy("Review the split")))
        .push(p1_regular(lead).style(theme::text::secondary));

    if flow.here.is_blake2b() && flow.signature_available {
        column = column.push(
            Column::new()
                .spacing(8)
                .push(p1_bold(localized_copy("How to split")))
                .push(method_choice(
                    flow.method == Method::Signature,
                    localized_copy("Replay-protected signature"),
                    localized_copy("No split coin needed.").to_string(),
                    Method::Signature,
                ))
                .push(method_choice(
                    flow.method == Method::SplitKey,
                    localized_copy("Split coin"),
                    localized_format!(
                        "Spends a coin that exists only on {} with these coins.",
                        "Gasta una moneda que solo existe en {} junto con estas monedas.",
                        here
                    ),
                    Method::SplitKey,
                )),
        );
    }

    let mut rows = Column::new()
        .spacing(0)
        .push(iced::widget::rule::horizontal(1));
    for coin in &flow.coins {
        rows = rows
            .push(coin_row(flow, coin, RowMark::Check))
            .push(iced::widget::rule::horizontal(1));
    }
    if let Some(key) = flow.key_spent() {
        let row = ReviewCoin {
            outpoint: OutPoint::null(),
            amount: key,
            label: String::new(),
            included: true,
        };
        rows = rows
            .push(coin_row(flow, &row, RowMark::Lock))
            .push(iced::widget::rule::horizontal(1));
    }
    column = column.push(rows);

    let (count, spent) = flow.spent();
    let fee_detail = match (flow.fee(), flow.feerate) {
        (Some(fee), Some(rate)) => localized_format!(
            "≈ {} at {} sat/vB",
            "≈ {} a {} sat/vB",
            flow.money(fee),
            rate
        ),
        _ if flow.fee_failed => localized_copy("Fee rate unavailable").to_string(),
        _ => localized_copy("Getting the fee rate…").to_string(),
    };
    let dash =
        || -> Element<'a, Message> { p1_regular("—").style(theme::text::secondary).into() };
    let mut ledger = Column::new()
        .spacing(10)
        .push(ledger_line(
            localized_copy("Splits").to_string(),
            Some(coins_count(count)),
            flow.amount_cell(spent),
            false,
        ))
        .push(ledger_line(
            localized_copy("Network fee").to_string(),
            Some(fee_detail),
            flow.fee().map_or_else(dash, |fee| flow.amount_cell(fee)),
            false,
        ))
        .push(iced::widget::rule::horizontal(1))
        .push(ledger_line(
            localized_copy("Back to this Vault").to_string(),
            flow.key_spent()
                .map(|_| localized_copy("includes the split coin").to_string()),
            flow.back().map_or_else(dash, |back| flow.amount_cell(back)),
            true,
        ))
        .push(
            Row::new().padding([0, 4]).push(
                p2_regular(localized_copy(
                    "To a new address in this Vault. The signing screen shows it before you sign.",
                ))
                .style(theme::text::secondary),
            ),
        );
    ledger = ledger.padding(iced::Padding::ZERO.top(16));
    column = column.push(ledger);

    column = column.push(
        Column::new()
            .spacing(6)
            .push(
                p2_regular(localized_format!(
                    "After {} confirmations the coins are separate. Their {} copies stay where they are and can be spent on their own.",
                    "Tras {} confirmaciones las monedas quedan separadas. Sus copias en {} se quedan donde están y se pueden gastar por su cuenta.",
                    SPLIT_CONFIRMATIONS,
                    other
                ))
                .style(theme::text::secondary),
            )
            .push(
                p2_regular(localized_copy(
                    "Spending these coins together links them to each other on the blockchain.",
                ))
                .style(theme::text::secondary),
            )
            .padding(iced::Padding::ZERO.top(8)),
    );
    if let Some(error) = &flow.error {
        column = column
            .push(p2_regular(coincube_ui::locale::notice(error.clone())).style(theme::text::error));
    }
    let blocked = flow.blocked();
    let sign_label = if flow.working {
        localized_copy("Building…")
    } else {
        localized_copy("Sign and send")
    };
    let mut actions =
        Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(if blocked.is_none() {
                button::primary(None, sign_label)
                    .width(Length::Shrink)
                    .on_press(Message::Split(SplitMessage::Sign))
            } else {
                button::primary(None, sign_label).width(Length::Shrink)
            });
    if let Some(reason) = blocked.filter(|_| !flow.working) {
        actions = actions.push(p2_regular(localized_copy(reason)).style(theme::text::secondary));
    }
    column = column.push(actions);
    panel(column)
}

fn unsigned_step<'a>(txid: Txid) -> Element<'a, Message> {
    panel(
        Column::new()
            .push(h4_bold(localized_copy("Waiting to be signed")))
            .push(
                p1_regular(localized_copy(
                    "The split is built. Sign and send it from PSBT proposals; this page follows its confirmations once it's sent.",
                ))
                .style(theme::text::secondary),
            )
            .push(
                button::primary(None, localized_copy("Open in PSBT proposals"))
                    .width(Length::Shrink)
                    .on_press(
                    Message::Menu(Menu::Vault(VaultSubMenu::PSBTs(Some(txid)))),
                ),
            )
            .push(
                p2_regular(localized_copy(
                    "Deleted it? Its coins can be split again 14 days after it was built.",
                ))
                .style(theme::text::secondary),
            ),
    )
}

fn sent_step<'a>(confirmations: u32) -> Element<'a, Message> {
    let line = if confirmations == 0 {
        localized_copy("Waiting for the first confirmation").to_string()
    } else {
        localized_format!(
            "{} of {} confirmations",
            "{} de {} confirmaciones",
            confirmations,
            SPLIT_CONFIRMATIONS
        )
    };
    let pips = (0..SPLIT_CONFIRMATIONS).fold(Row::new().spacing(4), |row, i| {
        row.push(
            Container::new(Space::new())
                .width(Length::Fixed(32.0))
                .height(Length::Fixed(6.0))
                .style(if i < confirmations {
                    theme::pill::success
                } else {
                    theme::pill::simple
                }),
        )
    });
    panel(
        Column::new()
            .push(h4_bold(localized_copy("Split sent")))
            .push(
                p1_regular(localized_copy(
                    "The coins are on their way back to this Vault. You can leave this page; Coin control shows the same progress.",
                ))
                .style(theme::text::secondary),
            )
            .push(
                Container::new(
                    Column::new()
                        .spacing(10)
                        .push(p1_bold(line))
                        .push(pips),
                )
                .padding(16)
                .style(if confirmations > 0 {
                    theme::card::border
                } else {
                    theme::card::warning
                })
                .width(Length::Fill),
            ),
    )
}

fn rest_step<'a>(flow: &SplitFlow, split: usize) -> Element<'a, Message> {
    let left = flow.coins.len();
    let title = if left == 1 {
        localized_copy("1 coin is still on both chains").to_string()
    } else {
        localized_format!(
            "{} coins are still on both chains",
            "{} monedas siguen en ambas cadenas",
            left
        )
    };
    let ready = flow.method == Method::Signature || flow.key.is_some();
    let lead = if ready {
        localized_copy(
            "Left out of the last split, or received since. You don't need another split coin, so this split starts at Review.",
        )
    } else {
        localized_copy(
            "Left out of the last split, or received since. This split needs a split coin first.",
        )
    };
    let mut rows = Column::new()
        .spacing(0)
        .push(iced::widget::rule::horizontal(1));
    for coin in &flow.coins {
        rows = rows
            .push(coin_row(flow, coin, RowMark::None))
            .push(iced::widget::rule::horizontal(1));
    }
    panel(
        Column::new()
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(icon::check_icon().style(theme::text::success))
                    .push(
                        p2_bold(localized_format!(
                            "{} split",
                            "{} separadas",
                            coins_count(split)
                        ))
                        .style(theme::text::success),
                    ),
            )
            .push(h4_bold(title))
            .push(p1_regular(lead).style(theme::text::secondary))
            .push(rows)
            .push(
                button::primary(None, localized_copy("Split the rest"))
                    .width(Length::Shrink)
                    .on_press(Message::Split(SplitMessage::SplitRest)),
            ),
    )
}

fn verifying_step<'a>(count: usize) -> Element<'a, Message> {
    let title = if count == 1 {
        localized_copy("1 coin not verified yet").to_string()
    } else {
        localized_format!(
            "{} coins not verified yet",
            "{} monedas aún sin verificar",
            count
        )
    };
    panel(
        Column::new().push(status_box(
            title,
            localized_copy(
                "Nothing is known to be on both chains, but these coins have no answer yet. Coin control shows each coin's status.",
            )
            .to_string(),
            Tone::Busy,
        )),
    )
}

fn done_step<'a>(flow: &SplitFlow) -> Element<'a, Message> {
    let mut detail = localized_format!(
        "Nothing in this Vault exists on both chains any more. Transactions here can't be copied to {}.",
        "Ya nada de este Vault existe en ambas cadenas. Las transacciones de aquí no se pueden copiar a {}.",
        flow.other()
    );
    if let Some(key) = flow.key {
        detail.push(' ');
        detail.push_str(&localized_format!(
            "Your split coin ({}) stays here for the next time you receive a coin that exists on both chains.",
            "Tu moneda separada ({}) se queda aquí para la próxima vez que recibas una moneda que exista en ambas cadenas.",
            flow.money(key)
        ));
    }
    status_box(
        localized_copy("Every coin is split").to_string(),
        detail,
        Tone::Good,
    )
}

fn side_card<'a>(
    title: String,
    body: String,
    action: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    Container::new(
        Column::new()
            .spacing(10)
            .push(p1_bold(title))
            .push(p2_regular(body).style(theme::text::secondary))
            .push(action),
    )
    .padding(20)
    .style(theme::card::simple)
    .width(Length::Fill)
    .into()
}

fn cards<'a>(flow: &SplitFlow) -> Element<'a, Message> {
    let copy: Option<Element<'a, Message>> = match flow.copy {
        CopyCard::Create => Some(side_card(
            localized_copy("Bitcoin Blake2b copy").to_string(),
            localized_copy(
                "To see and spend your Bitcoin Blake2b coins, create a second Cube that holds this Vault on Bitcoin Blake2b. You can do it before or after the split.",
            )
            .to_string(),
            Some(
                button::secondary(None, localized_copy("Create copy"))
                    .width(Length::Shrink)
                    .on_press(Message::CreateBlake2bCopy)
                    .into(),
            ),
        )),
        CopyCard::Exists => Some(side_card(
            localized_copy("Bitcoin Blake2b copy").to_string(),
            localized_copy(
                "Holds this Vault's Bitcoin Blake2b coins. Switch to it from the Cube menu at the top of the sidebar.",
            )
            .to_string(),
            None,
        )),
        CopyCard::ThisIsTheCopy => Some(side_card(
            localized_copy("Bitcoin Cube").to_string(),
            localized_copy(
                "This Cube is the Bitcoin Blake2b copy of a Bitcoin Vault. A split from either Cube separates the coins on both, so you only need to do it once.",
            )
            .to_string(),
            None,
        )),
        CopyCard::None => None,
    };
    let why = side_card(
        localized_copy("Why split?").to_string(),
        localized_format!(
            "Coins from before the fork exist on both chains with the same keys. A payment here can be rebroadcast on {} by anyone, and pay the same recipient twice. After a split, each chain's coins move on their own.",
            "Las monedas de antes de la bifurcación existen en ambas cadenas con las mismas claves. Cualquiera puede retransmitir en {} un pago hecho aquí y pagar dos veces al mismo destinatario. Tras separarlas, las monedas de cada cadena se mueven por su cuenta.",
            flow.other()
        ),
        None,
    );
    Row::new()
        .spacing(16)
        .align_y(Alignment::Start)
        .push(copy)
        .push(why)
        .into()
}

/// Tools › Split coins.
pub fn split_flow_view<'a>(
    flow: &SplitFlow,
    qr: Option<&'a qr_code::Data>,
) -> Element<'a, Message> {
    let mut column = Column::new()
        .spacing(20)
        .push(h3(localized_copy("Split coins")).bold());
    let (both, total) = (
        flow.coins.len(),
        flow.coins.iter().map(|coin| coin.amount).sum::<Amount>(),
    );
    if both > 0 && !matches!(flow.step, Step::Sent { .. }) {
        let line = if both == 1 {
            localized_format!(
                "{} · {} also exists on {}. Until it's split, a transaction here can be copied there and move both. Splitting once, from either chain, is enough.",
                "{} · {} también existe en {}. Hasta separarla, una transacción de aquí se puede copiar allí y mover ambas. Basta con separar una vez, desde cualquiera de las dos cadenas.",
                coins_count(both),
                flow.money(total),
                flow.other()
            )
        } else {
            localized_format!(
                "{} · {} also exist on {}. Until they're split, a transaction here can be copied there and move both. Splitting once, from either chain, is enough.",
                "{} · {} también existen en {}. Hasta separarlas, una transacción de aquí se puede copiar allí y mover ambas. Basta con separar una vez, desde cualquiera de las dos cadenas.",
                coins_count(both),
                flow.money(total),
                flow.other()
            )
        };
        column = column.push(p1_regular(line).style(theme::text::secondary));
    }
    column = column.push(stepper(flow));
    column = column.push(match flow.step {
        Step::Key(status) => key_step(flow, status, qr),
        Step::Review => review_step(flow),
        Step::Unsigned { txid } => unsigned_step(txid),
        Step::Sent { confirmations } => sent_step(confirmations),
        Step::Rest { split } => rest_step(flow, split),
        Step::Verifying { count } => verifying_step(count),
        Step::Done => done_step(flow),
    });
    column.push(cards(flow)).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use coincube_ui::locale::{render_in, Language};
    use std::str::FromStr;

    fn coin(n: u8, sats: u64) -> ReviewCoin {
        ReviewCoin {
            outpoint: OutPoint::from_str(&format!("{}:0", format!("{:02x}", n).repeat(32)))
                .unwrap(),
            amount: Amount::from_sat(sats),
            label: "Cold storage".into(),
            included: true,
        }
    }

    fn flow() -> SplitFlow {
        SplitFlow {
            here: ChainId::Bitcoin,
            step: Step::Review,
            coins: vec![coin(1, 100_000), coin(2, 100_000)],
            key: Some(Amount::from_sat(30_000)),
            method: Method::SplitKey,
            signature_available: false,
            address: Some("bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg".into()),
            address_error: None,
            feerate: Some(5),
            fee_failed: false,
            vbytes: Some(300),
            working: false,
            error: None,
            copy: CopyCard::Create,
            mask_amounts: false,
            unit: BitcoinDisplayUnit::BTC,
        }
    }

    fn mount(name: &str, flow: &SplitFlow, size: (u32, u32)) -> capture::Inspection {
        capture::inspect(
            name,
            split_flow_view(flow, None),
            size,
            "dark",
            coincube_ui::theme::Theme::dark(),
        )
    }

    #[test]
    fn spends_the_included_coins_and_the_split_coin() {
        let f = flow();
        assert_eq!(f.spent(), (2, Amount::from_sat(200_000)));
        assert_eq!(f.total_in(), Amount::from_sat(230_000));
        assert_eq!(f.fee(), Some(Amount::from_sat(1_500)));
        assert_eq!(f.back(), Some(Amount::from_sat(228_500)));
        assert_eq!(f.blocked(), None);
        let signature = SplitFlow {
            method: Method::Signature,
            signature_available: true,
            ..flow()
        };
        assert_eq!(
            signature.total_in(),
            Amount::from_sat(200_000),
            "a signature split spends no split coin"
        );
    }

    #[test]
    fn blocks_without_coins_split_coin_or_fee() {
        let mut f = flow();
        f.coins.iter_mut().for_each(|c| c.included = false);
        assert_eq!(f.blocked(), Some("Select at least one coin."));
        let f = SplitFlow {
            key: None,
            ..flow()
        };
        assert_eq!(f.blocked(), Some("Waiting for a split coin."));
        let f = SplitFlow {
            feerate: None,
            fee_failed: true,
            ..flow()
        };
        assert_eq!(f.blocked(), Some("Fee rate unavailable. Try again later."));
        let f = SplitFlow {
            method: Method::Signature,
            ..flow()
        };
        assert_eq!(
            f.blocked(),
            Some("This Vault can't make a replay-protected signature.")
        );
        // Every reason has its Spanish.
        for reason in [
            "Building the split…",
            "Select at least one coin.",
            "Waiting for a split coin.",
            "This Vault can't make a replay-protected signature.",
            "Fee rate unavailable. Try again later.",
            "Getting the fee rate…",
        ] {
            let es = render_in(Language::Spanish, || localized_copy(reason).to_string());
            assert_ne!(es, reason, "no Spanish for {reason:?}");
        }
    }

    /// Review lists the coins, the split coin as a locked row that comes
    /// back, and the ledger. Nothing says "Splitting" before anything is sent.
    #[test]
    fn review_lists_coins_the_split_coin_and_the_ledger() {
        let mounted = mount("vault-split-review", &flow(), (1280, 900));
        assert!(mounted.contains("Review the split"));
        assert!(mounted.contains("Get a split coin"));
        assert!(mounted.contains("Review and sign"));
        assert!(mounted.contains("On both chains"));
        assert!(mounted.contains("Split coin"));
        assert!(mounted.contains("Comes back with the split"));
        assert!(mounted.contains("Only on Bitcoin"));
        assert!(mounted.contains("≈ 1,500 sats at 5 sat/vB"));
        assert!(mounted.contains("includes the split coin"));
        assert!(mounted.contains("Sign and send"));
        assert!(mounted.contains("Create copy"));
        assert!(
            !mounted.text.iter().any(|(text, _)| text == "Splitting"),
            "no Splitting pill before anything is sent"
        );
        assert!(!mounted.contains("How to split"), "Bitcoin has one method");
        assert!(!mounted.contains("Prototype"));
    }

    /// On Bitcoin Blake2b the signature shows only when the Vault can make
    /// it, and then the stepper drops the split-coin step.
    #[test]
    fn blake2b_offers_the_signature_only_when_the_vault_can_make_it() {
        let cannot = SplitFlow {
            here: ChainId::BitcoinBlake2b,
            copy: CopyCard::ThisIsTheCopy,
            ..flow()
        };
        let mounted = mount("vault-split-review-blake2b-imported", &cannot, (1280, 900));
        assert!(!mounted.contains("How to split"));
        assert!(!mounted.contains("Not available"));
        assert!(mounted.contains("Bitcoin Cube"));

        let can = SplitFlow {
            here: ChainId::BitcoinBlake2b,
            method: Method::Signature,
            signature_available: true,
            key: None,
            copy: CopyCard::ThisIsTheCopy,
            ..flow()
        };
        let mounted = mount("vault-split-review-blake2b-signature", &can, (1280, 900));
        assert!(mounted.contains("How to split"));
        assert!(mounted.contains("Replay-protected signature"));
        assert!(!mounted.contains("Get a split coin"), "no split coin step");
        assert!(!mounted.contains("Comes back with the split"));
    }

    #[test]
    fn key_step_shows_the_address_and_each_status() {
        let mut f = flow();
        f.key = None;
        let statuses = [
            (KeyStatus::Waiting, "Waiting for a coin"),
            (
                KeyStatus::Confirming {
                    amount: Amount::from_sat(25_000),
                },
                "25,000 sats received · waiting for its first confirmation",
            ),
            (
                KeyStatus::Checking {
                    amount: Amount::from_sat(25_000),
                    confirmations: 1,
                },
                "Checking it exists only on Bitcoin",
            ),
            (
                KeyStatus::BothChains {
                    amount: Amount::from_sat(25_000),
                },
                "This coin exists on both chains",
            ),
            (
                KeyStatus::CouldntVerify {
                    amount: Amount::from_sat(25_000),
                },
                "Couldn't confirm this coin exists only on Bitcoin",
            ),
        ];
        for (status, line) in statuses {
            f.step = Step::Key(status);
            let mounted = mount("vault-split-key", &f, (1024, 768));
            assert!(mounted.contains("Send a split coin to this Vault"));
            assert!(mounted.contains("Send at least 10,000 sats to"));
            assert!(mounted.contains("bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg"));
            assert!(mounted.contains("Copy"));
            assert!(mounted.contains(line), "{}", line);
            assert!(!mounted.contains("Prototype"));
            assert!(!mounted.contains("Sign and send"));
        }
    }

    #[test]
    fn later_steps_read_as_written() {
        let mut f = flow();
        f.step = Step::Sent { confirmations: 2 };
        let mounted = mount("vault-split-sent", &f, (1024, 768));
        assert!(mounted.contains("Split sent"));
        assert!(mounted.contains("2 of 6 confirmations"));

        f.step = Step::Unsigned {
            txid: Txid::from_str(&"ab".repeat(32)).unwrap(),
        };
        let mounted = mount("vault-split-unsigned", &f, (1024, 768));
        assert!(mounted.contains("Open in PSBT proposals"));

        f.step = Step::Rest { split: 1 };
        f.coins.truncate(1);
        let mounted = mount("vault-split-rest", &f, (1024, 768));
        assert!(mounted.contains("1 coin split"));
        assert!(mounted.contains("1 coin is still on both chains"));
        assert!(mounted.contains("Split the rest"));

        f.step = Step::Done;
        f.coins.clear();
        let mounted = mount("vault-split-done", &f, (1024, 768));
        assert!(mounted.contains("Every coin is split"));
        assert!(mounted.contains("Your split coin (30,000 sats) stays here"));

        f.step = Step::Verifying { count: 2 };
        let mounted = mount("vault-split-verifying", &f, (1024, 768));
        assert!(mounted.contains("2 coins not verified yet"));
    }

    /// Every step reads in Spanish.
    #[test]
    fn every_step_reads_in_spanish() {
        let mut blake2b = SplitFlow {
            here: ChainId::BitcoinBlake2b,
            method: Method::Signature,
            signature_available: true,
            copy: CopyCard::ThisIsTheCopy,
            ..flow()
        };
        let mut steps: Vec<(&str, SplitFlow)> = vec![("review", flow())];
        steps.push(("review-blake2b", blake2b.clone()));
        blake2b.step = Step::Sent { confirmations: 0 };
        steps.push(("sent", blake2b.clone()));
        for status in [
            KeyStatus::Waiting,
            KeyStatus::Confirming {
                amount: Amount::from_sat(25_000),
            },
            KeyStatus::Checking {
                amount: Amount::from_sat(25_000),
                confirmations: 2,
            },
            KeyStatus::BothChains {
                amount: Amount::from_sat(25_000),
            },
            KeyStatus::CouldntVerify {
                amount: Amount::from_sat(25_000),
            },
        ] {
            steps.push((
                "key",
                SplitFlow {
                    step: Step::Key(status),
                    key: None,
                    ..flow()
                },
            ));
        }
        steps.push((
            "unsigned",
            SplitFlow {
                step: Step::Unsigned {
                    txid: Txid::from_str(&"ab".repeat(32)).unwrap(),
                },
                ..flow()
            },
        ));
        steps.push((
            "rest",
            SplitFlow {
                step: Step::Rest { split: 2 },
                copy: CopyCard::Exists,
                ..flow()
            },
        ));
        steps.push((
            "done",
            SplitFlow {
                step: Step::Done,
                coins: Vec::new(),
                ..flow()
            },
        ));
        steps.push((
            "verifying",
            SplitFlow {
                step: Step::Verifying { count: 1 },
                coins: Vec::new(),
                error: None,
                ..flow()
            },
        ));
        for (name, f) in steps {
            let mounted = render_in(Language::Spanish, || {
                mount(&format!("vault-split-{name}-es"), &f, (1280, 900))
            });
            mounted.assert_spanish(&format!("split coins {name}"));
        }
    }

    /// The review's Sign and send stays on screen in the smallest window:
    /// the flow is part of the page, never a dialog with a hidden footer.
    #[test]
    fn sign_and_send_is_part_of_the_page() {
        let mounted = capture::inspect_full_window(
            "vault-split-review-small",
            split_flow_view(&flow(), None),
            (1024, 640),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        assert!(mounted.contains("Sign and send"));
    }

    /// The send warning names an imported Vault's reason, in English and
    /// Spanish, and still lets it send after the tick.
    #[test]
    fn blake2b_send_warning_words_the_reason_for_an_imported_vault() {
        let vault = capture::inspect(
            "vault-split-coins-send-warning",
            send_warning(ChainId::BitcoinBlake2b, 2, false, false),
            (1024, 768),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        assert!(vault.contains("A Bitcoin Blake2b transaction that spends them"));
        assert!(vault.contains("2 selected coins also exist on Bitcoin"));
        let imported = capture::inspect(
            "vault-split-coins-send-warning-imported",
            send_warning(ChainId::BitcoinBlake2b, 2, true, true),
            (1024, 768),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        assert!(imported.contains("This imported wallet signs the standard Bitcoin way"));
        assert!(imported.contains("Send anyway"));
        let spanish = render_in(Language::Spanish, || {
            capture::inspect(
                "vault-split-coins-send-warning-imported-es",
                send_warning(ChainId::BitcoinBlake2b, 2, true, true),
                (1024, 768),
                "dark",
                coincube_ui::theme::Theme::dark(),
            )
        });
        assert!(spanish.contains("Esta cartera importada firma de la forma estándar de Bitcoin"));
        spanish.assert_spanish("send warning");
    }
}
